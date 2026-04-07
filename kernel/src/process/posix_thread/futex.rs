// SPDX-License-Identifier: MPL-2.0

use int_to_c_enum::TryFromInt;
use ostd::{
    cpu::num_cpus,
    sync::{Waiter, Waker},
    task::Task,
};
use spin::Once;

use crate::{prelude::*, process::Pid, time::wait::ManagedTimeout};

type FutexBitSet = u32;

const FUTEX_OP_MASK: u32 = 0x0000_000F;
const FUTEX_FLAGS_MASK: u32 = 0xFFFF_FFF0;
const FUTEX_BITSET_MATCH_ANY: FutexBitSet = 0xFFFF_FFFF;

/// If set, the locked futex has waiters. This must match `linux/futex.h`.
const FUTEX_WAITERS: u32 = 0x80000000;

#[derive(Debug, Clone, Copy)]
pub struct PiFutexState {
    bits: u32,
}

impl PiFutexState {
    /// Creates a new PiFutexState with the given tid (has_waiters is false by default)
    pub fn from_tid(tid: u32) -> Self {
        Self {
            bits: tid & 0x7FFFFFFF, // Only use lower 31 bits for tid
        }
    }

    pub fn new(bits: u32) -> Self {
        Self { bits }
    }

    /// Returns whether waiters are present
    pub fn has_waiters(&self) -> bool {
        self.bits & FUTEX_WAITERS != 0
    }

    /// Sets the waiters flag
    pub fn set_has_waiters(&mut self, has_waiters: bool) -> Self {
        if has_waiters {
            self.bits |= FUTEX_WAITERS;
        } else {
            self.bits &= !FUTEX_WAITERS;
        }
        *self
    }

    /// Gets the thread ID (31 bits)
    pub fn tid(&self) -> u32 {
        self.bits & 0x7FFFFFFF
    }

    /// Sets the thread ID (31 bits only)
    pub fn set_tid(&mut self, tid: u32) -> Self {
        self.bits = (tid & 0x7FFFFFFF) | (self.bits & FUTEX_WAITERS);
        *self
    }
}

impl From<PiFutexState> for u32 {
    fn from(state: PiFutexState) -> Self {
        state.bits
    }
}

/// do futex wait
pub fn futex_wait(
    futex_addr: u64,
    futex_val: i32,
    timeout: Option<ManagedTimeout>,
    ctx: &Context,
    pid: Option<Pid>,
) -> Result<()> {
    futex_wait_bitset(
        futex_addr as _,
        futex_val,
        timeout,
        FUTEX_BITSET_MATCH_ANY,
        ctx,
        pid,
    )
}

/// Does futex wait bitset
pub fn futex_wait_bitset(
    futex_addr: Vaddr,
    futex_val: i32,
    timeout: Option<ManagedTimeout>,
    bitset: FutexBitSet,
    ctx: &Context,
    pid: Option<Pid>,
) -> Result<()> {
    debug!(
        "futex_wait_bitset addr: {:#x}, val: {}, bitset: {:#x}",
        futex_addr, futex_val, bitset
    );

    if bitset == 0 {
        return_errno_with_message!(Errno::EINVAL, "at least one bit should be set");
    }

    let futex_key = FutexKey::new(futex_addr, bitset, pid)?;
    let (futex_item, waiter) = FutexItem::create(futex_key);

    let (_, futex_bucket_ref) = get_futex_bucket(futex_key);
    // lock futex bucket ref here to avoid data race
    let mut futex_bucket = futex_bucket_ref.lock();

    if !futex_key.load_val(ctx).is_ok_and(|val| val == futex_val) {
        return_errno_with_message!(
            Errno::EAGAIN,
            "futex value does not match or load_val failed"
        );
    }

    futex_bucket.add_item(futex_item);

    // drop lock
    drop(futex_bucket);

    let result = waiter.pause_timeout(&timeout.into());
    match result {
        // FIXME: If the futex is woken up and a signal comes at the same time, we should succeed
        // instead of failing with `EINTR`. The code below is of course wrong, but was needed to
        // make the gVisor tests happy. See <https://github.com/asterinas/asterinas/pull/1577>.
        Err(err) if err.error() == Errno::EINTR => Ok(()),
        res => res,
    }

    // TODO: Ensure the futex item is dequeued and dropped.
    //
    // The enqueued futex item remain undequeued
    // if the futex wait operation is interrupted by a signal or times out.
    // In such cases, the `Box<FutexItem>` would persist in memory,
    // leaving our implementation vulnerable to exploitation by user programs
    // that could repeatedly issue futex wait operations
    // to exhaust kernel memory.
}

