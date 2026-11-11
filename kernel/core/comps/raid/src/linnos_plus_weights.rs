// SPDX-License-Identifier: MPL-2.0

// LinnOSPlus neural network weights hardcoded for 3 devices.
// Each device has:
//   - hidden layer 1: 31 x 8 matrix + 8 bias (ReLU)
//   - hidden layer 2: 8 x 8 matrix + 8 bias (ReLU)
//   - output layer: 8 x 2 matrix + 2 bias
//
// PLACEHOLDER (all-zero) weights. Regenerate with generate_linnos_plus_weights.py
// from trained models for meaningful predictions.

/// Number of devices with hardcoded weights.
pub const NUM_DEVICES: usize = 3;

/// Hidden layer 1 size (number of neurons).
pub const HIDDEN1_SIZE: usize = 8;

/// Hidden layer 2 size (number of neurons).
pub const HIDDEN2_SIZE: usize = 8;

/// Hidden layer 1 weights for device 0: 31 inputs -> 8 neurons
pub static HIDDEN1_WEIGHTS_0: [[f32; 8]; 31] = [[0.0; 8]; 31];
/// Hidden layer 1 bias for device 0
pub static HIDDEN1_BIAS_0: [f32; 8] = [0.0; 8];
/// Hidden layer 1 weights for device 1: 31 inputs -> 8 neurons
pub static HIDDEN1_WEIGHTS_1: [[f32; 8]; 31] = [[0.0; 8]; 31];
/// Hidden layer 1 bias for device 1
pub static HIDDEN1_BIAS_1: [f32; 8] = [0.0; 8];
/// Hidden layer 1 weights for device 2: 31 inputs -> 8 neurons
pub static HIDDEN1_WEIGHTS_2: [[f32; 8]; 31] = [[0.0; 8]; 31];
/// Hidden layer 1 bias for device 2
pub static HIDDEN1_BIAS_2: [f32; 8] = [0.0; 8];

/// Hidden layer 2 weights for device 0: 8 -> 8 neurons
pub static HIDDEN2_WEIGHTS_0: [[f32; 8]; 8] = [[0.0; 8]; 8];
/// Hidden layer 2 bias for device 0
pub static HIDDEN2_BIAS_0: [f32; 8] = [0.0; 8];
/// Hidden layer 2 weights for device 1: 8 -> 8 neurons
pub static HIDDEN2_WEIGHTS_1: [[f32; 8]; 8] = [[0.0; 8]; 8];
/// Hidden layer 2 bias for device 1
pub static HIDDEN2_BIAS_1: [f32; 8] = [0.0; 8];
/// Hidden layer 2 weights for device 2: 8 -> 8 neurons
pub static HIDDEN2_WEIGHTS_2: [[f32; 8]; 8] = [[0.0; 8]; 8];
/// Hidden layer 2 bias for device 2
pub static HIDDEN2_BIAS_2: [f32; 8] = [0.0; 8];

/// Output layer weights for device 0: 8 neurons -> 2 classes
pub static OUTPUT_WEIGHTS_0: [[f32; 2]; 8] = [[0.0; 2]; 8];
/// Output layer bias for device 0
pub static OUTPUT_BIAS_0: [f32; 2] = [0.0; 2];
/// Output layer weights for device 1: 8 neurons -> 2 classes
pub static OUTPUT_WEIGHTS_1: [[f32; 2]; 8] = [[0.0; 2]; 8];
/// Output layer bias for device 1
pub static OUTPUT_BIAS_1: [f32; 2] = [0.0; 2];
/// Output layer weights for device 2: 8 neurons -> 2 classes
pub static OUTPUT_WEIGHTS_2: [[f32; 2]; 8] = [[0.0; 2]; 8];
/// Output layer bias for device 2
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
