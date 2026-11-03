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

#[ostd::main]
fn main() {
    println!("Early-boot tests...");

    println!("Tests passed.");

    poweroff(ExitCode::Success);
}
