// SPDX-License-Identifier: MPL-2.0

//! CPU-selection policies: decide which CPU's run queue a runnable task joins.
//!
//! The scheduler asks the active policy every time a task becomes runnable. The policy is chosen
//! at boot with the `sched.select_cpu=<name>` kernel parameter, and can be replaced at run time
//! with [`inject_select_cpu_policy`](super::inject_select_cpu_policy).

use alloc::sync::Arc;
use core::{ops::Bound, sync::atomic::Ordering};

use ostd::{
    cpu::{CpuId, CpuSet},
    task::scheduler::EnqueueFlags,
};

use crate::thread::Thread;

mod last_cpu;
mod round_robin;
#[cfg(not(baseline_asterinas))]
mod userspace;

#[cfg(not(baseline_asterinas))]
pub use self::userspace::UserspacePolicy;
pub use self::{last_cpu::LastCpuPolicy, round_robin::RoundRobinPolicy};

/// Why the scheduler is asking for a CPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectCpuReason {
    /// A new task runs for the first time.
    Spawn,
    /// A sleeping task was woken.
    Wake,
    /// A sleeping task was woken and must run immediately.
    ForceSchedule,
    /// A running task was preempted (or yielded) and is still runnable.
    Preempt,
}

impl From<EnqueueFlags> for SelectCpuReason {
    fn from(flags: EnqueueFlags) -> Self {
        match flags {
            EnqueueFlags::Spawn => Self::Spawn,
            EnqueueFlags::Wake => Self::Wake,
            EnqueueFlags::ForceSchedule => Self::ForceSchedule,
        }
    }
}

/// Everything a policy is told about one placement decision.
pub struct SelectCpuInput<'a> {
    pub reason: SelectCpuReason,
    /// The thread being placed.
    pub thread: &'a Arc<Thread>,
    /// Returns the number of tasks queued on a CPU. Takes that CPU's run-queue lock.
    pub queue_len: &'a dyn Fn(CpuId) -> u32,
}

impl SelectCpuInput<'_> {
    /// Returns the CPU in the thread's affinity mask with the shortest queue.
    ///
    /// Among equally loaded CPUs, it returns the first one after `after` (wrapping around), or the
    /// lowest-numbered one if `after` is `None`.
    pub fn least_loaded_cpu(&self, after: Option<CpuId>) -> CpuId {
        let affinity = self.thread.atomic_cpu_affinity().load(Ordering::Relaxed);
        let mut selected = None;
        let mut minimum_load = u32::MAX;
        let mut test_candidate = |candidate: CpuId| {
            let load = (self.queue_len)(candidate);
            if load < minimum_load {
                minimum_load = load;
                selected = Some(candidate);
            }
        };
        match after {
            Some(cpu) => cycle_after(cpu, &affinity).for_each(&mut test_candidate),
            None => affinity.iter().for_each(&mut test_candidate),
        }
        // `sched_setaffinity` rejects an empty mask, so a CPU is always found.
        selected.unwrap_or(CpuId::bsp())
    }
}

/// A CPU-selection policy.
pub trait SelectCpuPolicy: Send + Sync {
    /// Returns the CPU whose run queue the thread in `input` joins.
    ///
    /// It is called with local IRQs disabled, possibly in interrupt context, and with no run-queue
    /// lock held. It must not sleep, block or allocate. Any state the policy needs lives in the
    /// policy object. The returned CPU should be in the thread's affinity mask.
    fn select_cpu(&self, input: &SelectCpuInput) -> CpuId;
}

/// Returns the CPUs in `cpu_set` in ascending order, starting *after* `cpu` and wrapping around.
///
/// If `cpu` is in the set, it comes last.
pub fn cycle_after(cpu: CpuId, cpu_set: &CpuSet) -> impl Iterator<Item = CpuId> + '_ {
    cpu_set
        .iter_in((Bound::Excluded(cpu), Bound::Unbounded))
        .chain(cpu_set.iter_in(..=cpu))
}
