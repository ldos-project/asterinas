// SPDX-License-Identifier: MPL-2.0

//! Virtual memory (VM).
//!
//! There are two primary VM abstractions:
//!  * The VMAR (used to be Virtual Memory Address Region, now an orphan
//!    initialism) represents the entire virtual address space of a process;
//!  * The VMO (Virtual Memory Object) is a set of logically contiguous memory
//!    frames that can be mapped into one virtual address range. Frames in a
//!    VMO can be non-contiguous in physical memory.

use alloc::{sync::Arc, vec::Vec};

use align_ext::AlignExt;
use osdk_frame_allocator::FrameAllocator;
use osdk_heap_allocator::{HeapAllocator, type_from_layout};
use ostd::{
    error::InvalidArgsSnafu,
    mm::{
        FrameAllocOptions, PageFlags, PageProperty, PagingConsts, UFrame,
        io::util::HasVmReaderWriter, page_size,
    },
    task::disable_preempt,
};

use crate::{
    init::get_init_process,
    prelude::Error,
    process::{PauseProcGuard, Process},
    vm::vmar::{PageFaultOQueueMessage, RssType, VMAR_CAP_ADDR},
};

#[cfg(not(baseline_asterinas))]
pub mod hugepaged;
pub mod perms;
pub mod vmar;
pub mod vmo;

#[ostd::global_frame_allocator]
static FRAME_ALLOCATOR: FrameAllocator = FrameAllocator;

#[ostd::global_heap_allocator]
static HEAP_ALLOCATOR: HeapAllocator = HeapAllocator;

#[ostd::global_heap_allocator_slot_map]
const fn slot_type_from_layout(layout: core::alloc::Layout) -> Option<ostd::mm::heap::SlotInfo> {
    type_from_layout(layout)
}

/// Total physical memory in the entire system in bytes.
pub fn mem_total() -> usize {
    use ostd::boot::{boot_info, memory_region::MemoryRegionType};

    let regions = &boot_info().memory_regions;
    regions
        .iter()
        .filter(|region| region.typ() == MemoryRegionType::Usable)
        .map(|region| region.len())
        .sum::<usize>()
}

static PROMOTED_PAGE_SIZE: usize = page_size::<PagingConsts>(2);

pub fn num_anon_hugepages() -> i32 {
    let mut count = 0;
    let mut procs: Vec<Arc<Process>> = Vec::new();
    let Some(initproc) = get_init_process() else {
        // Handle the case for integration tests when hugepages haven't been allocated
        return 0;
    };

    procs.push(initproc.clone());
    while let Some(proc) = procs.pop() {
        proc.current_children()
            .iter()
            .for_each(|c| procs.push(c.clone()));

        let proc_vm_guard = proc.lock_vmar();
        let Some(proc_vmar) = proc_vm_guard.as_ref() else {
            continue;
        };
        let preempt_guard = disable_preempt();
        let vm_space = proc_vmar.vm_space();
        let Ok(mut cursor) = vm_space.cursor_mut(&preempt_guard, &(0..VMAR_CAP_ADDR)) else {
            continue;
        };
        cursor
            .do_for_each_submapping(0, VMAR_CAP_ADDR, |range, _, _| {
                if (range.end - range.start) >= PROMOTED_PAGE_SIZE
                    && range.start % PROMOTED_PAGE_SIZE == 0
                {
                    count += 1;
                }
                Ok(())
            })
            .ok();
    }
    count
}

