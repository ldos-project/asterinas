// SPDX-License-Identifier: MPL-2.0

//! `raid_policy_decision_tree`: per-device binary decision trees over the same 31-element LinnOS
//! feature vector (as `u8` digits). Candidate-loop control flow mirrors the kernel
//! `DecisionTreePolicy`: first candidate whose tree predicts "fast" wins, round-robin fallback when
//! all predict "slow".

mod predictions;

use raid_policy_common::{SelectionPolicy, SelectionContext, SelectionRequest};

struct DecisionTreePolicy;

impl DecisionTreePolicy {
    fn predict(&self, ctx: &SelectionContext, req: &SelectionRequest, device_idx: u32) -> bool {
        let input = ctx.feature_digits_u8(req, device_idx);
        let prediction = match device_idx {
            0 => predictions::predict_device0(&input),
            1 => predictions::predict_device1(&input),
            2 => predictions::predict_device2(&input),
            // Unknown device: predict fast, matching the kernel `DecisionTreePolicy`.
            _ => 1,
        };
        prediction == 1
    }
}

impl SelectionPolicy for DecisionTreePolicy {
    const NAME: &'static str = "decision_tree";

    fn select_block_device(&self, ctx: &SelectionContext, req: &SelectionRequest) -> u32 {
        ctx.run_candidate_loop(req, |device_idx| self.predict(ctx, req, device_idx))
    }
}

fn main() {
    raid_policy_common::run(DecisionTreePolicy);
}
