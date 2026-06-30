// SPDX-License-Identifier: MPL-2.0

use serde::Serialize;

use super::{Interval, RssDelta, Vmar};
#[cfg(not(baseline_asterinas))]
use crate::vm::vmar::{PageFaultOQueueMessage, oqueues};
use crate::{prelude::*, vm::perms::VmPerms};

impl Vmar {
    pub fn handle_page_fault(&self, page_fault_info: &PageFaultInfo) -> Result<()> {
        let inner = self.inner.read();

        let address = page_fault_info.address;
        if let Some(vm_mapping) = inner.vm_mappings.find_one(&address) {
            debug_assert!(vm_mapping.range().contains(&address));

            let mut rss_delta = RssDelta::new(self);
            let res = vm_mapping.handle_page_fault(&self.vm_space, page_fault_info, &mut rss_delta);
            #[cfg(not(baseline_asterinas))]
            if res.is_ok() {
                self.page_fault_oqueue_producer
                    .produce(oqueues::ObservableEvent::new(PageFaultOQueueMessage {
                        vm_space_id: self.vm_space.id(),
                        fault_info: *page_fault_info,
                    }))?;
            }
            return res;
        }

        return_errno_with_message!(
            Errno::EACCES,
            "no VM mappings contain the page fault address"
        );
    }
}

/// Page fault information converted from [`CpuException`].
///
/// `TryFrom<CpuException>` should be implemented for this struct.
/// If [`CpuException`] is a page fault, `try_from` should return `Ok(PageFaultInfo)`,
/// or `Err(())` (no error information) otherwise.
///
/// [`CpuException`]: ostd::arch::cpu::context::CpuException
#[derive(Clone, Copy, Debug, Serialize)]
pub struct PageFaultInfo {
    /// The virtual address where a page fault occurred.
    pub(crate) address: Vaddr,

    /// The [`VmPerms`] required by the memory operation that causes page fault.
    /// For example, a "store" operation may require `VmPerms::WRITE`.
    pub(crate) required_perms: VmPerms,

    /// Whether this page fault is forced (e.g., manually triggered by `ptrace`).
    /// A forced page fault may bypass some permission checks.
    pub(crate) is_forced: bool,
}

impl PageFaultInfo {
    /// Creates a new `PageFaultInfo`.
    pub fn new(address: Vaddr, required_perms: VmPerms) -> Self {
        Self {
            address,
            required_perms,
            is_forced: false,
        }
    }

    /// Returns whether this page fault is forced.
    pub(in crate::vm::vmar) fn is_forced(&self) -> bool {
        self.is_forced
    }

    /// Marks this page fault as forced.
    pub(super) fn force(mut self) -> Self {
        self.is_forced = true;
        self
    }
}
