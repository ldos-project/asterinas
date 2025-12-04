// SPDX-License-Identifier: MPL-2.0

//! Virtual memory (VM).
//!
//! There are two primary VM abstractions:
//!  * Virtual Memory Address Regions (VMARs) a type of capability that manages
//!    user address spaces.
//!  * Virtual Memory Objects (VMOs) are are a type of capability that
//!    represents a set of memory pages.
//!
//! The concepts of VMARs and VMOs are originally introduced by
//! [Zircon](https://fuchsia.dev/fuchsia-src/reference/kernel_objects/vm_object).
//! As capabilities, the two abstractions are aligned with our goal
//! of everything-is-a-capability, although their specifications and
//! implementations in C/C++ cannot apply directly to Asterinas.
//! In Asterinas, VMARs and VMOs, as well as other capabilities, are implemented
//! as zero-cost capabilities.

use alloc::{sync::Arc, vec::Vec};
use core::{ops::Range, time::Duration};

use osdk_frame_allocator::FrameAllocator;
use osdk_heap_allocator::{HeapAllocator, type_from_layout};
use ostd::{
    mm::{
        AnyUFrameMeta, Frame, FrameAllocOptions, PageFlags, PageProperty, PagingConsts, UFrame,
        UntypedMem, Vaddr, page_size, vm_space::CursorMut,
    },
    sync::WaitQueue,
    task::disable_preempt,
};

use crate::{
    prelude::WaitTimeout,
    process::{Process, signal::constants::SIGSTOP},
};

pub mod page_fault_handler;
pub mod perms;
pub mod util;
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
    let total = regions
        .iter()
        .filter(|region| region.typ() == MemoryRegionType::Usable)
        .map(|region| region.len())
        .sum::<usize>();

    total
}

/// On construction, PauseProcGaurd will stop the process it holds a reference to, and when dropped
/// will resume the process. It is safe to resume the process before the drop.
struct PauseProcGaurd {
    proc: Arc<Process>,
}

impl PauseProcGaurd {
    fn new(proc: Arc<Process>) -> Self {
        // TODO(aneesh): Can this be done at the scheduler level instead?
        proc.stop(SIGSTOP);
        PauseProcGaurd { proc }
    }
}

impl Drop for PauseProcGaurd {
    fn drop(&mut self) {
        self.proc.resume()
    }
}

static PROMOTED_PAGE_SIZE: usize = page_size::<PagingConsts>(2);

fn do_for_each_submapping<F>(cursor: &mut CursorMut, start: usize, mut f: F) -> Result<(), ()>
where
    F: FnMut(&Range<Vaddr>, &Frame<dyn AnyUFrameMeta>, &PageProperty) -> Result<(), ()>,
{
    // Copy range.start -> range.start + page.
    let search_end = start + PROMOTED_PAGE_SIZE;
    cursor.jump(start).map_err(|_| ())?;

    while cursor.virt_addr() < search_end {
        // Query under the current cursor. If the virtual address is unmapped then we
        // fail and cannot map a huge page.
        let (sub_range, sub_mapping) = cursor.query().map_err(|_| ())?;

        // The mapping might not exist if the page hasn't been faulted in yet. We can
        // skip over such mappings.
        if let Some((ref sub_frame, sub_props)) = sub_mapping {
            f(&sub_range, sub_frame, &sub_props)?;
        }

        // Advance the cursor to the end of this mapping
        cursor.jump(sub_range.end).map_err(|_| ())?;
    }

    Ok(())
}

