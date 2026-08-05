// SPDX-License-Identifier: MPL-2.0

// PLACEHOLDER (all-zero) LinnOSPlus neural-network weights for 3 devices.
//
// Regenerate real weights (which MUST NOT be committed) from trained PyTorch checkpoints with:
//
//   python kernel/comps/raid/python/generate_linnos_plus_weights.py \
//       --models m0.pt m1.pt m2.pt \
//       --template kernel/comps/raid/templates/linnos_plus_weights.rs.j2 \
//       --output  test/initramfs/src/raid_policies/linnos_plus/src/weights.rs
//
// (A dummy-checkpoint generator, kernel/comps/raid/python/generate_dummy_checkpoints.py, produces
// throwaway checkpoints so the pipeline can be exercised end to end.)
//
// The committed placeholders use Rust array-repeat syntax to stay tiny. With all-zero weights the
// net outputs [0.0, 0.0], so argmax never predicts "fast" and the policy falls through to its
// round-robin fallback — expected for CI, which does not need an accurate policy.
//
// Each device has:
//   - hidden layer 1: 31 x 8 matrix + 8 bias (ReLU)
//   - hidden layer 2:  8 x 8 matrix + 8 bias (ReLU)
//   - output layer:    8 x 2 matrix + 2 bias

/// Number of devices with hardcoded weights.
pub const NUM_DEVICES: usize = 3;

pub static HIDDEN1_WEIGHTS_0: [[f32; 8]; 31] = [[0.0; 8]; 31];
pub static HIDDEN1_BIAS_0: [f32; 8] = [0.0; 8];
pub static HIDDEN1_WEIGHTS_1: [[f32; 8]; 31] = [[0.0; 8]; 31];
pub static HIDDEN1_BIAS_1: [f32; 8] = [0.0; 8];
pub static HIDDEN1_WEIGHTS_2: [[f32; 8]; 31] = [[0.0; 8]; 31];
pub static HIDDEN1_BIAS_2: [f32; 8] = [0.0; 8];

pub static HIDDEN2_WEIGHTS_0: [[f32; 8]; 8] = [[0.0; 8]; 8];
pub static HIDDEN2_BIAS_0: [f32; 8] = [0.0; 8];
pub static HIDDEN2_WEIGHTS_1: [[f32; 8]; 8] = [[0.0; 8]; 8];
pub static HIDDEN2_BIAS_1: [f32; 8] = [0.0; 8];
pub static HIDDEN2_WEIGHTS_2: [[f32; 8]; 8] = [[0.0; 8]; 8];
pub static HIDDEN2_BIAS_2: [f32; 8] = [0.0; 8];

pub static OUTPUT_WEIGHTS_0: [[f32; 2]; 8] = [[0.0; 2]; 8];
pub static OUTPUT_BIAS_0: [f32; 2] = [0.0; 2];
pub static OUTPUT_WEIGHTS_1: [[f32; 2]; 8] = [[0.0; 2]; 8];
pub static OUTPUT_BIAS_1: [f32; 2] = [0.0; 2];
pub static OUTPUT_WEIGHTS_2: [[f32; 2]; 8] = [[0.0; 2]; 8];
pub static OUTPUT_BIAS_2: [f32; 2] = [0.0; 2];

/// All hidden layer 1 weights indexed by device.
pub static HIDDEN1_WEIGHTS: [&[[f32; 8]; 31]; NUM_DEVICES] =
    [&HIDDEN1_WEIGHTS_0, &HIDDEN1_WEIGHTS_1, &HIDDEN1_WEIGHTS_2];
/// All hidden layer 1 biases indexed by device.
pub static HIDDEN1_BIASES: [&[f32; 8]; NUM_DEVICES] =
    [&HIDDEN1_BIAS_0, &HIDDEN1_BIAS_1, &HIDDEN1_BIAS_2];
/// All hidden layer 2 weights indexed by device.
pub static HIDDEN2_WEIGHTS: [&[[f32; 8]; 8]; NUM_DEVICES] =
    [&HIDDEN2_WEIGHTS_0, &HIDDEN2_WEIGHTS_1, &HIDDEN2_WEIGHTS_2];
/// All hidden layer 2 biases indexed by device.
pub static HIDDEN2_BIASES: [&[f32; 8]; NUM_DEVICES] =
    [&HIDDEN2_BIAS_0, &HIDDEN2_BIAS_1, &HIDDEN2_BIAS_2];
/// All output layer weights indexed by device.
pub static OUTPUT_WEIGHTS: [&[[f32; 2]; 8]; NUM_DEVICES] =
    [&OUTPUT_WEIGHTS_0, &OUTPUT_WEIGHTS_1, &OUTPUT_WEIGHTS_2];
/// All output layer biases indexed by device.
pub static OUTPUT_BIASES: [&[f32; 2]; NUM_DEVICES] =
    [&OUTPUT_BIAS_0, &OUTPUT_BIAS_1, &OUTPUT_BIAS_2];
