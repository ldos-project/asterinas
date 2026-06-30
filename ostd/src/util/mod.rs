// SPDX-License-Identifier: MPL-2.0

//! Utility types and methods.

pub mod callback_counter;
mod either;
pub mod id_set;
pub mod ignore_error;
mod macros;
pub(crate) mod ops;
pub(crate) mod range_alloc;
pub(crate) mod range_counter;
pub mod untyped_box;

pub use either::Either;
