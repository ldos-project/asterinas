// SPDX-License-Identifier: MPL-2.0

//! User address space management.

pub mod huge_pages;
mod interval_set;
mod util;
mod vm_mapping;

mod vmar_impls;

use ostd::mm::Vaddr;
pub use vmar_impls::{RssType, Vmar, map::VmarMapOffset, page_fault::PageFaultInfo};

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
#[derive(Clone, Copy)]
pub struct PageFaultOQueueMessage {
    /// Opaque identifier for which vm_space the fault corresponds to
    pub vm_space_id: u64,
    /// The fault information provided to the page fault handler
    pub fault_info: PageFaultInfo,
}

#[cfg(not(baseline_asterinas))]
pub mod oqueues {
    use alloc::sync::Arc;
    use core::{sync::atomic::AtomicUsize, time::Duration};

    use ostd::orpc::legacy_oqueue::ringbuffer::MPMCOQueue;
    use spin::Once;

    use super::PageFaultOQueueMessage;
    use crate::time::{Clock, clocks::MonotonicRawClock};

    // TODO(aneesh): Move this somewhere more generic
    #[derive(Clone, Copy)]
    pub struct ObservableEvent<T> {
        pub event: T,
        #[expect(unused)]
        pub timestamp: Duration,
    }

    impl<T> ObservableEvent<T> {
        pub fn new(event: T) -> Self {
            Self {
                event,
                timestamp: MonotonicRawClock::get().read_time(),
            }
        }
    }

    pub(super) static PAGE_FAULT_OQUEUE: Once<
        Arc<MPMCOQueue<ObservableEvent<PageFaultOQueueMessage>>>,
    > = Once::new();

    pub static GLOBAL_RSS: AtomicUsize = AtomicUsize::new(0);

    pub(super) static RSS_DELTA_OQUEUE: Once<Arc<MPMCOQueue<ObservableEvent<isize>>>> = Once::new();

    #[expect(unused)]
    pub fn get_rss_delta_oqueue() -> Arc<MPMCOQueue<ObservableEvent<isize>>> {
        RSS_DELTA_OQUEUE.wait().clone()
    }

    pub fn get_page_fault_oqueue() -> Arc<MPMCOQueue<ObservableEvent<PageFaultOQueueMessage>>> {
        PAGE_FAULT_OQUEUE.wait().clone()
    }
}

pub fn init_in_first_kthread() {
    #[cfg(not(baseline_asterinas))]
    {
        use ostd::orpc::legacy_oqueue::ringbuffer::MPMCOQueue;
        // Only support a single strong observer for now - hugepaged.
        oqueues::PAGE_FAULT_OQUEUE.call_once(|| MPMCOQueue::new(64, 1));
        oqueues::RSS_DELTA_OQUEUE.call_once(|| MPMCOQueue::new(64, 1));
    }
}
