// SPDX-License-Identifier: MPL-2.0

use alloc::sync::Arc;
use core::{
    cell::UnsafeCell,
    fmt,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicBool, Ordering},
};

use super::WaitQueue;
use crate::stack_info::StackInfo;

/// A mutex with waitqueue.
///
///
pub struct Mutex<T: ?Sized> {
    lock: AtomicBool,
    queue: WaitQueue,
    /// The captured context information from acquire. This lock should only be held to copy the
    /// bytes in an out.
    #[cfg(feature = "track_mutex")]
    acquire_info: Option<super::SpinLock<StackInfo>>,
    val: UnsafeCell<T>,
}

// TODO(#70): Implement mutex poisioning.
impl<T: ?Sized> core::panic::UnwindSafe for Mutex<T> {}
impl<T: ?Sized> core::panic::RefUnwindSafe for Mutex<T> {}

impl<T> Mutex<T> {
    /// Creates a new mutex.
    pub const fn new(val: T) -> Self {
        Self {
            lock: AtomicBool::new(false),
            queue: WaitQueue::new(),
            #[cfg(feature = "track_mutex")]
            acquire_info: None,
            val: UnsafeCell::new(val),
        }
    }

    /// Set mutex to capture acquire information.
    ///
    /// This captures the call context (stack, thread, etc) when the lock is acquired and prints it
    /// when a thread blocks on the lock.
    ///
    /// This is a no-op if the library is built without the `track_mutex` feature.
    ///
    /// ## Usage
    ///
    /// When this is enabled, any time a thread fails to acquire the lock it will log (at the `info` level):
    ///
    /// ```text
    /// INFO: Failed to acquire lock:
    /// Held by: [information about holder]
    /// Failed at: [information about failing acquire]
    /// ```
    /// (The information is provided by [`StackInfo`].)
    ///
    /// The result can be processed with `cargo osdk enhance-log`.
    pub const fn with_acquire_info(self, capture_acquire_info: bool) -> Self {
        self.maybe_with_acquire_info(capture_acquire_info)
    }

    // The private method is used to simplify the `cfg`s.

    #[cfg(feature = "track_mutex")]
    const fn maybe_with_acquire_info(mut self, capture_acquire_info: bool) -> Self {
        if capture_acquire_info {
            self.acquire_info = Some(super::SpinLock::new(StackInfo::empty()));
        } else {
            self.acquire_info = None;
        }
        self
    }

    #[cfg(not(feature = "track_mutex"))]
    const fn maybe_with_acquire_info(self, _capture_acquire_info: bool) -> Self {
        self
    }
}

impl<T: Default> Default for Mutex<T> {
    fn default() -> Self {
        Self::new(Default::default())
    }
}

impl<T: ?Sized> Mutex<T> {
    #[cfg(feature = "track_mutex")]
    #[track_caller]
    fn report_acquire_failure(&self) {
        use crate::early_println;

        if let Some(acquire_info) = &self.acquire_info {
            let info = *acquire_info.lock();
            early_println!(
                "Blocking on Mutex:\nHeld by: {}\nFailed to acquire at: {}",
                info,
                StackInfo::new(2)
            );
        }
    }

    /// Acquires the mutex.
    ///
    /// This method runs in a block way until the mutex can be acquired.
    #[track_caller]
    pub fn lock(&self) -> MutexGuard<'_, T> {
        #[cfg(feature = "track_mutex")]
        if let Some(r) = self.try_lock() {
            return r;
        } else {
            self.report_acquire_failure();
        }
        self.queue.wait_until(|| self.try_lock())
    }

    /// Acquires the mutex through an [`Arc`].
    ///
    /// The method is similar to [`lock`], but it doesn't have the requirement
    /// for compile-time checked lifetimes of the mutex guard.
    ///
    /// [`lock`]: Self::lock
    #[track_caller]
    pub fn lock_arc(self: &Arc<Self>) -> ArcMutexGuard<T> {
        #[cfg(feature = "track_mutex")]
        if let Some(r) = self.try_lock_arc() {
            return r;
        } else {
            self.report_acquire_failure();
        }
        self.queue.wait_until(|| self.try_lock_arc())
    }

    /// Tries Acquire the mutex immedidately.
    #[track_caller]
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        // Cannot be reduced to `then_some`, or the possible dropping of the temporary
        // guard will cause an unexpected unlock.
        // SAFETY: The lock is successfully acquired when creating the guard.
        self.acquire_lock()
            .then(|| unsafe { MutexGuard::new(self) })
    }

    /// Tries acquire the mutex through an [`Arc`].
    ///
    /// The method is similar to [`try_lock`], but it doesn't have the requirement
    /// for compile-time checked lifetimes of the mutex guard.
    ///
    /// [`try_lock`]: Self::try_lock
    #[track_caller]
    pub fn try_lock_arc(self: &Arc<Self>) -> Option<ArcMutexGuard<T>> {
        self.acquire_lock().then(|| ArcMutexGuard {
            mutex: self.clone(),
        })
    }

    /// Returns a mutable reference to the underlying data.
    ///
    /// This method is zero-cost: By holding a mutable reference to the lock, the compiler has
    /// already statically guaranteed that access to the data is exclusive.
    pub fn get_mut(&mut self) -> &mut T {
        self.val.get_mut()
    }

    /// Releases the mutex and wake up one thread which is blocked on this mutex.
    fn unlock(&self) {
        self.release_lock();
        self.queue.wake_one();
    }

    #[track_caller]
    fn acquire_lock(&self) -> bool {
        let succeeded = self
            .lock
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok();
        #[cfg(feature = "track_mutex")]
        if succeeded && let Some(acquire_info) = &self.acquire_info {
            let info = StackInfo::new(2);
            *acquire_info.lock() = info;
        }
        succeeded
    }

    fn release_lock(&self) {
        self.lock.store(false, Ordering::Release);
    }

    /// Get information about the last point in the program this mutex was acquired.
    pub fn last_acquire_info(&self) -> Option<StackInfo> {
        #[cfg(feature = "track_mutex")]
        return self.acquire_info.as_ref().map(|i| *i.lock());
        #[cfg(not(feature = "track_mutex"))]
        return None;
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for Mutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&self.val, f)
    }
}

