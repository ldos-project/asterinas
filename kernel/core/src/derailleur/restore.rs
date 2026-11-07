// SPDX-License-Identifier: MPL-2.0

//! The cassette client: the calling (single-threaded) process becomes the
//! process VM1 offered.
//!
//! Prototype stage 1: the process's pages are copied out of VM1's RAM (seen
//! through the peer BAR) into ordinary anonymous memory here, during the
//! switch, and VM1 is released right after.  Lending VM1's frames in place
//! needs frame metadata for the BAR (an OSTD change); that is the next stage.
//! One thread; no files handed over (the stub's stdio stays); no premap.

use alloc::vec;
use core::{ops::Range, time::Duration};

use aster_time::read_monotonic_time;
use ostd::{
    arch::cpu::context::{FpuContext, FsBase, GsBase, UserContext},
    mm::VmIo,
};

use super::{
    device::{Req, Resp, Shared, shared},
    wire::{self, layout as ly, rec, slot as sl, task as tk, thread as th, u32_at, u64_at, vdso as vd},
};
use crate::{
    prelude::*,
    process::{
        derailleur_activate_vmar, derailleur_new_vmar,
        posix_thread::ContextPthreadAdminApi,
        signal::sig_mask::SigMask,
    },
    vm::{perms::VmPerms, vmar::VmarMapOffset},
};

/// The stub waits here until VM1 offers a process; returns its pid.
pub fn offered_pid(ctx: &Context) -> Result<i32> {
    shared()?.wait_offer(ctx)
}

fn act(key: &str) -> Req<'_> {
    Req { op: wire::CAS_ACT, key }
}
fn get(key: &str) -> Req<'_> {
    Req { op: wire::CAS_GET, key }
}

/// A record of `typ` (version 1), at least `min` bytes.
fn record(r: &Resp, typ: u32, min: usize) -> Result<&[u8]> {
    let b = &r.value[..];
    if r.status < 0
        || b.len() < rec::HDR.max(min)
        || u32_at(b, 0) != typ
        || (u32_at(b, 4) & 0xffff) != 1
        || u64_at(b, 8) as usize != b.len()
    {
        return_errno_with_message!(Errno::EPROTO, "bad cassette record");
    }
    Ok(b)
}

fn abort(sh: &Shared) {
    let _ = sh.call(&[act("resume")]);
}

fn perms_of(prot: u32) -> VmPerms {
    let mut p = VmPerms::empty();
    if prot & ly::PROT_READ != 0 {
        p |= VmPerms::READ;
    }
    if prot & ly::PROT_WRITE != 0 {
        p |= VmPerms::WRITE;
    }
    if prot & ly::PROT_EXEC != 0 {
        p |= VmPerms::EXEC;
    }
    p
}

