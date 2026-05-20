// SPDX-License-Identifier: MPL-2.0

//! OQueue exposure support for `/proc/oqueues/...`.
//!
//! The procfs inode adapters live in the kernel crate because procfs is currently
//! implemented there. This crate owns registration, stream construction, CBOR
//! encoding, and per-reader buffering.

#![no_std]
#![deny(unsafe_code)]

extern crate alloc;

mod config;
mod error;
mod registry;
mod ring_buffer;
mod stream;

use component::{ComponentInitError, init_component};
pub use error::ProcfsError;
pub use registry::{has_registered_prefix, open_stream, paths, register_oqueue};
pub use stream::StreamSource;

#[init_component]
fn init() -> Result<(), ComponentInitError> {
    Ok(())
}
