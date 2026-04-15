#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0

"""
Load trained PyTorch Heimdall models and generate the Rust weights file
using the Jinja2 template.

The HeimdallNet architecture has three linear layers (one model per device):
    Linear(input_dim, 128) -> ReLU -> Linear(128, 16) -> ReLU -> Linear(16, 1) -> Sigmoid

PyTorch state dict keys:
    fc1.weight [128, input_dim]   fc1.bias [128]
    fc2.weight [16, 128]          fc2.bias [16]
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

import argparse
from pathlib import Path

import torch
from jinja2 import Environment, FileSystemLoader


def load_model(path: str) -> dict:
    """Load a model checkpoint and return its state dict."""
    state = torch.load(path, map_location="cpu", weights_only=False)
    return state


def print_architecture(state: dict, device_idx: int) -> None:
    """Print model architecture for sanity check."""
    print(f"  Device {device_idx}:")
    for name, tensor in state.items():
        print(f"    {name:20s}  shape={str(list(tensor.shape)):16s}  dtype={tensor.dtype}")


def tensor_to_list(tensor: torch.Tensor) -> list:
    """Convert a tensor to a nested Python list of floats."""
    return tensor.tolist()


def main():
    parser = argparse.ArgumentParser(
        description="Generate Heimdall Rust weight file from PyTorch models"
    )
    parser.add_argument(
        "--models", nargs="+", required=True,
        help="Paths to .pt model files, one per device in order",
    )
    parser.add_argument(
        "--template", required=True,
        help="Path to the Jinja2 template (.rs.j2)",
    )
    parser.add_argument(
        "--output", required=True,
        help="Path for the generated Rust file (.rs)",
    )
    args = parser.parse_args()

    # Load all models
    models = []
    for path in args.models:
        models.append(load_model(path))

    num_devices = len(models)

    # Sanity check: print architecture
    print(f"Loaded {num_devices} model(s).\n")
    print("Model architecture:")
    for i, state in enumerate(models):
        print_architecture(state, i)
    print()

    # Extract dimensions from the first model
    # fc1: Linear(input_dim, hidden1_size)
    # fc2: Linear(hidden1_size, hidden2_size)
    # fc3: Linear(hidden2_size, 1)
    input_dim = models[0]["fc1.weight"].shape[1]
    hidden1_size = models[0]["fc1.weight"].shape[0]
    hidden2_size = models[0]["fc2.weight"].shape[0]
    output_size = models[0]["fc3.weight"].shape[0]

    assert output_size == 1, f"Expected output size 1 (sigmoid), got {output_size}"

    print(f"Network: {input_dim} -> {hidden1_size} (ReLU) -> {hidden2_size} (ReLU) -> {output_size} (Sigmoid)")
    print()

    # Extract weights and biases for each device
    # PyTorch stores weights as [out_features, in_features].
    # In Rust we index as weights[input][output], so we transpose.
    fc1_weights = []
    fc1_biases = []
    fc2_weights = []
    fc2_biases = []
    fc3_weights = []
    fc3_biases = []

    for i, state in enumerate(models):
        # fc1: [hidden1_size, input_dim] -> [input_dim, hidden1_size]
        w1 = state["fc1.weight"].T
        fc1_weights.append(tensor_to_list(w1))
        fc1_biases.append(tensor_to_list(state["fc1.bias"]))

        # fc2: [hidden2_size, hidden1_size] -> [hidden1_size, hidden2_size]
        w2 = state["fc2.weight"].T
        fc2_weights.append(tensor_to_list(w2))
        fc2_biases.append(tensor_to_list(state["fc2.bias"]))

        # fc3: [1, hidden2_size] -> [hidden2_size] (squeeze since output is scalar)
        w3 = state["fc3.weight"].squeeze(0)
        fc3_weights.append(tensor_to_list(w3))
        fc3_biases.append(state["fc3.bias"].item())

    # Render template
    template_path = Path(args.template)
    env = Environment(
        loader=FileSystemLoader(str(template_path.parent)),
        keep_trailing_newline=True,
    )
    template = env.get_template(template_path.name)

    rendered = template.render(
        num_devices=num_devices,
        input_dim=input_dim,
        hidden1_size=hidden1_size,
        hidden2_size=hidden2_size,
        fc1_weights=fc1_weights,
        fc1_biases=fc1_biases,
        fc2_weights=fc2_weights,
        fc2_biases=fc2_biases,
        fc3_weights=fc3_weights,
        fc3_biases=fc3_biases,
    )

    Path(args.output).write_text(rendered)
    print(f"Generated {args.output} ({len(rendered)} bytes)")


if __name__ == "__main__":
    main()
