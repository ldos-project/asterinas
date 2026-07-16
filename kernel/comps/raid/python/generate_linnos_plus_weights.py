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
        description="Generate LinnOSPlus Rust weight file from PyTorch models"
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
    # net.0: Linear(31, hidden1_size)
    # net.2: Linear(hidden1_size, hidden2_size)
    # net.4: Linear(hidden2_size, 2)
    hidden1_size = models[0]["net.0.weight"].shape[0]
    input_size = models[0]["net.0.weight"].shape[1]
    hidden2_size = models[0]["net.2.weight"].shape[0]
    output_size = models[0]["net.4.weight"].shape[0]

    print(f"Network: {input_size} -> {hidden1_size} (ReLU) -> {hidden2_size} (ReLU) -> {output_size}")
    print()

    # Extract weights and biases for each device
    # PyTorch stores weights as [out_features, in_features].
    # In Rust we index as weights[input][output], so we transpose.
    hidden1_weights = []
    hidden1_biases = []
    hidden2_weights = []
    hidden2_biases = []
    output_weights = []
    output_biases = []

    for i, state in enumerate(models):
        # Hidden layer 1: [hidden1_size, 31] -> [31, hidden1_size]
        hw1 = state["net.0.weight"].T
        hidden1_weights.append(tensor_to_list(hw1))
        hidden1_biases.append(tensor_to_list(state["net.0.bias"]))

        # Hidden layer 2: [hidden2_size, hidden1_size] -> [hidden1_size, hidden2_size]
        hw2 = state["net.2.weight"].T
        hidden2_weights.append(tensor_to_list(hw2))
        hidden2_biases.append(tensor_to_list(state["net.2.bias"]))

        # Output layer: [2, hidden2_size] -> [hidden2_size, 2]
        ow = state["net.4.weight"].T
        output_weights.append(tensor_to_list(ow))
        output_biases.append(tensor_to_list(state["net.4.bias"]))

    # Render template
    template_path = Path(args.template)
    env = Environment(
        loader=FileSystemLoader(str(template_path.parent)),
        keep_trailing_newline=True,
    )
    template = env.get_template(template_path.name)

    rendered = template.render(
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

    Path(args.output).write_text(rendered)
    print(f"Generated {args.output} ({len(rendered)} bytes)")


if __name__ == "__main__":
    main()
