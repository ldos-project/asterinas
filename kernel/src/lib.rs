// SPDX-License-Identifier: MPL-2.0

//! Aster-nix is the Asterinas kernel, a safe, efficient unix-like
//! operating system kernel built on top of OSTD and OSDK.

#![no_std]
#![no_main]
#![deny(unsafe_code)]
#![feature(btree_cursors)]
#![feature(debug_closure_helpers)]
#![feature(extend_one)]
#![feature(fn_traits)]
#![feature(int_roundings)]
#![feature(linked_list_cursors)]
#![feature(linked_list_remove)]
#![feature(linked_list_retain)]
#![feature(negative_impls)]
#![feature(panic_can_unwind)]
#![feature(register_tool)]
#![feature(step_trait)]
#![feature(trait_alias)]
#![feature(associated_type_defaults)]
#![feature(closure_track_caller)]
#![register_tool(component_access_control)]

use aster_framebuffer::FRAMEBUFFER_CONSOLE;
use kcmdline::KCmdlineArg;
use mariposa_data_capture::{
    DataCaptureDevice, DataCaptureDeviceServer, FileDescriptor, ObserverRegistration,
};
use mariposa_data_capture::legacy::{
    DataCaptureDevice as LegacyDataCaptureDevice, DataCaptureDeviceServer as LegacyDataCaptureDeviceServer, FileDescriptor as LegacyFileDescriptor, ObserverRegistration as LegacyObserverRegistration,
};
use ostd::{
    arch::qemu::{QemuExitCode, exit_qemu},
    boot::boot_info,
    cpu::{CpuId, CpuSet},
    orpc::oqueue::{OQueueBase, ObservationQuery},
    orpc::legacy_oqueue::OQueue,
    path,
};
use process::{Process, spawn_init_process};
use sched::SchedPolicy;

