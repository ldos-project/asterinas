// SPDX-License-Identifier: MPL-2.0

// PLACEHOLDER (all-zero) LinnOS neural-network weights for 3 devices.
//
// Regenerate real weights (which MUST NOT be committed) from trained PyTorch checkpoints with:
//
//   python kernel/comps/raid/python/generate_linnos_weights.py \
//       --models m0.pt m1.pt m2.pt \
//       --template kernel/comps/raid/templates/linnos_weights.rs.j2 \
//       --output  test/initramfs/src/raid_policies/linnos/src/weights.rs
//
// (A dummy-checkpoint generator, kernel/comps/raid/python/generate_dummy_checkpoints.py, produces
// throwaway checkpoints so the pipeline can be exercised end to end.)
//
// The committed placeholders use Rust array-repeat syntax to stay tiny. With all-zero weights the
// net outputs [0.0, 0.0], so argmax never predicts "fast" and the policy falls through to its
// round-robin fallback — expected for CI, which does not need an accurate policy.
//
// Each device has:
//   - hidden layer: 31 x 256 matrix + 256 bias (ReLU)
//   - output layer: 256 x 2 matrix + 2 bias

/// Number of devices with hardcoded weights.
pub const NUM_DEVICES: usize = 3;

pub static HIDDEN_WEIGHTS_0: [[f32; 256]; 31] = [[0.0; 256]; 31];
pub static HIDDEN_BIAS_0: [f32; 256] = [0.0; 256];
pub static HIDDEN_WEIGHTS_1: [[f32; 256]; 31] = [[0.0; 256]; 31];
pub static HIDDEN_BIAS_1: [f32; 256] = [0.0; 256];
pub static HIDDEN_WEIGHTS_2: [[f32; 256]; 31] = [[0.0; 256]; 31];
pub static HIDDEN_BIAS_2: [f32; 256] = [0.0; 256];

pub static OUTPUT_WEIGHTS_0: [[f32; 2]; 256] = [[0.0; 2]; 256];
pub static OUTPUT_BIAS_0: [f32; 2] = [0.0; 2];
pub static OUTPUT_WEIGHTS_1: [[f32; 2]; 256] = [[0.0; 2]; 256];
pub static OUTPUT_BIAS_1: [f32; 2] = [0.0; 2];
pub static OUTPUT_WEIGHTS_2: [[f32; 2]; 256] = [[0.0; 2]; 256];
pub static OUTPUT_BIAS_2: [f32; 2] = [0.0; 2];

/// All hidden layer weights indexed by device.
pub static HIDDEN_WEIGHTS: [&[[f32; 256]; 31]; NUM_DEVICES] =
    [&HIDDEN_WEIGHTS_0, &HIDDEN_WEIGHTS_1, &HIDDEN_WEIGHTS_2];
/// All hidden layer biases indexed by device.
pub static HIDDEN_BIASES: [&[f32; 256]; NUM_DEVICES] =
    [&HIDDEN_BIAS_0, &HIDDEN_BIAS_1, &HIDDEN_BIAS_2];
/// All output layer weights indexed by device.
pub static OUTPUT_WEIGHTS: [&[[f32; 2]; 256]; NUM_DEVICES] =
    [&OUTPUT_WEIGHTS_0, &OUTPUT_WEIGHTS_1, &OUTPUT_WEIGHTS_2];
/// All output layer biases indexed by device.
pub static OUTPUT_BIASES: [&[f32; 2]; NUM_DEVICES] =
    [&OUTPUT_BIAS_0, &OUTPUT_BIAS_1, &OUTPUT_BIAS_2];