/// Restores the offered process into the caller.  On success the caller
/// returns to user mode as that process (`user_ctx` is replaced).
pub fn restore(ctx: &Context, user_ctx: &mut UserContext) -> Result<()> {
    let sh = shared()?;
    sh.open()?;

    // Before anything changes on VM1: is it something this stage can take?
    let r = sh.call(&[get("proc"), act("prepare")])?;
    let proc_rec = record(&r[0], rec::T_PROC, 32)?;
    let nr_threads = u32_at(proc_rec, 20);
    if r[1].status < 0 || nr_threads != 1 {
        abort(&sh);
        ostd::early_println!("derailleur: refusing: {} threads (stage 1 takes one)", nr_threads);
        return_errno_with_message!(Errno::EOPNOTSUPP, "multi-threaded process");
    }

    // The downtime: stop it, take its state, retire VM1's copy.
    let t0 = read_monotonic_time();
    let r = sh.call(&[
        act("stop"),
        get("task"),
        get("threads"),
        get("mm/layout"),
        get("vdso"),
        act("retire"),
    ])?;
    if r[5].status < 0 {
        ostd::early_println!(
            "derailleur: stop batch failed: {:?}",
            r.iter().map(|x| x.status).collect::<Vec<_>>()
        );
        abort(&sh);
        return_errno_with_message!(Errno::EIO, "the server refused the stop batch");
    }
    // Retired: the process is ours from here on.
    let task = record(&r[1], rec::T_TASK, tk::SIZE)?;
    let threads = record(&r[2], rec::T_THREADS, th::ENTRIES + th::XSTATE)?;
    let layout = record(&r[3], rec::T_LAYOUT, ly::VMAS)?;
    let vdso = record(&r[4], rec::T_VDSO, vd::IMAGE)?;
    let t_state = read_monotonic_time();

    // The address space: VM1's mappings at the same addresses, contents copied.
    let exe = {
        let vmar = ctx.thread_local.vmar().borrow();
        vmar.as_ref().unwrap().process_vm().executable_file().clone()
    };
    let new_vmar = derailleur_new_vmar(exe);
    let nr = u32_at(layout, ly::NR) as usize;
    if layout.len() < ly::VMAS + nr * ly::VMA_SIZE {
        return_errno_with_message!(Errno::EPROTO, "short layout record");
    }
    let mut buf = vec![0u8; PAGE_SIZE];
    let (mut pages, mut zero) = (0usize, 0usize);
    for i in 0..nr {
        let v = &layout[ly::VMAS + i * ly::VMA_SIZE..];
        let (start, end) = (u64_at(v, ly::V_START) as usize, u64_at(v, ly::V_END) as usize);
        let prot = u32_at(v, ly::V_PROT);
        let shared_map = u32_at(v, ly::V_KIND) == ly::KIND_SHARED;
        new_vmar
            .new_map(end - start, perms_of(prot))?
            .may_perms(VmPerms::ALL_MAY_PERMS)
            .is_shared(shared_map)
            .offset(VmarMapOffset::FixedReplace(start))
            .build()?;
        let mut addr = start;
        while addr < end {
            let n = ((end - addr) / PAGE_SIZE).min(sl::PAGES);
            let gpas = sh.resolve(0, addr as u64, n)?;
            for (k, g) in gpas.iter().enumerate() {
                if *g == sl::GPA_NONE || *g & sl::GPA_ZERO != 0 {
                    zero += 1;
                    continue; // nothing there: zero on first touch
                }
                let paddr = (*g & !sl::GPA_FLAGS) as usize;
                sh.peer.read_bytes(paddr, &mut buf)?;
                let mut reader = VmReader::from(&buf[..]).to_fallible();
                new_vmar
                    .write_alien(addr + k * PAGE_SIZE, &mut reader)
                    .map_err(|(e, _)| e)?;
                pages += 1;
            }
            addr += n * PAGE_SIZE;
        }
    }

    map_vdso_stubs(&new_vmar, vdso)?;

    // glibc's rseq area: nobody updates it here; make sched_getcpu() ask the kernel.
    let t0x = &threads[th::ENTRIES..];
    let rseq = u64_at(t0x, th::RSEQ_PTR) as usize;
    if rseq != 0 {
        let failed = (-2i32).to_le_bytes(); // RSEQ_CPU_ID_REGISTRATION_FAILED
        let mut reader = VmReader::from(&failed[..]).to_fallible();
        let _ = new_vmar.write_alien(rseq + 4, &mut reader);
    }

    // VM1 may forget the process now: everything was copied.
    let _ = sh.call(&[act("release")]);

    // Become it.
    let range = |a: usize, b: usize| -> Range<Vaddr> {
        let (a, b) = (u64_at(task, a) as usize, u64_at(task, b) as usize);
        a..b.max(a)
    };
    let brk = u64_at(task, tk::BRK) as usize;
    let start_brk = u64_at(task, tk::START_BRK) as usize;
    derailleur_activate_vmar(
        ctx,
        new_vmar,
        range(tk::START_CODE, tk::END_CODE),
        range(tk::START_DATA, tk::END_DATA),
        start_brk..brk.max(start_brk).next_multiple_of(PAGE_SIZE).max(start_brk + PAGE_SIZE),
    );

    // CPU state.
    let reg = |i: usize| u64_at(t0x, th::REGS + i * 8) as usize;
    {
        let g = user_ctx.general_regs_mut();
        g.r15 = reg(0);
        g.r14 = reg(1);
        g.r13 = reg(2);
        g.r12 = reg(3);
        g.rbp = reg(4);
        g.rbx = reg(5);
        g.r11 = reg(6);
        g.r10 = reg(7);
        g.r9 = reg(8);
        g.r8 = reg(9);
        g.rax = reg(10);
        g.rcx = reg(11);
        g.rdx = reg(12);
        g.rsi = reg(13);
        g.rdi = reg(14);
        g.rip = reg(15);
        g.rflags = reg(16);
        g.rsp = reg(17);
    }
    let supp = ctx.thread_local.supp_user_context();
    supp.fs_base().set(FsBase::new(reg(18)));
    supp.gs_base().set(GsBase::new(reg(19)));
    let xsize = u32_at(t0x, th::XSTATE_SIZE) as usize;
    let mut fpu = FpuContext::new();
    {
        let area = fpu.as_bytes_mut();
        let n = xsize.min(area.len()).min(t0x.len().saturating_sub(th::XSTATE));
        area[..n].copy_from_slice(&t0x[th::XSTATE..th::XSTATE + n]);
        if n >= 528 {
            // XSTATE_BV: only x87, SSE, AVX and AVX-512 (what Asterinas enables);
            // XCOMP_BV: standard format.
            let bv = u64::from_le_bytes(area[512..520].try_into().unwrap()) & 0xe7;
            area[512..520].copy_from_slice(&bv.to_le_bytes());
            area[520..528].fill(0);
        }
    }
    supp.fpu().set(fpu);

    // Thread and signal state.
    ctx.set_sig_mask(SigMask::from(u64_at(t0x, th::BLOCKED)));
    let robust = u64_at(t0x, th::ROBUST_LIST) as usize;
    ctx.thread_local
        .robust_list()
        .set(if robust != 0 { Some(robust) } else { None });
    ctx.thread_local
        .clear_child_tid()
        .set(u64_at(t0x, th::CLEAR_CHILD_TID) as usize);

    let t_end = read_monotonic_time();
    let us = |d: Duration| d.as_micros();
    ostd::early_println!(
        "derailleur: restored pid {} (VM1 tid {}): {} vmas, {} pages copied, {} empty; \
         stop+state {} us, memory {} us",
        ctx.process.pid(),
        u32_at(t0x, th::TID) as i32,
        nr,
        pages,
        zero,
        us(t_state - t0),
        us(t_end - t_state)
    );
    Ok(())
}

