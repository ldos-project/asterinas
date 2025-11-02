// ostd/src/orpc/framework/shutdown.rs
use core::sync::atomic::AtomicBool;

use orpc_macros::orpc_trait;

use crate::orpc::{framework::errors::RPCError, oqueue::{OQueueRef, locking::ObservableLockingQueue, reply::ReplyQueue}};

/// Trait that allows a server to be shut down gracefully.
#[orpc_trait]
pub trait Shutdown {
    /// Shuts down the server.
    fn shutdown(&self) -> Result<(), RPCError>;
}

/// State used to manage the shutdown of a server.
///
/// # Example
///
/// ```ignore
/// #[orpc_server(orpc::Shutdown)]
/// struct Server {
///     state: ShutdownState,
/// }
///
/// #[orpc_impl]
/// impl Shutdown for Server {
///     fn shutdown(&self) -> Result<(), RPCError> {
///         self.state.shutdown();
///         Ok(())
///     }
/// }
/// 
/// 
///
/// fn main() {
///     let server = Arc::new(Server {
///         state: ShutdownState::new(),
///     });
///
///     // Simulate shutting down the server
///     if let Err(e) = server.shutdown() {
///         eprintln!("Failed to shut down server: {}", e);
///     }
/// }
/// ```
pub struct ShutdownState {
    is_shutdown: AtomicBool,
    pub shutdown_oqueue: OQueueRef<()>
}

impl Default for ShutdownState {
    fn default() -> Self {
        Self { is_shutdown: Default::default(), shutdown_oqueue: ObservableLockingQueue::new(2, 4) }
    }
}

impl ShutdownState {
    /// Marks the server as shut down.
    pub fn shutdown(&self) {
        self.is_shutdown.store(true, core::sync::atomic::Ordering::Release);
        self.shutdown_oqueue.produce(());
    }

    /// Checks if the server has been shut down.
    ///
    /// # Errors
    ///
    /// Returns `RPCError::ServerMissing` if the server is not shut down.
    pub fn check(&self) -> Result<(), RPCError> {
        if !self.is_shutdown.load(core::sync::atomic::Ordering::Acquire) {
            Ok(())
        } else {
            Err(RPCError::ServerMissing)
        }
    }
}