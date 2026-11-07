// SPDX-License-Identifier: MPL-2.0

//! The cassette client: the calling (single-threaded) process becomes the
//! process VM1 offered.
//!
//! The `derailleur restore` tool drives it in two steps:
//! 1. `take` (syscall 1003): open the session, stop the process on VM1, and
//!    return its open files as neutral records (`fds`).  The tool recreates
//!    them at the same numbers with ordinary syscalls.
//! 2. `restore` (syscall 1001): take the rest (task, threads, layout, vDSO),
//!    retire VM1's copy, build the address space, recreate the threads, and
//!    return to user mode as the process.
//!
//! Prototype limits: pages are copied out of VM1's RAM (seen through the peer
//! BAR) into ordinary memory here, and VM1 is released right after; lending
//! frames in place needs frame metadata for the BAR (an OSTD change).  No
//! premap.  Established connections are not moved (recreated unconnected).

use alloc::vec;
use core::{ops::Range, time::Duration};

use aster_time::read_monotonic_time;
use ostd::{
    arch::cpu::context::{FpuContext, FsBase, GsBase, UserContext},
    mm::VmIo,
};

use super::{
    device::{Req, Resp, Shared, shared},
    wire::{
        self, layout as ly, rec, slot as sl, task as tk, thread as th, u32_at, u64_at, vdso as vd,
    },
};
use crate::{
    fs::vfs::path::FsPath,
    prelude::*,
    process::{
        derailleur_activate_vmar, derailleur_new_vmar, derailleur_spawn_thread,
        posix_thread::{ContextPthreadAdminApi, ThreadName},
        signal::{
            SigStack, SigStackFlags,
            c_types::sigaction_t,
            constants::{SIGKILL, SIGSTOP},
            sig_action::SigAction,
            sig_mask::SigMask,
            sig_num::SigNum,
        },
    },
    thread::AsThread,
    vm::{
        perms::VmPerms,
        vmar::{Vmar, VmarMapOffset},
    },
};

/// The session's progress: `take` stopped the process.
static STOPPED: Mutex<bool> = Mutex::new(false);

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
    *STOPPED.lock() = false;
}

