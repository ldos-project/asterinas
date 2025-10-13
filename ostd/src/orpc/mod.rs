//! ORPC - Observable Remote Procedure Call

extern crate alloc;

// This declaration exposes the current crate under the name `orpc`. This is required to make the orpc macros work
// correctly in this crate, since they generate references to `::orpc`.
extern crate self as orpc;

pub mod oqueue;
pub mod orpc_impl;
pub mod sync;
