#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0

"""Shared helpers for the RAID weight-generation scripts.

Each ``generate_*_weights.py`` script loads per-device PyTorch checkpoints and
renders a Rust weights file from a Jinja2 template. The model-independent
plumbing — argument parsing, checkpoint loading, architecture printing, and
template rendering — lives here so the per-model scripts only contain the
weight-extraction logic specific to their architecture.
"""

import argparse
from pathlib import Path

import torch
from jinja2 import Environment, FileSystemLoader


def build_arg_parser(description: str) -> argparse.ArgumentParser:
    """Build an argument parser with the arguments common to all generators."""
    parser = argparse.ArgumentParser(description=description)
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
    return parser


def load_model(path: str) -> dict:
    """Load a model checkpoint and return its state dict."""
    return torch.load(path, map_location="cpu", weights_only=False)


def print_architecture(state: dict, device_idx: int) -> None:
    """Print model architecture for sanity check."""
    print(f"  Device {device_idx}:")
    for name, tensor in state.items():
        print(f"    {name:20s}  shape={str(list(tensor.shape)):16s}  dtype={tensor.dtype}")


def load_models(paths: list) -> list:
    """Load all checkpoints, printing a per-device architecture summary."""
    models = [load_model(path) for path in paths]
    print(f"Loaded {len(models)} model(s).\n")
    print("Model architecture:")
    for device_idx, state in enumerate(models):
        print_architecture(state, device_idx)
    print()
    return models


def render_and_write(template: str, output: str, **context) -> None:
    """Render the Jinja2 template with ``context`` and write it to ``output``."""
    template_path = Path(template)
    env = Environment(
        loader=FileSystemLoader(str(template_path.parent)),
        keep_trailing_newline=True,
    )
    rendered = env.get_template(template_path.name).render(**context)
    Path(output).write_text(rendered)
    print(f"Generated {output} ({len(rendered)} bytes)")
