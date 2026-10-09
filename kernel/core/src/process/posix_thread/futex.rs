// SPDX-License-Identifier: MPL-2.0

use int_to_c_enum::TryFromInt;
use ostd::{
    cpu::num_cpus,
    sync::{PreemptDisabled, Waiter, Waker},
};
use spin::Once;

use crate::{
    context::current_userspace,
    prelude::*,
    process::{VmarSnapshot, pid_table},
    sched::SchedPolicy,
    thread::{AsThread, Thread, Tid},
    time::wait::{ManagedTimeout, TimeoutExt},
    vm::{page_cache::Vmo, perms::VmPerms, vmar::PageFaultInfo},
};

type FutexBitSet = u32;

const FUTEX_BITSET_MATCH_ANY: FutexBitSet = 0xFFFF_FFFF;

/// Set in a PI futex word when the kernel may have waiters queued on it, forcing the owner into
/// [`futex_unlock_pi`] instead of releasing the lock in userspace.
pub(super) const FUTEX_WAITERS: u32 = 0x8000_0000;
/// Set in a robust futex word when its owner exited while holding it.
pub(super) const FUTEX_OWNER_DIED: u32 = 0x4000_0000;
/// The bits of a PI or robust futex word that hold the owner's TID.
pub(super) const FUTEX_TID_MASK: u32 = 0x3FFF_FFFF;

/// Specifies whether a futex is scoped to the current process or shared through
/// its backing mapping.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FutexVisibility {
    /// Uses the calling process's private futex namespace.
    Private,
    /// Derives the futex identity from the backing mapping when possible, and
    /// otherwise falls back to a process-local shared identity.
    Shared,
}

impl From<FutexFlags> for FutexVisibility {
    fn from(flags: FutexFlags) -> Self {
        if flags.contains(FutexFlags::FUTEX_PRIVATE) {
            Self::Private
        } else {
            Self::Shared
        }
    }
}

pub fn futex_wait(
    futex_addr: u64,
    futex_val: i32,
    timeout: Option<ManagedTimeout>,
    ctx: &Context,
    visibility: FutexVisibility,
) -> Result<()> {
    futex_wait_bitset(
        futex_addr as _,
        futex_val,
        timeout,
        FUTEX_BITSET_MATCH_ANY,
        ctx,
        visibility,
    )
}

pub fn futex_wait_bitset(
    futex_addr: Vaddr,
    futex_val: i32,
    timeout: Option<ManagedTimeout>,
    bitset: FutexBitSet,
    ctx: &Context,
    visibility: FutexVisibility,
) -> Result<()> {
    debug!(
        "futex_wait_bitset: addr = {:#x}, val = {}, bitset = {:#x}",
        futex_addr, futex_val, bitset
    );

    if bitset == 0 {
        return_errno_with_message!(Errno::EINVAL, "at least one bit should be set");
    }

    let user_space = ctx.user_space();
    let futex_key = FutexKey::new(futex_addr, bitset, &user_space, visibility)?;
    let (_, futex_bucket_ref) = get_futex_bucket(&futex_key);
    let (futex_item, waiter) = FutexItem::create(futex_key);

    let (mut futex_bucket, val) = loop {
        // Lock the futex bucket first to avoid race conditions.
        let futex_bucket = futex_bucket_ref.lock();

        let pf_result = ctx
            .thread_local
            .with_page_fault_disabled(|| user_space.atomic_load::<u32>(futex_addr));
        if let Some(result) = pf_result {
            break (futex_bucket, result?);
        }

        drop(futex_bucket);

        // The futex word is aligned on a 4-byte boundary, so it cannot cross the page boundary.
        user_space
            .vmar()
            .handle_page_fault(&PageFaultInfo::new(futex_addr, VmPerms::READ))
            .map_err(|_| {
                Error::with_message(
                    Errno::EFAULT,
                    "the page fault of the futex word cannot be resolved",
                )
            })?;
    };

    if val != futex_val.cast_unsigned() {
        return_errno_with_message!(
            Errno::EAGAIN,
            "the futex word does not contain the expected value"
        );
    }

    futex_bucket.add_item(futex_item);

    // Release the lock.
    drop(futex_bucket);

    let result = waiter.pause_timeout(&timeout.into());

    // If the futex wait operation was interrupted by a signal or timed out, the
    // `FutexItem` must be dequeued and dropped. Otherwise, malicious user programs
    // could repeatedly issue futex wait operations to exhaust kernel memory.
    let item = futex_bucket_ref.lock().remove_by_waker(&waiter.waker());
    if item.is_none() {
        // The futex item will be removed asynchronously if and only if it has been woken up. In
        // that case, we should report success to the user.
        // FIXME: `pause_timeout` should return `Ok(())` in this case, but it currently may return
        // errors due to race conditions.
        Ok(())
    } else if let Err(err) = result {
        Err(err)
    } else {
        // Spurious wakeups. Return `EINTR` anyway.
        return_errno_with_message!(
            Errno::EINTR,
            "the current thread is interrupted by a signal"
        );
    }
}

