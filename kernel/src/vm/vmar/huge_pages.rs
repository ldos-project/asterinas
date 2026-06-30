// SPDX-License-Identifier: MPL-2.0

use core::sync::atomic::{AtomicBool, Ordering};

use ostd::{
    mm::{
        PagingConsts, PagingLevel, page_size,
        vm_space::{VmMappingPolicy, VmMappingRequest},
    },
    orpc::{errors::RPCError, orpc_impl, orpc_server},
};

static MAP_HUGE_ENABLED: AtomicBool = AtomicBool::new(false);

/// Returns true if huge pages were enabled on the kernel CLI
pub fn huge_mapping_enabled() -> bool {
    MAP_HUGE_ENABLED.load(Ordering::Relaxed)
}

pub fn set_huge_mapping_enabled(value: bool) {
    MAP_HUGE_ENABLED.store(value, Ordering::Relaxed)
}

static MAP_HUGE_PRESERVE_ON_DONTNEED: AtomicBool = AtomicBool::new(false);

/// Returns true if huge page mappings should be preserved when a MADV_DONTNEED is issued.
pub fn huge_mapping_preserve_on_dontneed() -> bool {
    MAP_HUGE_PRESERVE_ON_DONTNEED.load(Ordering::Relaxed)
}

pub fn set_huge_mapping_preserve_on_dontneed(value: bool) {
    MAP_HUGE_PRESERVE_ON_DONTNEED.store(value, Ordering::Relaxed)
}

/// VmMappingPolicy implementation that always maps a huge page when possible.
#[orpc_server(ostd::mm::vm_space::VmMappingPolicy)]
pub(super) struct VmMappingPolicyGreedyHugeMapping {}

#[orpc_impl]
impl VmMappingPolicy for VmMappingPolicyGreedyHugeMapping {
    fn get_page_level(
        &self,
        req: &VmMappingRequest,
    ) -> core::result::Result<PagingLevel, RPCError> {
        Ok(
            // Check if the address is aligned to a level 2 page. If it is not aligned, it cannot be
            // mapped at a level larger than 1.
            if req
                .page_aligned_addr
                .is_multiple_of(page_size::<PagingConsts>(2))
            {
                2
            } else {
                1
            },
        )
    }
}
