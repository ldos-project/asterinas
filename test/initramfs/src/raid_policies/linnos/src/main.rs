// SPDX-License-Identifier: MPL-2.0

//! `raid_policy_linnos`: the LinnOS neural-network read-selection policy.
//!
//! One model per device (31 -> 256 ReLU -> 2). For each admitted candidate in round-robin order,
//! run its model on the 31-element feature vector; the first candidate predicted "fast" wins. If
//! every candidate predicts "slow", fall back to round-robin among them. This mirrors the kernel
//! `LinnOSPolicy` exactly (only the FPU-preemption guard is kernel-specific and unneeded here).

mod weights;

use raid_policy_common::{SelectionPolicy, SelectionContext, SelectionRequest};

struct LinnOSPolicy;

impl LinnOSPolicy {
    /// Runs device `device_idx`'s model on its feature vector and returns whether it predicts fast.
    fn predict(&self, ctx: &SelectionContext, req: &SelectionRequest, device_idx: u32) -> bool {
        let d = device_idx as usize;
        // A candidate is always a valid member index (< members.len() <= NUM_DEVICES); guard anyway
        // so an out-of-range index degrades to "slow" (round-robin fallback) rather than panicking.
        if d >= weights::NUM_DEVICES {
            return false;
        }

        let input = ctx.feature_digits_f32(req, device_idx);

        // Hidden layer: input (31) x hidden_weights (31x256) + bias (256) -> hidden_out (256), ReLU.
        let hidden_weights = weights::HIDDEN_WEIGHTS[d];
        let hidden_bias = weights::HIDDEN_BIASES[d];
        let mut hidden_out = [0.0f32; 256];
        for (j, out) in hidden_out.iter_mut().enumerate() {
            let mut sum = hidden_bias[j];
            for i in 0..31 {
                sum += input[i] * hidden_weights[i][j];
            }
            *out = if sum > 0.0 { sum } else { 0.0 };
        }

        // Output layer: hidden_out (256) x output_weights (256x2) + bias (2) -> output (2).
        let output_weights = weights::OUTPUT_WEIGHTS[d];
        let output_bias = weights::OUTPUT_BIASES[d];
        let mut output = [output_bias[0], output_bias[1]];
        for k in 0..2 {
            for j in 0..256 {
                output[k] += hidden_out[j] * output_weights[j][k];
            }
        }

        // Argmax: output[0] < output[1] means fast.
        output[0] < output[1]
    }
}

impl SelectionPolicy for LinnOSPolicy {
    const NAME: &'static str = "linnos";

    fn select_block_device(&self, ctx: &SelectionContext, req: &SelectionRequest) -> u32 {
        ctx.run_candidate_loop(req, |device_idx| self.predict(ctx, req, device_idx))
    }
}

fn main() {
    raid_policy_common::run(LinnOSPolicy);
}
