// SPDX-License-Identifier: MPL-2.0

// LinnOS neural network weights hardcoded for 3 devices.
// Each device has:
//   - hidden layer: 31 x 256 matrix
//   - output layer: 256 x 2 matrix
//
// These weights will be filled in by a Python script with trained values.
// For now, all weights are initialized to 0.0 as placeholders.
//
// The actual weights numbers are expected to be filled with the jinja2
// templates by the Python scripts that trains the model.

/// Number of devices with hardcoded weights.
pub const NUM_DEVICES: usize = 3;

/// Hidden layer weights for device 0: 31 inputs -> 256 neurons
pub static HIDDEN_WEIGHTS_0: [[f32; 256]; 31] = [[0.0; 256]; 31];

/// Hidden layer weights for device 1
pub static HIDDEN_WEIGHTS_1: [[f32; 256]; 31] = [[0.0; 256]; 31];

/// Hidden layer weights for device 2
pub static HIDDEN_WEIGHTS_2: [[f32; 256]; 31] = [[0.0; 256]; 31];

/// Output layer weights for device 0: 256 neurons -> 2 classes
pub static OUTPUT_WEIGHTS_0: [[f32; 2]; 256] = [[0.0; 2]; 256];

/// Output layer weights for device 1
pub static OUTPUT_WEIGHTS_1: [[f32; 2]; 256] = [[0.0; 2]; 256];

/// Output layer weights for device 2
pub static OUTPUT_WEIGHTS_2: [[f32; 2]; 256] = [[0.0; 2]; 256];

/// All hidden layer weights indexed by device.
pub static HIDDEN_WEIGHTS: [&[[f32; 256]; 31]; NUM_DEVICES] =
    [&HIDDEN_WEIGHTS_0, &HIDDEN_WEIGHTS_1, &HIDDEN_WEIGHTS_2];

/// All output layer weights indexed by device.
pub static OUTPUT_WEIGHTS: [&[[f32; 2]; 256]; NUM_DEVICES] =
    [&OUTPUT_WEIGHTS_0, &OUTPUT_WEIGHTS_1, &OUTPUT_WEIGHTS_2];
