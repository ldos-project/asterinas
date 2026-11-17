// SPDX-License-Identifier: MPL-2.0
//! OQueue-based communication and observation.

extern crate alloc;

pub mod oqueue;
pub mod path;
pub mod serialization;
pub mod sync;

pub use orpc_macros::{TupleSerialize, trace_methods};

mod tuple_serialize_test;
