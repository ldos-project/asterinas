// SPDX-License-Identifier: MPL-2.0

//! Microbenchmark for the kernel -> user -> kernel OQFS round trip.
//!
//! A kernel driver thread produces a request into one OQueue and blocks; the userspace peer
//! (`oqbench_server`) replies into a second OQueue; the kernel wakes on the reply. Each iteration
//! captures four timestamps on the shared guest TSC -- t0 before producing, t1 after the request
//! read, t2 before the reply, t3 after consuming it -- which decompose the round trip into transport
//! and scheduler latencies. Inert unless enabled with `oqbench.enable` on the kernel command line.
//!
//! The measured samples never leave the kernel during the run. After the loop finishes the driver
//! streams the stored array to the peer over a third OQueue and the peer writes them to a plain-text
//! file in the guest; the host then fetches that file over scp.

#![no_std]
#![feature(format_args_nl)]

extern crate alloc;

use alloc::vec::Vec;
use core::{
    mem::size_of,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};

use aster_logger::println;
use ostd::{
    arch::{read_tsc, tsc_freq},
    orpc::{
        oqueue::{
            ConsumableOQueue as _, ConsumableOQueueRef, Consumer, OQueue as _, OQueueBase as _,
            OQueueRef, RefProducer,
            registry,
        },
        sync::{BlockOnMany, Blocker, TimeoutBlocker},
    },
    power::{ExitCode, poweroff},
    timer::TIMER_FREQ,
};

/// Prints an `OQBENCH|error` line and powers the machine off with a failure exit code. A plain
/// `panic!` here is only a per-thread oops, which would hang the boot, so anomalies power off.
fn fatal(message: core::fmt::Arguments<'_>) -> ! {
    println!("OQBENCH|error {}", message);
    poweroff(ExitCode::Failure)
}

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

/// Warmup iterations excluded from the dumped samples (`oqbench.warmup`).
static WARMUP: AtomicU32 = AtomicU32::new(10_000);
aster_cmdline::define_kv_param!("oqbench.warmup", WARMUP);

/// Per-reply timeout in milliseconds (`oqbench.timeout_ms`); a timeout is fatal.
static TIMEOUT_MS: AtomicU32 = AtomicU32::new(10_000);
aster_cmdline::define_kv_param!("oqbench.timeout_ms", TIMEOUT_MS);

/// Request OQueue capacity (`oqbench.request_capacity`).
static REQUEST_CAPACITY: AtomicU32 = AtomicU32::new(2);
aster_cmdline::define_kv_param!("oqbench.request_capacity", REQUEST_CAPACITY);

/// Reply OQueue capacity (`oqbench.reply_capacity`).
static REPLY_CAPACITY: AtomicU32 = AtomicU32::new(2);
aster_cmdline::define_kv_param!("oqbench.reply_capacity", REPLY_CAPACITY);

/// Run the driver thread under real-time scheduling (`oqbench.realtime`) instead of the fair policy.
static REALTIME: AtomicBool = AtomicBool::new(false);
aster_cmdline::define_flag_param!("oqbench.realtime", REALTIME);

/// Real-time priority for the driver thread when `oqbench.realtime` is set (`oqbench.rt_prio`,
/// `1..=99`).
static RT_PRIO: AtomicU32 = AtomicU32::new(50);
aster_cmdline::define_kv_param!("oqbench.rt_prio", RT_PRIO);

/// The userspace peer's synthetic work per request (`oqbench.peer_compute`). Consumed in userspace;
/// registered here only so it is a recognized parameter and can be echoed for provenance.
static PEER_COMPUTE: AtomicU32 = AtomicU32::new(0);
aster_cmdline::define_kv_param!("oqbench.peer_compute", PEER_COMPUTE);

/// Number of competing busy-loop processes during the run (`oqbench.busy_procs`). Consumed in
/// userspace; registered here only so it is a recognized parameter and can be echoed for provenance.
static BUSY_PROCS: AtomicU32 = AtomicU32::new(0);
aster_cmdline::define_kv_param!("oqbench.busy_procs", BUSY_PROCS);

