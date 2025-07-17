//! This provides [`WaitMechanism`] to generalize code over how to wait for other threads.

use core::marker::PhantomData;

use crate::task::Task;

/// Mechanisms for waiting for other threads. The most obvious implementation of this is [`WaitQueue`], however
/// [simply spinning](`WaitBySpin`) is also a valid implementation.
pub trait WaitMechanism {
    /// Waits until some condition is met.
    ///
    /// This method takes a closure that tests a user-given condition.
    /// The method only returns if the condition returns `Some(_)`.
    /// A waker thread should first make the condition `Some(_)`, then invoke the
    /// `wake`-family method. This ordering is important to ensure that waiter
    /// threads do not lose any wakeup notifications.
    ///
    /// By taking a condition closure, this wait-wakeup mechanism becomes
    /// more efficient and robust.
    fn wait_until<F, R>(&self, cond: F) -> R
    where
        F: FnMut() -> Option<R>;

    /// Wakes up one waiting thread, if there is one at the point of time when this method is
    /// called.
    fn wake_one(&self);

    /// Wakes up all waiting threads.
    fn wake_all(&self);
}

#[derive(Copy)]
#[doc(hidden)]
pub struct WaitByLoopThen<SF, W, const HAS_NEXT: bool = true> {
    iterations: usize,
    next: Option<W>,
    _phantom: PhantomData<SF>,
}

impl<SF: Default, W> Default for WaitByLoopThen<SF, W, false> {
    fn default() -> Self {
        Self {
            iterations: Default::default(),
            next: None,
            _phantom: Default::default(),
        }
    }
}

impl<SF, W: Clone, const HAS_NEXT: bool> Clone for WaitByLoopThen<SF, W, HAS_NEXT> {
    fn clone(&self) -> Self {
        Self {
            iterations: self.iterations,
            next: self.next.clone(),
            _phantom: PhantomData,
        }
    }
}

impl<SF, W> WaitByLoopThen<SF, W, true> {
    pub fn new(iterations: usize, next: Option<W>) -> Self {
        Self {
            iterations,
            next,
            _phantom: PhantomData,
        }
    }
}

impl<SF: SpinFunc, W: WaitMechanism, const HAS_NEXT: bool> WaitMechanism
    for WaitByLoopThen<SF, W, HAS_NEXT>
{
    fn wait_until<F, R>(&self, mut cond: F) -> R
    where
        F: FnMut() -> Option<R>,
    {
        let mut i = 0;
        loop {
            if let Some(v) = cond() {
                return v;
            }
            SF::spin_hint();
            i += 1;
            // If we have done our iterations and there is a next, call it and let it handle the
            // rest of the waiting.
            if i > self.iterations
                && HAS_NEXT
                && let Some(next) = &self.next
            {
                return next.wait_until(cond);
            }
        }
    }

    fn wake_one(&self) {
        if let Some(next) = &self.next {
            next.wake_one();
        }
    }

    fn wake_all(&self) {
        if let Some(next) = &self.next {
            next.wake_all();
        }
    }
}

/// A statically provided function which is called to "yield" inside a spin loop.
pub trait SpinFunc {
    fn spin_hint();
}

/// A spin function which yields to the OS scheduler.
#[derive(Default)]
pub struct YieldSpinFunc;

impl SpinFunc for YieldSpinFunc {
    fn spin_hint() {
        Task::yield_now();
    }
}

/// A spin function which tells the CPU we are spinning.
#[derive(Default)]
pub struct BusySpinFunc;

impl SpinFunc for BusySpinFunc {
    fn spin_hint() {
        core::hint::spin_loop();
    }
}

/// A waiting mechanism which yields to the schedule a number of times, followed by some other
/// wait mechanism.
pub type WaitByYieldThen<W> = WaitByLoopThen<YieldSpinFunc, W>;
/// A waiting mechanism which spins with a CPU spin hint a number of times, followed by some other
/// wait mechanism.
pub type WaitBySpinThen<W> = WaitByLoopThen<BusySpinFunc, W>;
/// A waiting mechanism which yields to the scheduler in a loop.
pub type WaitByYield = WaitByLoopThen<YieldSpinFunc, KeepGoing, false>;
/// A waiting mechanism which spins with a CPU spin hint.
pub type WaitBySpin = WaitByLoopThen<BusySpinFunc, KeepGoing, false>;

/// A "null" wait mechanism used to reduce the duplication in the implementations of the spinning
/// wait mechanisms. This should never be constructed.
#[doc(hidden)]
pub struct KeepGoing(());

impl WaitMechanism for KeepGoing {
    fn wait_until<F, R>(&self, _: F) -> R
    where
        F: FnMut() -> Option<R>,
    {
        unreachable!()
    }

    fn wake_one(&self) {
        unreachable!()
    }

    fn wake_all(&self) {
        unreachable!()
    }
}

#[cfg(ktest)]
mod test {
    use alloc::sync::Arc;
    use core::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::{prelude::*, sync::Waiter, task::TaskOptions};

    fn test_wake_kind<WM: WaitMechanism + Send + Sync + 'static>(
        wm: WM,
        wake: impl Fn(&WM) + Send + 'static,
    ) {
        let (test_waiter, test_waker) = Waiter::new_pair();
        let wm = Arc::new(wm);
        let cond = Arc::new(AtomicBool::new(false));

        TaskOptions::new({
            let wm = wm.clone();
            let cond = cond.clone();
            move || {
                Task::yield_now();

                cond.store(true, Ordering::Relaxed);
                wake(&wm);
            }
        })
        .spawn()
        .unwrap();

        TaskOptions::new(move || {
            wm.wait_until(|| cond.load(Ordering::Relaxed).then_some(()));

            assert!(cond.load(Ordering::Relaxed));
            test_waker.wake_up();
        })
        .spawn()
        .unwrap();

        test_waiter.wait();
    }

    fn test_wait_mechanism<WM: WaitMechanism + Sync + Send + 'static>(make_wm: impl Fn() -> WM) {
        test_wake_kind(make_wm(), |wm| wm.wake_all());
        test_wake_kind(make_wm(), |wm| wm.wake_one());
    }

    #[ktest]
    fn test_wait_by_yield() {
        test_wait_mechanism(WaitByYield::default)
    }

    // TODO: This test hangs. I think it has to do with the scheduler not interruption busy waiting processes. #[ktest]
    #[expect(unused)]
    fn test_wait_by_spin() {
        test_wait_mechanism(WaitBySpin::default)
    }

    #[ktest]
    fn test_wait_by_spin_then() {
        test_wait_mechanism(|| WaitBySpinThen::new(10, Some(WaitByYield::default())));
    }
}
