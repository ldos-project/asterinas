use core::any::Any;

use snafu::Snafu;

/// An error during an RPC call: failure via panic and the server not running.
/// 
/// Any error returned from an ORPC method must implement `From<RPCError>` to allow reporting these errors to the user.
/// (An easy way to do this us to use `Snafu` and have a [transparent
/// variant](https://docs.rs/snafu/latest/snafu/derive.Snafu.html#delegating-to-the-underlying-error) for
/// `orpc::errors::RPCError`.)
#[derive(Snafu, Debug)]
pub enum RPCError {
    /// A panic occurred in the server during the call. The panic payload will be converted to a string, if possible. If
    /// it cannot be, then the string will be a generic error message.
    #[snafu(display("{message}"))]
    Panic { message: String },
    /// The server does not exist or is not running. This can happen when a server already crashed or has been shutdown.
    #[snafu(display("Server does not exist or not running"))]
    ServerMissing,
}

/// Convert a payload to a string if possible. This simply performs downcasts.
fn payload_as_str(payload: &dyn Any) -> Option<&str> {
    if let Some(s) = payload.downcast_ref::<&str>() {
        Some(s)
    } else if let Some(s) = payload.downcast_ref::<String>() {
        Some(s)
    } else {
        None
    }
}

impl RPCError {
    /// Convert a panic payload into an RPCError.
    ///
    /// This take ownership of the payload to allow it to be implemented allocation free in as many cases as possible.
    /// This is important since allocating on an error path can cause issues.
    pub fn from_panic(payload: Box<dyn Any>) -> Self {
        // TODO: The `to_string` call could potentially allocate which could be an issue on the panic path. A better way
        // to do this may be needed.
        Self::Panic {
            message: payload_as_str(payload.as_ref())
                .unwrap_or("Non-string panic payload")
                .to_string(),
        }
    }
}

