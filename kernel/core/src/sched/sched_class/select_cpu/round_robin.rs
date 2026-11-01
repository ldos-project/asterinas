// SPDX-License-Identifier: MPL-2.0

use ostd::cpu::CpuId;

use super::{SelectCpuInput, SelectCpuPolicy, cycle_after};

/// Sends a task to the next allowed CPU after the one it last ran on, wrapping around.
///
/// A new task starts on the next allowed CPU after the spawner's CPU. This policy exists to
/// exercise migration: every placement moves the task to another CPU, unless its affinity mask
/// allows only one.
pub(in crate::sched::sched_class) struct RoundRobinPolicy;

impl SelectCpuPolicy for RoundRobinPolicy {
    fn select_cpu(&self, input: &SelectCpuInput) -> CpuId {
        let from = input.last_cpu.unwrap_or(input.current_cpu);
        // `cycle_after` yields `from` last, so a one-CPU mask keeps the task in place.
        cycle_after(from, input.affinity).next().unwrap_or(from)
    }
}
