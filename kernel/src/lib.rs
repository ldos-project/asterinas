// SPDX-License-Identifier: MPL-2.0

//! Aster-nix is the Asterinas kernel, a safe, efficient unix-like
//! operating system kernel built on top of OSTD and OSDK.

#![no_std]
#![no_main]
// #![deny(unsafe_code)]
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
#![register_tool(component_access_control)]

use aster_framebuffer::FRAMEBUFFER_CONSOLE;
use kcmdline::KCmdlineArg;
use ostd::{
    arch::qemu::{QemuExitCode, exit_qemu},
    boot::boot_info,
    cpu::{CpuId, CpuSet},
    orpc::oqueue::{Consumer, OQueue, Producer},
};
use process::{Process, spawn_init_process};
use sched::SchedPolicy;

use crate::{kcmdline::set_kernel_cmd_line, prelude::*, thread::kernel_thread::ThreadOptions};

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

trait MyTrait: Any {
    fn do_work(&mut self);
}

struct MyStruct {
    calls: usize,
}

impl MyTrait for MyStruct {
    #[inline(never)]
    fn do_work(&mut self) {
        self.calls += 1;
    }
}

struct MyStruct2 {
    calls: usize,
}

impl MyTrait for MyStruct2 {
    #[inline(never)]
    fn do_work(&mut self) {
        self.calls += 2;
    }
}

#[inline(never)]
fn bench_call_overhead(s: &mut Box<MyStruct>) {
    let now = time::clocks::RealTimeClock::get().read_time();
    for _ in 0..1000000 {
        s.do_work();
    }
    let end = time::clocks::RealTimeClock::get().read_time();
    println!("1M direct calls in {:?}", end - now);
}

#[inline(never)]
fn bench_dyn_call_overhead(s: &mut Box<dyn MyTrait>) {
    let now = time::clocks::RealTimeClock::get().read_time();
    for _ in 0..1000000 {
        s.do_work();
    }
    let end = time::clocks::RealTimeClock::get().read_time();
    println!("1M dyn calls in in {:?}", end - now);
}

fn init_thread() {
    println!("[kernel] Spawn init thread");
    // Work queue should be initialized before interrupt is enabled,
    // in case any irq handler uses work queue as bottom half
    thread::work_queue::init();

    let karg: KCmdlineArg = boot_info().kernel_cmdline.as_str().into();
    set_kernel_cmd_line(karg.clone());

    #[cfg(target_arch = "x86_64")]
    net::lazy_init();
    // fs::lazy_init();
    ipc::init();
    // driver::pci::virtio::block::block_device_test();
    let thread = ThreadOptions::new(|| {
        println!("[kernel] Hello world from kernel!");
    })
    .spawn();
    thread.join();

    print_banner();

    // FIXME: CI fails due to suspected performance issues with the framebuffer console.
    // Additionally, userspace program may render GUIs using the framebuffer,
    // so we disable the framebuffer console here.
    if let Some(console) = FRAMEBUFFER_CONSOLE.get() {
        console.disable();
    };

    const N_PRODUCERS: usize = 4;
    const N_MESSAGES: usize = 200000;
    const N_MESSAGES_PER_PRODUCER: usize = N_MESSAGES / N_PRODUCERS;

    let completed = Arc::new(core::sync::atomic::AtomicUsize::new(0));
    let completed_wq = Arc::new(ostd::sync::WaitQueue::new());

    // let q = ostd::orpc::oqueue::ringbuffer::SPSCOQueue::<u64>::new(1024, 10, 0);
    // let q = ostd::orpc::oqueue::locking::ObservableLockingQueue::<u64>::new(10, 0);
    let q = ostd::orpc::oqueue::ringbuffer::MPMCOQueue::<u64, false, false>::new(2 << 20, 0);
    //let q = ostd::orpc::oqueue::ringbuffer::mpmc::Rigtorp::<u64>::new(1024);

    println!("Starting producers");
    // Start all producers
    for tid in 0..N_PRODUCERS {
        let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
        cpu_set.add(ostd::cpu::CpuId::try_from(1).unwrap());
        ThreadOptions::new({
            let completed = completed.clone();
            let completed_wq = completed_wq.clone();
            let producer = q.attach_producer().unwrap();
            // let producer = q.clone(); //.attach_producer().unwrap();
            move || {
                let now = time::clocks::RealTimeClock::get().read_time();
                for _ in 0..N_MESSAGES_PER_PRODUCER {
                    producer.produce(0);
                }
                let end = time::clocks::RealTimeClock::get().read_time();
                println!(
                    "[producer-{}-{:?}] sent msg in {:?}",
                    tid,
                    ostd::cpu::CpuId::current_racy(),
                    end - now
                );
                completed.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                completed_wq.wake_all();
            }
        })
        .cpu_affinity(cpu_set)
        .spawn();
    }

    let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
    cpu_set.add(ostd::cpu::CpuId::try_from(1).unwrap());

    // Start consumer
    println!("Starting consumer");
    // ThreadOptions::new({
    // let consumer = q.attach_consumer().unwrap();
    //let consumer: Arc<dyn ostd::orpc::oqueue::ringbuffer::mpmc::Queue<u64>> =
    //    q.attach_consumer().unwrap();
    // let consumer_any: Box<dyn Any> = consumer;
    // let consumer: Box<ostd::orpc::oqueue::ringbuffer::MPMCConsumer<u64, false, false>> =
    //     consumer_any.downcast().unwrap();
    // let consumer = q.clone(); //.attach_consumer().unwrap();
    //move || {
    // for _ in 0..N_MESSAGES {
    //     consumer.consume();
    //     // println!("[consumer-{:?}] got msg", ostd::cpu::CpuId::current_racy());
    // }

    //}
    // })
    // .cpu_affinity(cpu_set)
    // .spawn();

    println!("Waiting for benchmark to complete");
    // Exit after benchmark completes
    completed_wq
        .wait_until(|| (completed.load(core::sync::atomic::Ordering::Relaxed) == 2).then_some(()));

    println!("done");
    exit_qemu(QemuExitCode::Success);

    let initproc = spawn_init_process(
        karg.get_initproc_path().unwrap(),
        karg.get_initproc_argv().to_vec(),
        karg.get_initproc_envp().to_vec(),
    )
    .expect("Run init process failed.");

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
