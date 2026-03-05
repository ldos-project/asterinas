// SPDX-License-Identifier: MPL-2.0

//! Stub implementation for orpc which is used to simplify writing code that supports both ORPC and
//! non-ORPC modes. This is exported as `crate::orpc` when `cfg(baseline_asterinas)` is set.

pub use orpc_macros::{noop as orpc_server, noop as orpc_impl, noop as orpc_trait};

pub use crate::{new_server, orpc_common::errors};
