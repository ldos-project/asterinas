// SPDX-License-Identifier: MPL-2.0

//! Startup for the OQFS round-trip microbenchmark.
//!
//! The measurement loop lives in the `mariposa_benchmark` component crate, which resolves its
//! scheduling policy from the kernel command line but cannot depend on the kernel's scheduler types
//! or its data capture device. This module is the kernel-side half: it owns those, and hands them to
//! the benchmark when it spawns its kernel thread.

use mariposa_benchmark::oqueue_roundtrip::{self, DriverSchedulingPolicy, RoundTripSample};

use crate::{
    data_capture::new_data_capture_file,
    prelude::*,
    sched::{RealTimePolicy, SchedPolicy},
    thread::kernel_thread::ThreadOptions,
};

/// Starts the benchmark on its own kernel thread.
///
/// Must be called after the init process has been spawned: the benchmark blocks until its userspace
/// peer attaches to the request OQueue.
pub(crate) fn init_after_init_process() {
    let Some(benchmark) = oqueue_roundtrip::prepare() else {
        return;
    };

    // Declare output data capture file and allocate space.
    let Some(capture_file) =
        new_data_capture_file::<RoundTripSample>(mariposa_data_capture::FileDescriptor {
            path: ostd::path!(oqbench.samples),
            length: benchmark.capture_length(),
        })
    else {
        error!("[oqbench] no data capture device; set `data_capture.device` to run the benchmark");
        return;
    };

    let scheduling_policy = benchmark.scheduling_policy();
    let mut options = ThreadOptions::new(move || benchmark.run(capture_file));
    if let DriverSchedulingPolicy::RealTime { rt_prio } = scheduling_policy {
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
