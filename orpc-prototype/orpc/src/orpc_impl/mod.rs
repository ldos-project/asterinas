/// The module containing implementations of the ORPC framework.

use std::{panic::RefUnwindSafe, sync::Arc};

use crate::orpc_impl::framework::ServerBase;

pub mod errors;
pub mod framework;
mod integration_test;

/// The primary trait for all server. This provides access to information and capabilities common to all servers.
pub trait Server: Sync + Send + RefUnwindSafe + 'static {
    /// **INTERNAL** User code should never call this directly, however it cannot be private because generated code must
    /// use it.
    ///
    /// Get a reference to the struct implementing all the fundamental server operations. This is effectively the base
    /// class pointer of this server.
    #[doc(hidden)]
    fn orpc_server_base(&self) -> &ServerBase;
}

/// A reference to a server. `T` is the server type and should always implement [`Server`]. Due to Rust limitations,
/// this is not checked.
pub type ServerRef<T> = Arc<T>;
