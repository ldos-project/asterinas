// SPDX-License-Identifier: MPL-2.0

//! Kernel space support for the oqfs_test regression test.

use ostd::{
    info,
    orpc::oqueue::{ConsumableOQueue as _, ConsumableOQueueRef, OQueueBase as _, registry},
    path,
};

use crate::thread::kernel_thread::ThreadOptions;

/// Number of values produced into the observe queue per trigger.
pub(crate) const BATCH: u32 = 16;

/// Capacity of the `io` and `observe` queues.
pub(crate) const CAPACITY: u32 = 64;

pub(crate) fn init_after_init_process() {
    // The `io` queue is produced and consumed entirely in userspace; the kernel keeps only the
    // strong reference so the queue (and its OQFS export) stays alive.
    let io_path = path!(oqfs_test.produce_consume);
    let io = ConsumableOQueueRef::<u32>::new(CAPACITY as usize, io_path.clone());
    registry::register_producible(&io_path, &io);

    // The `observe` queue is observable: the kernel produces into it and userspace reads it through
    // `strong_observe` or `weak_observe`.
    let observe_path = path!(oqfs_test.observe);
    let observe = ConsumableOQueueRef::<u32>::new(CAPACITY as usize, observe_path.clone());
    registry::register(&observe_path, &observe.as_any_oqueue());
    let observe_producer = observe
        .attach_value_producer()
        .expect("the oqfs_test observe OQueue always allows a value producer");

    // The `trigger` queue is produced by userspace and consumed by the test task.
    let trigger_path = path!(oqfs_test.trigger);
    let trigger = ConsumableOQueueRef::<u32>::new(4, trigger_path.clone());
    registry::register_producible(&trigger_path, &trigger);
    let trigger_consumer = trigger
        .attach_consumer()
        .expect("the oqfs_test trigger OQueue always allows attaching its consumer");

    ThreadOptions::new(move || {
        // Force io to be kept alive in this close.
        let _ = &io;
        info!("In oqfs_test thread");
        loop {
            // Block (without spinning) until userspace writes to the trigger queue.
            let _ = trigger_consumer.consume();
            info!("In oqfs_test observe test");
            // Produce the batch that userspace observes through strong/weak observation.
            for value in 0..BATCH {
                observe_producer.produce(value);
            }
        }
    })
    .spawn();
}
