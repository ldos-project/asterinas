// SPDX-License-Identifier: MPL-2.0

//! Logging and error reporting utilities for [`Result`]s.
//!
//! Sometimes [`Result::Err`] needs to be ignored. However, the error should not be dropped since it
//! might be important to know it happened. These macros provide a way both silence the warning
//! about unused [`Result`] and log the error if once occurs.

// TODO(arthurp): clippy::format_args is broken by another lint, see
// https://github.com/rust-lang/rust/issues/98291#issuecomment-2673505799. When we figure out how,
// we should use it for these macros to improve the tooling help for using these macros.

/// `ignore_err!(expr)` logs the error if `expr` is `Err` and otherwise does nothing. This should be
/// used when an error happens, but the code needs to or can continue executing. The text provided
/// should describe the errors effect on the system.
///
/// The macro supports an optional log level and additional message. Using a different log level is
/// useful in cases where an error may occur frequently or is not very important. The default level
/// is `Error`. An additional message may be useful to specify why the error was ignored.
///
/// ```no_run
/// # use log::Level;
/// # let expr: core::result::Result<u16, usize> = Err(32);
/// # let x = 42;
/// ignore_err!(expr)
/// ignore_err!(expr, Level::Info)
/// ignore_err!(expr, Level::Info, "additional info {}", x)
/// ```
///
/// This macro requires that the error implement `Display`. If it does not, use [`Result::map_err`]
/// to convert it to something which does.
#[macro_export]
macro_rules! ignore_err {
    ($result:expr) => {
        ignore_err!($result, ::log::Level::Error)
    };
    ($result:expr, $level:expr) => {{
        if let Err(e) = $result {
            ::log::log!($level, "error ignored: {}", e)
        }
    }};
    ($result:expr, $level:expr, $($args:tt)+) => {{
        if let Err(e) = $result {
            ::log::log!($level, "error ignored: {}: {}", alloc::format!($($args)+), e)
        }
    }};
}

#[cfg(ktest)]
mod test {
    use log::Level;

    use crate::prelude::*;

    static ERROR: core::result::Result<u16, usize> = Err(32);

    #[ktest]
    fn test_ignore_err() {
        ignore_err!(ERROR);
        ignore_err!(ERROR, Level::Info);
        ignore_err!(ERROR, Level::Info, "not able to {}", "test");
    }
}