/// libc cached pointers into VM1's vDSO.  This kernel can't run that image
/// (its data page is Linux's), so map, at the same address and size, a page
/// with a syscall stub at each function the image exports: `mov $nr, %eax;
/// syscall; ret`.  Everything else traps (int3).  Slower than a vDSO, but
/// every cached pointer still works.
fn map_vdso_stubs(vmar: &crate::vm::vmar::Vmar, rec_vdso: &[u8]) -> Result<()> {
    use xmas_elf::{ElfFile, sections::SectionData, symbol_table::Entry};

    let text = u64_at(rec_vdso, vd::TEXT) as usize;
    let size = u64_at(rec_vdso, vd::SIZE) as usize;
    if text == 0 || size == 0 || rec_vdso.len() < vd::IMAGE + size {
        return Ok(());
    }
    let image = &rec_vdso[vd::IMAGE..vd::IMAGE + size];
    let len = size.next_multiple_of(PAGE_SIZE);
    let mut page = vec![0xccu8; len];
    let mut stubs = 0;
    if let Ok(elf) = ElfFile::new(image)
        && let Some(sh) = elf.find_section_by_name(".dynsym")
        && let Ok(SectionData::DynSymbolTable64(syms)) = sh.get_data(&elf)
    {
        for sym in syms {
            let Ok(name) = sym.get_name(&elf) else { continue };
            let nr: u32 = match name.trim_start_matches("__vdso_") {
                "clock_gettime" => 228,
                "gettimeofday" => 96,
                "time" => 201,
                "clock_getres" => 229,
                "getcpu" => 309,
                "getrandom" => 318,
                _ => continue,
            };
            let at = sym.value() as usize;
            if at + 8 > len {
                continue;
            }
            let [a, b, ..] = nr.to_le_bytes();
            page[at..at + 8].copy_from_slice(&[0xb8, a, b, 0, 0, 0x0f, 0x05, 0xc3]);
            stubs += 1;
        }
    }
    vmar.new_map(len, VmPerms::READ | VmPerms::EXEC)?
        .may_perms(VmPerms::ALL_MAY_PERMS)
        .offset(VmarMapOffset::FixedReplace(text))
        .build()?;
    let mut reader = VmReader::from(&page[..]).to_fallible();
    vmar.write_alien(text, &mut reader).map_err(|(e, _)| e)?;
    ostd::early_println!("derailleur: vDSO at {:#x}: {} syscall stubs", text, stubs);
    Ok(())
}
