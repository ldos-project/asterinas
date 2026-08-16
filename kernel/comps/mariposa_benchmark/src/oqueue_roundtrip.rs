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

use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use aster_logger::println;
use mariposa_data_capture::DataCaptureFile;
use ostd::{
    arch::read_tsc,
    orpc::{
        TupleSerialize,
        oqueue::{
            ConsumableOQueue as _, ConsumableOQueueRef, Consumer, OQueue as _, OQueueBase as _,
            OQueueRef, RefProducer, registry,
        },
        sync::{BlockOnMany, Blocker, TimeoutBlocker},
    },
    ostd_error,
    timer::TIMER_FREQ,
};
use serde::{Deserialize, Serialize, Serializer};
use snafu::Snafu;

use crate::framework::{self, PREFIX};

/// The name this benchmark reports itself under.
const NAME: &str = "oqueue_roundtrip";

/// Bounded wait for a signal from the userspace peer before giving up (ms).
const PEER_SIGNAL_TIMEOUT_MS: u32 = 60_000;

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

/// Which of the three things a [`Request`] is saying.
///
/// Serialized as its discriminant rather than its name, so it costs one CBOR byte instead of the
/// length of the variant's name.
#[derive(Clone, Copy, Debug)]
#[repr(u8)]
enum RequestKind {
    /// Time the round trip identified by the request's sequence number.
    Measure = 0,
    /// Every sample is captured; the peer may shut the machine down.
    Finished = 1,
    /// The run failed and the reason is on the console; the peer should shut down and say so.
    Failed = 2,
}

impl Serialize for RequestKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(*self as u8)
    }
}

/// What the kernel asks the peer to do next.
///
/// Exactly one of the two terminal kinds ends every run, and the peer's exit status follows it, so a
/// run that failed can never be mistaken for one that finished.
///
/// [`TupleSerialize`] puts this on the wire as the flat two-element array `[seq, kind]`, so neither
/// the field names nor the kind's name are repeated on every request. `seq` is meaningless for the
/// terminal kinds and is sent as zero.
#[derive(Clone, Copy, Debug, TupleSerialize)]
struct Request {
    seq: u64,
    kind: RequestKind,
}

impl Request {
    /// A request to time the round trip numbered `seq`.
    fn measure(seq: u64) -> Self {
        Self {
            seq,
            kind: RequestKind::Measure,
        }
    }

    /// The request that ends a run, reporting whether it succeeded.
    fn ending(outcome: &Result<(), Error>) -> Self {
        Self {
            seq: 0,
            kind: match outcome {
                Ok(()) => RequestKind::Finished,
                Err(_) => RequestKind::Failed,
            },
        }
    }
}

/// What the peer tells the kernel about its own lifecycle, on the control OQueue.
///
/// The kernel cannot ask an OQueue whether somebody is listening and get a useful answer -- an
/// attached observer is not the same as a peer that is ready to serve, or one that has finished
/// reading. So the peer says so explicitly, and the kernel waits for the value.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
enum PeerSignal {
    /// Attached to every stream and ready to serve requests. Nothing is measured before this.
    Ready,
    /// The run's result has been read; the queues can be torn down.
    Done,
}

/// Every way this benchmark's run can end other than by finishing.
///
/// The transparent variant absorbs the framework's own failures, so a caller has one type to report
/// and the two kinds of failure cannot drift apart in how they are handled.
#[ostd_error]
#[derive(Debug, Snafu)]
enum Error {
    #[ostd(context(source))]
    #[snafu(transparent)]
    Framework { source: framework::Error },

    #[snafu(display("oqbench.iterations must be non-zero ({context})"))]
    ZeroIterations,

    #[snafu(display("oqbench.rt_prio must be in 1..=99, got {rt_prio} ({context})"))]
    BadRealTimePriority { rt_prio: u32 },

