// SPDX-License-Identifier: MPL-2.0

//! Round-trip latency of the kernel -> user -> kernel path over OQFS.
//!
//! A kernel thread produces a request into one OQueue and blocks; the userspace peer
//! (`oqbench_server`) replies into a second OQueue; the kernel thread wakes on the reply. Four
//! timestamps per iteration split each round trip into its transport and scheduler parts. The
//! samples are buffered in memory during the run and written to the data capture device once it is
//! over, so nothing but the round trip itself is measured.
//!
//! Inert unless enabled with `oqbench.enable` on the kernel command line.

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use aster_logger::println;
use mariposa_data_capture::DataCaptureFile;
use ostd::{
    arch::read_tsc,
    orpc::{
        oqueue::{
            ConsumableOQueue as _, ConsumableOQueueRef, Consumer, OQueue as _, OQueueBase as _,
            OQueueRef, RefProducer, registry,
        },
        sync::{BlockOnMany, Blocker, TimeoutBlocker},
    },
    timer::TIMER_FREQ,
};
use serde::Serialize;

use crate::framework::{PREFIX, report};

/// The name this benchmark reports itself under.
const NAME: &str = "oqueue_roundtrip";

/// Poll interval while waiting for the userspace peer to attach (ms).
const ATTACH_POLL_MS: u32 = 50;

/// Bounded wait for the userspace peer to attach before giving up (ms).
const ATTACH_TIMEOUT_MS: u32 = 60_000;

/// Interval between "still waiting for the userspace peer" progress warnings (ms).
const ATTACH_WARN_MS: u32 = 2_000;

/// Master switch (`oqbench.enable`); the benchmark is inert unless set.
static ENABLE: AtomicBool = AtomicBool::new(false);
aster_cmdline::define_flag_param!("oqbench.enable", ENABLE);

/// Measured iteration count (`oqbench.iterations`).
static ITERATIONS: AtomicU32 = AtomicU32::new(1_000_000);
aster_cmdline::define_kv_param!("oqbench.iterations", ITERATIONS);

/// Per-reply timeout in milliseconds (`oqbench.timeout_ms`); a timeout is fatal.
static TIMEOUT_MS: AtomicU32 = AtomicU32::new(10_000);
aster_cmdline::define_kv_param!("oqbench.timeout_ms", TIMEOUT_MS);

/// Request OQueue capacity (`oqbench.request_capacity`).
static REQUEST_CAPACITY: AtomicU32 = AtomicU32::new(2);
aster_cmdline::define_kv_param!("oqbench.request_capacity", REQUEST_CAPACITY);

/// Reply OQueue capacity (`oqbench.reply_capacity`).
static REPLY_CAPACITY: AtomicU32 = AtomicU32::new(2);
aster_cmdline::define_kv_param!("oqbench.reply_capacity", REPLY_CAPACITY);

/// Run the kernel thread under real-time scheduling (`oqbench.realtime`) instead of the fair policy.
static REALTIME: AtomicBool = AtomicBool::new(false);
aster_cmdline::define_flag_param!("oqbench.realtime", REALTIME);

/// Real-time priority for the kernel thread when `oqbench.realtime` is set (`oqbench.rt_prio`,
/// `1..=99`).
static RT_PRIO: AtomicU32 = AtomicU32::new(50);
aster_cmdline::define_kv_param!("oqbench.rt_prio", RT_PRIO);

/// The userspace peer's synthetic work per request, in TSC cycles (`oqbench.peer_compute`). Consumed
/// in userspace; registered here only so it is a recognized parameter and can be reported.
static PEER_COMPUTE: AtomicU32 = AtomicU32::new(0);
aster_cmdline::define_kv_param!("oqbench.peer_compute", PEER_COMPUTE);

/// Number of competing busy-loop processes during the run (`oqbench.busy_procs`). Consumed in
/// userspace; registered here only so it is a recognized parameter and can be reported.
static BUSY_PROCS: AtomicU32 = AtomicU32::new(0);
aster_cmdline::define_kv_param!("oqbench.busy_procs", BUSY_PROCS);

