// SPDX-License-Identifier: MPL-2.0

//! Aster-nix is the Asterinas kernel, a safe, efficient unix-like
//! operating system kernel built on top of OSTD and OSDK.

#![no_std]
#![no_main]
#![deny(unsafe_code)]
#![feature(btree_cursors)]
#![feature(btree_extract_if)]
#![feature(debug_closure_helpers)]
#![feature(extend_one)]
#![feature(extract_if)]
#![feature(fn_traits)]
#![feature(format_args_nl)]
#![feature(int_roundings)]
#![feature(integer_sign_cast)]
#![feature(let_chains)]
#![feature(linked_list_cursors)]
#![feature(linked_list_remove)]
#![feature(linked_list_retain)]
#![feature(negative_impls)]
#![feature(panic_can_unwind)]
#![feature(register_tool)]
#![feature(step_trait)]
#![feature(trait_alias)]
#![feature(trait_upcasting)]
#![feature(associated_type_defaults)]
#![feature(closure_track_caller)]
#![register_tool(component_access_control)]

use core::sync::atomic::{AtomicUsize, Ordering};

use aster_framebuffer::FRAMEBUFFER_CONSOLE;
use kcmdline::{KCmdlineArg, get_kernel_cmd_line};
use ostd::{
    arch::qemu::{QemuExitCode, exit_qemu},
    boot::boot_info,
    cpu::{CpuId, CpuSet},
    orpc::oqueue::{
        Consumer, Cursor, OQueue, Producer, StrongObserver, WeakObserver, ringbuffer::MPMCOQueue,
    },
};
use process::{Process, spawn_init_process};
use sched::SchedPolicy;

use crate::{
    kcmdline::set_kernel_cmd_line, prelude::*, thread::kernel_thread::ThreadOptions,
    vm::vmar::set_huge_mapping_enabled,
};

extern crate alloc;
extern crate lru;
#[macro_use]
extern crate controlled;
#[macro_use]
extern crate getset;

#[cfg(target_arch = "x86_64")]
#[path = "arch/x86/mod.rs"]
pub mod arch;
#[cfg(target_arch = "riscv64")]
#[path = "arch/riscv/mod.rs"]
pub mod arch;
pub mod context;
pub mod cpu;
pub mod device;
pub mod driver;
pub mod error;
pub mod events;
pub mod fs;
pub mod ipc;
pub mod kcmdline;
pub mod net;
pub(crate) mod orpc_utils;
pub mod prelude;
mod process;
mod sched;
pub mod syscall;
pub mod thread;
pub mod time;
mod util;
pub(crate) mod vdso;
pub mod vm;

#[ostd::main]
#[controlled]
pub fn main() {
    ostd::early_println!("[kernel] OSTD initialized. Preparing components.");
    component::init_all(component::parse_metadata!()).unwrap();
    init();

    // Spawn all AP idle threads.
    ostd::boot::smp::register_ap_entry(ap_init);

    // Spawn the first kernel thread on BSP.
    let mut affinity = CpuSet::new_empty();
    affinity.add(CpuId::bsp());
    ThreadOptions::new(init_thread)
        .cpu_affinity(affinity)
        .sched_policy(SchedPolicy::Idle)
        .spawn();
}

pub fn init() {
    thread::init();
    util::random::init();
    driver::init();
    time::init();
    #[cfg(target_arch = "x86_64")]
    net::init();
    sched::init();
    fs::rootfs::init(boot_info().initramfs.expect("No initramfs found!")).unwrap();
    device::init().unwrap();
    syscall::init();
    vdso::init();
    process::init();
}

fn ap_init() {
    fn ap_idle_thread() {
        log::info!(
            "Kernel idle thread for CPU #{} started.",
            // No races because `ap_idle_thread` runs on a certain AP.
            CpuId::current_racy().as_usize(),
        );

        loop {
            ostd::task::halt_cpu();
        }
    }

    ThreadOptions::new(ap_idle_thread)
        // No races because `ap_init` runs on a certain AP.
        .cpu_affinity(CpuId::current_racy().into())
        .sched_policy(SchedPolicy::Idle)
        .spawn();
}