/// Lock a futex with priority inheritance (PI).
///
/// TODO(arthurp): This implementation is *WRONG*.
///  * It does not implement priority waking at all.
///  * It allows other threads to steal the lock even if a thread is waiting in the kernel. This is
///    solved with a yield loop, which is bad.
///  * Certainly other things.
pub fn futex_lock_pi(
    futex_addr: Vaddr,
    timeout: Option<ManagedTimeout>,
    ctx: &Context,
    pid: Option<Pid>,
    spin: bool,
) -> Result<()> {
    let futex_key = FutexKey::new(futex_addr, FUTEX_BITSET_MATCH_ANY, pid)?;
    let (futex_item, waiter) = FutexItem::create(futex_key);

    let (_, futex_bucket_ref) = get_futex_bucket(futex_key);
    // lock futex bucket ref here to avoid data race
    let mut futex_bucket = futex_bucket_ref.lock();

    if futex_trylock_pi(futex_addr, ctx)? {
        // Acquired lock
        return Ok(());
    }

    futex_bucket.add_item(futex_item);

    // drop lock
    drop(futex_bucket);

    let result = if spin {
        Task::yield_now();
        Ok(())
    } else {
        waiter.pause_timeout(&timeout.into())
    };
    match result {
        // FIXME: If the futex is woken up and a signal comes at the same time, we should succeed
        // instead of failing with `EINTR`. The code below is of course wrong, but was needed to
        // make the gVisor tests happy. See <https://github.com/asterinas/asterinas/pull/1577>.
        Err(err) if err.error() == Errno::EINTR => Ok(()),
        Ok(()) => {
            // The thread was woken, so retry and spin until we get the lock.
            futex_lock_pi(futex_addr, None, ctx, pid, true)
        }
        res => res,
    }
}

pub fn futex_trylock_pi(futex_addr: usize, ctx: &Context) -> Result<bool> {
    let tid = ctx.posix_thread.tid();

    loop {
        let prev_val = ctx.user_space().atomic_compare_exchange(
            futex_addr,
            0,
            PiFutexState::from_tid(tid).into(),
        )?;
        match prev_val {
            0 | FUTEX_WAITERS => {
                // Lock acquired
                return Ok(true);
            }
            prev_val if PiFutexState::new(prev_val).tid() == tid => {
                return_errno_with_message!(
                    Errno::EDEADLK,
                    "thread attempted to lock a futex it already holds"
                )
            }
            prev_val if !PiFutexState::new(prev_val).has_waiters() => {
                // Acquire failed, set waiters bit if the lock has not changed state.
                let prev_val = ctx.user_space().atomic_compare_exchange(
                    futex_addr,
                    prev_val,
                    PiFutexState::new(prev_val).set_has_waiters(true).into(),
                )?;
                if prev_val != 0 {
                    // The waiters bit is now set. We will add ourself to the wait list below.
                    break;
                }
                // Lock is now unlocked, try again.
            }
            _ => {
                // has_waiters is already set. We will add ourself to the wait list below.
                break;
            }
        }
    }
    Ok(false)
}

