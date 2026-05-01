// SPDX-License-Identifier: MPL-2.0
//! Error module for ORPC

use alloc::{
    format,
    string::{String, ToString},
};
use core::any::Any;

use ostd_macros::ostd_error;
use snafu::Snafu;

use crate::{panic::CaughtPanic, prelude::Box, stack_info::StackInfo};

/// An error during an RPC call: failure via panic and the server not running.
///
/// Any error returned from an ORPC method must implement `From<RPCError>` to allow reporting these errors to the user.
/// (An easy way to do this us to use `Snafu` and have a [transparent
/// variant](https://docs.rs/snafu/latest/snafu/derive.Snafu.html#delegating-to-the-underlying-error) for
/// `orpc::errors::RPCError`.)
#[ostd_error]
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum RPCError {
    /// A panic occurred in the server during the call. The panic payload will be converted to a string, if possible. If
    /// it cannot be, then the string will be a generic error message.
    #[snafu(display("{message} ({context})"))]
    Panic {
        /// The formatted panic message, including source file and line.
        message: String,
    },
    /// The server does not exist or is not running. This can happen when a server already crashed or has been shutdown.
    #[snafu(display("Server does not exist or is not running ({context})"))]
    ServerMissing,
}

/// Convert a payload to a string if possible. This simply performs downcasts.
fn payload_as_caught_panic(payload: Box<dyn Any + Send + 'static>) -> Option<CaughtPanic> {
    // Attempt downcasts to 4 different types: &str, String, CaughtPanic, ostd_test::PanicInfo
    let payload = payload
        .downcast::<&str>()
        .map(|s| CaughtPanic {
            message: s.to_string(),
            context: None,
        })
        .or_else(|payload| {
            payload.downcast::<String>().map(|s| CaughtPanic {
                message: *s,
                context: None,
            })
        })
        .or_else(|payload| payload.downcast::<CaughtPanic>().map(|c| *c))
        .or_else(|payload| {
            payload
                .downcast::<ostd_test::PanicInfo>()
                .map(|c| (*c).into())
        });
    payload.ok()
}

impl RPCError {
    /// Convert a panic payload into an RPCError.
    ///
    /// This take ownership of the payload to allow it to be implemented allocation free in as many cases as possible.
    /// This is important since allocating on an error path can cause issues.
    pub fn from_panic(payload: Box<dyn Any + Send + 'static>) -> Self {
        let panic = payload_as_caught_panic(payload).unwrap_or_else(|| CaughtPanic {
            message: "[unknown panic payload]".to_string(),
            context: None,
        });
        let (message, context) = if let Some(context) = panic.context {
            (panic.message, context)
        } else {
            (
                format!("{} (with error conversion context)", panic.message),
                StackInfo::new(0),
            )
        };
        RPCError::Panic {
            message,
            context: Box::new(context),
        }
    }
}