pub fn futex_wake(
    futex_addr: Vaddr,
    max_count: usize,
    visibility: FutexVisibility,
) -> Result<usize> {
    futex_wake_bitset(futex_addr, max_count, FUTEX_BITSET_MATCH_ANY, visibility)
}

pub fn futex_wake_bitset(
    futex_addr: Vaddr,
    max_count: usize,
    bitset: FutexBitSet,
    visibility: FutexVisibility,
) -> Result<usize> {
    debug!(
        "futex_wake_bitset: addr = {:#x}, max_count = {}, bitset = {:#x}",
        futex_addr, max_count, bitset
    );

    if bitset == 0 {
        return_errno_with_message!(Errno::EINVAL, "at least one bit should be set");
    }

    let futex_key = FutexKey::new(futex_addr, bitset, &current_userspace!(), visibility)?;
    let (_, futex_bucket_ref) = get_futex_bucket(&futex_key);
    let mut futex_bucket = futex_bucket_ref.lock();
    let res = futex_bucket.remove_and_wake_items(&futex_key, max_count);

    Ok(res)
}

/// This struct encodes the operation and comparison that are to be performed during
/// the futex operation with `FUTEX_WAKE_OP`.
///
/// The encoding is as follows:
///
/// +---+---+-----------+-----------+
/// |op |cmp|   oparg   |  cmparg   |
/// +---+---+-----------+-----------+
///   4   4       12          12    <== # of bits
///
/// Reference: <https://man7.org/linux/man-pages/man2/futex.2.html>.
struct FutexWakeOpEncode {
    op: FutexWakeOp,
    /// A flag indicating that the operation will use `1 << oparg`
    /// as the operand instead of `oparg` when it is set `true`.
    ///
    /// e.g. With this flag, [`FutexWakeOp::FUTEX_OP_ADD`] will be interpreted
    /// as `res = (1 << oparg) + oldval`.
    is_oparg_shift: bool,
    cmp: FutexWakeCmp,
    oparg: u32,
    cmparg: u32,
}

#[expect(non_camel_case_types)]
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, TryFromInt)]
enum FutexWakeOp {
    /// Calculate `res = oparg`.
    FUTEX_OP_SET = 0,
    /// Calculate `res = oparg + oldval`.
    FUTEX_OP_ADD = 1,
    /// Calculate `res = oparg | oldval`.
    FUTEX_OP_OR = 2,
    /// Calculate `res = oparg & !oldval`.
    FUTEX_OP_ANDN = 3,
    /// Calculate `res = oparg ^ oldval`.
    FUTEX_OP_XOR = 4,
}

#[expect(non_camel_case_types)]
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, TryFromInt)]
enum FutexWakeCmp {
    /// If (oldval == cmparg) do wake.
    FUTEX_OP_CMP_EQ = 0,
    /// If (oldval != cmparg) do wake.
    FUTEX_OP_CMP_NE = 1,
    /// If (oldval < cmparg) do wake.
    FUTEX_OP_CMP_LT = 2,
    /// If (oldval <= cmparg) do wake.
    FUTEX_OP_CMP_LE = 3,
    /// If (oldval > cmparg) do wake.
    FUTEX_OP_CMP_GT = 4,
    /// If (oldval >= cmparg) do wake.
    FUTEX_OP_CMP_GE = 5,
}

impl FutexWakeOpEncode {
    fn from_u32(bits: u32) -> Result<Self> {
        let is_oparg_shift = (bits >> 31) & 1 == 1;
        let op = FutexWakeOp::try_from((bits >> 28) & 0x7)?;
        let cmp = FutexWakeCmp::try_from((bits >> 24) & 0xf)?;
        let oparg = (bits >> 12) & 0xfff;
        let cmparg = bits & 0xfff;

        Ok(FutexWakeOpEncode {
            op,
            is_oparg_shift,
            cmp,
            oparg,
            cmparg,
        })
    }

    fn calculate_new_val(&self, old_val: u32) -> u32 {
        let oparg = if self.is_oparg_shift {
            if self.oparg > 31 {
                // Linux might return EINVAL in the future
                // Reference: https://elixir.bootlin.com/linux/v6.15.2/source/kernel/futex/waitwake.c#L211-L222
                warn!("futex_wake_op: program tries to shift op by {}", self.oparg);
            }

            1 << (self.oparg & 31)
        } else {
            self.oparg
        };

        match self.op {
            FutexWakeOp::FUTEX_OP_SET => oparg,
            FutexWakeOp::FUTEX_OP_ADD => oparg.wrapping_add(old_val),
            FutexWakeOp::FUTEX_OP_OR => oparg | old_val,
            FutexWakeOp::FUTEX_OP_ANDN => !oparg & old_val,
            FutexWakeOp::FUTEX_OP_XOR => oparg ^ old_val,
        }
    }

