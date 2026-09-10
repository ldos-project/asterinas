// SPDX-License-Identifier: MPL-2.0

//! CPU context & state control and CPU local memory.

pub mod context;
pub mod cpuid;
pub mod extension;
/// Kernel FPU state management and scoped FPU access.
pub mod kernel_fpu;
pub mod local;