unsafe impl<T: ?Sized + Send> Send for Mutex<T> {}
unsafe impl<T: ?Sized + Send> Sync for Mutex<T> {}

#[clippy::has_significant_drop]
#[must_use]
pub struct MutexGuard_<T: ?Sized, R: Deref<Target = Mutex<T>>> {
    mutex: R,
}

/// A guard that provides exclusive access to the data protected by a [`Mutex`].
pub type MutexGuard<'a, T> = MutexGuard_<T, &'a Mutex<T>>;

impl<'a, T: ?Sized> MutexGuard<'a, T> {
    /// # Safety
    ///
    /// The caller must ensure that the given reference of [`Mutex`] lock has been successfully acquired
    /// in the current context. When the created [`MutexGuard`] is dropped, it will unlock the [`Mutex`].
    unsafe fn new(mutex: &'a Mutex<T>) -> MutexGuard<'a, T> {
        MutexGuard { mutex }
    }
}

/// An guard that provides exclusive access to the data protected by a `Arc<Mutex>`.
pub type ArcMutexGuard<T> = MutexGuard_<T, Arc<Mutex<T>>>;

impl<T: ?Sized, R: Deref<Target = Mutex<T>>> Deref for MutexGuard_<T, R> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.mutex.val.get() }
    }
}

impl<T: ?Sized, R: Deref<Target = Mutex<T>>> DerefMut for MutexGuard_<T, R> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.mutex.val.get() }
    }
}

impl<T: ?Sized, R: Deref<Target = Mutex<T>>> Drop for MutexGuard_<T, R> {
    fn drop(&mut self) {
        self.mutex.unlock();
    }
}

impl<T: ?Sized + fmt::Debug, R: Deref<Target = Mutex<T>>> fmt::Debug for MutexGuard_<T, R> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: ?Sized, R: Deref<Target = Mutex<T>>> !Send for MutexGuard_<T, R> {}

unsafe impl<T: ?Sized + Sync, R: Deref<Target = Mutex<T>> + Sync> Sync for MutexGuard_<T, R> {}

impl<'a, T: ?Sized> MutexGuard<'a, T> {
    pub fn get_lock(guard: &MutexGuard<'a, T>) -> &'a Mutex<T> {
        guard.mutex
    }
}

#[cfg(ktest)]
mod test {
    use core::time::Duration;

    use log::info;

    use super::*;
    use crate::{
        assertion::{sleep, sleep_with_predicate},
        prelude::*,
        task::TaskOptions,
    };

    // A regression test for a bug fixed in [#1279](https://github.com/asterinas/asterinas/pull/1279).
    #[ktest]
    fn test_mutex_try_lock_does_not_unlock() {
        let lock = Mutex::new(0);
        assert!(!lock.lock.load(Ordering::Relaxed));

        // A successful lock
        let guard1 = lock.lock();
        assert!(lock.lock.load(Ordering::Relaxed));

        // A failed `try_lock` won't drop the lock
        assert!(lock.try_lock().is_none());
        assert!(lock.lock.load(Ordering::Relaxed));

        // Ensure the lock is held until here
        drop(guard1);
    }

    #[ktest]
    fn test_mutex_tracking_threaded() {
        let mutex = Arc::new(Mutex::new(0).with_acquire_info(true));

        let guard = mutex.lock();

        let _handle = TaskOptions::new({
            let mutex = mutex.clone();
            move || {
                let mut v = mutex.lock();
                *v = 1;
            }
        })
        .spawn()
        .unwrap();

        sleep(Duration::from_millis(1000));

        drop(guard);

        sleep_with_predicate(Duration::from_millis(1000), || *mutex.lock() == 1);

        info!(
            "The test should have printed:
Failed to acquire lock:
Held by: [stack info from main thread]
Failed at: [stack info from spawned task]"
        );
    }
}