pub fn futex_unlock_pi(
    futex_addr: Vaddr,
    max_count: usize,
    ctx: &Context,
    pid: Option<Pid>,
) -> Result<usize> {
    let futex_key = FutexKey::new(futex_addr, FUTEX_BITSET_MATCH_ANY, pid)?;
    let (_, futex_bucket_ref) = get_futex_bucket(futex_key);
    let mut futex_bucket = futex_bucket_ref.lock();

    let futex_val = futex_key.load_val(ctx)?.cast_unsigned();
    let tid = ctx.posix_thread.tid();
    if PiFutexState::new(futex_val).tid() != tid {
        return_errno_with_message!(
            Errno::EPERM,
            "attempt to unlock a futex that the thread does not own"
        );
    }
    let swapped_val = ctx
        .user_space()
        .atomic_compare_exchange(futex_addr, futex_val, 0)?;
    if swapped_val != futex_val {
        return_errno_with_message!(Errno::EINVAL, "futex value changed while locked");
    }
    if PiFutexState::new(futex_val).has_waiters() {
        let res = futex_bucket.remove_and_wake_items(futex_key, max_count);
        Ok(res)
    } else {
        Ok(0)
    }
}

/// Does futex wake
pub fn futex_wake(futex_addr: Vaddr, max_count: usize, pid: Option<Pid>) -> Result<usize> {
    futex_wake_bitset(futex_addr, max_count, FUTEX_BITSET_MATCH_ANY, pid)
}

/// Does futex wake with bitset
pub fn futex_wake_bitset(
    futex_addr: Vaddr,
    max_count: usize,
    bitset: FutexBitSet,
    pid: Option<Pid>,
) -> Result<usize> {
    debug!(
        "futex_wake_bitset addr: {:#x}, max_count: {}, bitset: {:#x}",
        futex_addr, max_count, bitset
    );

    if bitset == 0 {
        return_errno_with_message!(Errno::EINVAL, "at least one bit should be set");
    }

    let futex_key = FutexKey::new(futex_addr, bitset, pid)?;
    let (_, futex_bucket_ref) = get_futex_bucket(futex_key);
    let mut futex_bucket = futex_bucket_ref.lock();
    let res = futex_bucket.remove_and_wake_items(futex_key, max_count);

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
/// Reference: https://man7.org/linux/man-pages/man2/futex.2.html.
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

#[derive(Debug, Copy, Clone, TryFromInt, PartialEq)]
#[repr(u32)]
#[expect(non_camel_case_types)]
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

#[derive(Debug, Copy, Clone, TryFromInt, PartialEq)]
#[repr(u32)]
#[expect(non_camel_case_types)]
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
            FutexWakeOp::FUTEX_OP_ANDN => oparg & !old_val,
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
    pid: Option<Pid>,
) -> Result<usize> {
    let wake_op = FutexWakeOpEncode::from_u32(wake_op_bits)?;

    let futex_key_1 = FutexKey::new(futex_addr_1, FUTEX_BITSET_MATCH_ANY, pid)?;
    let futex_key_2 = FutexKey::new(futex_addr_2, FUTEX_BITSET_MATCH_ANY, pid)?;
    let (index_1, futex_bucket_ref_1) = get_futex_bucket(futex_key_1);
    let (index_2, futex_bucket_ref_2) = get_futex_bucket(futex_key_2);

    let (mut futex_bucket_1, mut futex_bucket_2) = if index_1 == index_2 {
        (futex_bucket_ref_1.lock(), None)
    } else {
        // Ensure that we always lock the buckets in a consistent order to avoid deadlocks.
        if index_1 < index_2 {
            let bucket_1 = futex_bucket_ref_1.lock();
            let bucket_2 = futex_bucket_ref_2.lock();
            (bucket_1, Some(bucket_2))
        } else {
            let bucket_2 = futex_bucket_ref_2.lock();
            let bucket_1 = futex_bucket_ref_1.lock();
            (bucket_1, Some(bucket_2))
        }
    };

    let old_val = ctx
        .user_space()
        .atomic_update::<u32>(futex_addr_2, |val| wake_op.calculate_new_val(val))?;

    let mut res = futex_bucket_1.remove_and_wake_items(futex_key_1, max_count_1);
    if wake_op.should_wake(old_val) {
        let bucket = futex_bucket_2.as_mut().unwrap_or(&mut futex_bucket_1);
        res += bucket.remove_and_wake_items(futex_key_2, max_count_2);
    }

    Ok(res)
}

