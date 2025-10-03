//! A set of utilities for concurrency and synchronization which act similarly to those that will be available in the
//! Asterinas kernel.
//!
//! NOTE: These are testing stubs and will be replaced by real implementations in most cases. These implementations or
//! similar may be useful for user-space testing of kernel code. Related:
//! https://github.com/ldos-project/asterinas/issues/26

use alloc::fmt::Debug;
use core::marker::PhantomData;

pub mod task;
pub mod mutex;
