// SPDX-License-Identifier: MPL-2.0

//! Kernel initialization.

use aster_cmdline::INIT_PROC_ARGS;
use aster_time::Instant;
use component::InitStage;
#[cfg(not(baseline_asterinas))]
use mariposa_data_capture::ObserverRegistration;
#[cfg(not(baseline_asterinas))]
use ostd::orpc::oqueue::registry::lookup_by_path;
#[cfg(not(baseline_asterinas))]
use ostd::orpc::oqueue::{OQueueBase, ObservationQuery};
#[cfg(not(baseline_asterinas))]
use ostd::path;
use ostd::{
    boot::boot_info,
    cpu::CpuId,
    ignore_err,
    power::{ExitCode, poweroff},
    task::scheduler::{SchedulingEvent, SchedulingEventKind},
    util::id_set::Id,
};
use serde::Serialize;
use spin::once::Once;

#[cfg(not(baseline_asterinas))]
use crate::data_capture::{self, new_data_capture_file};
#[cfg(not(baseline_asterinas))]
use crate::event::{EventContext, TaskId};
use crate::{
    benchmarks,
    fs::vfs::path::{MountNamespace, PathResolver},
    kcmdline,
    kcmdline::{KCmdlineArg, set_kernel_cmd_line},
    prelude::*,
    process::{Process, spawn_init_process},
    sched::SchedPolicy,
    thread::{self, kernel_thread::ThreadOptions},
    vm::{
        self,
        vmar::huge_pages::{set_huge_mapping_enabled, set_huge_mapping_preserve_on_dontneed},
    },
};

pub(super) fn main() {
    // TODO(amp, BIG MERGE): Reconsider this initialization order, and placement of initialization code.

    // CRITICAL: Initialize scheduler and ORPC BEFORE component initialization!
    // - Scheduler must be injected first, otherwise OSTD's fallback FIFO scheduler gets used
    // - ORPC spawn function must be injected, otherwise ORPC threads are created without
    //   Thread association and are ignored by the ClassScheduler
    crate::thread::init();
    crate::sched::init();
    #[cfg(not(baseline_asterinas))]
    crate::orpc_utils::init();

    // Initialize the global states for all CPUs.
    ostd::early_println!("OSTD initialized. Preparing components.");
    component::init_all(InitStage::Bootstrap, component::parse_metadata!()).unwrap();
    init();

    // Initialize the per-CPU states for BSP.
    init_on_each_cpu();

    // Enable APs.
    ostd::boot::smp::register_ap_entry(ap_init);

    // Give the control of the BSP to the idle thread.
    ThreadOptions::new(bsp_idle_loop)
        .cpu_affinity(CpuId::bsp().into())
        .sched_policy(SchedPolicy::Idle)
        .spawn();
}

fn init() {
    crate::arch::init();
    crate::util::random::init();
    crate::driver::init();
    crate::time::init();
    crate::net::init();
    crate::process::init();
    crate::fs::init();
    crate::security::init();
}

fn init_on_each_cpu() {
    crate::sched::init_on_each_cpu();
    crate::process::init_on_each_cpu();
    crate::fs::init_on_each_cpu();
    crate::time::init_on_each_cpu();
}

fn ap_init() {
    // Initialize the per-CPU states for AP.
    init_on_each_cpu();

    ThreadOptions::new(ap_idle_loop)
        // No races because `ap_init` runs on a certain AP.
        .cpu_affinity(CpuId::current_racy().into())
        .sched_policy(SchedPolicy::Idle)
        .spawn();
}

//--------------------------------------------------------------------------
// Per-CPU idle threads
//--------------------------------------------------------------------------

// Note: Keep the code in the idle loop to the bare minimum.
//
// We do not want the idle loop to
// rely on the APIs of other kernel subsystems for two reasons.
// First, the idle task must never sleep or block.
// This property is relied upon by the scheduler.
// Second, the idle task is spawned before the kernel is fully initialized.
// So other subsystems may not be ready, yet.
//
// In addition,
// doing more work in the idle task may have negative impact on
// the latency to switching from the idle task to a useful, runnable one.