    fn should_wake(&self, old_val: u32) -> bool {
        match self.cmp {
            FutexWakeCmp::FUTEX_OP_CMP_EQ => old_val == self.cmparg,
            FutexWakeCmp::FUTEX_OP_CMP_NE => old_val != self.cmparg,
            FutexWakeCmp::FUTEX_OP_CMP_LT => old_val < self.cmparg,
            FutexWakeCmp::FUTEX_OP_CMP_LE => old_val <= self.cmparg,
            FutexWakeCmp::FUTEX_OP_CMP_GT => old_val > self.cmparg,
            FutexWakeCmp::FUTEX_OP_CMP_GE => old_val >= self.cmparg,
        }
    }
}

pub fn futex_wake_op(
    futex_addr_1: Vaddr,
    futex_addr_2: Vaddr,
    max_count_1: usize,
    max_count_2: usize,
    wake_op_bits: u32,
    ctx: &Context,
    visibility: FutexVisibility,
) -> Result<usize> {
    let wake_op = FutexWakeOpEncode::from_u32(wake_op_bits)?;

    let user_space = ctx.user_space();
    let futex_key_1 = FutexKey::new(
        futex_addr_1,
        FUTEX_BITSET_MATCH_ANY,
        &user_space,
        visibility,
    )?;
    let futex_key_2 = FutexKey::new(
        futex_addr_2,
        FUTEX_BITSET_MATCH_ANY,
        &user_space,
        visibility,
    )?;

    let (mut futex_bucket_1, mut futex_bucket_2, old_val) = loop {
        let (futex_bucket_1, futex_bucket_2) = lock_bucket_pairs(&futex_key_1, &futex_key_2);

        let pf_result = ctx.thread_local.with_page_fault_disabled(|| {
            user_space
                .atomic_fetch_update::<u32>(futex_addr_2, |val| wake_op.calculate_new_val(val))
        });
        if let Some(result) = pf_result {
            break (futex_bucket_1, futex_bucket_2, result?);
        }

        drop(futex_bucket_1);
        drop(futex_bucket_2);

        // The futex word is aligned on a 4-byte boundary, so it cannot cross the page boundary.
        user_space
            .vmar()
            .handle_page_fault(&PageFaultInfo::new(
                futex_addr_2,
                VmPerms::READ | VmPerms::WRITE,
            ))
            .map_err(|_| {
                Error::with_message(
                    Errno::EFAULT,
                    "the page fault of the futex word cannot be resolved",
                )
            })?;
    };

    let mut res = futex_bucket_1.remove_and_wake_items(&futex_key_1, max_count_1);
    if wake_op.should_wake(old_val) {
        let bucket = futex_bucket_2.as_mut().unwrap_or(&mut futex_bucket_1);
        res += bucket.remove_and_wake_items(&futex_key_2, max_count_2);
    }

    Ok(res)
}

pub fn futex_requeue(
    futex_addr: Vaddr,
    max_nwakes: usize,
    max_nrequeues: usize,
    futex_new_addr: Vaddr,
    ctx: &Context,
    visibility: FutexVisibility,
) -> Result<usize> {
    if futex_new_addr == futex_addr {
        return futex_wake(futex_addr, max_nwakes, visibility);
    }

    let user_space = ctx.user_space();
    let futex_key = FutexKey::new(futex_addr, FUTEX_BITSET_MATCH_ANY, &user_space, visibility)?;
    let futex_new_key = FutexKey::new(
        futex_new_addr,
        FUTEX_BITSET_MATCH_ANY,
        &user_space,
        visibility,
    )?;

    let (mut futex_bucket, futex_new_bucket) = lock_bucket_pairs(&futex_key, &futex_new_key);

    let nwakes = futex_bucket.remove_and_wake_items(&futex_key, max_nwakes);

    if let Some(mut futex_new_bucket) = futex_new_bucket {
        futex_bucket.requeue_items_to_another_bucket(
            &futex_key,
            &mut futex_new_bucket,
            &futex_new_key,
            max_nrequeues,
        );
    } else {
        futex_bucket.update_item_keys(&futex_key, &futex_new_key, max_nrequeues);
    }

    Ok(nwakes)
}

