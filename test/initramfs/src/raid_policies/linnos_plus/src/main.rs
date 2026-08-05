// SPDX-License-Identifier: MPL-2.0

//! `raid_policy_linnos_plus`: a deeper variant of the LinnOS network.
//!
//! One model per device (31 -> 8 ReLU -> 8 ReLU -> 2). Candidate-loop control flow is identical to
//! LinnOS and the kernel `LinnOSPlusPolicy`: first candidate predicted "fast" wins, round-robin
//! fallback when all predict "slow".

mod weights;

use raid_policy_common::{SelectionPolicy, SelectionContext, SelectionRequest};

struct LinnOSPlusPolicy;

impl LinnOSPlusPolicy {
    fn predict(&self, ctx: &SelectionContext, req: &SelectionRequest, device_idx: u32) -> bool {
        let d = device_idx as usize;
        if d >= weights::NUM_DEVICES {
            return false;
        }

        let input = ctx.feature_digits_f32(req, device_idx);

        // Hidden layer 1: input (31) x (31x8) + bias (8) -> (8), ReLU.
        let h1_weights = weights::HIDDEN1_WEIGHTS[d];
        let h1_bias = weights::HIDDEN1_BIASES[d];
        let mut hidden1_out = [0.0f32; 8];
        for (j, out) in hidden1_out.iter_mut().enumerate() {
            let mut sum = h1_bias[j];
            for i in 0..31 {
                sum += input[i] * h1_weights[i][j];
            }
            *out = if sum > 0.0 { sum } else { 0.0 };
        }

        // Hidden layer 2: hidden1_out (8) x (8x8) + bias (8) -> (8), ReLU.
        let h2_weights = weights::HIDDEN2_WEIGHTS[d];
        let h2_bias = weights::HIDDEN2_BIASES[d];
        let mut hidden2_out = [0.0f32; 8];
        for (j, out) in hidden2_out.iter_mut().enumerate() {
            let mut sum = h2_bias[j];
            for i in 0..8 {
                sum += hidden1_out[i] * h2_weights[i][j];
            }
            *out = if sum > 0.0 { sum } else { 0.0 };
        }

        // Output layer: hidden2_out (8) x (8x2) + bias (2) -> (2).
        let out_weights = weights::OUTPUT_WEIGHTS[d];
        let out_bias = weights::OUTPUT_BIASES[d];
        let mut output = [out_bias[0], out_bias[1]];
        for k in 0..2 {
            for j in 0..8 {
                output[k] += hidden2_out[j] * out_weights[j][k];
            }
        }

        output[0] < output[1]
    }
}

impl SelectionPolicy for LinnOSPlusPolicy {
    const NAME: &'static str = "linnos_plus";

    fn select_block_device(&self, ctx: &SelectionContext, req: &SelectionRequest) -> u32 {
        ctx.run_candidate_loop(req, |device_idx| self.predict(ctx, req, device_idx))
    }
}

fn main() {
    raid_policy_common::run(LinnOSPlusPolicy);
}