fn bsp_idle_loop() {
    ostd::info!("Idle thread for CPU #0 started");

    // Spawn the first non-idle kernel thread on BSP.
    ThreadOptions::new(first_kthread)
        .cpu_affinity(CpuId::bsp().into())
        .sched_policy(SchedPolicy::default())
        .spawn();

    // Wait till the init process is spawned.
    let init_process = loop {
        if let Some(init_process) = INIT_PROCESS.get() {
            break init_process;
        };

        ostd::task::halt_cpu();
    };

    // Wait till the init process becomes zombie.
    while !init_process.status().is_zombie() {
        ostd::task::halt_cpu();
    }

    // TODO(amp, BIG MERGE): This may or may not run depending on how shutdown is now handled.
    #[cfg(not(baseline_asterinas))]
    for f in data_capture::DATA_CAPTURE_FILE_FINALIZERS.lock().drain(..) {
        f();
    }

    panic!(
        "The init process terminates with code {:?}",
        init_process.status().exit_code()
    );
}

fn ap_idle_loop() {
    ostd::info!(
        "Idle thread for CPU #{} started",
        // No races because this function runs on a certain AP.
        CpuId::current_racy().as_usize(),
    );

    loop {
        ostd::task::halt_cpu();
    }
}

//--------------------------------------------------------------------------
// The first kernel thread
//--------------------------------------------------------------------------

// The main function of the first (non-idle) kernel thread
fn first_kthread() {
    println!("Spawn the first kernel thread");

    let init_mnt_ns = MountNamespace::get_init_singleton();
    let fs_resolver = init_mnt_ns.new_path_resolver();
    init_in_first_kthread(&fs_resolver);

    print_banner();

    // Run benchmarks when bench.name is set in the kernel args
    if kcmdline::get_kernel_cmd_line()
        .expect("no kernel command line")
        .get_module_args("bench")
        .is_some()
    {
        benchmarks::BenchmarkHarness::run(
            kcmdline::get_kernel_cmd_line().expect("no kernel command line"),
        );
        poweroff(ExitCode::Success);
    }

    INIT_PROCESS.call_once(|| {
        let karg = INIT_PROC_ARGS.get().unwrap();
        let init_path = INIT_PATH.get().map(|s| s.as_str());
        spawn_init_process(init_path, karg.argv().to_vec(), karg.envp().to_vec())
            .expect("Failed to run the init process")
    });

    #[cfg(not(baseline_asterinas))]
    if kcmdline::get_kernel_cmd_line()
        .expect("no kernel command line")
        .get_module_arg_by_name::<bool>("vm", "hugepaged_enabled")
        .unwrap_or(false)
    {
        vm::hugepaged::HugepagedServer::spawn(
            INIT_PROCESS.get().expect("initialed already").clone(),
        );
    }

    #[cfg(target_arch = "x86_64")]
    #[cfg(not(baseline_asterinas))]
    if kcmdline::get_kernel_cmd_line()
        .expect("no kernel command line")
        .get_module_arg_by_name::<bool>("pmu", "dtlb_enabled")
        .unwrap_or(false)
    {
        let pmu = crate::arch::pmu::PmuServer::spawn();
        pmu.reset();
        pmu.start();
    }

    #[cfg(not(baseline_asterinas))]
    if kcmdline::get_kernel_cmd_line()
        .expect("no kernel command line")
        .get_module_arg_by_name::<bool>("scheduler", "capture_data")
        .unwrap_or(false)
    {
        if let Some(oqueue) = lookup_by_path::<SchedulingEvent>(&path!(scheduler.events)) {
            #[derive(Debug, Clone, Copy, Serialize)]
            struct KernelSchedulingEvent {
                timestamp: Instant,
                task: TaskId,
                kind: SchedulingEventKind,
            }

            let capture_file = new_data_capture_file::<KernelSchedulingEvent>(
                mariposa_data_capture::FileDescriptor {
                    path: path!(scheduler.events),
                    length: 500 * 1024 * 1024,
                },
            );

            ignore_err!(
                capture_file.register_observer(ObserverRegistration {
                    path: path!(scheduler.events),
                    observer: oqueue
                        .attach_strong_observer(ObservationQuery::new(|e: &SchedulingEvent| {
                            let context = EventContext::new();
                            KernelSchedulingEvent {
                                timestamp: context.timestamp,
                                kind: e.kind,
                                task: TaskId::new(&e.task),
                            }
                        }))
                        .unwrap(),
                })
            );
            ignore_err!(capture_file.start());
        } else {
            error!("Could not find scheduler.events OQueue. Scheduler events will not be captured.")
        }
    }

    #[cfg(not(baseline_asterinas))]
    if kcmdline::get_kernel_cmd_line()
        .expect("no kernel command line")
        .get_module_arg_by_name("io", "capture_block_io")
        .unwrap_or(false)
    {
        use aster_block::bio::{BlockDeviceCompletionStats, SubmittedBio};

        use crate::data_capture::new_data_capture_data_file_by_type;

        #[derive(Clone, Copy, Serialize)]
        struct SubmittedBioEvent {
            byte_range: (usize, usize),
            timestamp: Option<Instant>,
            context: EventContext,
        }
        new_data_capture_data_file_by_type(path!(io.block.submitted), 500 * 1024 * 1024, || {
            ObservationQuery::new(|e: &SubmittedBio| {
                let sid_range = e.sid_range();
                let context = EventContext::new();
                SubmittedBioEvent {
                    byte_range: (sid_range.start.to_offset(), sid_range.end.to_offset()),
                    timestamp: e.submission_time().map(|t| t.into()),
                    context,
                }
            })
        });

        #[derive(Clone, Copy, Serialize)]
        struct BlockDeviceCompletionEvent {
            stats: BlockDeviceCompletionStats,
            context: EventContext,
        }
        new_data_capture_data_file_by_type(path!(io.block.completion), 500 * 1024 * 1024, || {
            ObservationQuery::new(|stats| {
                let context = EventContext::new();
                BlockDeviceCompletionEvent {
                    stats: *stats,
                    context,
                }
            })
        });
    }
}

