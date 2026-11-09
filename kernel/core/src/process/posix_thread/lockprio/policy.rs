// SPDX-License-Identifier: MPL-2.0

//! Policies that decide how to boost the owner of a contended PI futex.

use crate::{
    sched::{Nice, SchedPolicy},
    thread::Tid,
};

/// A thread waiting for a PI futex.
#[derive(Clone, Copy, Debug)]
pub struct PiWaiter {
    #[expect(
        dead_code,
        reason = "reserved for policies that select individual waiters"
    )]
    pub tid: Tid,
    /// The waiter's current scheduling policy. If the waiter itself owns a contended futex, this
    /// may already be boosted.
    pub policy: SchedPolicy,
    /// How long the waiter has been waiting, in nanoseconds.
    #[expect(
        dead_code,
        reason = "reserved for policies that account for waiting time"
    )]
    pub waited_ns: u64,
}

/// Decides how to boost the owner of a contended PI futex.
///
/// The policy is consulted whenever the set of waiters or the owner of a PI futex changes. While
/// the owner holds contended futexes, it runs with the most urgent of its own policy and the
/// policies returned for each of those futexes, so a policy can raise but not lower the owner.
///
/// It is called with a global lock held, so it must be quick and must not block.
pub trait LockPrioPolicy: Send + Sync {
    /// Returns the policy that `owner` should run with while it holds the futex, or `None` to
    /// leave its own policy.
    ///
    /// `owner_base` is the owner's own scheduling policy, without any boost. `waiters` are the
    /// threads waiting for the futex; it is never empty.
    fn boost(
        &self,
        owner: Tid,
        owner_base: SchedPolicy,
        waiters: &[PiWaiter],
    ) -> Option<SchedPolicy>;
}

/// Boosts the owner to the most urgent waiter's policy, like Linux's priority inheritance.
///
/// As in Linux, real-time waiters pass on their real-time policy, and all other waiters count as
/// a fair thread with the default nice value, whatever their own nice value is.
///
/// Ref: <https://elixir.bootlin.com/linux/v7.0-rc7/source/kernel/locking/rtmutex.c#L349>
pub struct InheritPolicy;

impl LockPrioPolicy for InheritPolicy {
    fn boost(
        &self,
        _owner: Tid,
        owner_base: SchedPolicy,
        waiters: &[PiWaiter],
    ) -> Option<SchedPolicy> {
        // A smaller `SchedPolicy` is more urgent.
        let top = waiters
            .iter()
            .map(|waiter| match waiter.policy {
                policy @ (SchedPolicy::Stop | SchedPolicy::RealTime { .. }) => policy,
                SchedPolicy::Fair(_) | SchedPolicy::Idle => SchedPolicy::Fair(Nice::default()),
            })
            .min()?;
        (top < owner_base).then_some(top)
    }
}
