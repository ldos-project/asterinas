// SPDX-License-Identifier: MPL-2.0

//! Utilities for concurrent tests by waiting for conditions to be true.

#![cfg(ktest)]

use core::{assert_matches::assert_matches, time::Duration};

use crate::{task::Task, timer::Jiffies};

/// The default timeout for eventual assertion macros.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(1);

/// The default time before first check for eventual assertion macros.
pub const DEFAULT_FIRST_CHECK_DELAY: Duration = Duration::from_millis(100);

/// Function that sleeps until a given predicate returns true or the specified duration elapses.
///
/// # Parameters:
/// - `d`: The duration to sleep for.
/// - `predicate`: A closure that returns a boolean. The function will exit as soon as this closure
///   returns `true`.
pub fn sleep_with_predicate(d: Duration, mut predicate: impl FnMut() -> bool) {
    let now = Jiffies::elapsed().as_duration();
    let target = now + d;

    while Jiffies::elapsed().as_duration() < target {
        if predicate() {
            break;
        }
        Task::yield_now();
    }
}

/// Function that sleeps for a given duration.
///
/// # Parameters:
/// - `d`: The duration to sleep for.
pub fn sleep(d: Duration) {
    sleep_with_predicate(d, || true);
}

/// Macro to assert that a condition becomes true within a timeout (specified or a
/// [default](`DEFAULT_TIMEOUT`)).
///
/// # Usage:
/// ```no_run
/// assert_eventually!(value == 1);
/// assert_eventually!(value == 1, "Value did not become 1");
/// assert_eventually!(value == 1, timeout = Duration::from_millis(200));
/// assert_eventually!(value == 1, timeout = Duration::from_millis(200), "Value did not become 1");
/// ```
#[macro_export]
macro_rules! assert_eventually {
    ($cond:expr, timeout = $timeout:expr $(, $($message:tt)*)?) => {{
        $crate::assert_eventually_helper!(assert!($cond$(, $($message)*)?), $cond, timeout=$timeout);
    }};
    ($cond:expr $(, $($message:tt)*)?) => {{
        $crate::assert_eventually_helper!(assert!($cond$(, $($message)*)?), $cond);
    }};
}

/// Macro to assert that two expressions become equal within a timeout (specified or a
/// [default](`DEFAULT_TIMEOUT`)).
///
/// # Usage:
/// ```no_run
/// assert_eq_eventually!(value, 1);
/// assert_eq_eventually!(value, 1, "Value should be equal to 1");
/// assert_eq_eventually!(value, 1, timeout = Duration::from_millis(200));
/// assert_eq_eventually!(value, 1, timeout = Duration::from_millis(200), "Value should be equal to 1");
/// ```
#[macro_export]
macro_rules! assert_eq_eventually {
    ($left:expr, $right:expr, timeout = $timeout:expr $(, $($message:tt)*)?) => {{
        $crate::assert_eventually_helper!(assert_eq!($left, $right $(, $($message)*)?), $left == $right, timeout=$timeout);
    }};
    ($left:expr, $right:expr $(, $($message:tt)*)?) => {{
        $crate::assert_eventually_helper!(assert_eq!($left, $right $(, $($message)*)?), $left == $right);
    }};
}

/// Macro to assert that two expressions become unequal within a timeout (specified or a
/// [default](`DEFAULT_TIMEOUT`)).
///
/// # Usage:
/// ```no_run
/// assert_ne_eventually!(value, 1);
/// assert_ne_eventually!(value, 1, "Value should not be equal to 1");
/// assert_ne_eventually!(value, 1, timeout = Duration::from_millis(200));
/// assert_ne_eventually!(value, 1, timeout = Duration::from_millis(200), "Value should not be equal to 1");
/// ```
#[macro_export]
macro_rules! assert_ne_eventually {
    ($left:expr, $right:expr, timeout = $timeout:expr $(, $($message:tt)*)?) => {{
        $crate::assert_eventually_helper!(assert_ne!($left, $right $(, $($message)*)?), $left != $right, timeout=$timeout);
    }};
    ($left:expr, $right:expr $(, $($message:tt)*)?) => {{
        $crate::assert_eventually_helper!(assert_ne!($left, $right $(, $($message)*)?), $left != $right);
    }};
}

