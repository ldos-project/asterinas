// SPDX-License-Identifier: MPL-2.0

use core::sync::atomic::{AtomicBool, Ordering};

use ostd::mm::{
    PagingConsts, PagingLevel, page_size,
    vm_space::{VmMappingPolicy, VmMappingRequest},
};

/// Enable hugepage usage using some default policy.
static MAP_HUGE_ENABLED: AtomicBool = AtomicBool::new(false);
aster_cmdline::define_flag_param!("vm.huge_mapping_enabled", MAP_HUGE_ENABLED);

/// Returns true if huge pages were enabled on the kernel CLI
pub fn huge_mapping_enabled() -> bool {
    MAP_HUGE_ENABLED.load(Ordering::Relaxed)
}

/// Attempt to preserve hugepage mapping when the user application advises the kernel it no longer
/// needs pages.
static MAP_HUGE_PRESERVE_ON_DONTNEED: AtomicBool = AtomicBool::new(false);
aster_cmdline::define_flag_param!(
    "vm.huge_mapping_preserve_on_dontneed",
    MAP_HUGE_PRESERVE_ON_DONTNEED
);

/// Returns true if huge page mappings should be preserved when a MADV_DONTNEED is issued.
#[expect(unused)]
pub(crate) fn huge_mapping_preserve_on_dontneed() -> bool {
    MAP_HUGE_PRESERVE_ON_DONTNEED.load(Ordering::Relaxed)
}

/// VmMappingPolicy implementation that always maps a huge page when possible.
pub(super) struct VmMappingPolicyGreedyHugeMapping {}

impl VmMappingPolicy for VmMappingPolicyGreedyHugeMapping {
    fn get_page_level(&self, req: &VmMappingRequest) -> PagingLevel {
        // Check if the address is aligned to a level 2 page. If it is not aligned, it cannot be
        // mapped at a level larger than 1.
        if req
            .page_aligned_addr
            .is_multiple_of(page_size::<PagingConsts>(2))
        {
            2
        } else {
            1
        }
    }
}
