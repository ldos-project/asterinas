// SPDX-License-Identifier: MPL-2.0

//! Re-exports used throughout the ext2 module.

pub(super) use core::{
    ops::{Deref, DerefMut, Range},
    time::Duration,
};

pub(super) use align_ext::AlignExt;
pub(super) use aster_block::{
    BLOCK_SIZE, BlockDevice, SECTOR_SIZE,
    bio::{BioDirection, BioSegment, BioStatus},
    id::Bid,
};
pub(super) use io_util::batch::IoBatch;
pub(super) use ostd::{
    mm::{Frame, FrameAllocOptions, Segment, USegment, VmIo, VmIoFill},
    sync::{RwMutex, RwMutexReadGuard, RwMutexWriteGuard},
};

pub(super) use super::{
    inode::{Ext2Bid, Ext2Ino, Iblock},
    utils::{Dirty, IsPowerOf},
};
#[cfg(baseline_asterinas)]
pub(super) use crate::fs::vfs::page_cache::PageCacheBackend;
pub(super) use crate::{
    fs::{
        file::InodeType,
        utils::{CStr256, DirentVisitor, Str16, Str64},
        vfs::page_cache::{CachePage, PageCache},
    },
    prelude::*,
    time::UnixTime,
    vm::page_cache::{BlockAsPageCacheBackend, PageCache, PageCacheBackend},
};