/// Keep the guest alive after a successful run instead of powering off (`oqbench.await_fetch`), so
/// the host CLI can scp the results file before shutting the guest down. The `AUTO_TEST=oqbench`
/// smoke target leaves this unset and thus powers off itself, so `make run_kernel` never hangs.
static AWAIT_FETCH: AtomicBool = AtomicBool::new(false);
aster_cmdline::define_flag_param!("oqbench.await_fetch", AWAIT_FETCH);

/// Dump OQueue capacity, in samples. The peer paces the kernel with periodic acks (its
/// `DUMP_ACK_EVERY`, well below this) so the kernel never runs more than this far ahead; the
/// revocable strong observer therefore never fills and drops records, which would truncate the dump.
const DUMP_WINDOW: usize = 8192;

/// Request value marking the end of the measurement phase: it breaks the peer out of its reply loop
/// and into dump-receive mode. Distinct from every real sequence number (`0..iterations+warmup`).
const DUMP_SENTINEL: u64 = u64::MAX;

/// Scheduling policy for the driver thread, applied by the kernel crate when it spawns the thread.
#[derive(Clone, Copy)]
pub enum DriverSched {
    Normal,
    RealTime { rt_prio: u8 },
}

/// One of the four decomposed intervals of a round trip; the discriminant indexes a `[u64; 4]`
/// sample and the name labels it in the result block.
#[derive(Clone, Copy)]
enum Interval {
    Roundtrip = 0,
    KernelToUser = 1,
    Compute = 2,
    UserToKernel = 3,
}

impl Interval {
    /// All four intervals in sample order.
    const ALL: [Interval; 4] = [
        Interval::Roundtrip,
        Interval::KernelToUser,
        Interval::Compute,
        Interval::UserToKernel,
    ];

    fn name(self) -> &'static str {
        match self {
            Interval::Roundtrip => "roundtrip",
            Interval::KernelToUser => "kernel_to_user",
            Interval::Compute => "compute",
            Interval::UserToKernel => "user_to_kernel",
        }
    }
}

/// The effective run configuration, snapshotted from the cmdline parameters.
#[derive(Clone, Copy)]
struct Config {
    iterations: u64,
    warmup: u64,
    timeout_ms: u64,
    request_capacity: usize,
    reply_capacity: usize,
    sched: DriverSched,
    peer_compute: u32,
    busy_procs: u32,
}

impl Config {
    fn from_params() -> Self {
        let sched = if REALTIME.load(Ordering::Relaxed) {
            let rt_prio = RT_PRIO.load(Ordering::Relaxed);
            if !(1..=99).contains(&rt_prio) {
                fatal(format_args!("oqbench.rt_prio must be in 1..=99, got {rt_prio}"));
            }
            DriverSched::RealTime {
                rt_prio: rt_prio as u8,
            }
        } else {
            DriverSched::Normal
        };
        Self {
            iterations: ITERATIONS.load(Ordering::Relaxed) as u64,
            warmup: WARMUP.load(Ordering::Relaxed) as u64,
            timeout_ms: TIMEOUT_MS.load(Ordering::Relaxed) as u64,
            request_capacity: REQUEST_CAPACITY.load(Ordering::Relaxed) as usize,
            reply_capacity: REPLY_CAPACITY.load(Ordering::Relaxed) as usize,
            sched,
            peer_compute: PEER_COMPUTE.load(Ordering::Relaxed),
            busy_procs: BUSY_PROCS.load(Ordering::Relaxed),
        }
    }

    /// The scheduling policy as a short label for the result block.
    fn sched_name(&self) -> &'static str {
        match self.sched {
            DriverSched::Normal => "normal",
            DriverSched::RealTime { .. } => "realtime",
        }
    }

    /// The real-time priority for the result block; 0 under the normal policy.
    fn rt_prio(&self) -> u8 {
        match self.sched {
            DriverSched::Normal => 0,
            DriverSched::RealTime { rt_prio } => rt_prio,
        }
    }
}

/// The prepared kernel side of one benchmark run: the effective configuration and the three OQueue
/// endpoints. Holding the producers and consumer keeps all three queues (and their OQFS exports)
/// alive for the whole run.
pub struct BenchDriver {
    config: Config,
    producer: RefProducer<u64>,
    consumer: Consumer<[u64; 3]>,
    dump_producer: RefProducer<[u64; 4]>,
}

