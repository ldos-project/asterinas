// SPDX-License-Identifier: MPL-2.0

// Heimdall neural network weights hardcoded for 3 devices.
// Each device has:
//   - fc1: 11 x 16 matrix + 16 bias (ReLU)
//   - fc3: 16 x 1 matrix + 1 bias (Sigmoid)
//
// PLACEHOLDER (all-zero) weights. Regenerate with generate_heimdall_weights.py
// from trained models for meaningful predictions.

/// Number of devices with hardcoded weights.
pub const NUM_DEVICES: usize = 3;

/// Input dimension.
pub const INPUT_DIM: usize = 11;

/// Hidden layer size.
pub const HIDDEN_SIZE: usize = 16;

/// fc1 weights for device 0: 11 inputs -> 16 neurons
pub static FC1_WEIGHTS_0: [[f32; 16]; 11] = [[0.0; 16]; 11];
/// fc1 bias for device 0
pub static FC1_BIAS_0: [f32; 16] = [0.0; 16];
/// fc1 weights for device 1: 11 inputs -> 16 neurons
pub static FC1_WEIGHTS_1: [[f32; 16]; 11] = [[0.0; 16]; 11];
/// fc1 bias for device 1
pub static FC1_BIAS_1: [f32; 16] = [0.0; 16];
/// fc1 weights for device 2: 11 inputs -> 16 neurons
pub static FC1_WEIGHTS_2: [[f32; 16]; 11] = [[0.0; 16]; 11];
/// fc1 bias for device 2
pub static FC1_BIAS_2: [f32; 16] = [0.0; 16];

/// fc3 weights for device 0: 16 -> 1 output
pub static FC3_WEIGHTS_0: [f32; 16] = [0.0; 16];
/// fc3 bias for device 0
pub static FC3_BIAS_0: f32 = 0.0;
/// fc3 weights for device 1: 16 -> 1 output
pub static FC3_WEIGHTS_1: [f32; 16] = [0.0; 16];
/// fc3 bias for device 1
pub static FC3_BIAS_1: f32 = 0.0;
/// fc3 weights for device 2: 16 -> 1 output
pub static FC3_WEIGHTS_2: [f32; 16] = [0.0; 16];
/// fc3 bias for device 2
pub static FC3_BIAS_2: f32 = 0.0;

/// All fc1 weights indexed by device.
pub static FC1_WEIGHTS: [&[[f32; 16]; 11]; NUM_DEVICES] =
    [&FC1_WEIGHTS_0, &FC1_WEIGHTS_1, &FC1_WEIGHTS_2];

/// All fc1 biases indexed by device.
pub static FC1_BIASES: [&[f32; 16]; NUM_DEVICES] = [&FC1_BIAS_0, &FC1_BIAS_1, &FC1_BIAS_2];

/// All fc3 weights indexed by device.
pub static FC3_WEIGHTS: [&[f32; 16]; NUM_DEVICES] =
    [&FC3_WEIGHTS_0, &FC3_WEIGHTS_1, &FC3_WEIGHTS_2];

/// All fc3 biases indexed by device.
pub static FC3_BIASES: [f32; NUM_DEVICES] = [FC3_BIAS_0, FC3_BIAS_1, FC3_BIAS_2];