/// The scheduling policy for the kernel thread, applied by the kernel crate when it spawns it.
#[derive(Clone, Copy, Debug)]
pub enum DriverSchedulingPolicy {
    Normal,
    RealTime { rt_prio: u8 },
}

/// What the kernel asks the peer to do next.
///
/// Exactly one of the two terminal variants ends every run, and the peer's exit status follows it, so
/// a run that failed can never be mistaken for one that finished.
#[derive(Clone, Copy, Debug, Serialize)]
enum Request {
    /// Time one round trip, carrying its sequence number.
    Measure(u64),
    /// Every sample is captured; the peer may shut the machine down.
    Finished,
    /// The run failed and the reason is on the console; the peer should shut down and say so.
    Failed,
}

/// The four intervals one round trip decomposes into, in TSC cycles. This is the record written to
/// the capture file, one per measured iteration.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct RoundTripSample {
    /// The whole round trip, `t3 - t0`.
    roundtrip: u64,
    /// Waking the userspace peer, `t1 - t0`.
    kernel_to_user: u64,
    /// The peer's own work, `t2 - t1`.
    compute: u64,
    /// Waking the kernel thread again, `t3 - t2`.
    user_to_kernel: u64,
}

/// The effective run configuration, snapshotted from the cmdline parameters and reported verbatim so
/// a results file can be matched back to the run that produced it.
#[derive(Clone, Copy, Debug)]
struct Config {
    iterations: u32,
    timeout_ms: u32,
    request_capacity: u32,
    reply_capacity: u32,
    scheduling_policy: DriverSchedulingPolicy,
    // These two are acted on in userspace; the kernel carries them only so they appear in the
    // reported configuration.
    #[expect(dead_code, reason = "read through the derived `Debug`")]
    peer_compute: u32,
    #[expect(dead_code, reason = "read through the derived `Debug`")]
    busy_procs: u32,
}

impl Config {
    /// Snapshots the parameters, along with the first thing wrong with them, if anything.
    ///
    /// A bad parameter is not reported here. The run has to reach the point where a peer is listening
    /// before it can tell anyone the run failed, so the complaint is carried until then; see
    /// [`OQueueRoundTrip::run`].
    fn from_params() -> (Self, Option<&'static str>) {
        let mut problem = None;

        let scheduling_policy = match RT_PRIO.load(Ordering::Relaxed) {
            _ if !REALTIME.load(Ordering::Relaxed) => DriverSchedulingPolicy::Normal,
            rt_prio if (1..=99).contains(&rt_prio) => DriverSchedulingPolicy::RealTime {
                rt_prio: rt_prio as u8,
            },
            _ => {
                problem = Some("oqbench.rt_prio must be in 1..=99");
                DriverSchedulingPolicy::Normal
            }
        };

        let iterations = ITERATIONS.load(Ordering::Relaxed);
        if iterations == 0 {
            problem = problem.or(Some("oqbench.iterations must be non-zero"));
        }

        let config = Self {
            iterations,
            timeout_ms: TIMEOUT_MS.load(Ordering::Relaxed),
            request_capacity: REQUEST_CAPACITY.load(Ordering::Relaxed),
            reply_capacity: REPLY_CAPACITY.load(Ordering::Relaxed),
            scheduling_policy,
            peer_compute: PEER_COMPUTE.load(Ordering::Relaxed),
            busy_procs: BUSY_PROCS.load(Ordering::Relaxed),
        };
        (config, problem)
    }
}

/// The prepared kernel side of one benchmark run: the effective configuration and the two OQueue
/// endpoints. Holding the producer and consumer keeps both queues (and their OQFS exports) alive for
/// the whole run.
pub struct OQueueRoundTrip {
    config: Config,
    /// What is wrong with the parameters, if anything; reported once a peer can hear it.
    config_problem: Option<&'static str>,
    producer: RefProducer<Request>,
    consumer: Consumer<[u64; 3]>,
}