/// Prepares a benchmark run, or returns `None` if `oqbench.enable` is not set.
pub fn prepare() -> Option<BenchDriver> {
    if !ENABLE.load(Ordering::Relaxed) {
        return None;
    }

    let config = Config::from_params();
    if config.iterations == 0 {
        println!("OQBENCH|error oqbench.iterations must be non-zero");
        return None;
    }

    let (producer, consumer, dump_producer) =
        setup_queues(config.request_capacity, config.reply_capacity);
    Some(BenchDriver {
        config,
        producer,
        consumer,
        dump_producer,
    })
}

/// Creates and exports the three OQueues on the `oqbench` OQFS subtree: the request (kernel -> user,
/// `strong_observe`) and reply (user -> kernel, `produce`) queues carrying the measurement ping-pong,
/// and the dump (kernel -> user, `strong_observe`) queue carrying the stored samples after the run.
/// Returns the request producer, reply consumer, and dump producer.
fn setup_queues(
    request_capacity: usize,
    reply_capacity: usize,
) -> (RefProducer<u64>, Consumer<[u64; 3]>, RefProducer<[u64; 4]>) {
    let request_path = ostd::path!(oqbench.request);
    let request_oqueue = OQueueRef::<u64>::new(request_capacity, request_path.clone());
    registry::register(&request_path, &request_oqueue.as_any_oqueue());
    let request_producer = request_oqueue
        .attach_ref_producer()
        .expect("the oqbench request OQueue always allows a ref producer");

    let reply_path = ostd::path!(oqbench.reply);
    let reply_oqueue = ConsumableOQueueRef::<[u64; 3]>::new(reply_capacity, reply_path.clone());
    registry::register_producible(&reply_path, &reply_oqueue);
    let reply_consumer = reply_oqueue
        .attach_consumer()
        .expect("the oqbench reply OQueue always allows attaching its consumer");

    let dump_path = ostd::path!(oqbench.dump);
    let dump_oqueue = OQueueRef::<[u64; 4]>::new(DUMP_WINDOW, dump_path.clone());
    registry::register(&dump_path, &dump_oqueue.as_any_oqueue());
    let dump_producer = dump_oqueue
        .attach_ref_producer()
        .expect("the oqbench dump OQueue always allows a ref producer");

    (request_producer, reply_consumer, dump_producer)
}

/// Blocks (without spinning) until the userspace peer attaches or the bounded wait elapses, warning
/// periodically. Returns `true` if the peer attached.
fn wait_for_observer(producer: &RefProducer<u64>) -> bool {
    let poll = TimeoutBlocker::new();
    let max_polls = ATTACH_TIMEOUT_MS / ATTACH_POLL_MS;
    let warn_every = (ATTACH_WARN_MS / ATTACH_POLL_MS).max(1);
    for poll_count in 0..max_polls {
        if producer.has_observers() {
            return true;
        }
        if poll_count != 0 && poll_count % warn_every == 0 {
            let waited_ms = poll_count * ATTACH_POLL_MS;
            println!(
                "OQBENCH|waiting for the userspace peer to attach ({}ms of {}ms elapsed)",
                waited_ms, ATTACH_TIMEOUT_MS
            );
        }
        poll.arm_after(ATTACH_POLL_MS as u64 * TIMER_FREQ / 1000);
        if let Some(task) = ostd::task::Task::current() {
            let blockers: [&dyn Blocker; 1] = [&*poll];
            task.block_on(&blockers);
        }
        poll.disarm();
    }
    producer.has_observers()
}

impl BenchDriver {
    /// The scheduling policy the driver thread should run under.
    pub fn sched(&self) -> DriverSched {
        self.config.sched
    }

