// SPDX-License-Identifier: MPL-2.0

//! CPU-selection policies: decide which CPU's run queue a runnable task joins.
//!
//! [`ClassScheduler`](super::ClassScheduler) asks the active policy every time a task becomes
//! runnable. The policy is chosen at boot with the `sched.select_cpu=<name>` kernel parameter (see
//! [`init_select_cpu_policy`](super::init_select_cpu_policy)).

use core::ops::Bound;

use ostd::{
    cpu::{CpuId, CpuSet},
    task::scheduler::EnqueueFlags,
};

mod last_cpu;
mod round_robin;

pub(super) use self::{last_cpu::LastCpuPolicy, round_robin::RoundRobinPolicy};

/// Why the scheduler is asking for a CPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SelectReason {
    /// A new task runs for the first time.
    Spawn,
    /// A sleeping task was woken.
    Wake,
    /// A sleeping task was woken and must run immediately.
    ForceSchedule,
}

impl From<EnqueueFlags> for SelectReason {
    fn from(flags: EnqueueFlags) -> Self {
        match flags {
            EnqueueFlags::Spawn => Self::Spawn,
            EnqueueFlags::Wake => Self::Wake,
            EnqueueFlags::ForceSchedule => Self::ForceSchedule,
        }
    }
}

/// Everything a policy is told about one placement decision.
pub(super) struct SelectCpuInput<'a> {
    pub(super) reason: SelectReason,
    /// The CPU the task last ran on, or `None` before its first placement.
    pub(super) last_cpu: Option<CpuId>,
    /// The CPUs the task may run on.
    pub(super) affinity: &'a CpuSet,
    /// The CPU making the decision (the spawner's or the waker's).
    pub(super) current_cpu: CpuId,
    /// Returns the number of tasks queued on a CPU. Takes that CPU's run-queue lock.
    pub(super) queue_len: &'a dyn Fn(CpuId) -> u32,
}

/// Picks the CPU a runnable task is queued on.
///
/// Contract:
/// - Called with local IRQs disabled, possibly in interrupt context, and with no run-queue lock
///   held. It must not sleep, block or allocate.
/// - Any state the policy uses lives inside the policy object.
/// - The policy must return a CPU in `affinity`. ([`LastCpuPolicy`] keeps the historical
///   behavior of ignoring a mask that changed after the first placement.)
pub(super) trait SelectCpuPolicy: Send + Sync {
    fn select_cpu(&self, input: &SelectCpuInput) -> CpuId;
}

/// Returns the CPUs in `cpu_set` in ascending order, starting *after* `cpu` and wrapping around.
///
/// If `cpu` is in the set, it comes last.
fn cycle_after(cpu: CpuId, cpu_set: &CpuSet) -> impl Iterator<Item = CpuId> + '_ {
    cpu_set
        .iter_in((Bound::Excluded(cpu), Bound::Unbounded))
        .chain(cpu_set.iter_in(..=cpu))
}
