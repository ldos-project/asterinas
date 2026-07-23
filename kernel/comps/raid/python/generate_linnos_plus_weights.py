#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0

"""
Load trained PyTorch LinnOSPlus models and generate the Rust weights file
using the Jinja2 template.

The LinnOSPlus architecture has three linear layers:
    Linear(31, 8) -> ReLU -> Linear(8, 8) -> ReLU -> Linear(8, 2)

PyTorch state dict keys:
    net.0.weight [8, 31]   net.0.bias [8]
    net.2.weight [8, 8]    net.2.bias [8]
    net.4.weight [2, 8]    net.4.bias [2]

Usage:
    python generate_linnos_plus_weights.py \
        --models models/linnos_plus_device0.pt \
                 models/linnos_plus_device1.pt \
                 models/linnos_plus_device2.pt \
        --template kernel/comps/raid/src/linnos_plus_weights.rs.j2 \
        --output   kernel/comps/raid/src/linnos_plus_weights.rs

Run from the repository root.
"""

from weight_export import build_arg_parser, load_models, render_and_write


def main():
    args = build_arg_parser(
        "Generate LinnOSPlus Rust weight file from PyTorch models"
    ).parse_args()

    models = load_models(args.models)
    num_devices = len(models)

    # Extract dimensions from the first model.
    # net.0: Linear(31, hidden1_size)
    # net.2: Linear(hidden1_size, hidden2_size)
    # net.4: Linear(hidden2_size, 2)
    hidden1_size = models[0]["net.0.weight"].shape[0]
    input_size = models[0]["net.0.weight"].shape[1]
    hidden2_size = models[0]["net.2.weight"].shape[0]
    output_size = models[0]["net.4.weight"].shape[0]

    print(f"Network: {input_size} -> {hidden1_size} (ReLU) -> {hidden2_size} (ReLU) -> {output_size}")
    print()

    # Extract weights and biases for each device.
    # PyTorch stores weights as [out_features, in_features].
    # In Rust we index as weights[input][output], so we transpose.
    hidden1_weights = []
    hidden1_biases = []
    hidden2_weights = []
    hidden2_biases = []
    output_weights = []
    output_biases = []

    for state in models:
        # Hidden layer 1: [hidden1_size, 31] -> [31, hidden1_size]
        hidden1_weights.append(state["net.0.weight"].T.tolist())
        hidden1_biases.append(state["net.0.bias"].tolist())

        # Hidden layer 2: [hidden2_size, hidden1_size] -> [hidden1_size, hidden2_size]
        hidden2_weights.append(state["net.2.weight"].T.tolist())
        hidden2_biases.append(state["net.2.bias"].tolist())

        # Output layer: [2, hidden2_size] -> [hidden2_size, 2]
        output_weights.append(state["net.4.weight"].T.tolist())
        output_biases.append(state["net.4.bias"].tolist())

    render_and_write(
        args.template,
        args.output,
        num_devices=num_devices,
        hidden1_size=hidden1_size,
        hidden2_size=hidden2_size,
        hidden1_weights=hidden1_weights,
        hidden1_biases=hidden1_biases,
        hidden2_weights=hidden2_weights,
        hidden2_biases=hidden2_biases,
        output_weights=output_weights,
        output_biases=output_biases,
    )


if __name__ == "__main__":
    main()