    /// Runs the measurement loop, streams every stored sample to the userspace peer, and powers the
    /// machine off (unless `oqbench.await_fetch` keeps it alive for the host to fetch the results).
    /// Must run on a dedicated kernel thread, not the boot thread, because it blocks on replies.
    /// Every anomaly is fatal.
    pub fn run(self) {
        let config = self.config;
        let producer = self.producer;
        let consumer = self.consumer;
        let dump_producer = self.dump_producer;

        if !wait_for_observer(&producer) {
            println!("OQBENCH|begin v1");
            println!(
                "OQBENCH|error userspace peer did not attach within {}ms",
                ATTACH_TIMEOUT_MS
            );
            println!("OQBENCH|end");
            poweroff(ExitCode::Failure);
        }

        // Preallocate the whole sample array so recording on the hot path is a single indexed store.
        let sample_bytes = config.iterations as usize * size_of::<[u64; 4]>();
        let mut samples: Vec<[u64; 4]> = Vec::new();
        if samples
            .try_reserve_exact(config.iterations as usize)
            .is_err()
        {
            fatal(format_args!(
                "oqbench: cannot allocate the {}-byte sample array for {} iterations",
                sample_bytes, config.iterations
            ));
        }
        samples.resize(config.iterations as usize, [0; 4]);
        println!(
            "OQBENCH|alloc sample_bytes={} iterations={}",
            sample_bytes, config.iterations
        );

        let timeout = TimeoutBlocker::new();
        let timeout_jiffies = config.timeout_ms * TIMER_FREQ / 1000;
        let mut block_on_many = BlockOnMany::new();

        let mut measured: u64 = 0;
        let total = config.warmup + config.iterations;
        for seq in 0..total {
            // A value consumable before this iteration's request is produced is a stale reply.
            if let Some(stale) = consumer.try_consume() {
                fatal(format_args!(
                    "oqbench: stale reply in the queue before producing seq {} (its seq={})",
                    seq, stale[0]
                ));
            }

            let t0 = read_tsc();
            producer.produce_ref(&seq);
            timeout.arm_after(timeout_jiffies);

            let (t3, reply) = loop {
                if let Some(reply) = consumer.try_consume() {
                    // Stamp t3 before any loop-exit work so its overhead is not counted.
                    let t3 = read_tsc();
                    if reply[0] != seq {
                        fatal(format_args!(
                            "oqbench: out-of-sequence reply: expected seq {}, got seq {}",
                            seq, reply[0]
                        ));
                    }
                    break (t3, reply);
                }
                if timeout.should_try() {
                    let elapsed = read_tsc().wrapping_sub(t0);
                    fatal(format_args!(
                        "oqbench: reply timeout at seq {} after {}ms ({} cycles)",
                        seq, config.timeout_ms, elapsed
                    ));
                }
                let blockers: [&dyn Blocker; 2] = [&consumer, &*timeout];
                block_on_many.block_on(blockers.into_iter());
            };
            timeout.disarm();

            let (t1, t2) = (reply[1], reply[2]);
            if seq >= config.warmup {
                let mut sample = [0u64; 4];
                sample[Interval::Roundtrip as usize] = t3.wrapping_sub(t0);
                sample[Interval::KernelToUser as usize] = t1.wrapping_sub(t0);
                sample[Interval::Compute as usize] = t2.wrapping_sub(t1);
                sample[Interval::UserToKernel as usize] = t3.wrapping_sub(t2);
                samples[measured as usize] = sample;
                measured += 1;
            }
        }

        // Stream the samples strictly after the loop, so the sample path never touches an OQueue
        // while iterations run. `dump_ms` is the post-run transfer cost the task asks us to report.
        let dump_ms = dump_samples(
            &producer,
            &consumer,
            &dump_producer,
            &samples[..measured as usize],
            config.timeout_ms,
        );

        report(&config, measured, dump_ms);

        // The success marker has no `OQBENCH|` prefix so the smoke test greps a fixed string.
        println!("OQBENCH: run complete ({} iterations)", measured);

        if AWAIT_FETCH.load(Ordering::Relaxed) {
            // The host CLI fetches the results file over scp and only then powers the guest off;
            // returning here ends the driver thread while `init` keeps the guest alive.
            return;
        }
        poweroff(ExitCode::Success);
    }
}

