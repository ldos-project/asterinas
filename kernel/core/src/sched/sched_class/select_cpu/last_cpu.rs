// SPDX-License-Identifier: MPL-2.0

use ostd::{cpu::CpuId, task::AtomicCpuId};

use super::{SelectCpuInput, SelectCpuPolicy, least_loaded_cpu};

/// Keeps every task on the CPU it was first placed on.
///
/// On spawn it picks the least-loaded allowed CPU, breaking ties round-robin after the CPU it chose
/// last time. After that it always returns the task's last CPU, even if the affinity mask has
/// changed since.
pub(in crate::sched::sched_class) struct LastCpuPolicy {
    /// The CPU chosen for the most recent spawn; ties are broken starting after it.
    last_chosen_cpu: AtomicCpuId,
}

impl LastCpuPolicy {
    pub(in crate::sched::sched_class) fn new() -> Self {
        Self {
            last_chosen_cpu: AtomicCpuId::default(),
        }
    }
}

impl SelectCpuPolicy for LastCpuPolicy {
    fn select_cpu(&self, input: &SelectCpuInput) -> CpuId {
        if let Some(last_cpu) = input.last_cpu {
            return last_cpu;
        }
        // If the task is spawned for the first time, it falls through to here and is assigned the
        // CPU with the least load.
        least_loaded_cpu(input, &self.last_chosen_cpu)
    }
}
