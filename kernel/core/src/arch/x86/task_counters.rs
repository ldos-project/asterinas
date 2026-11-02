// SPDX-License-Identifier: MPL-2.0

//! The `pmu.task_counters` OQueue, which attributes hardware events to tasks.
//!
//! When enabled with the `pmu.task_counters` kernel parameter, every CPU samples its performance
//! counters at each context switch and reports the events since the previous switch, i.e., while
//! the outgoing task was running, including interrupts handled on its behalf. Together with
//! `scheduler.events` and `futex.pi.events`, this shows, for example, how many cycles a lock holder
//! spends stalled on cache misses while a higher-priority thread waits for it.
//!
//! The counters are attributed by sampling in the pre-schedule handler, so the cost of producing a
//! sample is counted towards the next task. Samples are produced best-effort: if the queue is
//! full, the sample is dropped rather than stalling the context switch.

use core::cell::Cell;

use ostd::{
    arch::{
        pmu::{TaskCounters, task_counters_read, task_counters_start, task_counters_supported},
        read_tsc,
    },
    cpu::{CpuId, CpuSet},
    cpu_local,
    irq::DisabledLocalIrqGuard,
    orpc::oqueue::{GenericOQueueRef, OQueue as _, OQueueBase as _, RefProducer, registry},
    path,
    task::Task,
};
use serde::Serialize;
use spin::Once;

use crate::{prelude::*, process::posix_thread::AsPosixThread, thread::Tid};

/// The hardware events counted while a task ran on a CPU.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct TaskCounterSample {
    /// The task's [`Task::id`], which also identifies it in `scheduler.events`.
    pub task: usize,
    /// The task's thread ID, if it is a POSIX thread.
    pub tid: Option<Tid>,
    /// The CPU the task ran on.
    pub cpu: u32,
    /// The TSC ticks that elapsed while the task ran.
    pub tsc_ticks: u64,
    pub cycles: u64,
    pub instructions: u64,
    pub llc_misses: u64,
    /// Cycles stalled with an outstanding L3 miss. This event is model-specific: it is only
    /// meaningful from Skylake through Golden Cove.
    pub l3_miss_stall_cycles: u64,
}

static PRODUCER: Once<RefProducer<TaskCounterSample>> = Once::new();

cpu_local! {
    /// The TSC and counter values at the last context switch on this CPU, or `None` if the
    /// task counters have not been started on this CPU.
    static LAST_SAMPLE: Cell<Option<(u64, TaskCounters)>> = Cell::new(None);
}

/// Starts the task counters on all CPUs, if the CPU supports them.
pub fn init() {
    if !task_counters_supported() {
        warn!("pmu.task_counters: the CPU's PMU does not support the task counters");
        return;
    }

    PRODUCER.call_once(|| {
        let path = path!(pmu.task_counters);
        let oqueue = GenericOQueueRef::new(1024, path.clone());
        // Export the queue under the OQueue filesystem (`/oqueues/pmu/task_counters`).
        registry::register(&path, &oqueue.as_any_oqueue());
        oqueue
            .attach_ref_producer()
            .expect("a new OQueue always allows a ref producer")
    });

    ostd::smp::inter_processor_call(&CpuSet::new_full(), start_on_current_cpu);
}

fn start_on_current_cpu() {
    let irq_guard = ostd::irq::disable_local();
    task_counters_start();
    LAST_SAMPLE
        .get_with(&irq_guard)
        .set(Some((read_tsc(), task_counters_read())));
}

/// Reports the events counted while the current task ran since the last context switch.
///
/// This must be called just before switching away from the current task.
pub fn sample(irq_guard: &DisabledLocalIrqGuard) {
    let Some(producer) = PRODUCER.get() else {
        return;
    };
    let last_sample = LAST_SAMPLE.get_with(irq_guard);
    let Some((last_tsc, last_counters)) = last_sample.get() else {
        return;
    };

    let counters = task_counters_read();
    let tsc = read_tsc();
    last_sample.set(Some((tsc, counters)));

    let Some(task) = Task::current() else {
        return;
    };
    let delta = counters.since(&last_counters);
    let sample = TaskCounterSample {
        task: task.id().get(),
        tid: task.as_posix_thread().map(|thread| thread.tid()),
        cpu: CpuId::current_racy().into(),
        tsc_ticks: tsc.wrapping_sub(last_tsc),
        cycles: delta.cycles,
        instructions: delta.instructions,
        llc_misses: delta.llc_misses,
        l3_miss_stall_cycles: delta.l3_miss_stall_cycles,
    };
    // Drop the sample if the queue is full; never stall a context switch on an observer.
    let _ = producer.try_produce_ref(&sample);
}
