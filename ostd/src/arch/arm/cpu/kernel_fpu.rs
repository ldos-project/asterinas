// SPDX-License-Identifier: MPL-2.0

//! Kernel FPU state management and scoped FPU access.

/// A stub for a kernel FPU section on architectures without FPU state management.
pub struct InKernelFpuGuard;

/// Start a kernel FPU section.
pub fn enable_kernel_fpu() -> InKernelFpuGuard {
    panic!("In-kernel FPU not yet supported on arm.")
}
