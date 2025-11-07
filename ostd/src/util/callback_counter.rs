// SPDX-License-Identifier: MPL-2.0

//! A utility for tracking the number of times a callback has been called and eventually forwarding
//! to another function.

use alloc::{boxed::Box, sync::Arc};
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::sync::Mutex;

/// A utility to call a function when a counter reaches zero. The counter is not fixed and can be
/// increased after this is created.
///
/// The usage pattern should be:
///
/// 1. Create an [incrementer](`CallbackCounterIncrementer`)/tracker pair.
/// 2. Call the incrementer as needed to update the count and pass out the tracker as needed to
///    allow decrementing the count.
/// 3. Drop the incrementer.
///
/// Once the incrementer is dropped, the the callback will be called as soon as the counter is zero.
/// If the counter is zero, when the incrementer is dropped, the callback will be called
/// immediately. The incrementer is used to avoid cases where the counter can increase again after
/// it has called the callback.
///
/// The callback will be called synchronously in the last decrementing thread or the thread which
/// drops the incrementer.
pub struct CallbackCounter<F> {
    inner: Arc<CallbackCounterInner<F>>,
}

impl<F> Clone for CallbackCounter<F> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

struct CallbackCounterInner<F> {
    count: AtomicUsize,
    complete_fn: Mutex<Option<Box<F>>>,
}

impl<F> CallbackCounter<F>
where
    F: FnOnce() + Send + 'static,
{
    /// Create a new counter with the function that should be called.
    pub fn new(complete_fn: F) -> (Self, CallbackCounterIncrementer<F>) {
        let this = CallbackCounter {
            inner: Arc::new(CallbackCounterInner {
                // Start with a count of 1 which will be decremented by CallbackCounterIncrementer::drop
                count: AtomicUsize::new(1),
                complete_fn: Mutex::new(Some(Box::new(complete_fn))),
            }),
        };
        (this.clone(), CallbackCounterIncrementer(this))
    }

    /// Decrement the counter and call the underlying function if it reaches 0. Once the counter has
    /// reached 0, calls to this are ignored.
    pub fn decrement_and_check(&self) {
        let prev = self.inner.count.fetch_sub(1, Ordering::Relaxed);
        if prev == 1 {
            let mut complete_fn = self.inner.complete_fn.lock();
            if let Some(complete_fn) = complete_fn.take() {
                complete_fn();
            }
        }
    }
}

/// The handle to a [`CallbackCounter`] which allows incrementing. One is created along with the the
/// `CallbackCounter`. This cannot be cloned, meaning that all increments must happen before this is
/// dropped.
pub struct CallbackCounterIncrementer<F>(CallbackCounter<F>)
where
    F: FnOnce() + Send + 'static;

impl<F> Drop for CallbackCounterIncrementer<F>
where
    F: FnOnce() + Send + 'static,
{
    fn drop(&mut self) {
        self.0.decrement_and_check();
    }
}

impl<F> CallbackCounterIncrementer<F>
where
    F: FnOnce() + Send + 'static,
{
    /// Increment the counter increasing the number of times [`decrement_and_check`] must be called
    /// before the underlying function is called.
    pub fn increment(&self) {
        self.0.inner.count.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(ktest)]
mod test {
    use core::sync::atomic::AtomicBool;

    use ostd::prelude::*;

    use super::*;

    #[ktest]
    fn test_callback_counter_called_on_drop_when_zero() {
        let called = Arc::new(AtomicBool::new(false));
        let complete_fn = Box::new({
            let called = called.clone();
            move || {
                called.store(true, Ordering::SeqCst);
            }
        });
        let (counter, incrementer) = CallbackCounter::new(complete_fn);

        incrementer.increment();
        counter.decrement_and_check();
        assert!(!called.load(Ordering::SeqCst));

        assert_eq!(counter.inner.count.load(Ordering::Relaxed), 1);
        drop(incrementer);

        assert_eq!(counter.inner.count.load(Ordering::Relaxed), 0);
        assert!(called.load(Ordering::SeqCst));
    }

    #[ktest]
    fn test_callback_counter_decrements() {
        let called = Arc::new(AtomicBool::new(false));
        let complete_fn = Box::new({
            let called = called.clone();
            move || {
                called.store(true, Ordering::SeqCst);
            }
        });
        let (counter, incrementer) = CallbackCounter::new(complete_fn);

        incrementer.increment();
        incrementer.increment();
        drop(incrementer);

        counter.decrement_and_check();
        assert_eq!(
            counter
                .inner
                .count
                .load(core::sync::atomic::Ordering::Relaxed),
            1
        );
        assert!(!called.load(Ordering::SeqCst));

        counter.decrement_and_check();
        assert_eq!(
            counter
                .inner
                .count
                .load(core::sync::atomic::Ordering::Relaxed),
            0
        );
        assert!(called.load(Ordering::SeqCst));
    }
}
