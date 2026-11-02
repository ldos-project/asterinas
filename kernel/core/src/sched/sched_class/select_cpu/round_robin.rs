// SPDX-License-Identifier: MPL-2.0

use ostd::{cpu::CpuId, task::AtomicCpuId};

use super::{SelectCpuInput, SelectCpuPolicy, cycle_after, least_loaded_cpu};

/// Sends a task to the next allowed CPU after the one it last ran on, wrapping around.
///
/// A new task starts on the least-loaded allowed CPU. This policy exists to exercise migration:
/// every later placement moves the task to another CPU, unless its affinity mask allows only one.
pub(in crate::sched::sched_class) struct RoundRobinPolicy {
    /// The CPU chosen for the most recent spawn; ties are broken starting after it.
    last_chosen_cpu: AtomicCpuId,
}

impl RoundRobinPolicy {
    pub(in crate::sched::sched_class) fn new() -> Self {
        Self {
            last_chosen_cpu: AtomicCpuId::default(),
        }
    }
}

impl SelectCpuPolicy for RoundRobinPolicy {
    fn select_cpu(&self, input: &SelectCpuInput) -> CpuId {
        if let Some(last_cpu) = input.last_cpu {
            // `cycle_after` yields `last_cpu` last, so a one-CPU mask keeps the task in place.
            return cycle_after(last_cpu, input.affinity)
                .next()
                .unwrap_or(last_cpu);
        }
        // If the task is spawned for the first time, it falls through to here and is assigned the
        // CPU with the least load.
        least_loaded_cpu(input, &self.last_chosen_cpu)
    }
}
