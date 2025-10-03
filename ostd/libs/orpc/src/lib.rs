extern crate alloc;

// This declaration exposes the current crate under the name `orpc`. This is required to make the orpc macros work
// correctly in this crate, since they generate references to `::rpc`.
extern crate self as orpc;

pub mod oqueue;
pub mod orpc_impl;
pub mod sync;

pub use orpc_impl::{Server, ServerRef};
pub use orpc_impl::errors::RPCError;
pub use orpc_impl::framework::{CurrentServer, spawn_thread};
pub use orpc_macros::{orpc_impl, orpc_server, orpc_trait};