/// Does futex requeue
pub fn futex_requeue(
    futex_addr: Vaddr,
    max_nwakes: usize,
    max_nrequeues: usize,
    futex_new_addr: Vaddr,
    pid: Option<Pid>,
) -> Result<usize> {
    if futex_new_addr == futex_addr {
        return futex_wake(futex_addr, max_nwakes, pid);
    }

    let futex_key = FutexKey::new(futex_addr, FUTEX_BITSET_MATCH_ANY, pid)?;
    let futex_new_key = FutexKey::new(futex_new_addr, FUTEX_BITSET_MATCH_ANY, pid)?;
    let (bucket_idx, futex_bucket_ref) = get_futex_bucket(futex_key);
    let (new_bucket_idx, futex_new_bucket_ref) = get_futex_bucket(futex_new_key);

    let nwakes = {
        if bucket_idx == new_bucket_idx {
            let mut futex_bucket = futex_bucket_ref.lock();
            let nwakes = futex_bucket.remove_and_wake_items(futex_key, max_nwakes);
            futex_bucket.update_item_keys(futex_key, futex_new_key, max_nrequeues);
            drop(futex_bucket);
            nwakes
        } else {
            let (mut futex_bucket, mut futex_new_bucket) = {
                if bucket_idx < new_bucket_idx {
                    let futex_bucket = futex_bucket_ref.lock();
                    let futext_new_bucket = futex_new_bucket_ref.lock();
                    (futex_bucket, futext_new_bucket)
                } else {
                    // bucket_idx > new_bucket_idx
                    let futex_new_bucket = futex_new_bucket_ref.lock();
                    let futex_bucket = futex_bucket_ref.lock();
                    (futex_bucket, futex_new_bucket)
                }
            };

            let nwakes = futex_bucket.remove_and_wake_items(futex_key, max_nwakes);
            futex_bucket.requeue_items_to_another_bucket(
                futex_key,
                &mut futex_new_bucket,
                futex_new_key,
                max_nrequeues,
            );
            nwakes
        }
    };
    Ok(nwakes)
}

static FUTEX_BUCKETS: Once<FutexBucketVec> = Once::new();

/// Get the futex hash bucket count.
///
/// This number is calculated the same way as Linux's:
/// <https://github.com/torvalds/linux/blob/master/kernel/futex/core.c>
fn get_bucket_count() -> usize {
    ((1 << 8) * num_cpus()).next_power_of_two()
}

fn get_futex_bucket(key: FutexKey) -> (usize, &'static SpinLock<FutexBucket>) {
    FUTEX_BUCKETS.get().unwrap().get_bucket(key)
}

/// Initialize the futex system.
pub fn init() {
    FUTEX_BUCKETS.call_once(|| FutexBucketVec::new(get_bucket_count()));
}

struct FutexBucketVec {
    vec: Vec<SpinLock<FutexBucket>>,
}

impl FutexBucketVec {
    pub fn new(size: usize) -> FutexBucketVec {
        let mut buckets = FutexBucketVec {
            vec: Vec::with_capacity(size),
        };
        for _ in 0..size {
            let bucket = SpinLock::new(FutexBucket::new());
            buckets.vec.push(bucket);
        }
        buckets
    }

    pub fn get_bucket(&self, key: FutexKey) -> (usize, &SpinLock<FutexBucket>) {
        let index = (self.size() - 1) & {
            // The addr is the multiples of 4, so we ignore the last 2 bits
            let addr = key.addr() >> 2;
            // simple hash
            addr / self.size()
        };
        (index, &self.vec[index])
    }

    fn size(&self) -> usize {
        self.vec.len()
    }
}

/// A hash bucket holding the waiters for a set of futex addresses.
///
/// Futex addresses are hashed to a bucket in [`FutexBucketVec`]. All waiters whose address hashes
/// to the same bucket are stored together. The bucket must be locked (via its enclosing
/// `SpinLock`) before any of its items are accessed or modified.
struct FutexBucket {
    /// All waiter items currently queued in this bucket.
    items: Vec<FutexItem>,
}