fn init_thread() {
    println!("[kernel] Spawn init thread");
    // Work queue should be initialized before interrupt is enabled,
    // in case any irq handler uses work queue as bottom half
    thread::work_queue::init();

    let karg: KCmdlineArg = boot_info().kernel_cmdline.as_str().into();
    set_kernel_cmd_line(karg.clone());

    let huge_mapping_enabled = karg
        .get_module_arg_by_name::<bool>("vm", "huge_mapping_enabled")
        .unwrap_or(false);
    set_huge_mapping_enabled(huge_mapping_enabled);

    #[cfg(target_arch = "x86_64")]
    net::lazy_init();
    fs::lazy_init();
    ipc::init();
    // driver::pci::virtio::block::block_device_test();
    let thread = ThreadOptions::new(|| {
        println!("[kernel] Hello world from kernel!");
    })
    .spawn();
    thread.join();

    print_banner();

    benchmark();
    exit_qemu(QemuExitCode::Success);

    // FIXME: CI fails due to suspected performance issues with the framebuffer console.
    // Additionally, userspace program may render GUIs using the framebuffer,
    // so we disable the framebuffer console here.
    if let Some(console) = FRAMEBUFFER_CONSOLE.get() {
        console.disable();
    };

    let initproc = spawn_init_process(
        karg.get_initproc_path().unwrap(),
        karg.get_initproc_argv().to_vec(),
        karg.get_initproc_envp().to_vec(),
    )
    .expect("Run init process failed.");

    if karg
        .get_module_arg_by_name::<bool>("vm", "hugepaged_enabled")
        .unwrap_or(false)
    {
        let hugepaged = vm::HugepagedServer::new().unwrap();
        let initproc = initproc.clone();

        ThreadOptions::new(move || hugepaged.main(initproc)).spawn();
    }
    // Wait till initproc become zombie.
    while !initproc.status().is_zombie() {
        ostd::task::halt_cpu();
    }

    // TODO: exit via qemu isa debug device should not be the only way.
    let exit_code = if initproc.status().exit_code() == 0 {
        QemuExitCode::Success
    } else {
        QemuExitCode::Failed
    };
    exit_qemu(exit_code);
}

