// SPDX-License-Identifier: MPL-2.0

//! Boosting the owners of contended PI futexes, and the `lockprio.decisions` OQueue.
//!
//! The PI futex code reports the waiters of a futex and its owner whenever either changes, with
//! the futex's bucket locked, so the records here always reflect the latest state of each futex.
//! The owner's scheduling policy is then recomputed from those records by a [`LockPrioPolicy`]
//! and applied with the global lock held, so concurrent changes for one owner are applied in
//! order.
//!
//! The policy is chosen with the `lockprio.policy` kernel parameter: `inherit` (the default)
//! boosts like Linux's priority inheritance, and `none` disables boosting.
//!
//! Limitations:
//! - Boosts are not propagated along chains: if a boosted owner is itself waiting for another
//!   futex, that futex's owner sees the boosted policy only at the next change to that futex.
//! - Changing a boosted thread's policy with `sched_setscheduler` replaces its own policy, which
//!   it keeps once unboosted, but `sched_getscheduler` reports the boosted policy meanwhile.
//!   Threads it clones meanwhile do start with its own policy, as on Linux.

pub mod policy;

use spin::Once;

pub use self::policy::{InheritPolicy, LockPrioPolicy, PiContention, PiWaiter};
use super::futex::FutexKey;
use crate::{
    prelude::*,
    sched::{SchedPolicy, set_policy_requeued},
    thread::{Thread, Tid},
};

/// The waiters of a PI futex and its owner.
struct FutexRecord {
    key: FutexKey,
    owner: Tid,
    waiters: Vec<PiWaiter>,
}

/// A thread whose policy may be boosted.
struct OwnerRecord {
    thread: Arc<Thread>,
    /// The thread's own policy.
    base: SchedPolicy,
    /// The policy last applied to the thread.
    applied: SchedPolicy,
}

#[derive(Default)]
struct State {
    /// The contended futexes. There are few, so a vector is fine.
    futexes: Vec<FutexRecord>,
    owners: BTreeMap<Tid, OwnerRecord>,
}

static STATE: SpinLock<State> = SpinLock::new(State {
    futexes: Vec::new(),
    owners: BTreeMap::new(),
});

static POLICY: Once<Box<dyn LockPrioPolicy>> = Once::new();

static POLICY_NAME: Once<String> = Once::new();
aster_cmdline::define_kv_param!("lockprio.policy", POLICY_NAME);

pub(super) fn init() {
    let name = POLICY_NAME.get().map_or("inherit", String::as_str);
    let policy: Box<dyn LockPrioPolicy> = match name {
        // Leave `POLICY` unset so that the futex code does not report to us at all.
        "none" => return,
        "inherit" => Box::new(InheritPolicy),
        _ => {
            warn!(
                "lockprio.policy: unknown policy {:?}, using \"inherit\"",
                name
            );
            Box::new(InheritPolicy)
        }
    };
    POLICY.call_once(|| policy);
    decisions::init();
}

/// Returns whether owners may be boosted, i.e., whether the futex code needs to report to us.
pub(super) fn is_enabled() -> bool {
    POLICY.get().is_some()
}

/// Records that `waiters` wait for the PI futex `key` owned by `owner`, or by its previous owner
/// if `owner` is `None`. An empty `waiters` means the futex is no longer contended.
///
/// This must be called with the futex's bucket locked, so that records are updated in the order
/// that the futex changes. Returns the owners whose policy may need to change, which the caller
/// must pass to [`apply`] once the bucket is unlocked.
pub(super) fn record(
    key: &FutexKey,
    owner: Option<Tid>,
    waiters: Vec<PiWaiter>,
) -> [Option<Tid>; 2] {
    let mut state = STATE.lock();
    let index = state
        .futexes
        .iter()
        .position(|record| record.key.match_up(key));
    let old_owner = index.map(|index| state.futexes[index].owner);
    let Some(new_owner) = owner.or(old_owner) else {
        // The futex is not recorded and its owner is unknown. A later report will name it.
        return [None, None];
    };

    match (index, waiters.is_empty()) {
        (Some(index), true) => {
            state.futexes.swap_remove(index);
        }
        (Some(index), false) => {
            let record = &mut state.futexes[index];
            record.owner = new_owner;
            record.waiters = waiters;
        }
        (None, true) => {}
        (None, false) => state.futexes.push(FutexRecord {
            key: key.clone(),
            owner: new_owner,
            waiters,
        }),
    }

    [old_owner.filter(|&old| old != new_owner), Some(new_owner)]
}