static INIT_PROCESS: Once<Arc<Process>> = Once::new();

pub(crate) fn get_init_process() -> Option<&'static Arc<Process>> {
    INIT_PROCESS.get()
}

fn init_in_first_kthread(path_resolver: &PathResolver) {
    component::init_all(InitStage::Kthread, component::parse_metadata!()).unwrap();
    // Work queue should be initialized before interrupt is enabled,
    // in case any irq handler uses work queue as bottom half
    crate::thread::work_queue::init_in_first_kthread();

    let karg: KCmdlineArg = boot_info().kernel_cmdline.as_str().into();
    set_kernel_cmd_line(karg.clone());

    thread::oops::configure();

    let huge_mapping_enabled = karg
        .get_module_arg_by_name::<bool>("vm", "huge_mapping_enabled")
        .unwrap_or(false);
    set_huge_mapping_enabled(huge_mapping_enabled);

    let huge_mapping_preserve_on_dontneed = karg
        .get_module_arg_by_name::<bool>("vm", "huge_mapping_preserve_on_dontneed")
        .unwrap_or(false);
    set_huge_mapping_preserve_on_dontneed(huge_mapping_preserve_on_dontneed);

    #[cfg(not(baseline_asterinas))]
    {
        data_capture::start_capture_devices();
    }

    crate::device::init_in_first_kthread();
    crate::net::init_in_first_kthread();
    crate::fs::init_in_first_kthread(path_resolver);
    #[cfg(any(target_arch = "x86_64", target_arch = "riscv64"))]
    crate::vdso::init_in_first_kthread();
    crate::vm::vmar::init_in_first_kthread();
    crate::syscall::init_in_first_kthread();
    crate::fs::vfs::init_in_first_kthread();
}

fn print_banner() {
    println!("");
    println!("{}", logo_ascii_art::get_gradient_color_version());
}

pub(super) fn on_first_process_startup(ctx: &Context) {
    component::init_all(InitStage::Process, component::parse_metadata!()).unwrap();
    crate::device::init_in_first_process(ctx).unwrap();
    crate::fs::init_in_first_process(ctx);
}

static INIT_PATH: Once<String> = Once::new();
aster_cmdline::define_kv_param!("init", INIT_PATH);
