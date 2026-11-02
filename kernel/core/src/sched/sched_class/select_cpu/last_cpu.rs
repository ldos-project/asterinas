// SPDX-License-Identifier: MPL-2.0

use ostd::{cpu::CpuId, task::AtomicCpuId};

use super::{SelectCpuInput, SelectCpuPolicy, least_loaded_cpu};

/// Keeps every task on the CPU it was first placed on.
///
/// A new task starts on the least-loaded allowed CPU. After that it always gets its last CPU, even
/// if its affinity mask has changed since.
pub(in crate::sched::sched_class) struct LastCpuPolicy {
    /// The CPU chosen for the most recent spawn.
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