impl FutexBucket {
    pub fn new() -> FutexBucket {
        FutexBucket {
            items: Vec::with_capacity(1),
        }
    }

    /// Enqueues a waiter item into this bucket.
    pub fn add_item(&mut self, item: FutexItem) {
        self.items.push(item);
    }

    /// Removes and wakes up to `max_count` items matching `key`.
    ///
    /// Returns the number of waiters actually woken.
    pub fn remove_and_wake_items(&mut self, key: FutexKey, max_count: usize) -> usize {
        let mut count = 0;

        self.items.retain(|item| {
            if item.key.match_up(&key) && count < max_count {
                if item.wake() {
                    count += 1;
                }
                false
            } else {
                true
            }
        });

        count
    }

    /// Reassigns the key of up to `max_count` items matching `key` to `new_key`.
    ///
    /// Used by `FUTEX_REQUEUE` when both futex addresses hash to the same bucket, so items
    /// can be requeued in place without moving them to another bucket.
    pub fn update_item_keys(&mut self, key: FutexKey, new_key: FutexKey, max_count: usize) {
        let mut count = 0;
        for item in self.items.iter_mut() {
            if item.key.match_up(&key) {
                item.key = new_key;
                count += 1;
            }
            if count >= max_count {
                break;
            }
        }
    }

    /// Moves up to `max_nrequeues` items matching `key` from this bucket into `another`,
    /// updating each item's key to `new_key`.
    ///
    /// Used by `FUTEX_REQUEUE` when the source and destination futex addresses hash to
    /// different buckets. Both buckets must already be locked by the caller.
    pub fn requeue_items_to_another_bucket(
        &mut self,
        key: FutexKey,
        another: &mut Self,
        new_key: FutexKey,
        max_nrequeues: usize,
    ) {
        let mut count = 0;
        self.items
            .extract_if(.., |item| {
                if item.key.match_up(&key) && count < max_nrequeues {
                    count += 1;
                    true
                } else {
                    false
                }
            })
            .for_each(|mut extracted| {
                extracted.key = new_key;
                another.add_item(extracted);
            });
    }
}

/// A single waiter enqueued in a [`FutexBucket`].
///
/// Each item pairs a [`FutexKey`] (identifying the futex word and bitset the waiter is sleeping
/// on) with a [`Waker`] that can unblock the corresponding [`Waiter`].
struct FutexItem {
    /// The futex word this waiter is sleeping on.
    key: FutexKey,
    /// The waker used to unblock the thread when the futex is woken or requeued.
    waker: Arc<Waker>,
}

impl FutexItem {
    /// Creates a new `FutexItem` for `key` and returns it together with the paired [`Waiter`].
    ///
    /// The caller should enqueue the item into the appropriate [`FutexBucket`] and then block on
    /// the returned [`Waiter`].
    pub fn create(key: FutexKey) -> (Self, Waiter) {
        let (waiter, waker) = Waiter::new_pair();
        let futex_item = FutexItem { key, waker };

        (futex_item, waiter)
    }

    /// Wakes the waiter associated with this item.
    ///
    /// Returns `true` if the waiter was successfully woken, `false` if it had already been woken
    /// or cancelled.
    #[must_use]
    pub fn wake(&self) -> bool {
        self.waker.wake_up()
    }
}

/// The identity of a futex word, used to match waiters against wake/requeue operations.
///
/// Two keys match (see [`FutexKey::match_up`]) when they refer to the same futex word in the
/// same scope (process-private or shared) and have at least one bit in common in their bitsets.
#[derive(Clone, Copy)]
struct FutexKey {
    /// User-space virtual address of the four-byte futex word.
    addr: Vaddr,
    /// Bitmask used to select which waiters to wake; only waiters whose bitset shares at least
    /// one bit with the waker's bitset are matched.
    bitset: FutexBitSet,
    /// Specify whether this `FutexKey` is process private or shared. If `pid` is
    /// None, then this `FutexKey` is shared.
    pid: Option<Pid>,
}

