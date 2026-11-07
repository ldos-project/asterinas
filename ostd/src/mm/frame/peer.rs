// SPDX-License-Identifier: MPL-2.0

//! Peer memory: the RAM of another VM on the same host, seen through a PCI
//! BAR, whose pages can be used as ordinary untyped frames (lent in place).
//!
//! derailleur moves a process from another VM to this one without copying
//! its memory: the other VM keeps its pages pinned, and this kernel maps them
//! into the process as if they were its own.  Registering the BAR gives its
//! frames metadata slots and a linear mapping, like RAM, but they are never
//! handed to the frame allocator: dropping the last reference to one only
//! marks its slot unused again.
//!
//! The other VM may write these pages at any time (until it gives them up),
//! which is why they can only be untyped frames.

use core::{
    ops::Range,
    sync::atomic::{AtomicUsize, Ordering},
};

use super::{Frame, meta::init_unused_slots, untyped::UFrame};
use crate::{
    Result,
    error::{AccessDeniedSnafu, InvalidArgsSnafu},
    io::IoMem,
    mm::{FrameAllocOptions, HasPaddr, HasSize, PAGE_SIZE, Paddr, kspace::map_peer_region},
};

/// The registered peer region (one at most).  `PEER_END` is published last.
static PEER_START: AtomicUsize = AtomicUsize::new(0);
static PEER_END: AtomicUsize = AtomicUsize::new(0);
/// Peer frames currently referenced here.
static LIVE: AtomicUsize = AtomicUsize::new(0);

/// Whether `paddr` is in the registered peer region.
pub(crate) fn contains(paddr: Paddr) -> bool {
    let end = PEER_END.load(Ordering::Acquire);
    end != 0 && paddr >= PEER_START.load(Ordering::Relaxed) && paddr < end
}

/// Metadata of a frame lent by the peer VM.
#[derive(Debug)]
pub struct PeerFrameMeta;

// SAFETY: untyped frames can be safely read.
unsafe impl super::meta::AnyFrameMeta for PeerFrameMeta {
    fn on_drop(&mut self, _reader: &mut crate::mm::VmReader<crate::mm::Infallible>) {
        LIVE.fetch_sub(1, Ordering::Relaxed);
    }

    fn is_untyped(&self) -> bool {
        true
    }
}
impl super::untyped::AnyUFrameMeta for PeerFrameMeta {}
crate::check_frame_meta_layout!(PeerFrameMeta);

/// The peer VM's RAM, registered.
#[derive(Debug)]
pub struct PeerMemory {
    range: Range<Paddr>,
    _io_mem: IoMem,
}

impl PeerMemory {
    /// Registers the memory behind `io_mem` (a BAR exposing another VM's
    /// RAM, write-back) as peer memory.  Only one region can be registered.
    pub fn register(io_mem: IoMem) -> Result<Self> {
        let start = io_mem.paddr();
        let range = start..start + io_mem.size();
        // One metadata page covers this many frames; keep pages whole.
        let per_meta_page = PAGE_SIZE / super::meta::META_SLOT_SIZE;
        if !start.is_multiple_of(PAGE_SIZE * per_meta_page)
            || !range.len().is_multiple_of(PAGE_SIZE * per_meta_page)
            || start < super::max_paddr()
            || PEER_END.load(Ordering::Acquire) != 0
        {
            return Err(InvalidArgsSnafu.build());
        }
        let nr_meta = range.len() / PAGE_SIZE / per_meta_page;
        let mut meta_pages = alloc::vec::Vec::with_capacity(nr_meta);
        for _ in 0..nr_meta {
            // Leaked on purpose: the slots live as long as the kernel.
            let frame = FrameAllocOptions::new().zeroed(false).alloc_frame()?;
            let paddr = frame.into_raw();
            // SAFETY: the page was just allocated and leaked: ours alone.
            unsafe { init_unused_slots(paddr) };
            meta_pages.push(paddr);
        }
        // SAFETY: the range is a BAR above all RAM (checked), never given to
        // the frame allocator; the metadata pages hold its unused slots.
        unsafe { map_peer_region(&range, &meta_pages) };
        PEER_START.store(range.start, Ordering::Relaxed);
        PEER_END.store(range.end, Ordering::Release);
        crate::early_println!(
            "peer memory: {:#x}..{:#x} registered ({} metadata pages)",
            range.start,
            range.end,
            nr_meta
        );
        Ok(Self {
            range,
            _io_mem: io_mem,
        })
    }

    /// The registered range.
    pub fn range(&self) -> &Range<Paddr> {
        &self.range
    }

    /// The peer's frame at `paddr`, to map like any untyped frame.  Fails if
    /// it is outside the region or already in use here.
    pub fn frame(&self, paddr: Paddr) -> Result<UFrame> {
        if !self.range.contains(&paddr) || !paddr.is_multiple_of(PAGE_SIZE) {
            return Err(InvalidArgsSnafu.build());
        }
        let frame = Frame::from_unused(paddr, PeerFrameMeta, 1).map_err(|_| AccessDeniedSnafu.build())?;
        LIVE.fetch_add(1, Ordering::Relaxed);
        Ok(frame.into())
    }

    /// Peer frames currently referenced here (mapped or held).
    pub fn live_frames() -> usize {
        LIVE.load(Ordering::Relaxed)
    }
}