fn benchmark() {
    // large number of producers pushing a fixed # of msgs with:
    //  1 consumer + 0 sobs + 0 wobs
    //  0 consumer + 1 sobs + 0 wobs
    //  0 consumer + 0 sobs + 1 wobs
    // measure producer throughput (and latency?)A

    const N_MESSAGES_PER_PRODUCER: usize = 1024 * 1024;

    let n_threads = get_kernel_cmd_line()
        .unwrap()
        .get_module_arg_by_name::<usize>("bench", "n_threads")
        .unwrap_or(2);
    let n_producers: usize = n_threads - 1;

    let test = get_kernel_cmd_line()
        .unwrap()
        .get_module_arg_by_name::<String>("bench", "type")
        .unwrap_or("consumer".to_string());

    let completed = Arc::new(AtomicUsize::new(0));
    let completed_wq = Arc::new(ostd::sync::WaitQueue::new());

    fn producer_thread(q: Box<dyn Producer<()>>) {
        for _ in 0..N_MESSAGES_PER_PRODUCER {
            q.produce(());
        }
    }
    let consumer_thread = move |q: Vec<Box<dyn Consumer<()>>>| {
        let mut n_recv = 0;
        while n_recv < (n_producers * N_MESSAGES_PER_PRODUCER) {
            for c in &q {
                if c.try_consume().is_some() {
                    n_recv += 1;
                }
            }
        }
    };
    let strong_obs_thread = move |q: Vec<Box<dyn StrongObserver<()>>>| {
        let mut n_recv = 0;
        while n_recv < (n_producers * N_MESSAGES_PER_PRODUCER) {
            for c in &q {
                if c.try_strong_observe().is_some() {
                    n_recv += 1;
                }
            }
        }
    };
    let weak_obs_thread = {
        let completed = completed.clone();
        move |q: Vec<Box<dyn WeakObserver<()>>>| {
            let mut cursors = Vec::<Cursor>::new();
            for c in &q {
                cursors.push(c.oldest_cursor());
            }

            let mut n_recv = 0;
            while completed.load(Ordering::Relaxed) < n_producers
                && n_recv < (n_producers * N_MESSAGES_PER_PRODUCER)
            {
                for (c, cursor) in q.iter().zip(cursors.iter_mut()) {
                    while c.weak_observe(*cursor).is_some() {
                        cursor.advance();
                        n_recv += 1;
                    }
                    *cursor = c.recent_cursor();
                }
            }
            println!(
                "Weak Observer, observed {}/{}  msgs",
                n_recv,
                (n_producers * N_MESSAGES_PER_PRODUCER)
            );
        }
    };

    println!("Starting producers");
    let barrier = Arc::new(AtomicUsize::new(n_threads));

    let mut queues: Vec<Arc<dyn OQueue<()>>> = Vec::new();
    for _ in 0..(n_threads - 1) {
        queues.push(MPMCOQueue::<(), true, true>::new(1024, 1));
    }

    // Start all producers
    for tid in 0..(n_threads - 1) {
        let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
        cpu_set.add(ostd::cpu::CpuId::try_from(tid + 1).unwrap());
        ThreadOptions::new({
            let barrier = barrier.clone();
            let completed = completed.clone();
            let completed_wq = completed_wq.clone();
            let producer = queues[tid].attach_producer().unwrap();
            move || {
                barrier.fetch_sub(1, Ordering::Acquire);
                while barrier.load(Ordering::Relaxed) > 0 {}
                let now = time::clocks::RealTimeClock::get().read_time();
                println!(
                    "[producer-{}-{:?}] start",
                    tid,
                    ostd::cpu::CpuId::current_racy()
                );
                producer_thread(producer);
                let end = time::clocks::RealTimeClock::get().read_time();
                println!(
                    "[producer-{}-{:?}] sent msg in {:?}",
                    tid,
                    ostd::cpu::CpuId::current_racy(),
                    end - now
                );
                completed.fetch_add(1, Ordering::Relaxed);
                completed_wq.wake_all();
            }
        })
        .cpu_affinity(cpu_set)
        .spawn();
    }

    if test == "consumer" {
        // Start conumser
        let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
        cpu_set.add(ostd::cpu::CpuId::try_from(n_threads).unwrap());
        ThreadOptions::new({
            let barrier = barrier.clone();
            let completed = completed.clone();
            let completed_wq = completed_wq.clone();
            let handles = queues
                .iter()
                .map(|q| q.attach_consumer().unwrap())
                .collect();
            move || {
                barrier.fetch_sub(1, Ordering::Acquire);
                while barrier.load(Ordering::Relaxed) > 0 {}
                let now = time::clocks::RealTimeClock::get().read_time();
                consumer_thread(handles);
                let end = time::clocks::RealTimeClock::get().read_time();
                println!(
                    "[consumer-{:?}] recv msg in {:?}",
                    ostd::cpu::CpuId::current_racy(),
                    end - now
                );
                completed.fetch_add(1, Ordering::Relaxed);
                completed_wq.wake_all();
            }
        })
        .cpu_affinity(cpu_set)
        .spawn();
    } else if test == "strong_obs" {
        // Start strong observer
        let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
        cpu_set.add(ostd::cpu::CpuId::try_from(n_threads).unwrap());
        ThreadOptions::new({
            let barrier = barrier.clone();
            let completed = completed.clone();
            let completed_wq = completed_wq.clone();
            let handles = queues
                .iter()
                .map(|q| q.attach_strong_observer().unwrap())
                .collect();
            move || {
                barrier.fetch_sub(1, Ordering::Acquire);
                while barrier.load(Ordering::Relaxed) > 0 {}
                let now = time::clocks::RealTimeClock::get().read_time();
                strong_obs_thread(handles);
                let end = time::clocks::RealTimeClock::get().read_time();
                println!(
                    "[consumer-{:?}] recv msg in {:?}",
                    ostd::cpu::CpuId::current_racy(),
                    end - now
                );
                completed.fetch_add(1, Ordering::Relaxed);
                completed_wq.wake_all();
            }
        })
        .cpu_affinity(cpu_set)
        .spawn();
    } else {
        // Start weak observer
        let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
        cpu_set.add(ostd::cpu::CpuId::try_from(n_threads).unwrap());
        ThreadOptions::new({
            let barrier = barrier.clone();
            let completed = completed.clone();
            let completed_wq = completed_wq.clone();
            let handles = queues
                .iter()
                .map(|q| q.attach_weak_observer().unwrap())
                .collect();
            move || {
                barrier.fetch_sub(1, Ordering::Acquire);
                while barrier.load(Ordering::Relaxed) > 0 {}
                let now = time::clocks::RealTimeClock::get().read_time();
                weak_obs_thread(handles);
                let end = time::clocks::RealTimeClock::get().read_time();
                println!(
                    "[consumer-{:?}] recv msg in {:?}",
                    ostd::cpu::CpuId::current_racy(),
                    end - now
                );
                completed.fetch_add(1, Ordering::Relaxed);
                completed_wq.wake_all();
            }
        })
        .cpu_affinity(cpu_set)
        .spawn();
    };

    println!("Waiting for benchmark to complete");
    // Exit after benchmark completes
    completed_wq.wait_until(|| (completed.load(Ordering::Relaxed) == n_threads).then_some(()));
    let end = time::clocks::RealTimeClock::get().read_time();
}

fn print_banner() {
    println!("\x1B[36m");
    println!(
        r"
   _   ___ _____ ___ ___ ___ _  _   _   ___
  /_\ / __|_   _| __| _ \_ _| \| | /_\ / __|
 / _ \\__ \ | | | _||   /| || .` |/ _ \\__ \
/_/ \_\___/ |_| |___|_|_\___|_|\_/_/ \_\___/
"
    );
    println!("\x1B[0m");
}