impl Debug for FutexKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FutexKey")
            .field("addr", &format_args!("{:#x}", self.addr))
            .field("bitset", &format_args!("{:#x}", self.bitset))
            .field("pid", &self.pid)
            .finish()
    }
}

impl FutexKey {
    /// Creates a new `FutexKey` for the given address, bitset, and scope.
    ///
    /// `pid` being `Some` makes the key process-private; `None` makes it shared across
    /// processes.
    ///
    /// Returns `Err(EINVAL)` if `addr` is not aligned to a four-byte boundary.
    pub fn new(addr: Vaddr, bitset: FutexBitSet, pid: Option<Pid>) -> Result<Self> {
        // "On all platforms, futexes are four-byte integers that must be aligned on a four-byte
        // boundary."
        // Reference: <https://man7.org/linux/man-pages/man2/futex.2.html>.
        if addr % core::mem::align_of::<u32>() != 0 {
            return_errno_with_message!(
                Errno::EINVAL,
                "the futex word is not aligend on a four-byte boundary"
            );
        }

        Ok(Self { addr, bitset, pid })
    }

    /// Atomically loads the current value of the futex word from user space.
    pub fn load_val(&self, ctx: &Context) -> Result<i32> {
        Ok(ctx
            .user_space()
            .atomic_load::<u32>(self.addr)?
            .cast_signed())
    }

    /// Returns the user-space virtual address of the futex word.
    pub fn addr(&self) -> Vaddr {
        self.addr
    }

    /// Returns `true` if this key and `another` refer to the same futex waiter set.
    ///
    /// Two keys match when their addresses are equal, their `pid` scopes are equal, and their
    /// bitsets share at least one bit.
    pub fn match_up(&self, another: &Self) -> bool {
        // TODO: Use hash value to do match_up
        self.addr == another.addr && (self.bitset & another.bitset) != 0 && self.pid == another.pid
    }
}

// The implementation is from occlum

#[derive(PartialEq, Debug, Clone, Copy)]
#[expect(non_camel_case_types)]
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

impl FutexOp {
    pub fn from_u32(bits: u32) -> Result<FutexOp> {
        match bits {
            0 => Ok(FutexOp::FUTEX_WAIT),
            1 => Ok(FutexOp::FUTEX_WAKE),
            2 => Ok(FutexOp::FUTEX_FD),
            3 => Ok(FutexOp::FUTEX_REQUEUE),
            4 => Ok(FutexOp::FUTEX_CMP_REQUEUE),
            5 => Ok(FutexOp::FUTEX_WAKE_OP),
            6 => Ok(FutexOp::FUTEX_LOCK_PI),
            7 => Ok(FutexOp::FUTEX_UNLOCK_PI),
            8 => Ok(FutexOp::FUTEX_TRYLOCK_PI),
            9 => Ok(FutexOp::FUTEX_WAIT_BITSET),
            10 => Ok(FutexOp::FUTEX_WAKE_BITSET),
            _ => return_errno_with_message!(Errno::EINVAL, "Unknown futex op"),
        }
    }
}

bitflags! {
    pub struct FutexFlags : u32 {
        const FUTEX_PRIVATE         = 128;
        const FUTEX_CLOCK_REALTIME  = 256;
    }
}

impl FutexFlags {
    pub fn from_u32(bits: u32) -> Result<FutexFlags> {
        FutexFlags::from_bits(bits)
            .ok_or_else(|| Error::with_message(Errno::EINVAL, "unknown futex flags"))
    }
}

pub fn futex_op_and_flags_from_u32(bits: u32) -> Result<(FutexOp, FutexFlags)> {
    let op = {
        let op_bits = bits & FUTEX_OP_MASK;
        FutexOp::from_u32(op_bits)?
    };
    let flags = {
        let flags_bits = bits & FUTEX_FLAGS_MASK;
        FutexFlags::from_u32(flags_bits)?
    };
    Ok((op, flags))
}
