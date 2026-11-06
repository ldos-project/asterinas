// SPDX-License-Identifier: MPL-2.0

//! Kernel initialization.

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

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
    fs::{
        initramfs, rootfs,
        vfs::path::{FsPath, MountNamespace, Path, PathResolver},
    },
    prelude::*,
    process::{Process, spawn_init_process},
    sched,
    sched::SchedPolicy,
    thread,
    thread::kernel_thread::ThreadOptions,
    vm,
};

pub(super) fn main() {
    // TODO(amp, BIG MERGE): Reconsider this initialization order, and placement of initialization code.

    // CRITICAL: Initialize scheduler and ORPC BEFORE component initialization!
    // - Scheduler must be injected first, otherwise OSTD's fallback FIFO scheduler gets used
    // - ORPC spawn function must be injected, otherwise ORPC threads are created without
    //   Thread association and are ignored by the ClassScheduler
    thread::init();
    sched::init();
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

pub(super) fn on_first_process_startup(ctx: &Context) {
    component::init_all(InitStage::Process, component::parse_metadata!()).unwrap();
    crate::device::init_in_first_process().unwrap();
    crate::fs::init_in_first_process(ctx);
}

fn init() {
    crate::power::init();
    crate::arch::init();
    crate::util::random::init();
    crate::driver::init();
    crate::time::init();
    crate::net::init();
    crate::process::init();
    crate::fs::init();
    crate::security::init();
}

/// Initialize kernel subsystems for ktests. This is not a complete initialization, but does load
/// all components.
///
/// This is idempotent and should be called in *every* test that needs it. This avoids at least some
/// test order dependence.
///
/// TODO(arthurp, https://github.com/ldos-project/asterinas/issues/221): Something less ad-hoc.
#[cfg(ktest)]
pub(crate) fn init_for_ktest() {
    static INITIALIZED: Once<()> = Once::new();

    INITIALIZED.call_once(|| {
        component::init_all(InitStage::Bootstrap, component::parse_metadata!()).unwrap();
        crate::time::init();
        vm::vmar::init_in_first_kthread();
    });
}

fn init_on_each_cpu() {
    sched::init_on_each_cpu();
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
    let boot_init = prepare_boot_init(fs_resolver);

    print_banner();

    // Run benchmarks when bench.benchmark is set in the kernel args
    if benchmarks::is_benchmark_enabled() {
        benchmarks::BenchmarkHarness::run();
        poweroff(ExitCode::Success);
    }

    INIT_PROCESS.call_once(|| {
        let karg = INIT_PROC_ARGS.get().unwrap();
        let argv = karg.argv().to_vec();
        let envp = karg.envp().to_vec();
        boot_init
            .spawn(argv, envp)
            .expect("failed to run the init process")
    });

    #[cfg(not(baseline_asterinas))]
    benchmarks::oqueue_roundtrip::init_after_init_process();

    #[cfg(not(baseline_asterinas))]
    if VM_HUGEPAGED_ENABLED.load(Ordering::Relaxed) {
        vm::hugepaged::HugepagedServer::spawn(
            INIT_PROCESS.get().expect("initialed already").clone(),
        );
    }

    #[cfg(target_arch = "x86_64")]
    #[cfg(not(baseline_asterinas))]
    if PMU_DTLB_ENABLED.load(Ordering::Relaxed) {
        let pmu = crate::arch::pmu::PmuServer::spawn();
        pmu.reset();
        pmu.start();
    }

    #[cfg(not(baseline_asterinas))]
    if SCHEDULER_CAPTURE_DATA.load(Ordering::Relaxed) {
        use ostd::orpc::oqueue::LifetimelessElementDescriptor;

        if let Some(oqueue) = lookup_by_path::<LifetimelessElementDescriptor<SchedulingEvent>>(
            &path!(scheduler.events),
        ) {
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

            if let Some(capture_file) = capture_file {
                ignore_err!(
                    capture_file.register_observer(ObserverRegistration {
                        path: path!(scheduler.events),
                        observer: oqueue
                            .attach_strong_observer(ObservationQuery::new(|e: &SchedulingEvent| {
                                let context = EventContext::new();
                                KernelSchedulingEvent {
                                    timestamp: context.timestamp,
                                    task: TaskId::new(&e.task),
                                    kind: e.kind,
                                }
                            }))
                            .unwrap(),
                    })
                );
                ignore_err!(capture_file.start());
            }
        } else {
            error!("Could not find scheduler.events OQueue. Scheduler events will not be captured.")
        }
    }

    #[cfg(not(baseline_asterinas))]
    if IO_CAPTURE_BLOCK_IO.load(Ordering::Relaxed) {
        use aster_block::bio::{BlockDeviceCompletionStats, SubmittedBio};

        use crate::data_capture::new_data_capture_data_file_by_type;

        #[derive(Clone, Copy, Serialize)]
        struct SubmittedBioEvent {
            byte_range: (usize, usize),
            timestamp: Option<Instant>,
            context: EventContext,
        }
        new_data_capture_data_file_by_type::<SubmittedBio, _>(
            path!(io.block.submitted),
            500 * 1024 * 1024,
            || {
                ObservationQuery::new(|e: &SubmittedBio| {
                    let sid_range = e.sid_range();
                    let context = EventContext::new();
                    SubmittedBioEvent {
                        byte_range: (sid_range.start.to_offset(), sid_range.end.to_offset()),
                        timestamp: e.submission_time().map(|t| t.into()),
                        context,
                    }
                })
            },
        );

        #[derive(Clone, Copy, Serialize)]
        struct BlockDeviceCompletionEvent {
            stats: BlockDeviceCompletionStats,
            context: EventContext,
        }
        new_data_capture_data_file_by_type::<BlockDeviceCompletionStats, _>(
            path!(io.block.completion),
            500 * 1024 * 1024,
            || {
                ObservationQuery::new(|stats| {
                    let context = EventContext::new();
                    BlockDeviceCompletionEvent {
                        stats: *stats,
                        context,
                    }
                })
            },
        );
    }
}

struct BootInit {
    path_resolver: PathResolver,
    init_path: Option<(Path, &'static str)>,
}

#[repr(u8)]
enum BootSource {
    Initramfs = 0,
    Rootfs = 1,
}

pub(crate) fn booted_from_rootfs() -> bool {
    BOOT_SOURCE.load(Ordering::Relaxed) == BootSource::Rootfs as u8
}

impl BootInit {
    fn spawn(self, argv: Vec<CString>, envp: Vec<CString>) -> Result<Arc<Process>> {
        let Self {
            path_resolver,
            init_path,
        } = self;

        // Only rootfs boot may have no init path; initramfs boot always provides one.
        let Some((init_path, init_name)) = init_path else {
            return spawn_default_rootfs_init(path_resolver, argv, envp);
        };

        println!("[kernel] running {} as the init process", init_name);
        spawn_init_process(
            path_resolver,
            init_path,
            with_init_argv0(init_name, argv),
            envp,
        )
    }
}

fn prepare_boot_init(mut path_resolver: PathResolver) -> BootInit {
    if let Ok(init_path) = initramfs::find_init(&path_resolver) {
        BOOT_SOURCE.store(BootSource::Initramfs as u8, Ordering::Relaxed);
        return BootInit {
            path_resolver,
            init_path: Some(init_path),
        };
    }

    rootfs::switch_to_rootfs(&mut path_resolver)
        .expect("neither an initramfs init nor a usable root filesystem was available");
    let init_path = rootfs::find_init(&path_resolver).expect("failed to resolve rootfs init path");
    BOOT_SOURCE.store(BootSource::Rootfs as u8, Ordering::Relaxed);
    BootInit {
        path_resolver,
        init_path,
    }
}

fn spawn_default_rootfs_init(
    path_resolver: PathResolver,
    argv: Vec<CString>,
    envp: Vec<CString>,
) -> Result<Arc<Process>> {
    // Linux probes the fallback init executables in this order:
    // Reference: <https://elixir.bootlin.com/linux/v6.19/source/init/main.c#L1634>.
    const DEFAULT_INIT_EXEC_PATHS: &[&str] = &["/sbin/init", "/etc/init", "/bin/init", "/bin/sh"];

    let mut last_error = None;

    for &init_name in DEFAULT_INIT_EXEC_PATHS {
        // FIXME: Avoid cloning `argv` and `envp` for each fallback candidate.
        let init_path = match path_resolver.lookup(&FsPath::try_from(init_name).unwrap()) {
            Ok(path) => path,
            Err(error) => {
                last_error = Some(error);
                continue;
            }
        };

        match spawn_init_process(
            path_resolver.clone(),
            init_path,
            with_init_argv0(init_name, argv.clone()),
            envp.clone(),
        ) {
            Ok(process) => {
                println!("[kernel] running {} as the rootfs init", init_name);
                return Ok(process);
            }
            Err(error) => last_error = Some(error),
        }
    }

    Err(last_error.unwrap())
}

fn with_init_argv0(init_name: &str, mut argv: Vec<CString>) -> Vec<CString> {
    // Linux prepends the init executable path as `argv[0]`.
    // Reference: <https://elixir.bootlin.com/linux/v6.19/source/init/main.c#L1491>.
    argv.insert(0, CString::new(init_name).unwrap());
    argv
}

static INIT_PROCESS: Once<Arc<Process>> = Once::new();

pub(crate) fn get_init_process() -> Option<&'static Arc<Process>> {
    INIT_PROCESS.get()
}

fn init_in_first_kthread(path_resolver: &PathResolver) {
    component::init_all(InitStage::Kthread, component::parse_metadata!()).unwrap();
    // Initializes the work queue and the other kernel threads' state. This must happen
    // before interrupts are enabled, in case any IRQ handler uses the work queue as a
    // bottom half.
    thread::init_in_first_kthread();
    crate::fs::init_in_first_kthread(path_resolver);

    #[cfg(not(baseline_asterinas))]
    {
        data_capture::start_capture_devices();
    }

    crate::device::init_in_first_kthread();
    crate::net::init_in_first_kthread();
    #[cfg(not(target_arch = "loongarch64"))]
    crate::vdso::init_in_first_kthread();
    vm::vmar::init_in_first_kthread();
    crate::syscall::init_in_first_kthread();
}

fn print_banner() {
    println!("");
    println!("{}", logo_ascii_art::get_gradient_color_version());
}

static BOOT_SOURCE: AtomicU8 = AtomicU8::new(BootSource::Initramfs as u8);

/// Run hugepaged.
static VM_HUGEPAGED_ENABLED: AtomicBool = AtomicBool::new(false);
aster_cmdline::define_flag_param!("vm.hugepaged_enabled", VM_HUGEPAGED_ENABLED);

/// Capture scheduling events.
static SCHEDULER_CAPTURE_DATA: AtomicBool = AtomicBool::new(false);
aster_cmdline::define_flag_param!("scheduler.capture_data", SCHEDULER_CAPTURE_DATA);

/// Capture block device I/O operations.
static IO_CAPTURE_BLOCK_IO: AtomicBool = AtomicBool::new(false);
aster_cmdline::define_flag_param!("io.capture_block_io", IO_CAPTURE_BLOCK_IO);

/// Produce TLB counter statistics into an OQueue.
#[cfg(target_arch = "x86_64")]
static PMU_DTLB_ENABLED: AtomicBool = AtomicBool::new(false);
#[cfg(target_arch = "x86_64")]
aster_cmdline::define_flag_param!("pmu.dtlb_enabled", PMU_DTLB_ENABLED);
