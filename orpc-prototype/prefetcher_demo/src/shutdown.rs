use std::sync::atomic::AtomicBool;

use orpc::{orpc_impl::errors::RPCError, orpc_trait};
use snafu::Snafu;

/// A trait for servers that can be shutdown explicitly.
#[orpc_trait]
pub trait Shutdown {
    fn shutdown(&self) -> Result<(), RPCError>;
}

/// A utility for servers implementing [`Shutdown`]. They should:
/// 
/// * forward [`Shutdown::shutdown`] to [`ShutdownState::shutdown`].
/// * call [`ShutdownState::check_for_shutdown`] periodically in their threads and methods.
#[derive(Debug, Default)]
pub struct ShutdownState {
    is_shutdown: AtomicBool,
}

impl ShutdownState {
    /// Check if the server is shutdown and return [`ShutdownError::CleanShutdown`] if it is.
    pub fn check_for_shutdown(&self) -> Result<(), ShutdownError> {
        if self.is_shutdown.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(ShutdownError::CleanShutdown);
        }
        Ok(())
    }

    /// Mark the server as shutdown. This will never fail, but returns a Result to make the forwarding method in the
    /// server be:
    /// 
    /// ```ignore
    /// fn shutdown(&self) -> Result<(), RPCError> {
    ///     self.shutdown.shutdown()
    /// }
    /// ```
    pub fn shutdown(&self) -> Result<(), RPCError> {
        self.is_shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
}


/// An error to represent clean server shutdown.
#[derive(Snafu, Debug)]
pub(crate) enum ShutdownError {
    /// An RPC failure.
    #[snafu(transparent)]
    RPCError { source: RPCError },
    /// An error in the IO operation.
    #[snafu(display("Server shutdown cleanly."))]
    CleanShutdown,
}
