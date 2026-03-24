// SPDX-License-Identifier: MPL-2.0

use alloc::{boxed::Box, sync::Arc};
use core::time::Duration;

use aster_time::Instant;
use ostd::{
    new_server,
    orpc::{
        framework::{notifier::Notifier, spawn_thread},
        oqueue::{OQueue, OQueueRef},
        orpc_server,
    },
    path,
};
use binary_serde::BinarySerde;
use snafu::Whatever;

use crate::util::timer::TimerServer;

/// Data TLB Misses instance struct
#[derive(BinarySerde, Debug, Clone, Copy)]
#[expect(dead_code)]
pub struct DtlbMisses {
    timestamp: u128,
    miss_l1_tlb: u64,
    miss_all_tlb: u64,
}

/// PMU daemon that periodically reads hw counters. Currently only supports dTLB misses on
/// Icelake-Server
// TODO(tewaro, after SOSP) actually support interesting option
// TODO(tewaro, after SOSP) actually support multi-process
#[orpc_server]
pub struct PmuServer {
    dtlb_miss_count_oqueue: OQueueRef<DtlbMisses>,
}

impl PmuServer {
    /// Create and spawn a new PmuServer.
    pub fn spawn() -> Arc<Self> {
        let pmud = Self::new().unwrap();
        // TODO(tewaro, after SOSP) needs to run inline with jiffies and defer push to oqueue
        spawn_thread(pmud.clone(), {
            let pmud = pmud.clone();
            move || pmud.main()
        });
        pmud
    }
    pub fn new() -> Result<Arc<Self>, Whatever> {
        let server = new_server!(|_| Self {
            dtlb_miss_count_oqueue: OQueueRef::<DtlbMisses>::new(32, path!(pmu.dtlb_miss_count)),
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

    pub fn main(&self) -> Result<(), Box<dyn core::error::Error>> {
        // TODO(tewaro, after SOSP): for now we assume one cpu, when we come back to make this
        // multi-cpu we should also investigate making this multi-process as it will be easier to
        // work with
        let notify_server = TimerServer::spawn(Duration::from_millis(100));

        let dtlb_miss_count_producer = self.dtlb_miss_count_oqueue.attach_ref_producer()?;

        let notify_observer = notify_server
            .notification_oqueue()
            .attach_strong_observer()?;
        loop {
            notify_observer.strong_observe();
            let (miss_l1_tlb, miss_all_tlb) = ostd::arch::pmu::pmu_read_dtlb();
            let misses = DtlbMisses {
                timestamp: aster_time::read_monotonic_time().as_nanos(),
                miss_l1_tlb,
                miss_all_tlb,
            };

            dtlb_miss_count_producer.produce_ref(&misses);
        }
    }
}
