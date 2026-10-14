// SPDX-License-Identifier: MPL-2.0

//! Regression-test support for the OQFS file operations (`produce`, `consume`,
//! `strong_observe`, and `weak_observe`).
//!
//! When enabled via the `oqfs_test.enable` kernel command-line flag, this module registers three
//! OQueues under the `oqfs_test` OQFS subtree and spawns a single kernel thread. That thread
//! blocks (without spinning) on the `trigger` queue and, each time userspace writes to
//! `trigger/produce`, produces the batch `0..BATCH` into the `observe` queue, which userspace
//! reads through `strong_observe` or `weak_observe`. The `io` queue is produced and consumed
//! entirely in userspace; the kernel only keeps it alive.
//!
//! The userspace drivers under `test/initramfs/src/regression/oqfs/` drive the queues through the
//! `/oqueues/oqfs_test/...` filesystem and verify the values they observe.

use core::sync::atomic::{AtomicBool, Ordering};

use ostd::{
    orpc::oqueue::{
        ConsumableOQueue, ConsumableOQueueRef, Consumer, OQueueBase, ValueProducer, registry,
    },
    path,
};

use crate::thread::kernel_thread::ThreadOptions;

/// Master switch (`oqfs_test.enable`); the module is inert unless set.
static ENABLE: AtomicBool = AtomicBool::new(false);
aster_cmdline::define_flag_param!("oqfs_test.enable", ENABLE);

/// Number of values produced into the observe queue per trigger.
const BATCH: u32 = 16;

/// Capacity of the `io` and `observe` queues.
const CAPACITY: u32 = 64;

/// The prepared OQFS test endpoints.
///
/// The registry holds only weak references, so these strong handles keep every queue (and its
/// OQFS export) alive for the whole boot; the owning thread never returns.
struct OqfsTest {
    /// Strong handle keeping the `io` queue alive; it is produced and consumed entirely in
    /// userspace and never read by the kernel.
    #[expect(dead_code, reason = "held only to keep the io OQueue (and its OQFS export) alive")]
    io: ConsumableOQueueRef<u32>,
    /// Strong handle keeping the `observe` queue alive; also used to produce the batch.
    observe_producer: ValueProducer<u32>,
    /// Strong handle keeping the `trigger` queue alive; consumed to learn when to produce.
    trigger_consumer: Consumer<u32>,
}

impl OqfsTest {
    /// Creates and exports the three OQueues on the `oqfs_test` OQFS subtree, or returns `None`
    /// if `oqfs_test.enable` is not set.
    fn prepare() -> Option<Self> {
        if !ENABLE.load(Ordering::Relaxed) {
            return None;
        }

        // The `io` queue is produced and consumed entirely in userspace; the kernel keeps only
        // the strong reference so the queue (and its OQFS export) stays alive.
        let io_path = path!(oqfs_test.io);
        let io = ConsumableOQueueRef::<u32>::new(CAPACITY as usize, io_path.clone());
        registry::register_producible(&io_path, &io);

        // The `observe` queue is observable: the kernel produces into it and userspace reads it
        // through `strong_observe` or `weak_observe`.
        let observe_path = path!(oqfs_test.observe);
        let observe = ConsumableOQueueRef::<u32>::new(CAPACITY as usize, observe_path.clone());
        registry::register(&observe_path, &observe.as_any_oqueue());
        let observe_producer = observe
            .attach_value_producer()
            .expect("the oqfs_test observe OQueue always allows a value producer");

        // The `trigger` queue is produced by userspace and consumed by the test thread.
        let trigger_path = path!(oqfs_test.trigger);
        let trigger = ConsumableOQueueRef::<u32>::new(4, trigger_path.clone());
        registry::register_producible(&trigger_path, &trigger);
        let trigger_consumer = trigger
            .attach_consumer()
            .expect("the oqfs_test trigger OQueue always allows attaching its consumer");

        Some(Self {
            io,
            observe_producer,
            trigger_consumer,
        })
    }

    /// The test thread body: block on the trigger queue and produce the batch on each trigger.
    ///
    /// `self` is owned for the whole thread and never dropped (the loop never returns), so every
    /// queue stays alive for the boot.
    fn run(self) {
        loop {
            // Block (without spinning) until userspace writes to the trigger queue.
            let _ = self.trigger_consumer.consume();
            // Produce the batch that userspace observes through strong/weak observation.
            for value in 0..BATCH {
                self.observe_producer.produce(value);
            }
        }
    }
}

/// Starts the OQFS test endpoints on their own kernel thread.
///
/// Must be called after the init process has been spawned.
pub(crate) fn init_after_init_process() {
    let Some(test) = OqfsTest::prepare() else {
        return;
    };
    ThreadOptions::new(move || test.run()).spawn();
}