/// Step 1: stop the process on VM1; copy its `fds` record to `buf`.
/// Returns the record's length (the tool's buffer must hold it).
pub fn take(ctx: &Context, buf: Vaddr, buf_len: usize) -> Result<usize> {
    let sh = shared()?;
    sh.open()?;
    let r = sh.call(&[get("proc"), act("prepare")])?;
    record(&r[0], rec::T_PROC, 32)?;
    if r[1].status < 0 {
        abort(&sh);
        return_errno_with_message!(Errno::EIO, "prepare failed on the server");
    }
    // The downtime starts here.
    let r = sh.call(&[act("net.hold"), act("stop"), act("net.cut"), get("fds")])?;
    if r[1].status < 0 || r[3].status < 0 {
        abort(&sh);
        return_errno_with_message!(Errno::EIO, "the server could not stop the process");
    }
    *STOPPED.lock() = true;
    let fds = &r[3].value;
    if fds.len() > buf_len {
        abort(&sh);
        return_errno_with_message!(Errno::ENOSPC, "fds record larger than the buffer");
    }
    ctx.user_space().write_bytes(buf, fds)?;
    Ok(fds.len())
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

/// One thread's state from its `cas_thread` entry.
struct ThreadState {
    tid: u32,
    regs: [usize; 20],
    fpu: FpuContext,
    blocked: u64,
    clear_child_tid: usize,
    robust_list: usize,
    rseq: usize,
    ss: (usize, usize, u32),
    comm: [u8; 16],
}

fn thread_state(e: &[u8]) -> ThreadState {
    let mut regs = [0usize; 20];
    for (i, r) in regs.iter_mut().enumerate() {
        *r = u64_at(e, th::REGS + i * 8) as usize;
    }
    let xsize = u32_at(e, th::XSTATE_SIZE) as usize;
    let mut fpu = FpuContext::new();
    {
        let area = fpu.as_bytes_mut();
        let n = xsize.min(area.len()).min(e.len().saturating_sub(th::XSTATE));
        area[..n].copy_from_slice(&e[th::XSTATE..th::XSTATE + n]);
        if n >= 528 {
            // XSTATE_BV: only x87, SSE, AVX and AVX-512 (what Asterinas enables);
            // XCOMP_BV: standard format.
            let bv = u64::from_le_bytes(area[512..520].try_into().unwrap()) & 0xe7;
            area[512..520].copy_from_slice(&bv.to_le_bytes());
            area[520..528].fill(0);
        }
    }
    let mut comm = [0u8; 16];
    comm.copy_from_slice(&e[th::COMM..th::COMM + 16]);
    ThreadState {
        tid: u32_at(e, th::TID),
        regs,
        fpu,
        blocked: u64_at(e, th::BLOCKED),
        clear_child_tid: u64_at(e, th::CLEAR_CHILD_TID) as usize,
        robust_list: u64_at(e, th::ROBUST_LIST) as usize,
        rseq: u64_at(e, th::RSEQ_PTR) as usize,
        ss: (
            u64_at(e, th::SS_SP) as usize,
            u64_at(e, th::SS_SIZE) as usize,
            u32_at(e, th::SS_FLAGS),
        ),
        comm,
    }
}

/// The general registers of a `cas_x86_regs` (order: r15 r14 r13 r12 bp bx
/// r11 r10 r9 r8 ax cx dx si di ip flags sp fsbase gsbase).
fn set_regs(user_ctx: &mut UserContext, r: &[usize; 20]) {
    let g = user_ctx.general_regs_mut();
    g.r15 = r[0];
    g.r14 = r[1];
    g.r13 = r[2];
    g.r12 = r[3];
    g.rbp = r[4];
    g.rbx = r[5];
    g.r11 = r[6];
    g.r10 = r[7];
    g.r9 = r[8];
    g.r8 = r[9];
    g.rax = r[10];
    g.rcx = r[11];
    g.rdx = r[12];
    g.rsi = r[13];
    g.rdi = r[14];
    g.rip = r[15];
    g.rflags = r[16];
    g.rsp = r[17];
}

/// Step 2: become the offered process.  On success the caller returns to user
/// mode as its leader (`user_ctx` is replaced) and the other threads run.
pub fn restore(ctx: &Context, user_ctx: &mut UserContext) -> Result<()> {
    let sh = shared()?;
    if !*STOPPED.lock() {
        // Called without `take` (no files to recreate): stop it here.
        sh.open()?;
        let r = sh.call(&[get("proc"), act("prepare"), act("stop")])?;
        if r[1].status < 0 || r[2].status < 0 {
            abort(&sh);
            return_errno_with_message!(Errno::EIO, "the server could not stop the process");
        }
    }
    let t0 = read_monotonic_time();
    let r = sh.call(&[
        get("task"),
        get("threads"),
        get("mm/layout"),
        get("vdso"),
        act("retire"),
    ])?;
    if r[4].status < 0 {
        ostd::early_println!(
            "derailleur: state batch failed: {:?}",
            r.iter().map(|x| x.status).collect::<Vec<_>>()
        );
        abort(&sh);
        return_errno_with_message!(Errno::EIO, "the server refused the state batch");
    }
    *STOPPED.lock() = false;
    // Retired: the process is ours from here on.
    let task = record(&r[0], rec::T_TASK, tk::SIZE)?;
    let threads = record(&r[1], rec::T_THREADS, th::ENTRIES + th::XSTATE)?;
    let layout = record(&r[2], rec::T_LAYOUT, ly::VMAS)?;
    let vdso = record(&r[3], rec::T_VDSO, vd::IMAGE)?;
    let t_state = read_monotonic_time();

    // Threads, leader first.
    let nr_threads = u32_at(threads, th::NR) as usize;
    let mut states = Vec::with_capacity(nr_threads);
    let mut at = th::ENTRIES;
    for _ in 0..nr_threads {
        let len = u32_at(threads, at) as usize;
        if len < th::XSTATE || at + len > threads.len() {
            return_errno_with_message!(Errno::EPROTO, "bad thread entry");
        }
        states.push(thread_state(&threads[at..at + len]));
        at += len;
    }
    if states.is_empty() {
        return_errno_with_message!(Errno::EPROTO, "no threads");
    }

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
    let mut buf = vec![0u8; sl::PAGES * PAGE_SIZE];
    let (mut pages, mut zero, mut lent) = (0usize, 0usize, 0usize);
    // derailleur.copy on the command line: copy everything (the old way).
    let lend = super::device::peer_memory().filter(|_| !super::NO_LEND.load(core::sync::atomic::Ordering::Relaxed));
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
            // Pages only the process had on VM1: map VM1's frame here (lent).
            // The rest: runs of present pages, gathered and copied.
            let lendable = |g: u64| {
                lend.is_some() && !shared_map && g != sl::GPA_NONE && g & sl::GPA_MOVABLE != 0
            };
            let copyable = |g: u64| g != sl::GPA_NONE && g & sl::GPA_ZERO == 0 && !lendable(g);
            let mut k = 0;
            while k < n {
                let g = gpas[k];
                if lendable(g) {
                    let frame = lend.unwrap().frame((g & !sl::GPA_FLAGS) as usize)?;
                    new_vmar.derailleur_install_frame(addr + k * PAGE_SIZE, frame)?;
                    lent += 1;
                    k += 1;
                    continue;
                }
                if !copyable(g) {
                    zero += 1;
                    k += 1;
                    continue; // nothing there: zero on first touch
                }
                let run = k;
                while k < n && copyable(gpas[k]) {
                    let paddr = (gpas[k] & !sl::GPA_FLAGS) as usize;
                    let at = (k - run) * PAGE_SIZE;
                    sh.peer.read_bytes(paddr, &mut buf[at..at + PAGE_SIZE])?;
                    k += 1;
                }
                let mut reader = VmReader::from(&buf[..(k - run) * PAGE_SIZE]).to_fallible();
                new_vmar
                    .write_alien(addr + run * PAGE_SIZE, &mut reader)
                    .map_err(|(e, _)| e)?;
                pages += k - run;
            }
            addr += n * PAGE_SIZE;
        }
    }
    map_vdso_stubs(&new_vmar, vdso)?;

    // glibc's rseq areas: nobody updates them here; make sched_getcpu() ask the kernel.
    for st in &states {
        if st.rseq != 0 {
            let failed = (-2i32).to_le_bytes(); // RSEQ_CPU_ID_REGISTRATION_FAILED
            let mut reader = VmReader::from(&failed[..]).to_fallible();
            let _ = new_vmar.write_alien(st.rseq + 4, &mut reader);
        }
    }

    // Copied only: VM1 may forget the process now.  Lent: it keeps the pages
    // pinned until `done` (the process exited and every lent frame is gone).
    if lent == 0 {
        let _ = sh.call(&[act("release")]);
    } else {
        *super::LENT.lock() = true;
    }

    // Process-wide state: signal dispositions, umask, cwd.
    {
        let dispositions = ctx.process.sig_dispositions().lock();
        let mut dispositions = dispositions.lock();
        for i in 0..64u8 {
            let Ok(num) = SigNum::try_from(i + 1) else {
                continue;
            };
            if num == SIGKILL || num == SIGSTOP {
                continue;
            }
            let at = tk::SIGACT + i as usize * tk::SIGACT_SIZE;
            let sa = SigAction::from(<sigaction_t as Pod>::from_bytes(&task[at..at + 32]));
            let _ = dispositions.set(num, sa);
        }
    }
    if let Ok(mask) = (u32_at(task, tk::UMASK) as u16 & 0o777).try_into() {
        ctx.thread_local.borrow_fs().swap_umask(mask);
    }
    {
        let cwd = &task[tk::CWD..tk::CWD + 256];
        let len = cwd.iter().position(|&b| b == 0).unwrap_or(256);
        if let Ok(name) = core::str::from_utf8(&cwd[..len])
            && let Ok(fs_path) = FsPath::try_from(name)
        {
            let fs_ref = ctx.thread_local.borrow_fs();
            let mut resolver = fs_ref.resolver().write();
            if let Ok(path) = resolver.lookup(&fs_path) {
                resolver.set_cwd(path);
            }
        }
    }

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
        (
            u64_at(task, tk::START_STACK) as usize,
            range(tk::ARG_START, tk::ARG_END),
            range(tk::ENV_START, tk::ENV_END),
        ),
    );

    // The other threads (their user context starts as a copy of ours).
    let mut tasks = Vec::new();
    for st in states.iter().skip(1) {
        let mut uc = user_ctx.clone();
        set_regs(&mut uc, &st.regs);
        tasks.push(derailleur_spawn_thread(
            ctx,
            uc,
            st.tid,
            st.fpu.clone(),
            st.regs[18],
            st.regs[19],
            SigMask::from(st.blocked),
            st.clear_child_tid,
            &st.comm,
        )?);
    }

    // The leader: this thread.
    let lead = &states[0];
    set_regs(user_ctx, &lead.regs);
    let supp = ctx.thread_local.supp_user_context();
    supp.fs_base().set(FsBase::new(lead.regs[18]));
    supp.gs_base().set(GsBase::new(lead.regs[19]));
    supp.fpu().set(lead.fpu.clone());
    ctx.set_sig_mask(SigMask::from(lead.blocked));
    ctx.thread_local.robust_list().set(if lead.robust_list != 0 {
        Some(lead.robust_list)
    } else {
        None
    });
    ctx.thread_local.clear_child_tid().set(lead.clear_child_tid);
    if lead.ss.1 != 0 {
        *ctx.thread_local.sig_stack().borrow_mut() = SigStack::new(
            lead.ss.0,
            SigStackFlags::from_bits_truncate(lead.ss.2),
            lead.ss.1,
        );
    }
    *ctx.posix_thread.thread_name().lock() = ThreadName::from_bytes_until_nul(&lead.comm);

    for t in &tasks {
        t.as_thread().unwrap().run();
    }

    let t_end = read_monotonic_time();
    let us = |d: Duration| d.as_micros();
    ostd::early_println!(
        "derailleur: restored pid {} (VM1 pid {}): {} threads, {} vmas, {} pages lent, {} copied, \
         {} empty; state {} us, memory+threads {} us",
        ctx.process.pid(),
        lead.tid as i32,
        states.len(),
        nr,
        lent,
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
fn map_vdso_stubs(vmar: &Vmar, rec_vdso: &[u8]) -> Result<()> {
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

/// The session's end (syscall 1002, after the process exited): once no lent
/// frame is referenced here any more, VM1 may unpin them.
pub fn done() -> Result<()> {
    if !core::mem::take(&mut *super::LENT.lock()) {
        return Ok(());
    }
    let sh = shared()?;
    let t0 = read_monotonic_time();
    while ostd::mm::frame::peer::PeerMemory::live_frames() != 0 {
        if read_monotonic_time() - t0 > Duration::from_secs(10) {
            ostd::early_println!(
                "derailleur: {} lent frames still referenced; not releasing VM1",
                ostd::mm::frame::peer::PeerMemory::live_frames()
            );
            return_errno!(Errno::EBUSY);
        }
        ostd::task::Task::yield_now();
    }
    let _ = sh.call(&[act("release")]);
    Ok(())
}
