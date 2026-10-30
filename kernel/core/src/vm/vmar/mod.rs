// SPDX-License-Identifier: MPL-2.0

//! User address space management.

mod handle;
pub mod huge_pages;
mod interval_set;
mod rmap;
mod util;
mod vm_mapping;

mod vmar_impls;

use alloc::sync::Arc;

use ostd::{
    DefaultProjection,
    mm::{Vaddr, VmSpace},
    orpc::oqueue::Element,
};

pub use self::{
    handle::VmarHandle,
    rmap::{Rmap, RmapEntry},
    vmar_impls::{
        RssType, Vmar, map::VmarMapOffset, page_fault::PageFaultInfo, remap::RemapOldMappingAction,
    },
};

pub const VMAR_LOWEST_ADDR: Vaddr = 0x001_0000; // 64 KiB is the Linux configurable default
pub const VMAR_CAP_ADDR: Vaddr = ostd::mm::MAX_USERSPACE_VADDR;

/// Returns whether the input `vaddr` is a legal user space virtual address.
pub fn is_userspace_vaddr(vaddr: Vaddr) -> bool {
    (VMAR_LOWEST_ADDR..VMAR_CAP_ADDR).contains(&vaddr)
}

/// Returns whether `vaddr` and `len` specify a legal user space virtual address range.
fn is_userspace_vaddr_range(vaddr: Vaddr, len: usize) -> bool {
    vaddr >= VMAR_LOWEST_ADDR
        && VMAR_CAP_ADDR
            .checked_sub(vaddr)
            .is_some_and(|gap| gap >= len)
}

// TODO(aneesh) can this just be PageFaultInfo directly?
/// Notification message to inform policies about a page fault
#[derive(Element, DefaultProjection)]
pub struct PageFaultMessage<'a> {
    /// The vm space of this page fault
    #[projection(opaque)]
    pub vm_space: &'a Arc<VmSpace>,
    /// The fault information provided to the page fault handler
    pub fault_info: PageFaultInfo,
}

#[cfg(not(baseline_asterinas))]
pub mod oqueues {
    use core::sync::atomic::AtomicUsize;

    use ostd::orpc::oqueue::GenericOQueueRef;
    use spin::Once;

    use crate::vm::vmar::PageFaultMessageDescriptor;

    pub(super) static PAGE_FAULT_OQUEUE: Once<GenericOQueueRef<PageFaultMessageDescriptor>> =
        Once::new();

    pub static GLOBAL_RSS: AtomicUsize = AtomicUsize::new(0);

    pub fn get_page_fault_oqueue() -> GenericOQueueRef<PageFaultMessageDescriptor> {
        PAGE_FAULT_OQUEUE.wait().clone()
    }
}

pub fn init_in_first_kthread() {
    #[cfg(not(baseline_asterinas))]
    {
        use ostd::orpc::oqueue::GenericOQueueRef;
        // Only support a single strong observer for now - hugepaged.
        oqueues::PAGE_FAULT_OQUEUE.call_once(|| GenericOQueueRef::new_anonymous(64));
    }
}
