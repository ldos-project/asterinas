// SPDX-License-Identifier: MPL-2.0
//! ORPC - Observable Remote Procedure Call

extern crate alloc;

pub mod framework;
pub mod oqueue;
pub mod statistics;
pub mod sync;

pub use orpc_macros::{orpc_impl, orpc_server, orpc_trait};
