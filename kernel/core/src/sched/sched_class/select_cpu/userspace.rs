// SPDX-License-Identifier: MPL-2.0

use core::sync::atomic::Ordering;

use ostd::{
    cpu::CpuId,
    orpc::oqueue::{
        ConsumableOQueue as _, ConsumableOQueueRef, InlineConsumer, OQueueBase as _,
        ObservationQuery, WeakObserver, registry,
    },
    path,
    sync::{LocalIrqDisabled, SpinLock},
};

use super::{LastCpuPolicy, SelectCpuInput, SelectCpuPolicy};

/// Sends every task to the CPU that userspace wrote most recently.
///
/// Userspace writes a CPU number, CBOR-encoded, to `/oqueues/scheduler/select_cpu/produce`. A task
/// whose affinity mask does not contain that CPU, or any task placed before the first write, is
/// placed as [`LastCpuPolicy`] would place it.
pub(crate) struct UserspacePolicy {
    attachments: SpinLock<Attachments, LocalIrqDisabled>,
    fallback: LastCpuPolicy,
}

struct Attachments {
    /// Reads the most recent CPU written by userspace.
    observer: WeakObserver<u32>,
    /// Discards each written value. Without a consumer, the OQueue warns on every write.
    _consumer: InlineConsumer<u32>,
}

impl UserspacePolicy {
    /// Creates the policy and exports its OQueue at `/oqueues/scheduler/select_cpu`.
    ///
    /// # Panics
    ///
    /// Panics if that OQueue is already exported, i.e. if this is called twice.
    pub(crate) fn new() -> Self {
        let path = path!(scheduler.select_cpu);
        let oqueue = ConsumableOQueueRef::<u32>::new(OQUEUE_CAPACITY, path.clone());
        registry::register_producible(&path, &oqueue);
        let observer = oqueue
            .attach_weak_observer(1, ObservationQuery::identity())
            .expect("a new OQueue accepts a weak observer");
        let consumer = oqueue
            .attach_consumer()
            .and_then(|consumer| consumer.consume_inline(|_| {}))
            .expect("a new OQueue accepts a consumer");
        Self {
            attachments: SpinLock::new(Attachments {
                observer,
                _consumer: consumer,
            }),
            fallback: LastCpuPolicy::new(),
        }
    }

    /// Returns the CPU most recently written by userspace, if any.
    fn requested_cpu(&self) -> Option<CpuId> {
        let attachments = self.attachments.lock();
        let observer = &attachments.observer;
        let cpu = observer.weak_observe(observer.newest_cursor()).ok()??;
        CpuId::try_from(cpu as usize).ok()
    }
}

impl SelectCpuPolicy for UserspacePolicy {
    fn select_cpu(&self, input: &SelectCpuInput) -> CpuId {
        let affinity = input.thread.atomic_cpu_affinity().load(Ordering::Relaxed);
        match self.requested_cpu() {
            Some(cpu) if affinity.contains(cpu) => cpu,
            _ => self.fallback.select_cpu(input),
        }
    }
}

/// The OQueue capacity. Producing never blocks, because the OQueue has no strong observer and its
/// consumer runs inline, so a small ring is enough.
const OQUEUE_CAPACITY: usize = 2;
