// SPDX-License-Identifier: MPL-2.0

//! The implementation of robust list is from occlum.

use ostd::{mm::VmIo, task::Task};

use crate::{
    context::current_userspace,
    prelude::*,
    process::posix_thread::futex::{FutexVisibility, futex_wake},
    thread::Tid,
};

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
struct RobustList {
    next: Vaddr, // *const Robust list
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct RobustListHead {
    /// Linked list of lock entries
    ///
    /// If it points to the head of the list, then it is the end of the list.
    /// If it is an invalid user space pointer or a null pointer, stop iterating
    /// the list.
    list: RobustList,
    /// Specifies the offset from the address of the lock entry to the address
    /// of the futex.
    futex_offset: isize,
    /// Contains transient copy of the address of the lock entry, during list
    /// insertion and removal.
    list_op_pending: Vaddr, // *const RobustList
}

/// A robust futex found while walking a robust list.
#[derive(Clone, Copy, Debug)]
pub struct RobustFutex {
    pub addr: Vaddr,
    /// Whether the futex is a PI futex, which user space marks by setting bit 0 of the pointer to
    /// its lock entry.
    pub is_pi: bool,
    /// Whether the futex comes from `list_op_pending`.
    pub is_pending: bool,
}

/// Splits a lock entry pointer into the entry address and whether its futex is a PI futex.
///
/// This corresponds to Linux's `fetch_robust_entry`.
fn split_entry_ptr(entry_ptr: Vaddr) -> (Vaddr, bool) {
    (entry_ptr & !1, entry_ptr & 1 != 0)
}

impl RobustListHead {
    /// Return an iterator for all futexes in the robust list.
    ///
    /// `self` must have been read from `head_addr` right before the walk, since user space keeps
    /// updating it.
    ///
    /// The futex referred to by `list_op_pending`, if any, will be returned as
    /// the last item.
    pub fn futexes(&self, head_addr: Vaddr) -> FutexIter<'_> {
        FutexIter::new(self, head_addr)
    }

    /// Get the futex address
    fn futex_addr(&self, entry: Vaddr) -> Option<Vaddr> {
        self.futex_offset
            .checked_add(entry as isize)
            .map(|result| result as Vaddr)
    }
}

pub struct FutexIter<'a> {
    robust_list: &'a RobustListHead,
    head_addr: Vaddr,
    entry: Vaddr,
    is_pi: bool,
    count: isize,
}

impl<'a> FutexIter<'a> {
    pub fn new(robust_list: &'a RobustListHead, head_addr: Vaddr) -> Self {
        let (entry, is_pi) = split_entry_ptr(robust_list.list.next);
        Self {
            robust_list,
            head_addr,
            entry,
            is_pi,
            count: 0,
        }
    }

    /// The `self.count` is normally a positive value used to iterate the list
    /// to avoid excessively long or circular list, we use a special value -1
    /// to represent the end of the Iterator.
    fn set_end(&mut self) {
        self.count = -1;
    }

    fn is_end(&self) -> bool {
        self.count < 0
    }
}

const ROBUST_LIST_LIMIT: isize = 2048;

impl Iterator for FutexIter<'_> {
    type Item = RobustFutex;

    fn next(&mut self) -> Option<Self::Item> {
        if self.is_end() {
            return None;
        }

        let (pending, pending_is_pi) = split_entry_ptr(self.robust_list.list_op_pending);
        // The list is circular, ending where it points back to the head.
        while self.entry != self.head_addr {
            if self.count == ROBUST_LIST_LIMIT {
                break;
            }
            if self.entry == 0 {
                return None;
            }
            // A pending lock might already be on the list, so don't process it twice.
            let futex = if self.entry != pending {
                self.robust_list
                    .futex_addr(self.entry)
                    .map(|addr| RobustFutex {
                        addr,
                        is_pi: self.is_pi,
                        is_pending: false,
                    })
            } else {
                None
            };
            let Ok(robust_list) = current_userspace!().read_val::<RobustList>(self.entry) else {
                return None;
            };
            (self.entry, self.is_pi) = split_entry_ptr(robust_list.next);
            self.count += 1;
            if futex.is_some() {
                return futex;
            }
        }
        self.set_end();
        if pending == 0 {
            return None;
        }
        self.robust_list
            .futex_addr(pending)
            .map(|addr| RobustFutex {
                addr,
                is_pi: pending_is_pi,
                is_pending: true,
            })
    }
}

const FUTEX_WAITERS: u32 = 0x8000_0000;
const FUTEX_OWNER_DIED: u32 = 0x4000_0000;
const FUTEX_TID_MASK: u32 = 0x3FFF_FFFF;

/// Attempts to wake a robust futex owned by the given thread.
///
/// If the futex is still owned by `tid`, it is marked with
/// `FUTEX_OWNER_DIED` and one waiter (if any) is woken.
/// If the futex is owned by another thread, the operation is canceled.
///
/// This corresponds to Linux's `handle_futex_death`.
pub fn wake_robust_futex(futex: RobustFutex, tid: Tid) -> Result<()> {
    let futex_addr = futex.addr;
    if !futex_addr.is_multiple_of(align_of::<u32>()) {
        return_errno_with_message!(
            Errno::EINVAL,
            "the futex word is not aligned on a four-byte boundary"
        );
    }

    let task = Task::current().unwrap();
    let user_space = CurrentUserSpace::new(task.as_thread_local().unwrap());

    // Instantiate a reader and a writer pointing at the same `futex_addr`, set up
    // for the same length: the length of an `u32`.
    let (reader, writer) = user_space.reader_writer(futex_addr, size_of::<u32>())?;

    let mut old_val: u32 = reader.atomic_load()?;
    loop {
        let owner = old_val & FUTEX_TID_MASK;

        // The thread may have died after releasing the lock in user space but before clearing
        // `list_op_pending`, possibly without waking a waiter. Wake one so it cannot get stuck.
        if futex.is_pending && !futex.is_pi && owner == 0 {
            futex_wake(futex_addr, 1, FutexVisibility::Shared)?;
            break;
        }

        // This futex may be held by another thread. If so, do nothing.
        if owner != tid {
            break;
        }

        let new_val = (old_val & FUTEX_WAITERS) | FUTEX_OWNER_DIED;
        match writer.atomic_compare_exchange(&reader, old_val, new_val)? {
            (cur_val, false) => old_val = cur_val, // Try again with `cur_val`.
            (_, true) => {
                // Wake up one waiter and break out from the loop. PI futexes are not supported
                // yet, so they have no waiters to wake here.
                if !futex.is_pi && new_val & FUTEX_WAITERS != 0 {
                    debug!("wake the robust futex at {:#x}", futex_addr);
                    futex_wake(futex_addr, 1, FutexVisibility::Shared)?;
                }
                break;
            }
        }
    }

    Ok(())
}
