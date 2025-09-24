// SPDX-License-Identifier: MPL-2.0

//! This module offers `/proc/uptime` file support, which provides
//! the time since boot. The time is represented in seconds with a 2 digit decimal.
//!
//! /proc/uptime should also report the time spent idling summed across all cores. This is currently
//! unimplemented.
//!
//! Reference: <https://man7.org/linux/man-pages/man5/proc_uptime.5.html>

use aster_time::read_monotonic_time;

use crate::{
    alloc::borrow::ToOwned,
    fs::{
        procfs::template::{FileOps, ProcFileBuilder},
        utils::Inode,
    },
    prelude::*,
};

use alloc::format;

/// Represents the inode at `/proc/uptime`.
pub struct UptimeFileOps;

impl UptimeFileOps {
    /// Create a new inode for `/proc/cpuinfo`.
    pub fn new_inode(parent: Weak<dyn Inode>) -> Arc<dyn Inode> {
        ProcFileBuilder::new(Self).parent(parent).build().unwrap()
    }

    pub fn get_uptime() -> String {
        let uptime: f64 = read_monotonic_time().as_secs_f64();
        format!("{:.2}\n", uptime).to_owned()
    }
}

impl FileOps for UptimeFileOps {
    /// Retrieve the data for `/proc/cpuinfo`.
    fn data(&self) -> Result<Vec<u8>> {
        let output = Self::get_uptime();
        Ok(output.into_bytes())
    }
}
