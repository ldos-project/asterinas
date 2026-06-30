// SPDX-License-Identifier: MPL-2.0

//! A minimal early-boot test kernel.
//!
//! This should include tests which cannot be ktests because they must executing before the ostd
//! kernel (or other components) are fully initialized.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use ostd::{
    power::{ExitCode, poweroff},
    prelude::println,
};

#[cfg(not(baseline_asterinas))]
mod orpc_tests;

#[cfg(not(baseline_asterinas))]
fn test_early_boot_server() {
    orpc_tests::test_early_boot_server()
}

#[cfg(baseline_asterinas)]
fn test_early_boot_server() {}

#[ostd::main]
fn main() {
    println!("Early-boot tests...");

    test_early_boot_server();

    println!("Tests passed.");

    poweroff(ExitCode::Success);
}
