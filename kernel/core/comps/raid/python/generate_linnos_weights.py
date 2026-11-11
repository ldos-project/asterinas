#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0

"""
Load trained PyTorch LinnOS models and generate the Rust weights file
using the Jinja2 template.

Usage:
    python generate_weights.py \
        --models models/model_device0_lr0.001_bs32768_ep20.pt \
                 models/model_device1_lr0.001_bs32768_ep20.pt \
                 models/model_device2_lr0.001_bs32768_ep20.pt \
        --template kernel/comps/raid/src/linnos_weights.rs.j2 \
        --output   kernel/comps/raid/src/linnos_weights.rs

Run from the repository root.
"""

from weight_export import build_arg_parser, load_models, render_and_write


def main():
    args = build_arg_parser(
        "Generate LinnOS Rust weight file from PyTorch models"
    ).parse_args()

    models = load_models(args.models)
    num_devices = len(models)

    # Extract dimensions from the first model
    hidden_weight_shape = models[0]["net.0.weight"].shape  # [hidden_size, 31]
    hidden_size = hidden_weight_shape[0]
    input_size = hidden_weight_shape[1]
    output_size = models[0]["net.2.weight"].shape[0]  # 2

    print(f"Network: {input_size} -> {hidden_size} (ReLU) -> {output_size}")
    print()

    # Extract weights and biases for each device.
    # Hidden weights: net.0.weight has shape [hidden_size, input_size].
    # In the Rust code, we index as hidden_weights[input][hidden],
    # so we transpose: [input_size, hidden_size] = [31][hidden_size].
    hidden_weights = []
    hidden_biases = []
    output_weights = []
    output_biases = []

    for state in models:
        # Transpose: [hidden_size, 31] -> [31, hidden_size]
        hidden_weights.append(state["net.0.weight"].T.tolist())
        hidden_biases.append(state["net.0.bias"].tolist())

        # Transpose: [2, hidden_size] -> [hidden_size, 2]
        output_weights.append(state["net.2.weight"].T.tolist())
        output_biases.append(state["net.2.bias"].tolist())

    render_and_write(
        args.template,
        args.output,
        num_devices=num_devices,
        hidden_size=hidden_size,
        hidden_weights=hidden_weights,
        hidden_biases=hidden_biases,
        output_weights=output_weights,
        output_biases=output_biases,
    )


if __name__ == "__main__":
    main()
