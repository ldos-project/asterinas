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

// TODO(arthurp, https://github.com/ldos-project/asterinas/issues/59): The scheduler doesn't support
// task migration. They are fixed to a CPU once they are scheduled.

mod nice;
mod sched_class;
mod stats;

pub use self::{
    nice::{AtomicNice, Nice},
    sched_class::{RealTimePolicy, RealTimePriority, SchedAttr, SchedPolicy, init},
    stats::{loadavg, nr_queued_and_running},
};
