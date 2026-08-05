// SPDX-License-Identifier: MPL-2.0

//! `raid_policy_lowest_index`: always select_block_device the smallest admitted candidate index.
//!
//! This deliberately-trivial policy exists to prove the extensibility bar: it was added as a brand
//! new crate with NO edit to `common`, to the supervisor, or to any other policy crate — only a new
//! directory, one line in the workspace `members`, and one line in the nix install list.

use raid_policy_common::{SelectionPolicy, SelectionContext, SelectionRequest};

struct LowestIndexPolicy;

impl SelectionPolicy for LowestIndexPolicy {
    const NAME: &'static str = "lowest_index";

    fn select_block_device(&self, _ctx: &SelectionContext, req: &SelectionRequest) -> u32 {
        // `candidates` is never empty (guaranteed by the kernel), so `min` always yields a value.
        *req.candidates.iter().min().expect("candidates is never empty")
    }
}

fn main() {
    raid_policy_common::run(LowestIndexPolicy);
}