/// Streams `samples` to the userspace peer and returns the transfer time in milliseconds.
///
/// The sentinel breaks the peer out of its reply loop; the samples then flow over the dump OQueue,
/// paced by the peer's acks on the reply OQueue so its revocable observer never fills and drops
/// records. The peer's final ack (sent only after it has written and flushed the whole file)
/// confirms a complete transfer; a missing or short ack is fatal, so a truncated dump can never be
/// mistaken for a whole one on either side.
fn dump_samples(
    producer: &RefProducer<u64>,
    consumer: &Consumer<[u64; 3]>,
    dump_producer: &RefProducer<[u64; 4]>,
    samples: &[[u64; 4]],
    timeout_ms: u64,
) -> u64 {
    let count = samples.len() as u64;
    let start = read_tsc();

    if !dump_producer.has_observers() {
        fatal(format_args!(
            "oqbench: the userspace peer is not observing the dump queue"
        ));
    }

    // Leave the reply loop; the peer then reads the header and `count` samples from the dump queue.
    producer.produce_ref(&DUMP_SENTINEL);
    dump_producer.produce_ref(&[count, 0, 0, 0]);

    let timeout = TimeoutBlocker::new();
    let timeout_jiffies = timeout_ms * TIMER_FREQ / 1000;
    let mut block_on_many = BlockOnMany::new();
    let mut acked: u64 = 0;

    for (i, sample) in samples.iter().enumerate() {
        // Stay within the observer's ring (header + unread samples) so it is never revoked.
        while i as u64 - acked >= (DUMP_WINDOW - 2) as u64 {
            acked = recv_ack(consumer, &timeout, timeout_jiffies, &mut block_on_many, acked, count);
        }
        dump_producer.produce_ref(sample);
    }

    // The final ack is sent only after the peer has flushed the whole file; wait for it.
    while acked < count {
        acked = recv_ack(consumer, &timeout, timeout_jiffies, &mut block_on_many, acked, count);
    }

    (read_tsc().wrapping_sub(start)) * 1000 / tsc_freq()
}

/// Consumes one dump ack `[consumed, 0, 0]` from the reply queue, returning the peer's cumulative
/// consumed count. A timeout, a regression, or a count past `total` means the transfer broke and is
/// fatal.
fn recv_ack(
    consumer: &Consumer<[u64; 3]>,
    timeout: &TimeoutBlocker,
    timeout_jiffies: u64,
    block_on_many: &mut BlockOnMany,
    prev: u64,
    total: u64,
) -> u64 {
    timeout.arm_after(timeout_jiffies);
    let ack = loop {
        if let Some(ack) = consumer.try_consume() {
            break ack;
        }
        if timeout.should_try() {
            fatal(format_args!(
                "oqbench: dump ack timeout after {} of {} samples",
                prev, total
            ));
        }
        let blockers: [&dyn Blocker; 2] = [consumer, timeout];
        block_on_many.block_on(blockers.into_iter());
    };
    timeout.disarm();
    let consumed = ack[0];
    if consumed < prev || consumed > total {
        fatal(format_args!(
            "oqbench: bad dump ack {} (previous {}, total {})",
            consumed, prev, total
        ));
    }
    consumed
}

/// Emits the self-delimiting metadata block needed to interpret the results file: TSC frequency,
/// effective config, scenario, measured count, and the post-run transfer time. Every line carries
/// the `OQBENCH|` prefix so a host tool can reassemble the block.
fn report(config: &Config, measured: u64, dump_ms: u64) {
    let mut fields = alloc::string::String::new();
    for (i, interval) in Interval::ALL.iter().enumerate() {
        if i != 0 {
            fields.push(',');
        }
        fields.push_str(interval.name());
    }

    println!("OQBENCH|begin v1");
    println!("OQBENCH|tsc_freq_hz {}", tsc_freq());
    println!(
        "OQBENCH|config iterations={} warmup={} timeout_ms={} request_capacity={} reply_capacity={} peer_compute={} sched={} rt_prio={}",
        config.iterations,
        config.warmup,
        config.timeout_ms,
        config.request_capacity,
        config.reply_capacity,
        config.peer_compute,
        config.sched_name(),
        config.rt_prio(),
    );
    println!("OQBENCH|scenario busy_procs={}", config.busy_procs);
    println!("OQBENCH|counts measured={} warmup={}", measured, config.warmup);
    println!(
        "OQBENCH|samples count={} fields={} dump_ms={}",
        measured, fields, dump_ms
    );
    println!("OQBENCH|end");
}
