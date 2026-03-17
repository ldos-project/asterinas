// SPDX-License-Identifier: MPL-2.0

use alloc::{boxed::Box, sync::Arc};
use core::time::Duration;

use ostd::orpc::{
    framework::{notifier::Notifier, spawn_thread},
    legacy_oqueue::{OQueue, ringbuffer::MPMCOQueue},
    orpc_server, orpc_trait,
    sync::select,
};
use snafu::Whatever;

use crate::{prelude::println, util::timer::TimerServer};

use aster_time::Instant;

/// PMU daemon that periodically read values and outputs to oq
/// Currently only supports dTLB misses on zerberus
/// TODO(after SOSP) actually support interesting option
/// TODO(after SOSP) actually support multi-process

#[orpc_trait]
pub(crate) trait PMUD {}

#[derive(Debug, Clone, Copy)]
struct DTLBMisses {
    ts: Instant,
    miss_l1_tlb: u64,
    miss_all_tlb: u64,
}

#[orpc_server(PMUD)]
pub struct PMUServer {
    dtlb_miss_count_oq: Arc<MPMCOQueue<DTLBMisses>>,
}

impl PMUServer {
    /// Create and spawn a new HugepagedServer.
    pub fn spawn() -> Arc<Self> {
        let pmud = Self::new().unwrap();
        // TODO(after SOSP) needs to run inline with jiffies and defer push to oqueue
        spawn_thread(pmud.clone(), {
            let pmud = pmud.clone();
            move || pmud.main()
        });
        pmud
    }
    pub fn new() -> Result<Arc<Self>, Whatever> {
        let server = Self::new_with(|orpc_internal, _| Self {
            orpc_internal,
            dtlb_miss_count_oq: MPMCOQueue::<DTLBMisses>::new(32, 2),
        });
        Ok(server)
    }

    /// Resets counter and automatically stops it from continuing.
    pub fn reset(&self) {
        ostd::arch::pmu::pmu_reset();
    }

    /// Starts collecting at the hardcoded interval set by the system.
    pub fn start(&self) {
        ostd::arch::pmu::pmu_start();
    }

    /// For now we assume one cpu, when we come back to make this multi-cpu we should also
    /// investigate making this multi-process as it will be easier to work with
    pub fn main(&self) -> Result<(), Box<dyn core::error::Error>> {
        let notify_server = TimerServer::spawn(Duration::from_millis(100));

        let dtlb_miss_count_producer = self.dtlb_miss_count_oq.attach_producer()?;

        let notify_observer = notify_server
            .notification_oqueue()
            .attach_strong_observer()?;
        loop {
            loop {
                select!(
                    if let _ = notify_observer.try_strong_observe() {
                        break;
                    }
                );
                ostd::task::Task::yield_now();
            }

            let (miss_l1_tlb, miss_all_tlb) = ostd::arch::pmu::pmu_read_dtlb();
            let misses = DTLBMisses {
                ts: aster_time::read_monotonic_time().into(),
                miss_l1_tlb,
                miss_all_tlb,
            };

            dtlb_miss_count_producer.produce(misses);
        }
    }
}