/// Macro to assert that an expression matches a pattern within a timeout (specified or a
/// [default](`DEFAULT_TIMEOUT`)).
///
/// # Usage:
/// ```no_run
/// assert_matches_eventually!(result, Some(_));
/// assert_matches_eventually!(result, Some(_), "Result should be Some");
/// assert_matches_eventually!(result, Some(_), timeout = Duration::from_millis(200));
/// assert_matches_eventually!(result, Some(_), timeout = Duration::from_millis(200), "Result should be Some");
/// ```
#[macro_export]
macro_rules! assert_matches_eventually {
    ($expr:expr, $pat:pat, timeout = $timeout:expr $(, $($message:tt)*)?) => {{
        $crate::assert_eventually_helper!(assert_matches!($expr, $pat $(, $($message)*)?), matches!($expr, $pat), timeout=$timeout);
    }};
    ($expr:expr, $pat:pat $(, $($message:tt)*)?) => {{
        $crate::assert_eventually_helper!(assert_matches!($expr, $pat $(, $($message)*)?), matches!($expr, $pat));
    }};
}

/// Internal helper macro to encapsulate common logic for the "eventually" assertion macros.
#[doc(hidden)]
#[macro_export]
macro_rules! assert_eventually_helper {
    ($assertion:expr, $cond:expr $(, timeout=$timeout:expr)?) => {{
        let predicate = || -> bool { $cond };
        #[allow(clippy::allow_attributes, unused_mut, unused_assignments)]
        let mut timeout = $crate::assertion::DEFAULT_TIMEOUT;
        $(timeout = $timeout;)*
        $crate::assertion::sleep($crate::assertion::DEFAULT_FIRST_CHECK_DELAY);
        $crate::assertion::sleep_with_predicate(timeout, predicate);
        $assertion;
    }};
}

#[cfg(ktest)]
mod tests {
    use alloc::sync::Arc;
    use core::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::{prelude::ktest, sync::Mutex};

    #[ktest]
    fn test_assert_eventually_success() {
        let value = Arc::new(AtomicUsize::new(0));
        crate::task::TaskOptions::new({
            let value = value.clone();
            move || {
                sleep(Duration::from_millis(50));
                value.store(1, Ordering::SeqCst);
            }
        })
        .spawn()
        .unwrap();

        assert_eventually!(value.load(Ordering::SeqCst) == 1);
        assert_eventually!(value.load(Ordering::SeqCst) == 1, "Value did not become 1");
        assert_eventually!(
            value.load(Ordering::SeqCst) == 1,
            timeout = Duration::from_millis(200)
        );
        assert_eventually!(
            value.load(Ordering::SeqCst) == 1,
            timeout = Duration::from_millis(200),
            "Value did not become 1"
        );
    }

    #[ktest]
    fn test_assert_ne_eventually_success() {
        let value = Arc::new(AtomicUsize::new(3));
        crate::task::TaskOptions::new({
            let value = value.clone();
            move || {
                sleep(Duration::from_millis(50));
                value.store(4, Ordering::SeqCst);
            }
        })
        .spawn()
        .unwrap();

        assert_ne_eventually!(value.load(Ordering::SeqCst), 3);
        assert_ne_eventually!(
            value.load(Ordering::SeqCst),
            3,
            "Value should not be equal to 1"
        );
        assert_ne_eventually!(
            value.load(Ordering::SeqCst),
            3,
            timeout = Duration::from_millis(200)
        );
        assert_ne_eventually!(
            value.load(Ordering::SeqCst),
            3,
            timeout = Duration::from_millis(200),
            "Value should not be equal to 1"
        );
    }

    #[ktest]
    fn test_assert_matches_eventually_success() {
        let result = Arc::new(Mutex::new(Option::<usize>::None));
        crate::task::TaskOptions::new({
            let result = result.clone();
            move || {
                sleep(Duration::from_millis(50));
                *result.lock() = Some(42);
            }
        })
        .spawn()
        .unwrap();

        assert_matches_eventually!(*result.lock(), Some(42));
        assert_matches_eventually!(*result.lock(), Some(42), "Result should be Some");
        assert_matches_eventually!(
            *result.lock(),
            Some(42),
            timeout = Duration::from_millis(200)
        );
        assert_matches_eventually!(
            *result.lock(),
            Some(42),
            timeout = Duration::from_millis(200),
            "Result should be Some"
        );
    }

    #[ktest]
    fn test_assert_eq_eventually_success() {
        let value = Arc::new(AtomicUsize::new(1));
        crate::task::TaskOptions::new({
            let value = value.clone();
            move || {
                sleep(Duration::from_millis(50));
                value.store(2, Ordering::SeqCst);
            }
        })
        .spawn()
        .unwrap();

        assert_eq_eventually!(value.load(Ordering::SeqCst), 2);
        assert_eq_eventually!(
            value.load(Ordering::SeqCst),
            2,
            "Value should be equal to 1"
        );
        assert_eq_eventually!(
            value.load(Ordering::SeqCst),
            2,
            timeout = Duration::from_millis(200)
        );
        assert_eq_eventually!(
            value.load(Ordering::SeqCst),
            2,
            timeout = Duration::from_millis(200),
            "Value should be equal to 1"
        );
    }
}
