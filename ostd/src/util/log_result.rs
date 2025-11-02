// SPDX-License-Identifier: MPL-2.0

//! Logging and error reporting utilities for [`Result`]s.
//!
//! Sometimes [`Result::Err`] needs to be ignored. However, the error should not be dropped since it
//! might be important to know it happened. These macros provide a way both silence the warning
//! about unused [`Result`] and log the error if once occurs.
//! 
//! Generally, use [`error_result`].

// TODO(arthurp): clippy::format_args is broken by another lint, see
// https://github.com/rust-lang/rust/issues/98291#issuecomment-2673505799. When we figure out how,
// we should use it for these macros to improve the tooling help for using these macros.

/// `log_result` logs the error if `result` is `Err` and otherwise does nothing. This should be used
/// when an error happens, but the code needs to or can continue executing. The text provided should
/// describe the errors effect on the system.
///
/// This macro requires that the error implement `Display`. If it does not, use [`Result::map_err`]
/// to convert it to something which does.
#[macro_export]
macro_rules! log_result {
    ($level:expr, $result:expr) => {{
        if let Err(e) = $result {
            log::log!($level, "error ignored: {}", e)
        }
    }};
    ($level:expr, $result:expr, $($args:tt)+) => {{
        if let Err(e) = $result {
            log::log!($level, "error ignored: {}: {}", alloc::format!($($args)+), e)
        }
    }};
}

/// [`log_result`] using error level.
#[macro_export]
macro_rules! error_result {
    ($($args:tt)+) => {
        $crate::log_result!(log::Level::Error, $($args)+)
    };
}

/// [`log_result`] using warn level.
#[macro_export]
macro_rules! warn_result {
    ($($args:tt)+) => {
        $crate::log_result!(log::Level::Warn, $($args)+)
    };
}

/// [`log_result`] using info level.
#[macro_export]
macro_rules! info_result {
    ($($args:tt)+) => {
        $crate::log_result!(log::Level::Info, $($args)+)
    };
}

#[cfg(ktest)]
mod test {
    use log::Level;

    use crate::prelude::*;

    /// A correctly typed error for tests.
    static ERROR: core::result::Result<u16, usize> = Err(32);

    #[ktest]
    fn test_log_result() {
        log_result!(Level::Warn, ERROR, "not able to {}", "test");
        log_result!(Level::Debug, ERROR);
    }

    #[ktest]
    fn test_warn_result() {
        warn_result!(ERROR, "not able to {}", "test");
    }

    #[ktest]
    fn test_error_result() {
        error_result!(ERROR, "not able to {}", "test");
    }

    #[ktest]
    fn test_info_result() {
        info_result!(ERROR, "not able to {}", "test");
    }
}