use crate::{
    kcmdline::set_kernel_cmd_line,
    prelude::*,
    thread::kernel_thread::ThreadOptions,
    vm::vmar::{set_huge_mapping_enabled, set_huge_mapping_preserve_on_dontneed},
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
#[cfg(not(baseline_asterinas))]
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

mod benchmarks;

#[ostd::main]
#[controlled]
pub fn main() {
    // CRITICAL: Initialize scheduler and ORPC BEFORE component initialization!
    // - Scheduler must be injected first, otherwise OSTD's fallback FIFO scheduler gets used
    // - ORPC spawn function must be injected, otherwise ORPC threads are created without
    //   Thread association and are ignored by the ClassScheduler
    thread::init();
    sched::init();
    #[cfg(not(baseline_asterinas))]
    orpc_utils::init();

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
    util::random::init();
    driver::init();
    time::init();
    #[cfg(target_arch = "x86_64")]
    net::init();
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

    let huge_mapping_preserve_on_dontneed = karg
        .get_module_arg_by_name::<bool>("vm", "huge_mapping_preserve_on_dontneed")
        .unwrap_or(false);
    set_huge_mapping_preserve_on_dontneed(huge_mapping_preserve_on_dontneed);

    println!("huge_mapping_enabled={}", huge_mapping_enabled);
    println!(
        "huge_mapping_preserve_on_dontneed={}",
        huge_mapping_preserve_on_dontneed
    );

    #[cfg(target_arch = "x86_64")]
    net::lazy_init();
    fs::lazy_init();
    ipc::init();

    vm::vmar::init();

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

    // Run benchmarks when bench.name is set in the kernel args
    if karg.get_module_args("bench").is_some() {
        benchmarks::BenchmarkHarness::run(&karg);
        exit_qemu(QemuExitCode::Success);
    }

    let initproc = spawn_init_process(
        karg.get_initproc_path().unwrap(),
        karg.get_initproc_argv().to_vec(),
        karg.get_initproc_envp().to_vec(),
    )
    .expect("Run init process failed.");

    #[cfg(not(baseline_asterinas))]
    if karg
        .get_module_arg_by_name::<bool>("vm", "hugepaged_enabled")
        .unwrap_or(false)
    {
        vm::hugepaged::HugepagedServer::spawn(initproc.clone());
    }

    let mut finalizers: Vec<Box<dyn Fn() -> ()>> = vec![];

    #[cfg(target_arch = "x86_64")]
    #[cfg(not(baseline_asterinas))]
    if karg
        .get_module_arg_by_name::<bool>("pmu", "dtlb_enabled")
        .unwrap_or(false)
    {
        let pmu = arch::pmu::PMUServer::spawn();
        pmu.reset();
        pmu.start();

        let device = fs::start_block_device("data0").unwrap();
        println!("[datadisk] 0 online");
        let dcdserver = DataCaptureDeviceServer::new(device.clone());
        let path = path!(test_capture);
        let builder = dcdserver
            .new_file(FileDescriptor {
                length: 1024 * 1024 * 1024,
                path: path.clone(),
            })
            .unwrap();
        let server = builder.build();
        let attachment = ObserverRegistration {
            path,
            observer: pmu
                .dtlb_miss_count_oq
                .attach_strong_observer(ObservationQuery::new(|x| *x))
                .unwrap(),
        };
        server.register_observer(attachment).unwrap();
        server.set_capturing(true).unwrap();
        finalizers.push(Box::new(move || {
            println!("Flushing pmu capture");
            server.flush().unwrap();
            server.sync().unwrap();
        }));
    }

    {
        let pagefault_oq = vm::vmar::oqueues::get_page_fault_oqueue();
        let device = fs::start_block_device("data1").unwrap();
        println!("[datadisk] 1 online");
        let dcdserver = LegacyDataCaptureDeviceServer::new(device.clone());
        let builder = dcdserver
            .new_file(LegacyFileDescriptor {
                length: 1024 * 1024 * 1024,
            })
            .unwrap();
        let server = builder.build();
        let attachment = LegacyObserverRegistration {
            observer: pagefault_oq
                .attach_strong_observer()
                .unwrap(),
        };
        server.register_observer(attachment).unwrap();
        server.set_capturing(true).unwrap();
        finalizers.push(Box::new(move || {
            println!("Flushing pagefault capture");
            server.flush().unwrap();
            server.sync().unwrap();
        }));
    }
    {
        let rss_oq = vm::vmar::oqueues::get_rss_delta_oqueue();
        let device = fs::start_block_device("data2").unwrap();
        println!("[datadisk] 2 online");
        let dcdserver = LegacyDataCaptureDeviceServer::new(device.clone());
        let builder = dcdserver
            .new_file(LegacyFileDescriptor {
                length: 1024 * 1024 * 1024,
            })
            .unwrap();
        let server = builder.build();
        let attachment = LegacyObserverRegistration {
            observer: rss_oq
                .attach_strong_observer()
                .unwrap(),
        };
        server.register_observer(attachment).unwrap();
        server.set_capturing(true).unwrap();
        finalizers.push(Box::new(move || {
            println!("Flushing rss capture");
            server.flush().unwrap();
            server.sync().unwrap();
        }));
    }
    {
        let connect_oq = syscall::get_accept_oq();
        let device = fs::start_block_device("data3").unwrap();
        println!("[datadisk] 3 online");
        let dcdserver = LegacyDataCaptureDeviceServer::new(device.clone());
        let builder = dcdserver
            .new_file(LegacyFileDescriptor {
                length: 1024 * 1024 * 1024,
            })
            .unwrap();
        let server = builder.build();
        server.set_flush_always(true).unwrap();
        let attachment = LegacyObserverRegistration {
            observer: connect_oq
                .attach_strong_observer()
                .unwrap(),
        };
        server.register_observer(attachment).unwrap();
        server.set_capturing(true).unwrap();
        finalizers.push(Box::new(move || {
            println!("Flushing connect capture");
            server.flush().unwrap();
            server.sync().unwrap();
        }));
    }

    // Wait till initproc become zombie.
    while !initproc.status().is_zombie() {
        ostd::task::halt_cpu();
    }

    for f in finalizers {
        f();
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