/// Acquires a priority-inheritance (PI) futex, blocking until it is handed over (`FUTEX_LOCK_PI`).
///
/// The futex word holds the owner's TID, so the kernel always knows which thread a waiter is
/// blocked on. Ownership is handed directly to the highest-priority waiter on unlock. This
/// implements the PI *protocol* only: it does not itself change the owner's priority. Boosting
/// the owner is left to a policy layered on top.
///
/// Unlike Linux, a non-robust owner that exits while holding the lock is not detected, so its
/// waiters block until they time out (glibc blocks forever in that case anyway).
pub fn futex_lock_pi(
    futex_addr: Vaddr,
    timeout: Option<ManagedTimeout>,
    ctx: &Context,
    visibility: FutexVisibility,
) -> Result<()> {
    debug!("futex_lock_pi: addr = {:#x}", futex_addr);

    let user_space = ctx.user_space();
    let futex_key = FutexKey::new(futex_addr, FUTEX_BITSET_MATCH_ANY, &user_space, visibility)?;
    let (_, futex_bucket_ref) = get_futex_bucket(&futex_key);
    let tid = ctx.posix_thread.tid();
    let thread = ctx.task.as_thread().unwrap().clone();
    let timeout: TimeoutExt = timeout.into();

    loop {
        let (item, waiter) = FutexPiItem::create(futex_key.clone(), tid, thread.clone());
        let mut item = Some(item);

        let (futex_bucket, acquired) =
            lock_bucket_for_word(futex_addr, futex_bucket_ref, ctx, |bucket| {
                let acquired = try_acquire_pi(&user_space, futex_addr, &futex_key, tid, bucket)?;
                if let PiAcquire::Owned(_) = acquired {
                    bucket.add_pi_item(item.take().unwrap());
                }
                Ok(acquired)
            })?;
        drop(futex_bucket);
        let owner = match acquired {
            PiAcquire::Acquired => return Ok(()),
            PiAcquire::Owned(owner) => owner,
        };

        // Like Linux, refuse to wait on a TID that does not exist. The PID table lock may sleep,
        // so this is checked after the bucket is unlocked.
        if pid_table::pid_table_mut().get_thread(owner).is_none() {
            if futex_bucket_ref
                .lock()
                .remove_pi_by_waker(&waiter.waker())
                .is_none()
            {
                return Ok(());
            }
            let val: u32 = user_space.atomic_load(futex_addr)?;
            if val & FUTEX_TID_MASK == owner {
                return_errno_with_message!(Errno::ESRCH, "the PI futex owner does not exist");
            }
            continue;
        }

        let result = waiter.pause_timeout(&timeout);

        // An unlocker hands over ownership by dequeuing our item, so if it is gone we own the
        // lock, even if the wait also timed out or was interrupted.
        if futex_bucket_ref
            .lock()
            .remove_pi_by_waker(&waiter.waker())
            .is_none()
        {
            return Ok(());
        }
        // Otherwise we were woken without a handover (a spurious wakeup, or the owner died
        // holding a robust futex), so retry unless the wait itself failed.
        result?;
    }
}

/// Acquires a PI futex only if it is free (`FUTEX_TRYLOCK_PI`).
pub fn futex_trylock_pi(
    futex_addr: Vaddr,
    ctx: &Context,
    visibility: FutexVisibility,
) -> Result<()> {
    debug!("futex_trylock_pi: addr = {:#x}", futex_addr);

    let user_space = ctx.user_space();
    let futex_key = FutexKey::new(futex_addr, FUTEX_BITSET_MATCH_ANY, &user_space, visibility)?;
    let (_, futex_bucket_ref) = get_futex_bucket(&futex_key);
    let tid = ctx.posix_thread.tid();

    let (_, acquired) = lock_bucket_for_word(futex_addr, futex_bucket_ref, ctx, |bucket| {
        let owner = user_space.atomic_load::<u32>(futex_addr)? & FUTEX_TID_MASK;
        // Unlike a blocking lock, a failed trylock must not set `FUTEX_WAITERS`.
        if owner != 0 && owner != tid {
            return Ok(PiAcquire::Owned(owner));
        }
        try_acquire_pi(&user_space, futex_addr, &futex_key, tid, bucket)
    })?;

    match acquired {
        PiAcquire::Acquired => Ok(()),
        PiAcquire::Owned(owner) if pid_table::pid_table_mut().get_thread(owner).is_none() => {
            return_errno_with_message!(Errno::ESRCH, "the PI futex owner does not exist")
        }
        PiAcquire::Owned(_) => {
            return_errno_with_message!(Errno::EAGAIN, "the PI futex is owned by another thread")
        }
    }
}

