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
    parser = argparse.ArgumentParser(description="Generate LinnOS Rust weight file from PyTorch models")
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
    hidden_weight_shape = models[0]["net.0.weight"].shape  # [hidden_size, 31]
    hidden_size = hidden_weight_shape[0]
    input_size = hidden_weight_shape[1]
    output_size = models[0]["net.2.weight"].shape[0]  # 2

    print(f"Network: {input_size} -> {hidden_size} (ReLU) -> {output_size}")
    print()

    # Extract weights and biases for each device
    # Hidden weights: net.0.weight has shape [hidden_size, input_size].
    # In the Rust code, we index as hidden_weights[input][hidden],
    # so we need to transpose: [input_size, hidden_size] = [31][hidden_size].
    hidden_weights = []
    hidden_biases = []
    output_weights = []
    output_biases = []

    for i, state in enumerate(models):
        # Transpose: [hidden_size, 31] -> [31, hidden_size]
        hw = state["net.0.weight"].T  # [31, hidden_size]
        hidden_weights.append(tensor_to_list(hw))
        hidden_biases.append(tensor_to_list(state["net.0.bias"]))

        # Transpose: [2, hidden_size] -> [hidden_size, 2]
        ow = state["net.2.weight"].T  # [hidden_size, 2]
        output_weights.append(tensor_to_list(ow))
        output_biases.append(tensor_to_list(state["net.2.bias"]))

    # Render template
    template_path = Path(args.template)
    env = Environment(
        loader=FileSystemLoader(str(template_path.parent)),
        keep_trailing_newline=True,
    )
    template = env.get_template(template_path.name)

    rendered = template.render(
        num_devices=num_devices,
        hidden_size=hidden_size,
        hidden_weights=hidden_weights,
        hidden_biases=hidden_biases,
        output_weights=output_weights,
        output_biases=output_biases,
    )

    Path(args.output).write_text(rendered)
    print(f"Generated {args.output} ({len(rendered)} bytes)")


if __name__ == "__main__":
    main()
