// SPDX-License-Identifier: MPL-2.0

//! # The Asterinas Scheduler
//!
//! This module implements the scheduler for Asterinas. This hooks into the [OSTD scheduling
//! framework](`ostd::task::scheduler`) replacing its simply FIFO scheduler.
//!
//! This scheduler supports 2 task classes:
//!
//! * [Real-time](`sched_class::real_time`) with round-robin or FIFO scheduling.
//! * [Fair](`sched_class::fair`) which attempts to provide equal runtime for tasks.

mod nice;
mod sched_class;
mod stats;

pub use self::{
    nice::{AtomicNice, Nice},
    sched_class::{
        LinuxSchedPolicy, RealTimePolicy, RealTimePriority, SchedAttr, SchedPolicy, init,
        init_on_each_cpu, init_select_cpu_policy,
    },
    stats::{loadavg, nr_queued_and_running},
};