/// Releases a PI futex owned by the current thread, handing it to the highest-priority waiter
/// (`FUTEX_UNLOCK_PI`).
pub fn futex_unlock_pi(
    futex_addr: Vaddr,
    ctx: &Context,
    visibility: FutexVisibility,
) -> Result<()> {
    debug!("futex_unlock_pi: addr = {:#x}", futex_addr);

    let user_space = ctx.user_space();
    let futex_key = FutexKey::new(futex_addr, FUTEX_BITSET_MATCH_ANY, &user_space, visibility)?;
    let (_, futex_bucket_ref) = get_futex_bucket(&futex_key);
    let tid = ctx.posix_thread.tid();

    let (_, new_owner) = lock_bucket_for_word(futex_addr, futex_bucket_ref, ctx, |bucket| {
        let mut val: u32 = user_space.atomic_load(futex_addr)?;
        let next = bucket.top_pi_waiter(&futex_key);
        // Like Linux, the new owner always gets `FUTEX_WAITERS`, so its release goes through the
        // kernel even if no other waiter is left.
        let new_val = next.map_or(0, |index| bucket.pi_items[index].tid | FUTEX_WAITERS);

        loop {
            if val & FUTEX_TID_MASK != tid {
                return_errno_with_message!(
                    Errno::EPERM,
                    "the PI futex is not owned by the current thread"
                );
            }
            match cmpxchg_futex_word(&user_space, futex_addr, val, new_val)? {
                (_, true) => break,
                (cur_val, false) => val = cur_val,
            }
        }

        // The word now names the new owner, so dequeue it to tell it that it owns the lock.
        Ok(next.map(|index| bucket.pi_items.remove(index)))
    })?;

    if let Some(new_owner) = new_owner {
        new_owner.waker.wake_up();
    }

    Ok(())
}

/// Wakes the top PI waiter of a futex without handing it ownership, so that it retries taking
/// the lock. This is used when a robust futex's owner dies and leaves the word unowned.
pub(super) fn futex_wake_pi_waiter(futex_addr: Vaddr, visibility: FutexVisibility) -> Result<()> {
    let futex_key = FutexKey::new(
        futex_addr,
        FUTEX_BITSET_MATCH_ANY,
        &current_userspace!(),
        visibility,
    )?;
    let (_, futex_bucket_ref) = get_futex_bucket(&futex_key);
    let futex_bucket = futex_bucket_ref.lock();
    if let Some(index) = futex_bucket.top_pi_waiter(&futex_key) {
        futex_bucket.pi_items[index].waker.wake_up();
    }
    Ok(())
}

/// The outcome of trying to take a PI futex.
enum PiAcquire {
    /// The current thread now owns the futex.
    Acquired,
    /// The thread with this TID owns the futex, and `FUTEX_WAITERS` is set in its word.
    Owned(Tid),
}

/// Takes the PI futex if it is unowned. Otherwise, makes sure `FUTEX_WAITERS` is set so that the
/// owner's release goes through the kernel.
///
/// Must be called with the futex's bucket locked and page faults disabled.
fn try_acquire_pi(
    user_space: &CurrentUserSpace,
    futex_addr: Vaddr,
    futex_key: &FutexKey,
    tid: Tid,
    bucket: &FutexBucket,
) -> Result<PiAcquire> {
    let mut val: u32 = user_space.atomic_load(futex_addr)?;
    loop {
        let owner = val & FUTEX_TID_MASK;
        let (new_val, outcome) = if owner == 0 {
            // Keep `FUTEX_OWNER_DIED` so that glibc reports `EOWNERDEAD` for robust mutexes.
            let waiters = if bucket.top_pi_waiter(futex_key).is_some() {
                FUTEX_WAITERS
            } else {
                0
            };
            (
                (val & FUTEX_OWNER_DIED) | waiters | tid,
                PiAcquire::Acquired,
            )
        } else if owner == tid {
            return_errno_with_message!(
                Errno::EDEADLK,
                "the PI futex is already owned by the caller"
            );
        } else if val & FUTEX_WAITERS != 0 {
            return Ok(PiAcquire::Owned(owner));
        } else {
            (val | FUTEX_WAITERS, PiAcquire::Owned(owner))
        };

        match cmpxchg_futex_word(user_space, futex_addr, val, new_val)? {
            (_, true) => return Ok(outcome),
            (cur_val, false) => val = cur_val,
        }
    }
}

/// Atomically compares and exchanges a futex word, returning the value it held and whether the
/// exchange happened.
fn cmpxchg_futex_word(
    user_space: &CurrentUserSpace,
    futex_addr: Vaddr,
    old_val: u32,
    new_val: u32,
) -> Result<(u32, bool)> {
    let (reader, writer) = user_space.reader_writer(futex_addr, size_of::<u32>())?;
    Ok(writer.atomic_compare_exchange(&reader, old_val, new_val)?)
}

