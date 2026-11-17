// SPDX-License-Identifier: MPL-2.0

//! This crate holds the hard-float elements of the kernel. It is built specially using hard-float
//! and then linked to the rest of the kernel.

#![no_std]

use ostd::arch::cpu::kernel_fpu::InKernelFpuGuard;

pub mod aes;
pub mod model;
