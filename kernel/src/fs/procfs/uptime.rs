// SPDX-License-Identifier: MPL-2.0

//! This module offers `/proc/uptime` file support, which provides the time since boot and the time
//! spent idling summed across all cores. The time is represented in seconds with a 2 digit decimal.
//!
//! Reference: <https://man7.org/linux/man-pages/man5/proc_uptime.5.html>

use alloc::format;

use aster_time::read_monotonic_time;
use ostd::task::scheduler::idle_time;

use crate::{
    alloc::borrow::ToOwned,
    fs::{
        procfs::template::{FileOps, ProcFileBuilder},
        utils::Inode,
    },
    prelude::*,
};

/// Represents the inode at `/proc/uptime`.
pub struct UptimeFileOps;

impl UptimeFileOps {
    /// Create a new inode for `/proc/cpuinfo`.
    pub fn new_inode(parent: Weak<dyn Inode>) -> Arc<dyn Inode> {
        ProcFileBuilder::new(Self).parent(parent).build().unwrap()
    }

    pub fn get_uptime() -> String {
        let uptime = read_monotonic_time();
        let total_idle_time = idle_time();
        format!(
            "{}.{:0>2} {}.{:0>2}\n",
            uptime.as_secs(),
            uptime.subsec_millis() / 10,
            total_idle_time.as_secs(),
            total_idle_time.subsec_millis() / 10
        )
        .to_owned()
    }
}

impl FileOps for UptimeFileOps {
    /// Retrieve the data for `/proc/cpuinfo`.
    fn data(&self) -> Result<Vec<u8>> {
        let output = Self::get_uptime();
        Ok(output.into_bytes())
    }
}
