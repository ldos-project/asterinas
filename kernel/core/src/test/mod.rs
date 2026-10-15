// SPDX-License-Identifier: MPL-2.0

//! A module for kernel-side components of user-space tests, and kernel-side tests which require
//! `kernel/core` features.
//!
//! This is somewhat ad-hoc, but structure should be added if it becomes clear what structure is
//! best.

#[cfg(not(baseline_asterinas))]
pub(crate) mod oqfs_test;

pub fn init_after_init_process() {
    #[cfg(not(baseline_asterinas))]
    oqfs_test::init_after_init_process();
}
