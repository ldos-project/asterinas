// SPDX-License-Identifier: MPL-2.0

//! Virtual File System (VFS) layer.
//!
//! The VFS provides a unified abstraction over different file system implementations,
//! serving as the bridge between system calls and concrete file systems.

mod fs_apis;
pub mod notify;
#[cfg_attr(baseline_asterinas, path = "page_cache_baseline.rs")]
pub mod page_cache;
#[cfg(not(baseline_asterinas))]
mod page_prefetch;
pub mod path;
pub mod range_lock;
pub mod server_traits;

// Re-export commonly used abstractions from `fs_apis`
pub use fs_apis::{file_system, inode, inode_ext, registry, xattr};

pub(super) fn init() {
    fs_apis::init();
    path::init();
}

pub(crate) fn init_in_first_kthread() {
    page_cache::init_in_first_kthread();
}