    #[snafu(display(
        "stale reply in the queue before producing seq {seq} (its seq={stale_seq}) ({context})"
    ))]
    StaleReply { seq: u64, stale_seq: u64 },

    #[snafu(display(
        "out-of-sequence reply: expected seq {seq}, got seq {replied_seq} ({context})"
    ))]
    OutOfSequence { seq: u64, replied_seq: u64 },

    #[snafu(display(
        "reply timeout at seq {seq} after {timeout_ms}ms ({elapsed} cycles) ({context})"
    ))]
    ReplyTimeout {
        seq: u64,
        timeout_ms: u32,
        elapsed: u64,
    },

    #[snafu(display(
        "the userspace peer did not send {expected:?} within {PEER_SIGNAL_TIMEOUT_MS}ms ({context})"
    ))]
    PeerSilent { expected: PeerSignal },

    #[snafu(display(
        "the userspace peer sent {received:?} while waiting for {expected:?} ({context})"
    ))]
    UnexpectedSignal {
        expected: PeerSignal,
        received: PeerSignal,
    },
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
    /// before it can tell anyone the run failed, so the complaint is built and then carried until
    /// then; see [`OQueueRoundTrip::run`].
    fn from_params() -> (Self, Option<Error>) {
        let mut problem = None;

        let scheduling_policy = match RT_PRIO.load(Ordering::Relaxed) {
            _ if !REALTIME.load(Ordering::Relaxed) => DriverSchedulingPolicy::Normal,
            rt_prio if (1..=99).contains(&rt_prio) => DriverSchedulingPolicy::RealTime {
                rt_prio: rt_prio as u8,
            },
            rt_prio => {
                problem = Some(BadRealTimePrioritySnafu { rt_prio }.build());
                DriverSchedulingPolicy::Normal
            }
        };

        let iterations = ITERATIONS.load(Ordering::Relaxed);
        if iterations == 0 {
            problem = problem.or_else(|| Some(ZeroIterationsSnafu.build()));
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
    config_problem: Option<Error>,
    producer: RefProducer<Request>,
    consumer: Consumer<[u64; 3]>,
    signals: Consumer<PeerSignal>,
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
    let (producer, consumer, signals) =
        setup_queues(config.request_capacity, config.reply_capacity);
    Some(OQueueRoundTrip {
        config,
        config_problem,
        producer,
        consumer,
        signals,
    })
}

/// Creates and exports the three OQueues on the `oqbench` OQFS subtree: the request queue
/// (kernel -> user, exported as `strong_observe`), the reply queue (user -> kernel, exported as
/// `produce`), and the control queue the peer reports its [`PeerSignal`]s on. The request queue also
/// carries the terminal [`Request`] that ends the run.
fn setup_queues(
    request_capacity: u32,
    reply_capacity: u32,
) -> (
    RefProducer<Request>,
    Consumer<[u64; 3]>,
    Consumer<PeerSignal>,
) {
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

    // Two is enough for the whole run: the peer sends `Ready` once and `Done` once.
    let control_path = ostd::path!(oqbench.control);
    let control_oqueue = ConsumableOQueueRef::<PeerSignal>::new(2, control_path.clone());
    registry::register_producible(&control_path, &control_oqueue);
    let control_consumer = control_oqueue
        .attach_consumer()
        .expect("the oqbench control OQueue always allows attaching its consumer");

    (request_producer, reply_consumer, control_consumer)
}

/// Blocks (without spinning) until the peer sends the `expected` signal, giving up after
/// [`PEER_SIGNAL_TIMEOUT_MS`].
///
/// The blocker is local and dropped on return, so nothing disarms it: its timer callback holds only
/// a weak reference and becomes a no-op.
fn wait_for_signal(signals: &Consumer<PeerSignal>, expected: PeerSignal) -> Result<(), Error> {
    let timeout = TimeoutBlocker::new();
    let mut block_on_many = BlockOnMany::new();
    timeout.arm_after(PEER_SIGNAL_TIMEOUT_MS as u64 * TIMER_FREQ / 1000);

    loop {
        if let Some(received) = signals.try_consume() {
            if received != expected {
                return UnexpectedSignalSnafu { expected, received }.fail();
            }
            return Ok(());
        }
        if timeout.should_try() {
            return PeerSilentSnafu { expected }.fail();
        }
        let blockers: [&dyn Blocker; 2] = [signals, &*timeout];
        block_on_many.block_on(blockers.into_iter());
    }
}

impl OQueueRoundTrip {
    /// The scheduling policy the kernel thread should run under.
    pub fn scheduling_policy(&self) -> DriverSchedulingPolicy {
        self.config.scheduling_policy
    }

    /// Runs the benchmark and then tells the peer how it went.
    ///
    /// Must run on a dedicated kernel thread, not the boot thread, because it blocks on replies.
    /// Nothing here stops the machine: every path ends by sending the peer a [`Request::ending`],
    /// and the peer owns the shutdown.
    pub fn run(self, capture_file: Arc<dyn DataCaptureFile<RoundTripSample>>) {
        let Self {
            config,
            config_problem,
            producer,
            consumer,
            signals,
        } = self;

        // Every way this run can end says so on the console in one place and one form.
        let report = |error: &Error| println!("{PREFIX}|error {NAME}: {error}");

        // Nothing is measured until the peer says it is ready. Without a peer there is also nobody
        // to hand a verdict to, so give up rather than run into a timeout for every iteration.
        if wait_for_signal(&signals, PeerSignal::Ready)
            .inspect_err(report)
            .is_err()
        {
            return;
        }

        let outcome = measure(&config, config_problem, &producer, &consumer, capture_file)
            .inspect_err(report);
        producer.produce_ref(&Request::ending(&outcome));

        // Returning drops the OQueues, which revokes the peer's observer, and a revoked observer
        // reads as end of stream. The peer treats that as a failed run, so hold the queues open until
        // it reports `Done`; otherwise a finished run could report as a failure.
        let _ = wait_for_signal(&signals, PeerSignal::Done).inspect_err(report);
    }
}

/// Measures every iteration and captures the samples, or reports why it could not.
///
/// `Err` means the reason has already been written to the console; the caller only has to pass the
/// verdict on to the peer.
fn measure(
    config: &Config,
    config_problem: Option<Error>,
    producer: &RefProducer<Request>,
    consumer: &Consumer<[u64; 3]>,
    capture_file: Arc<dyn DataCaptureFile<RoundTripSample>>,
) -> Result<(), Error> {
    if let Some(problem) = config_problem {
        return Err(problem);
    }

    // Set the blocking machinery up once rather than per round trip, so none of its cost lands
    // between the two timestamps.
    let timeout = TimeoutBlocker::new();
    let timeout_jiffies = config.timeout_ms as u64 * TIMER_FREQ / 1000;
    let mut block_on_many = BlockOnMany::new();

    let measured = framework::run(config.iterations, capture_file, |seq| {
        round_trip(
            config,
            producer,
            consumer,
            &timeout,
            timeout_jiffies,
            &mut block_on_many,
            seq,
        )
    })?;

    framework::report(NAME, config, measured);
    Ok(())
}

/// Times one round trip: produce request `seq`, block until its reply arrives, and split the four
/// timestamps into intervals.
///
/// An anomaly (a stale reply, an out-of-sequence reply, or no reply within the timeout) is reported
/// and returned as `Err`, which ends the run: a benchmark that quietly carries on past one would be
/// reporting numbers it cannot vouch for.
fn round_trip(
    config: &Config,
    producer: &RefProducer<Request>,
    consumer: &Consumer<[u64; 3]>,
    timeout: &TimeoutBlocker,
    timeout_jiffies: u64,
    block_on_many: &mut BlockOnMany,
    seq: u64,
) -> Result<RoundTripSample, Error> {
    // A value consumable before this iteration's request is produced is a stale reply.
    if let Some(stale) = consumer.try_consume() {
        return StaleReplySnafu {
            seq,
            stale_seq: stale[0],
        }
        .fail();
    }

    let t0 = read_tsc();
    producer.produce_ref(&Request::measure(seq));
    timeout.arm_after(timeout_jiffies);

    let (t3, reply) = loop {
        if let Some(reply) = consumer.try_consume() {
            // Stamp t3 before any loop-exit work so its overhead is not counted.
            let t3 = read_tsc();
            if reply[0] != seq {
                return OutOfSequenceSnafu {
                    seq,
                    replied_seq: reply[0],
                }
                .fail();
            }
            break (t3, reply);
        }
        if timeout.should_try() {
            return ReplyTimeoutSnafu {
                seq,
                timeout_ms: config.timeout_ms,
                elapsed: read_tsc().wrapping_sub(t0),
            }
            .fail();
        }
        let blockers: [&dyn Blocker; 2] = [consumer, timeout];
        block_on_many.block_on(blockers.into_iter());
    };
    timeout.disarm();

    let (t1, t2) = (reply[1], reply[2]);
    Ok(RoundTripSample {
        roundtrip: t3.wrapping_sub(t0),
        kernel_to_user: t1.wrapping_sub(t0),
        compute: t2.wrapping_sub(t1),
        user_to_kernel: t3.wrapping_sub(t2),
    })
}