/// Locks a futex bucket and runs `op`, which may access the futex word, with page faults
/// disabled. If `op` faults, the page is faulted in with the bucket unlocked and `op` is retried,
/// so `op` must not change the bucket before its last access to user memory.
fn lock_bucket_for_word<R>(
    futex_addr: Vaddr,
    futex_bucket_ref: &'static SpinLock<FutexBucket>,
    ctx: &Context,
    mut op: impl FnMut(&mut FutexBucket) -> Result<R>,
) -> Result<(SpinLockGuard<'static, FutexBucket, PreemptDisabled>, R)> {
    loop {
        let mut futex_bucket = futex_bucket_ref.lock();
        if let Some(result) = ctx
            .thread_local
            .with_page_fault_disabled(|| op(&mut futex_bucket))
        {
            return result.map(|ret| (futex_bucket, ret));
        }
        drop(futex_bucket);

        // The futex word is aligned on a 4-byte boundary, so it cannot cross the page boundary.
        ctx.user_space()
            .vmar()
            .handle_page_fault(&PageFaultInfo::new(
                futex_addr,
                VmPerms::READ | VmPerms::WRITE,
            ))
            .map_err(|_| {
                Error::with_message(
                    Errno::EFAULT,
                    "the page fault of the futex word cannot be resolved",
                )
            })?;
    }
}

static FUTEX_BUCKETS: Once<FutexBucketVec> = Once::new();

/// Gets the futex hash bucket count.
///
/// This number is calculated the same way as Linux's:
/// <https://elixir.bootlin.com/linux/v6.17/source/kernel/futex/core.c#L1981>.
fn get_bucket_count() -> usize {
    ((1 << 8) * num_cpus()).next_power_of_two()
}

fn get_futex_bucket(key: &FutexKey) -> (usize, &'static SpinLock<FutexBucket>) {
    FUTEX_BUCKETS.get().unwrap().get_bucket(key)
}

/// Locks the futex buckets associated with a given pair of keys in a consistent
/// order to avoid deadlock.
///
/// The order is defined by comparing the respective bucket indices and then locking
/// the one with the lowest index first. If both keys belong to the same bucket,
/// lock it and return the second lock guard as `None`.
fn lock_bucket_pairs(
    futex_key_1: &FutexKey,
    futex_key_2: &FutexKey,
) -> (
    SpinLockGuard<'static, FutexBucket, PreemptDisabled>,
    Option<SpinLockGuard<'static, FutexBucket, PreemptDisabled>>,
) {
    let (index_1, futex_bucket_ref_1) = get_futex_bucket(futex_key_1);
    let (index_2, futex_bucket_ref_2) = get_futex_bucket(futex_key_2);

    match index_1.cmp(&index_2) {
        core::cmp::Ordering::Equal => (futex_bucket_ref_1.lock(), None),
        core::cmp::Ordering::Less => {
            let bucket_1 = futex_bucket_ref_1.lock();
            let bucket_2 = futex_bucket_ref_2.lock();
            (bucket_1, Some(bucket_2))
        }
        core::cmp::Ordering::Greater => {
            let bucket_2 = futex_bucket_ref_2.lock();
            let bucket_1 = futex_bucket_ref_1.lock();
            (bucket_1, Some(bucket_2))
        }
    }
}

/// Initializes the futex system.
pub fn init() {
    FUTEX_BUCKETS.call_once(|| FutexBucketVec::new(get_bucket_count()));
}

struct FutexBucketVec {
    vec: Vec<SpinLock<FutexBucket>>,
}

impl FutexBucketVec {
    pub(self) fn new(size: usize) -> FutexBucketVec {
        let mut buckets = FutexBucketVec {
            vec: Vec::with_capacity(size),
        };
        for _ in 0..size {
            let bucket = SpinLock::new(FutexBucket::new());
            buckets.vec.push(bucket);
        }
        buckets
    }

    pub(self) fn get_bucket(&self, key: &FutexKey) -> (usize, &SpinLock<FutexBucket>) {
        // Since `self.size()` is known to be a power of 2, the following is
        // equivalent to `key.hash % self.size()`, buf faster.
        let index = key.hash & (self.size() - 1);
        (index, &self.vec[index])
    }

    pub(self) fn size(&self) -> usize {
        self.vec.len()
    }
}

struct FutexBucket {
    items: Vec<FutexItem>,
    /// Waiters blocked in [`futex_lock_pi`], kept apart from `items` so that the non-PI wake and
    /// requeue operations never dequeue them without handing over ownership.
    pi_items: Vec<FutexPiItem>,
}

impl FutexBucket {
    pub(self) fn new() -> FutexBucket {
        FutexBucket {
            items: Vec::with_capacity(1),
            pi_items: Vec::new(),
        }
    }

    pub(self) fn add_pi_item(&mut self, item: FutexPiItem) {
        self.pi_items.push(item);
    }

    pub(self) fn remove_pi_by_waker(&mut self, waker: &Arc<Waker>) -> Option<FutexPiItem> {
        let idx = self
            .pi_items
            .iter()
            .position(|item| Arc::ptr_eq(&item.waker, waker))?;
        Some(self.pi_items.remove(idx))
    }

