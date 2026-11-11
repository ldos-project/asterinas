// SPDX-License-Identifier: MPL-2.0

//! Benchmarks for Mariposa.
//!
//! Each benchmark lives in its own module. [`framework`] holds the parts that are not specific to
//! any one benchmark, so that new benchmarks do not have to reinvent them.

#![no_std]
#![deny(unsafe_code)]
#![feature(format_args_nl)]

extern crate alloc;

pub mod framework;
#[cfg(not(baseline_asterinas))]
pub mod oqueue_roundtrip;