/// Prepares a benchmark run, or returns `None` if `oqbench.enable` is not set.
///
/// The OQueues are created even for an unusable configuration, so the peer always has something to
/// attach to and always learns how the run ended instead of waiting forever.
pub fn prepare() -> Option<OQueueRoundTrip> {
    if !ENABLE.load(Ordering::Relaxed) {
        return None;
    }

    let (config, config_problem) = Config::from_params();
    let (producer, consumer) = setup_queues(config.request_capacity, config.reply_capacity);
    Some(OQueueRoundTrip {
        config,
        config_problem,
        producer,
        consumer,
    })
}

/// Creates and exports the two OQueues on the `oqbench` OQFS subtree: the request queue
/// (kernel -> user, exported as `strong_observe`) and the reply queue (user -> kernel, exported as
/// `produce`). The request queue also carries the terminal [`Request`] that ends the run.
fn setup_queues(
    request_capacity: u32,
    reply_capacity: u32,
) -> (RefProducer<Request>, Consumer<[u64; 3]>) {
    let request_path = ostd::path!(oqbench.request);
    let request_oqueue = OQueueRef::<Request>::new(request_capacity as usize, request_path.clone());
    registry::register(&request_path, &request_oqueue.as_any_oqueue());
    let request_producer = request_oqueue
        .attach_ref_producer()
        .expect("the oqbench request OQueue always allows a ref producer");

    let reply_path = ostd::path!(oqbench.reply);
    let reply_oqueue =
        ConsumableOQueueRef::<[u64; 3]>::new(reply_capacity as usize, reply_path.clone());
    registry::register_producible(&reply_path, &reply_oqueue);
    let reply_consumer = reply_oqueue
        .attach_consumer()
        .expect("the oqbench reply OQueue always allows attaching its consumer");

    (request_producer, reply_consumer)
}

/// Blocks (without spinning) until the peer is attached or not, whichever `want` asks for, or until
/// the bounded wait elapses. Returns whether the wait was satisfied.
fn wait_for_peer(producer: &RefProducer<Request>, want: PeerState, waiting_for: &str) -> bool {
    let satisfied_fn = || producer.has_observers() == matches!(want, PeerState::Attached);
    let poll = TimeoutBlocker::new();
    let max_polls = ATTACH_TIMEOUT_MS / ATTACH_POLL_MS;
    let warn_every = (ATTACH_WARN_MS / ATTACH_POLL_MS).max(1);
    for poll_count in 0..max_polls {
        if satisfied_fn() {
            return true;
        }
        if poll_count != 0 && poll_count % warn_every == 0 {
            let waited_ms = poll_count * ATTACH_POLL_MS;
            println!(
                "oqbench: waiting for the userspace peer to {waiting_for} ({waited_ms}ms of {ATTACH_TIMEOUT_MS}ms elapsed)"
            );
        }
        poll.arm_after(ATTACH_POLL_MS as u64 * TIMER_FREQ / 1000);
        if let Some(task) = ostd::task::Task::current() {
            let blockers: [&dyn Blocker; 1] = [&*poll];
            task.block_on(&blockers);
        }
        poll.disarm();
    }
    satisfied_fn()
}

/// Which side of the peer's lifetime [`wait_for_peer`] should wait for.
enum PeerState {
    Attached,
    Detached,
}

impl OQueueRoundTrip {
    /// The scheduling policy the kernel thread should run under.
    pub fn scheduling_policy(&self) -> DriverSchedulingPolicy {
        self.config.scheduling_policy
    }

    /// Runs the benchmark and then tells the peer how it went.
    ///
    /// Must run on a dedicated kernel thread, not the boot thread, because it blocks on replies.
    /// Nothing here stops the machine: every path ends by sending the peer either [`Request::Finished`]
    /// or [`Request::Failed`], and the peer owns the shutdown.
    pub fn run(self, capture_file: Arc<dyn DataCaptureFile<RoundTripSample>>) {
        let Self {
            config,
            config_problem,
            producer,
            consumer,
        } = self;

        let outcome = measure(&config, config_problem, &producer, &consumer, capture_file);
        producer.produce_ref(&match outcome {
            Ok(()) => Request::Finished,
            Err(()) => Request::Failed,
        });

        // Returning drops the OQueues, which revokes the peer's observer, and a revoked observer
        // reads as end of stream. The peer treats that as a failed run, so hold the queues open until
        // it has taken the verdict and detached; otherwise a finished run could report as a failure.
        wait_for_peer(&producer, PeerState::Detached, "take the result");
    }
}

