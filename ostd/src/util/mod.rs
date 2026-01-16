// SPDX-License-Identifier: MPL-2.0

//! Utility types and methods.

pub mod callback_counter;
mod either;
pub mod ignore_error;
mod macros;
pub(crate) mod ops;
pub(crate) mod range_alloc;

pub use either::Either;