/// Recomputes the policy of the thread `tid` from the futexes it owns and applies it.
///
/// `thread` is the thread, if the caller knows it. It is only needed to boost a thread for the
/// first time; one that has been boosted is remembered until it is unboosted.
pub(super) fn apply(tid: Tid, thread: Option<&Arc<Thread>>) {
    let Some(policy) = POLICY.get() else {
        return;
    };
    let mut state = STATE.lock();

    let (thread, base, applied) = match state.owners.get(&tid) {
        Some(owner) => {
            // If the thread's policy is not the one we applied, it was changed with
            // `sched_setscheduler`, which replaces its own policy.
            let current = owner.thread.sched_attr().policy();
            let base = if current == owner.applied {
                owner.base
            } else {
                current
            };
            (owner.thread.clone(), base, current)
        }
        None => {
            let Some(thread) = thread else {
                return;
            };
            let current = thread.sched_attr().policy();
            (thread.clone(), current, current)
        }
    };

    let mut contended_futexes = 0;
    let mut target = base;
    for record in state.futexes.iter().filter(|record| record.owner == tid) {
        contended_futexes += 1;
        let contention = PiContention {
            owner: tid,
            owner_base: base,
            waiters: &record.waiters,
        };
        if let Some(boost) = policy.boost(&contention) {
            // A smaller `SchedPolicy` is more urgent.
            target = target.min(boost);
        }
    }

    if target == base {
        state.owners.remove(&tid);
    } else {
        state.owners.insert(
            tid,
            OwnerRecord {
                thread: thread.clone(),
                base,
                applied: target,
            },
        );
    }
    if target != applied {
        // Apply with the lock held so that concurrent changes are applied in order.
        set_policy_requeued(&thread, target);
        decisions::produce(tid, applied, target, contended_futexes);
    }
}

/// Returns the policy that a thread cloned from `thread`, whose TID is `tid`, starts with.
///
/// Like Linux, a clone inherits the parent's own policy, not one the parent was boosted to.
pub(in crate::process) fn inheritable_policy(thread: &Thread, tid: Tid) -> SchedPolicy {
    let own_policy = thread.sched_attr().inheritable_policy();
    if !is_enabled() {
        return own_policy;
    }
    let state = STATE.lock();
    match state.owners.get(&tid) {
        // As in `apply`, a policy other than the applied one was set with `sched_setscheduler`.
        Some(owner) if own_policy == owner.applied => owner.base,
        _ => own_policy,
    }
}

/// Forgets the thread `tid`, which is exiting, so that a thread reusing its TID is not boosted.
pub(super) fn forget_thread(tid: Tid) {
    if !is_enabled() {
        return;
    }
    let mut state = STATE.lock();
    state.owners.remove(&tid);
    state.futexes.retain(|record| record.owner != tid);
}

#[cfg(not(baseline_asterinas))]
mod decisions {
    use ostd::{
        orpc::oqueue::{GenericOQueueRef, OQueue as _, OQueueBase as _, RefProducer, registry},
        path,
    };
    use serde::Serialize;
    use spin::Once;

    use crate::{
        process::posix_thread::pi_futex_events::now_ns,
        sched::{SchedPolicy, SchedPolicyView},
        thread::Tid,
    };

    /// A change to a PI futex owner's policy.
    #[derive(Clone, Copy, Debug, Serialize)]
    pub struct LockPrioDecision {
        /// The monotonic time of the change, in nanoseconds.
        pub time_ns: u64,
        pub owner: Tid,
        pub from: SchedPolicyView,
        pub to: SchedPolicyView,
        /// The number of contended PI futexes the owner holds.
        pub contended_futexes: u32,
    }

    static PRODUCER: Once<RefProducer<LockPrioDecision>> = Once::new();

    pub fn init() {
        PRODUCER.call_once(|| {
            let path = path!(lockprio.decisions);
            let oqueue = GenericOQueueRef::new(1024, path.clone());
            // Export the queue under the OQueue filesystem (`/oqueues/lockprio/decisions`).
            registry::register(&path, &oqueue.as_any_oqueue());
            oqueue
                .attach_ref_producer()
                .expect("a new OQueue always allows a ref producer")
        });
    }

    pub fn produce(owner: Tid, from: SchedPolicy, to: SchedPolicy, contended_futexes: u32) {
        if let Some(producer) = PRODUCER.get() {
            // Drop the decision if the queue is full; never block on an observer.
            let _ = producer.try_produce_ref(&LockPrioDecision {
                time_ns: now_ns(),
                owner,
                from: from.into(),
                to: to.into(),
                contended_futexes,
            });
        }
    }
}

#[cfg(baseline_asterinas)]
mod decisions {
    use crate::{sched::SchedPolicy, thread::Tid};

    pub fn init() {}

    pub fn produce(_owner: Tid, _from: SchedPolicy, _to: SchedPolicy, _contended_futexes: u32) {}
}
