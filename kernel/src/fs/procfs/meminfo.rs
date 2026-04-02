// SPDX-License-Identifier: MPL-2.0

//! This module offers `/proc/meminfo` file support, which tells the user space
//! about the memory statistics in the entire system. The definition of the
//! fields are similar to that of Linux's but there exist differences.
//!
//! Reference: <https://man7.org/linux/man-pages/man5/proc_meminfo.5.html>

use alloc::format;
use crate::vm::num_hugepages;
use crate::vm::num_free_hugepages;

use crate::{
    fs::{
        procfs::template::{FileOps, ProcFileBuilder},
        utils::Inode,
    },
    prelude::*,
};

/// Represents the inode at `/proc/meminfo`.
pub struct MemInfoFileOps;

impl MemInfoFileOps {
    pub fn new_inode(parent: Weak<dyn Inode>) -> Arc<dyn Inode> {
        ProcFileBuilder::new(Self).parent(parent).build().unwrap()
    }
}

impl FileOps for MemInfoFileOps {
    fn data(&self) -> Result<Vec<u8>> {
        // The total amount of physical memory available to the system.
        let total = crate::vm::mem_total();
        // An estimation of how much memory is available for starting new
        // applications, without disk operations.
        let available = osdk_frame_allocator::load_total_free_size();

        // Convert the values to KiB.
        let total = total / 1024;
        let available = available / 1024;
        let free = total - available;
        let total_hugepages = num_hugepages();
        let free_hugepages = num_free_hugepages();
        let hugepage_size = 2048;
        let output = format!(
            "MemTotal:        {:>8} kB\nMemFree:         {:>8} kB\nMemAvailable:    {:>8} kB\nHugePages_Total: {:>8}\nHugePages_Free:  {:>8}\nHugepagesize:    {:>8} kB\n",
            total, free, available, total_hugepages, free_hugepages, hugepage_size
        );
        Ok(output.into_bytes())
    }
}
