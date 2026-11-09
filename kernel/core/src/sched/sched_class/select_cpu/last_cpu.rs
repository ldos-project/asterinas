// SPDX-License-Identifier: MPL-2.0

use ostd::{cpu::CpuId, task::AtomicCpuId};

use super::{SelectCpuInput, SelectCpuPolicy, SelectCpuReason};

/// Keeps every task on the CPU it was first placed on.
///
/// A new task starts on the least-loaded allowed CPU.
pub(crate) struct LastCpuPolicy {
    /// The CPU chosen for the most recent spawn.
    last_chosen_cpu: AtomicCpuId,
}

impl LastCpuPolicy {
    pub(crate) fn new() -> Self {
        Self {
            last_chosen_cpu: AtomicCpuId::default(),
        }
    }
}

impl SelectCpuPolicy for LastCpuPolicy {
    /// Returns the task's last CPU, even if its affinity mask no longer contains that CPU.
    fn select_cpu(&self, input: &SelectCpuInput) -> CpuId {
        if input.reason != SelectCpuReason::Spawn
            && let Some(last_cpu) = input.thread.sched_attr().last_cpu()
        {
            return last_cpu;
        }
        let cpu = input.least_loaded_cpu(self.last_chosen_cpu.get());
        self.last_chosen_cpu.set_anyway(cpu);
        cpu
    }
}
