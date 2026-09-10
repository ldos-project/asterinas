// SPDX-License-Identifier: MPL-2.0

use crate::InKernelFpuSection;

pub fn linnos(
    _section: &InKernelFpuSection,
    input: &[f32; 31],
    hidden_weights: &[[f32; 256]; 31],
    hidden_bias: &[f32; 256],
    output_weights: &[[f32; 2]; 256],
    output_bias: &[f32; 2],
) -> bool {
    let mut hidden_out = [0.0f32; 256];
    for j in 0..256 {
        let mut sum = hidden_bias[j];
        for i in 0..31 {
            sum += input[i] * hidden_weights[i][j];
        }
        hidden_out[j] = sum.max(0.0);
    }

    let mut output = [output_bias[0], output_bias[1]];
    for k in 0..2 {
        for j in 0..256 {
            output[k] += hidden_out[j] * output_weights[j][k];
        }
    }
    output[0] < output[1]
}

pub fn linnos_plus(
    _section: &InKernelFpuSection,
    input: &[f32; 31],
    hidden1_weights: &[[f32; 8]; 31],
    hidden1_bias: &[f32; 8],
    hidden2_weights: &[[f32; 8]; 8],
    hidden2_bias: &[f32; 8],
    output: (&[[f32; 2]; 8], &[f32; 2]),
) -> bool {
    let (output_weights, output_bias) = output;
    let mut hidden1_out = [0.0f32; 8];
    for j in 0..8 {
        let mut sum = hidden1_bias[j];
        for i in 0..31 {
            sum += input[i] * hidden1_weights[i][j];
        }
        hidden1_out[j] = sum.max(0.0);
    }

    let mut hidden2_out = [0.0f32; 8];
    for j in 0..8 {
        let mut sum = hidden2_bias[j];
        for i in 0..8 {
            sum += hidden1_out[i] * hidden2_weights[i][j];
        }
        hidden2_out[j] = sum.max(0.0);
    }

    let mut output = [output_bias[0], output_bias[1]];
    for k in 0..2 {
        for j in 0..8 {
            output[k] += hidden2_out[j] * output_weights[j][k];
        }
    }
    output[0] < output[1]
}

pub fn heimdall(
    _section: &InKernelFpuSection,
    input: &[f32; 11],
    fc1_weights: &[[f32; 16]; 11],
    fc1_bias: &[f32; 16],
    fc3_weights: &[f32; 16],
    fc3_bias: f32,
) -> bool {
    let mut hidden = [0.0f32; 16];
    for j in 0..16 {
        let mut sum = fc1_bias[j];
        for i in 0..11 {
            sum += input[i] * fc1_weights[i][j];
        }
        hidden[j] = sum.max(0.0);
    }

    let mut logit = fc3_bias;
    for j in 0..16 {
        logit += hidden[j] * fc3_weights[j];
    }
    logit >= 0.0
}
