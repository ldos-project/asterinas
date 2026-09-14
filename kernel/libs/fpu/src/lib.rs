// SPDX-License-Identifier: MPL-2.0

#![no_std]

#[cfg(target_arch = "x86_64")]
use ostd::arch::cpu::kernel_fpu::InKernelFpuSection;
#[cfg(not(target_arch = "x86_64"))]
pub type InKernelFpuSection = ();

pub mod aes;
pub mod model;
