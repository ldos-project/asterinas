#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0

"""
Load trained PyTorch Heimdall models and generate the Rust weights file
using the Jinja2 template.

The HeimdallNet architecture has two linear layers (one model per device):
    Linear(input_dim, 16) -> ReLU -> Linear(16, 1) -> Sigmoid

PyTorch state dict keys:
    fc1.weight [16, input_dim]    fc1.bias [16]
    fc3.weight [1, 16]            fc3.bias [1]

Usage:
    python generate_heimdall_weights.py \\
        --models models/heimdall_device0.pt \\
                 models/heimdall_device1.pt \\
                 models/heimdall_device2.pt \\
        --template kernel/comps/raid/src/heimdall_weights.rs.j2 \\
        --output   kernel/comps/raid/src/heimdall_weights.rs

Run from the repository root.
"""

from weight_export import build_arg_parser, load_models, render_and_write


def main():
    args = build_arg_parser(
        "Generate Heimdall Rust weight file from PyTorch models"
    ).parse_args()

    models = load_models(args.models)
    num_devices = len(models)

    # Extract dimensions from the first model.
    # fc1: Linear(input_dim, hidden_size)
    # fc3: Linear(hidden_size, 1)
    input_dim = models[0]["fc1.weight"].shape[1]
    hidden_size = models[0]["fc1.weight"].shape[0]
    output_size = models[0]["fc3.weight"].shape[0]

    assert output_size == 1, f"Expected output size 1 (sigmoid), got {output_size}"

    print(f"Network: {input_dim} -> {hidden_size} (ReLU) -> {output_size} (Sigmoid)")
    print()

    # Extract weights and biases for each device.
    # PyTorch stores weights as [out_features, in_features].
    # In Rust we index as weights[input][output], so we transpose.
    fc1_weights = []
    fc1_biases = []
    fc3_weights = []
    fc3_biases = []

    for state in models:
        # fc1: [hidden_size, input_dim] -> [input_dim, hidden_size]
        fc1_weights.append(state["fc1.weight"].T.tolist())
        fc1_biases.append(state["fc1.bias"].tolist())

        # fc3: [1, hidden_size] -> [hidden_size] (squeeze since output is scalar)
        fc3_weights.append(state["fc3.weight"].squeeze(0).tolist())
        fc3_biases.append(state["fc3.bias"].item())

    render_and_write(
        args.template,
        args.output,
        num_devices=num_devices,
        input_dim=input_dim,
        hidden_size=hidden_size,
        fc1_weights=fc1_weights,
        fc1_biases=fc1_biases,
        fc3_weights=fc3_weights,
        fc3_biases=fc3_biases,
    )


if __name__ == "__main__":
    main()