    /// Returns the index of the PI waiter on `key` that should own the futex next.
    ///
    /// Like Linux, real-time waiters are ordered by priority and all other waiters rank equally,
    /// so their nice values do not matter. Ties go to the longest-waiting thread.
    ///
    /// Ref: <https://elixir.bootlin.com/linux/v7.0-rc7/source/kernel/locking/rtmutex.c#L349>
    pub(self) fn top_pi_waiter(&self, key: &FutexKey) -> Option<usize> {
        self.pi_items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.key.match_up(key))
            // `min_by_key` keeps the first of equal elements, i.e., the earliest waiter.
            .min_by_key(|(_, item)| match item.thread.sched_attr().policy() {
                // A lower `rt_prio` is a higher priority.
                SchedPolicy::RealTime { rt_prio, .. } => (0, rt_prio.get()),
                _ => (1, 0),
            })
            .map(|(index, _)| index)
    }

    pub(self) fn add_item(&mut self, item: FutexItem) {
        self.items.push(item);
    }

    pub(self) fn remove_by_waker(&mut self, waker: &Arc<Waker>) -> Option<FutexItem> {
        let idx = self
            .items
            .iter()
            .position(|item| Arc::ptr_eq(&item.waker, waker))?;
        Some(self.items.swap_remove(idx))
    }

    pub(self) fn remove_and_wake_items(&mut self, key: &FutexKey, max_count: usize) -> usize {
        let mut count = 0;

        self.items.retain(|item| {
            if item.key.match_up(key) && count < max_count && item.wake() {
                count += 1;
                false
            } else {
                true
            }
        });

        count
    }

    pub(self) fn update_item_keys(&mut self, key: &FutexKey, new_key: &FutexKey, max_count: usize) {
        let mut count = 0;
        for item in self.items.iter_mut() {
            if item.key.match_up(key) {
                item.key = new_key.clone();
                count += 1;
            }
            if count >= max_count {
                break;
            }
        }
    }

    pub(self) fn requeue_items_to_another_bucket(
        &mut self,
        key: &FutexKey,
        another: &mut Self,
        new_key: &FutexKey,
        max_nrequeues: usize,
    ) {
        let mut count = 0;
        self.items
            .extract_if(.., |item| {
                if item.key.match_up(key) && count < max_nrequeues {
                    count += 1;
                    true
                } else {
                    false
                }
            })
            .for_each(|mut extracted| {
                extracted.key = new_key.clone();
                another.add_item(extracted);
            });
    }
}

struct FutexItem {
    key: FutexKey,
    waker: Arc<Waker>,
}

impl FutexItem {
    pub(self) fn create(key: FutexKey) -> (Self, Waiter) {
        let (waiter, waker) = Waiter::new_pair();
        let futex_item = FutexItem { key, waker };

        (futex_item, waiter)
    }

    #[must_use]
    pub(self) fn wake(&self) -> bool {
        self.waker.wake_up()
    }
}

/// A thread blocked in [`futex_lock_pi`].
struct FutexPiItem {
    key: FutexKey,
    waker: Arc<Waker>,
    tid: Tid,
    thread: Arc<Thread>,
}

impl FutexPiItem {
    pub(self) fn create(key: FutexKey, tid: Tid, thread: Arc<Thread>) -> (Self, Waiter) {
        let (waiter, waker) = Waiter::new_pair();
        let item = FutexPiItem {
            key,
            waker,
            tid,
            thread,
        };

        (item, waiter)
    }
}

/// The identity used for futex namespace and backing selection.
#[derive(Clone, Debug)]
enum FutexIdentity {
    Private { vmar: VmarSnapshot, addr: Vaddr },
    SharedLocal { vmar: VmarSnapshot, addr: Vaddr },
    Shared { vmo: Weak<Vmo>, offset: usize },
}

/// The lookup key used for futex bucket selection and matching.
#[derive(Clone, Debug)]
struct FutexKey {
    /// A hash value deterministically computed from the futex identity.
    hash: usize,
    identity: FutexIdentity,
    bitset: FutexBitSet,
}

impl FutexKey {
    pub(self) fn new(
        addr: Vaddr,
        bitset: FutexBitSet,
        user_space: &CurrentUserSpace<'_>,
        visibility: FutexVisibility,
    ) -> Result<Self> {
        // "On all platforms, futexes are four-byte integers that must be aligned on a four-byte
        // boundary."
        // Reference: <https://man7.org/linux/man-pages/man2/futex.2.html>.
        if !addr.is_multiple_of(align_of::<u32>()) {
            return_errno_with_message!(
                Errno::EINVAL,
                "the futex word is not aligned on a four-byte boundary"
            );
        }

        let identity = match visibility {
            FutexVisibility::Private => FutexIdentity::Private {
                vmar: user_space.vmar_snapshot(),
                addr,
            },
            FutexVisibility::Shared => {
                let backing = user_space
                    .vmar()
                    .query(addr..addr + 1)
                    .iter()
                    .next()
                    .map(|mapping| mapping.futex_backing(addr))
                    .transpose()?
                    .flatten();

                backing.map_or(
                    FutexIdentity::SharedLocal {
                        vmar: user_space.vmar_snapshot(),
                        addr,
                    },
                    |(vmo, offset)| FutexIdentity::Shared { vmo, offset },
                )
            }
        };

        let hash = identity.hash();

        Ok(Self {
            hash,
            identity,
            bitset,
        })
    }