/// Measures every iteration and captures the samples, or reports why it could not.
///
/// `Err` means the reason has already been written to the console; the caller only has to pass the
/// verdict on to the peer.
fn measure(
    config: &Config,
    config_problem: Option<&'static str>,
    producer: &RefProducer<Request>,
    consumer: &Consumer<[u64; 3]>,
    capture_file: Arc<dyn DataCaptureFile<RoundTripSample>>,
) -> Result<(), ()> {
    // Wait for the peer before complaining about anything, so there is somebody to hear the verdict.
    if !wait_for_peer(producer, PeerState::Attached, "attach") {
        println!(
            "{PREFIX}|error oqbench: the userspace peer did not attach within {ATTACH_TIMEOUT_MS}ms"
        );
        return Err(());
    }

    if let Some(problem) = config_problem {
        println!("{PREFIX}|error oqbench: {problem}");
        return Err(());
    }

    // Reserve the whole sample array up front so recording never allocates on the hot path.
    let iterations = config.iterations as usize;
    let mut samples: Vec<RoundTripSample> = Vec::new();
    if samples.try_reserve_exact(iterations).is_err() {
        println!("{PREFIX}|error oqbench: cannot reserve room for {iterations} samples");
        return Err(());
    }

    let timeout = TimeoutBlocker::new();
    let timeout_jiffies = config.timeout_ms as u64 * TIMER_FREQ / 1000;
    let mut block_on_many = BlockOnMany::new();

    for seq in 0..config.iterations as u64 {
        // A value consumable before this iteration's request is produced is a stale reply.
        if let Some(stale) = consumer.try_consume() {
            println!(
                "{PREFIX}|error oqbench: stale reply in the queue before producing seq {seq} (its seq={})",
                stale[0]
            );
            return Err(());
        }

        let t0 = read_tsc();
        producer.produce_ref(&Request::Measure(seq));
        timeout.arm_after(timeout_jiffies);

        let (t3, reply) = loop {
            if let Some(reply) = consumer.try_consume() {
                // Stamp t3 before any loop-exit work so its overhead is not counted.
                let t3 = read_tsc();
                if reply[0] != seq {
                    println!(
                        "{PREFIX}|error oqbench: out-of-sequence reply: expected seq {seq}, got seq {}",
                        reply[0]
                    );
                    return Err(());
                }
                break (t3, reply);
            }
            if timeout.should_try() {
                let elapsed = read_tsc().wrapping_sub(t0);
                println!(
                    "{PREFIX}|error oqbench: reply timeout at seq {seq} after {}ms ({elapsed} cycles)",
                    config.timeout_ms
                );
                return Err(());
            }
            let blockers: [&dyn Blocker; 2] = [consumer, &*timeout];
            block_on_many.block_on(blockers.into_iter());
        };
        timeout.disarm();

        let (t1, t2) = (reply[1], reply[2]);
        samples.push(RoundTripSample {
            roundtrip: t3.wrapping_sub(t0),
            kernel_to_user: t1.wrapping_sub(t0),
            compute: t2.wrapping_sub(t1),
            user_to_kernel: t3.wrapping_sub(t2),
        });
    }

    let measured = samples.len();
    if capture_file
        .write_values(Box::new(samples.into_iter()))
        .is_err()
    {
        println!("{PREFIX}|error oqbench: could not write the samples");
        return Err(());
    }
    // Stopping the capture file syncs it and only returns once the server thread is done, so the
    // samples are on the device before the report claims the run succeeded.
    if capture_file.stop().is_err() {
        println!("{PREFIX}|error oqbench: could not sync the samples");
        return Err(());
    }

    report(NAME, config, measured);
    Ok(())
}