fn promote_hugepages(proc: &Arc<Process>, addr_hint: Option<Vaddr>) -> Result<(), ()> {
    // Ensure that the current process doesn't run until we have scanned it's mappings
    let _ = PauseProcGaurd::new(proc.clone());

    let proc_vm = proc.vm();
    let proc_vm_guard = proc_vm.lock_root_vmar();
    let proc_vmar = proc_vm_guard.unwrap();
    let preempt_guard = disable_preempt();
    let mut space_len = proc_vmar.size();
    let mut cursor = match proc_vmar
        .vm_space()
        .cursor_mut(&preempt_guard, &(0..space_len))
    {
        Ok(cursor) => cursor,
        _ => {
            return Ok(());
        }
    };

    // If we have an address hint, jump the cursor to that address, and only consider a single
    // region for promotion.
    if let Some(addr_hint) = addr_hint {
        cursor.jump(addr_hint).map_err(|_| ())?;
        space_len = PROMOTED_PAGE_SIZE;
    }

    while cursor.find_next(space_len - cursor.virt_addr()).is_some() {
        let (range, _) = match cursor.query() {
            Ok(v) => v,
            Err(_) => break,
        };

        // If the address is not hugepage aligned go to the next mapping
        if range.start % PROMOTED_PAGE_SIZE != 0 {
            let next = range.start - range.start % PROMOTED_PAGE_SIZE + PROMOTED_PAGE_SIZE;
            if next < space_len {
                if let Err(_) = cursor.jump(next) {
                    break;
                }
            } else {
                break;
            }
            continue;
        }

        if (range.end - range.start) >= PROMOTED_PAGE_SIZE {
            // Already huge, nothing to do here
            continue;
        }

        let start = range.start;

        let mut props: Option<PageProperty> = None;
        // Track if any sub pages were accessed or dirty
        let mut accessed = false;
        let mut dirty = false;

        // Find all subpages in this region and copy them to the new frame.
        let mut should_remap = true;

        let res = do_for_each_submapping(&mut cursor, range.start, |_, _, sub_props| {
            if let Some(page_props) = props {
                // We ignore the accessed and dirty bits from the page flags here
                // because the accessed/dirty bit of the huge page will be sum of all
                // the bits from the subflags.
                if !sub_props.equal_ignoring_ad(&page_props) {
                    should_remap = false;
                    return Err(());
                }
            } else {
                // Only consider writeable pages to avoid CoW/sharing issues.
                if !sub_props.flags.contains(PageFlags::W) {
                    should_remap = false;
                    return Err(());
                }
                props = Some(*sub_props);
            }
            accessed |= sub_props.flags.contains(PageFlags::ACCESSED);
            dirty |= sub_props.flags.contains(PageFlags::DIRTY);
            Ok(())
        });
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
                    return Err(());
                }
            };

            // Copy all pages into the huge page
            let mut writer = new_frame.writer();
            // Offset into the writer to track advancing the writer
            let mut last_copied = start;
            if do_for_each_submapping(&mut cursor, range.start, |sub_range, sub_frame, _| {
                // Advance the writer since not every part of the subspace might be
                // mapped.
                writer.skip(sub_range.start - last_copied);
                // Copy from this frame to the huge frame
                let mut reader = sub_frame.reader();
                reader.read(&mut writer);
                last_copied = sub_range.end;
                Ok(())
            })
            .is_err()
            {
                break;
            }

            let mut props = props.unwrap();
            if accessed {
                props.flags |= PageFlags::ACCESSED;
            }
            if dirty {
                props.flags |= PageFlags::DIRTY;
            }

            cursor.jump(range.start).unwrap();
            cursor.unmap(PROMOTED_PAGE_SIZE);
            cursor.jump(range.start).unwrap();
            cursor.map(new_frame, props);

            // The range has modified, get a new cursor
            drop(cursor);
            cursor = match proc_vmar
                .vm_space()
                .cursor_mut(&preempt_guard, &(0..space_len))
            {
                Ok(cursor) => cursor,
                _ => {
                    break;
                }
            };
        }
        if let Err(_) = cursor.jump(start + PROMOTED_PAGE_SIZE) {
            break;
        }
    }
    Ok(())
}

/// HugePage daemon that periodically attempts to promote pages to huge pages
pub fn hugepaged(initproc: Arc<Process>) {
    let sleep_queue = WaitQueue::new();
    let sleep_duration = Duration::from_secs(1);
    loop {
        // TODO(aneesh): this should be a select! over a timeout and a observation of an OQueue for
        // page mapping.
        let _ = sleep_queue.wait_until_or_timeout(|| -> Option<()> { None }, &sleep_duration);

        let mut procs: Vec<Arc<Process>> = Vec::new();
        procs.push(initproc.clone());
        while procs.len() > 0 {
            let proc = procs.pop().unwrap();
            proc.current_children()
                .iter()
                .for_each(|c| procs.push(c.clone()));

            if promote_hugepages(&proc, None).is_err() {
                break;
            }
        }
    }
}
