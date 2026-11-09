// SPDX-License-Identifier: MPL-2.0

use core::sync::atomic::Ordering;

use ostd::{cpu::CpuId, task::AtomicCpuId};

use super::{SelectCpuInput, SelectCpuPolicy, SelectCpuReason, cycle_after};

/// Sends a task to the next allowed CPU after the one it last ran on, wrapping around.
///
/// A new task starts on the least-loaded allowed CPU. Every later placement moves the task to
/// another CPU, unless its affinity mask allows only one.
pub(crate) struct RoundRobinPolicy {
    /// The CPU chosen for the most recent spawn.
    last_chosen_cpu: AtomicCpuId,
}

impl RoundRobinPolicy {
    pub(crate) fn new() -> Self {
        Self {
            last_chosen_cpu: AtomicCpuId::default(),
        }
    }
}

impl SelectCpuPolicy for RoundRobinPolicy {
    fn select_cpu(&self, input: &SelectCpuInput) -> CpuId {
        if input.reason != SelectCpuReason::Spawn
            && let Some(last_cpu) = input.thread.sched_attr().last_cpu()
        {
            let affinity = input.thread.atomic_cpu_affinity().load(Ordering::Relaxed);
            // `cycle_after` yields `last_cpu` last, so a one-CPU mask keeps the task in place.
            return cycle_after(last_cpu, &affinity).next().unwrap_or(last_cpu);
        }
        let cpu = input.least_loaded_cpu(self.last_chosen_cpu.get());
        self.last_chosen_cpu.set_anyway(cpu);
        cpu
    }
}
