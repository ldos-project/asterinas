// SPDX-License-Identifier: MPL-2.0

//! Startup for the OQFS round-trip microbenchmark's kernel driver.
//!
//! The measurement loop lives in the `aster_oqueue_roundtrip_bench` component crate, which resolves
//! its `DriverSched` from the kernel command line but does not depend on the kernel scheduler types.
//! This module is the kernel-side half: it owns `SchedPolicy`/`ThreadOptions` and applies the
//! resolved policy when it spawns the driver thread.

use aster_oqueue_roundtrip_bench::DriverSched;

use crate::{
    sched::{RealTimePolicy, SchedPolicy},
    thread::kernel_thread::ThreadOptions,
};

/// Starts the microbenchmark's kernel driver on its own thread (its loop blocks on replies). Inert
/// unless enabled with `oqbench.enable`.
pub(super) fn init_in_first_kthread() {
    let Some(driver) = aster_oqueue_roundtrip_bench::prepare() else {
        return;
    };

    let sched = driver.sched();
    let mut options = ThreadOptions::new(move || driver.run());
    if let DriverSched::RealTime { rt_prio } = sched {
        options = options.sched_policy(SchedPolicy::RealTime {
            rt_prio: rt_prio
                .try_into()
                .expect("oqbench.rt_prio is validated to be in 1..=99"),
            rt_policy: RealTimePolicy::RoundRobin {
                base_slice_factor: None,
            },
        });
    }
    options.spawn();
}
