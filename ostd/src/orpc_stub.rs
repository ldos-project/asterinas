// SPDX-License-Identifier: MPL-2.0

//! Stub implementation for orpc which is used to simplify writing code that supports both ORPC and
//! non-ORPC modes. This is exported as `crate::orpc` when `cfg(baseline_asterinas)` is set.

pub use orpc_macros::baseline_trace_methods as trace_methods;
