// SPDX-License-Identifier: MPL-2.0

use ostd::{cpu::CpuId, task::AtomicCpuId};

use super::{SelectCpuInput, SelectCpuPolicy, SelectReason, cycle_after};

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
        // TODO(arthurp, https://github.com/ldos-project/asterinas/issues/59): last_cpu is never
        // cleared, so once it is set, this policy never moves the task. Pick another policy with
        // `sched.select_cpu` to let tasks migrate.
        if let Some(last_cpu) = input.last_cpu {
            return last_cpu;
        }
        debug_assert!(input.reason == SelectReason::Spawn);

        let mut selected = input.current_cpu;
        let mut minimum_load = u32::MAX;

        // Set `selected` as `candidate` if the candidate's load is smaller.
        let test_candidate = |candidate: CpuId| {
            let load = (input.queue_len)(candidate);
            if load < minimum_load {
                minimum_load = load;
                selected = candidate;
            }
        };

        match self.last_chosen_cpu.get() {
            Some(cpu) => {
                // Perform a round-robin selection starting after the last chosen CPU.
                //
                // It still checks every CPU in the affinity set to find the one with the
                // minimum load, but avoids selecting the same CPU again in case of a tie.
                cycle_after(cpu, input.affinity).for_each(test_candidate)
            }
            None => input.affinity.iter().for_each(test_candidate),
        }

        self.last_chosen_cpu.set_anyway(selected);
        selected
    }
}
