// SPDX-License-Identifier: MPL-2.0

//! `raid_policy_roundrobin`: cycle through the admitted candidates, mirroring the kernel's
//! `RoundRobinPolicy`.

use std::sync::atomic::Ordering;

use raid_policy_common::{SelectionPolicy, SelectionContext, SelectionRequest};

struct RoundRobinPolicy;

impl SelectionPolicy for RoundRobinPolicy {
    const NAME: &'static str = "roundrobin";

    fn select_block_device(&self, ctx: &SelectionContext, req: &SelectionRequest) -> u32 {
        let idx = ctx.read_cursor().fetch_add(1, Ordering::Relaxed);
        req.candidates[idx % req.candidates.len()]
    }
}

fn main() {
    raid_policy_common::run(RoundRobinPolicy);
}
