// SPDX-License-Identifier: MPL-2.0

//! Regression-test support for the OQFS file operations (`produce`, `consume`, `strong_observe`,
//! and `weak_observe`).
//!
//! When enabled via the `oqfs_test.enable` kernel command-line flag, this module registers three
//! OQueues under the `oqfs_test` OQFS subtree and spawns a single kernel thread. That thread blocks
//! on the `trigger` queue and, each time userspace writes to `trigger/produce`, produces the batch
//! `0..BATCH` into the `observe` queue, which userspace reads through `strong_observe` or
//! `weak_observe`. The `userspace_produce_consume` queue is produced and consumed entirely in
//! userspace; the kernel only keeps it alive.
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

/// Capacity of the `userspace_produce_consume` and `observe` queues.
const CAPACITY: usize = 64;

/// The prepared OQFS test endpoints.
struct OqfsTest {
    /// Strong handle keeping the `userspace_produce_consume` queue alive; it is produced and consumed entirely in
    /// userspace and never read by the kernel.
    #[expect(
        dead_code,
        reason = "held only to keep the userspace_produce_consume OQueue (and its OQFS export) alive"
    )]
    userspace_produce_consume: ConsumableOQueueRef<u32>,
    observe_producer: ValueProducer<u32>,
    trigger_consumer: Consumer<u32>,
}

impl OqfsTest {
    /// Creates and exports the three OQueues on the `oqfs_test` OQFS subtree, or returns `None`
    /// if `oqfs_test.enable` is not set.
    fn prepare() -> Option<Self> {
        if !ENABLE.load(Ordering::Relaxed) {
            return None;
        }

        // The `userspace_produce_consume` queue is produced and consumed entirely in userspace; the kernel keeps only
        // the strong reference so the queue (and its OQFS export) stays alive.
        let userspace_produce_consume_path = path!(oqfs_test.userspace_produce_consume);
        let userspace_produce_consume =
            ConsumableOQueueRef::<u32>::new(CAPACITY, userspace_produce_consume_path.clone());
        registry::register_producible(&userspace_produce_consume_path, &userspace_produce_consume);

        // The `observe` queue is observable: the kernel produces into it and userspace reads it
        // through `strong_observe` or `weak_observe`.
        let observe_path = path!(oqfs_test.observe);
        let observe = ConsumableOQueueRef::<u32>::new(CAPACITY, observe_path.clone());
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
            userspace_produce_consume,
            observe_producer,
            trigger_consumer,
        })
    }

    /// The test thread body: block on the trigger queue and produce the batch on each trigger.
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
pub(crate) fn init_after_init_process() {
    let Some(test) = OqfsTest::prepare() else {
        return;
    };
    ThreadOptions::new(move || test.run()).spawn();
}