#[cfg(not(baseline_asterinas))]
fn promote_hugepages(
    proc: &Arc<Process>,
    fault_hint: Option<PageFaultOQueueMessage>,
) -> Result<(), Error> {
    // Ensure that the current process doesn't run until we have scanned it's mappings
    let _ = PauseProcGuard::new(proc.clone());

    let proc_vm_guard = proc.lock_vmar();
    let Some(proc_vmar) = proc_vm_guard.as_ref() else {
        // The process may have exited right as we attempted to pause it.
        return Ok(());
    };
    let preempt_guard = disable_preempt();
    let vm_space = proc_vmar.vm_space();
    let Ok(mut cursor) = vm_space.cursor_mut(&preempt_guard, &(0..VMAR_CAP_ADDR)) else {
        return Ok(());
    };

    let mut real_rss = 0;
    cursor
        .do_for_each_submapping(0, VMAR_CAP_ADDR, |range, _, _| {
            real_rss += range.end - range.start;
            Ok(())
        })
        .unwrap();
    // TODO(aneesh): produce this into an OQueue instead of logging it.
    crate::prelude::info!(
        "proc={} RSS={} real_rss_n_pages={}",
        proc.pid(),
        proc_vmar.get_rss_counter(RssType::Anon),
        real_rss / 4096
    );
    cursor.jump(0).unwrap();

    // TODO(arthurp): Because the range is static and as large as possible (0..VMAR_CAP_ADDR), some
    // of the checks below may be unnecessary.

    // If we have an address hint, jump the cursor to that address, and only consider a single
    // region for promotion.
    if let Some(fault_hint) = fault_hint {
        // If the fault was not for this process, then ignore it
        if vm_space.id() != fault_hint.vm_space_id {
            return Ok(());
        }
        let addr_hint = fault_hint.fault_info.address.align_down(PROMOTED_PAGE_SIZE);
        cursor.jump(addr_hint)?;
        // We need to ensure that we are refining the space, not expanding it
        let huge_region_end = addr_hint + PROMOTED_PAGE_SIZE;
        if huge_region_end > VMAR_CAP_ADDR {
            return Ok(());
        }
    }

    while cursor
        .find_next(VMAR_CAP_ADDR - cursor.virt_addr())
        .is_some()
    {
        let Ok((range, _)) = cursor.query() else {
            return Ok(());
        };

        // If the address is not hugepage aligned go to the next mapping
        if range.start % PROMOTED_PAGE_SIZE != 0 {
            let next = range.start - range.start % PROMOTED_PAGE_SIZE + PROMOTED_PAGE_SIZE;
            if next < VMAR_CAP_ADDR {
                if cursor.jump(next).is_err() {
                    return Ok(());
                }
            } else {
                return Ok(());
            }
            continue;
        }

        if (range.end - range.start) >= PROMOTED_PAGE_SIZE {
            // Already huge, nothing to do here
            if cursor.jump(range.end).is_err() {
                return Ok(());
            }
            continue;
        }

        let start = range.start;

        let mut props: Option<PageProperty> = None;
        // Track if any sub pages were accessed or dirty
        let mut accessed = false;
        let mut dirty = false;

        // Tracks whether we can remap the following region as huge are not. This depends on every
        // page in the region being mapped and having compatible flags.
        let mut should_remap = true;
        let res = cursor.do_for_each_submapping(
            range.start,
            range.start + PROMOTED_PAGE_SIZE,
            |_, _, sub_props| {
                if let Some(page_props) = props {
                    // We ignore the accessed and dirty bits from the page flags here
                    // because the accessed/dirty bit of the huge page will be sum of all
                    // the bits from the subflags.
                    if !sub_props.equal_ignoring_accessed_dirty(&page_props) {
                        should_remap = false;
                        return InvalidArgsSnafu.fail();
                    }
                } else {
                    // Only consider writeable pages to avoid CoW/sharing issues.
                    if !sub_props.flags.contains(PageFlags::W) {
                        should_remap = false;
                        return InvalidArgsSnafu.fail()?;
                    }
                    props = Some(*sub_props);
                }
                accessed |= sub_props.flags.contains(PageFlags::ACCESSED);
                dirty |= sub_props.flags.contains(PageFlags::DIRTY);
                Ok(())
            },
        );
        should_remap &= res.is_ok();
        // If we never obtained the page properties, that means that we never saw any mapped
        // subregions. There's no need to remap this region.
        should_remap &= props.is_some();

        // TODO(aneesh): This is where we need an injectable policy - should we remap these
        // pages or not?

        if should_remap {
            // If we can't allocate huge pages, no point in checking other
            // processes - break out of the outer loop and go back to sleep.
            let new_frame: UFrame = match FrameAllocOptions::new()
                .with_level(2)
                .zeroed(true)
                .alloc_frame()
            {
                Ok(f) => f.into(),
                Err(_) => {
                    return InvalidArgsSnafu.fail()?;
                }
            };

            // Copy all pages into the huge page
            let mut writer = new_frame.writer();
            // Offset into the writer to track advancing the writer
            let mut last_copied = start;
            if cursor
                .do_for_each_submapping(
                    range.start,
                    range.start + PROMOTED_PAGE_SIZE,
                    |sub_range, sub_frame, _| {
                        // Advance the writer since not every part of the subspace might be
                        // mapped.
                        writer.skip(sub_range.start - last_copied);
                        // Copy from this frame to the huge frame
                        let mut reader = sub_frame.reader();
                        reader.read(&mut writer);
                        last_copied = sub_range.end;
                        Ok(())
                    },
                )
                .is_err()
            {
                return Ok(());
            }

            // Set the accessed and dirty bits if any page in the region was accessed or marked
            // dirty respectively.
            let mut props = props.unwrap();
            if accessed {
                props.flags |= PageFlags::ACCESSED;
            }
            if dirty {
                props.flags |= PageFlags::DIRTY;
            }

            // Actually do the remapping
            cursor.jump(range.start).unwrap();
            cursor.unmap(PROMOTED_PAGE_SIZE);
            cursor.jump(range.start).unwrap();
            cursor.map(new_frame, props);

            // The range has modified, get a new cursor
            drop(cursor);
            cursor = match proc_vmar
                .vm_space()
                .cursor_mut(&preempt_guard, &(0..VMAR_CAP_ADDR))
            {
                Ok(cursor) => cursor,
                _ => {
                    return Ok(());
                }
            };
        }
        if cursor.jump(start + PROMOTED_PAGE_SIZE).is_err() {
            return Ok(());
        }
    }
    Ok(())
}