    pub(self) fn match_up(&self, another: &Self) -> bool {
        self.identity.match_up(&another.identity) && (self.bitset & another.bitset) != 0
    }
}

impl FutexIdentity {
    fn hash(&self) -> usize {
        // Use a distinct `initval` salt for each futex namespace so that
        // `Private`, `SharedLocal`, and `Shared` keys do not collapse into the
        // same buckets when their folded key material happens to match.
        match self {
            FutexIdentity::Private { vmar, addr } => {
                let vmar_id = vmar.as_ptr() as u64;
                let vmar_mix = (vmar_id as u32) ^ ((vmar_id >> 32) as u32).rotate_left(16);
                jhash::jhash_3vals(*addr as u32, (*addr >> 32) as u32, vmar_mix, 0) as usize
            }
            FutexIdentity::SharedLocal { vmar, addr } => {
                let vmar_id = vmar.as_ptr() as u64;
                let vmar_mix = (vmar_id as u32) ^ ((vmar_id >> 32) as u32).rotate_left(16);
                jhash::jhash_3vals(*addr as u32, (*addr >> 32) as u32, vmar_mix, 1) as usize
            }
            FutexIdentity::Shared { vmo, offset } => {
                let vmo_id = Weak::as_ptr(vmo) as u64;
                let vmo_mix = (vmo_id as u32) ^ ((vmo_id >> 32) as u32).rotate_left(16);
                jhash::jhash_3vals(*offset as u32, (*offset >> 32) as u32, vmo_mix, 2) as usize
            }
        }
    }

    fn match_up(&self, another: &Self) -> bool {
        match (self, another) {
            (
                FutexIdentity::Private {
                    vmar: left_vmar,
                    addr: left_addr,
                },
                FutexIdentity::Private {
                    vmar: right_vmar,
                    addr: right_addr,
                },
            )
            | (
                FutexIdentity::SharedLocal {
                    vmar: left_vmar,
                    addr: left_addr,
                },
                FutexIdentity::SharedLocal {
                    vmar: right_vmar,
                    addr: right_addr,
                },
            ) => left_addr == right_addr && left_vmar.ptr_eq(right_vmar),
            (
                FutexIdentity::Shared {
                    vmo: left_vmo,
                    offset: left_offset,
                },
                FutexIdentity::Shared {
                    vmo: right_vmo,
                    offset: right_offset,
                },
            ) => left_offset == right_offset && Weak::ptr_eq(left_vmo, right_vmo),
            _ => false,
        }
    }
}

// Reference: <https://elixir.bootlin.com/linux/v6.18.2/source/include/uapi/linux/futex.h#L11>
#[expect(non_camel_case_types)]
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, TryFromInt)]
pub enum FutexOp {
    FUTEX_WAIT = 0,
    FUTEX_WAKE = 1,
    FUTEX_FD = 2,
    FUTEX_REQUEUE = 3,
    FUTEX_CMP_REQUEUE = 4,
    FUTEX_WAKE_OP = 5,
    FUTEX_LOCK_PI = 6,
    FUTEX_UNLOCK_PI = 7,
    FUTEX_TRYLOCK_PI = 8,
    FUTEX_WAIT_BITSET = 9,
    FUTEX_WAKE_BITSET = 10,
}

bitflags! {
    // Reference: <https://elixir.bootlin.com/linux/v6.18.2/source/include/uapi/linux/futex.h#L26>
    pub struct FutexFlags : u32 {
        const FUTEX_PRIVATE         = 128;
        const FUTEX_CLOCK_REALTIME  = 256;
    }
}

pub fn futex_op_and_flags_from_u32(bits: u32) -> Result<(FutexOp, FutexFlags)> {
    const FUTEX_OP_MASK: u32 = 0x0000_000F;
    const FUTEX_FLAGS_MASK: u32 = 0xFFFF_FFF0;

    let op = {
        let op_bits = bits & FUTEX_OP_MASK;
        FutexOp::try_from(op_bits)?
    };

    let flags = {
        let flags_bits = bits & FUTEX_FLAGS_MASK;
        FutexFlags::from_bits(flags_bits)
            .ok_or_else(|| Error::with_message(Errno::EINVAL, "invalid futex flags"))?
    };

    Ok((op, flags))
}
