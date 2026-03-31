#!/usr/bin/env python3
"""Generate linnos_weights.rs from trained model weights using Jinja2.

Usage:
    python generate_weights.py

Modify load_weights() to load your actual trained model parameters.
"""

from pathlib import Path
from jinja2 import Environment, FileSystemLoader

SCRIPT_DIR = Path(__file__).parent
TEMPLATE_DIR = SCRIPT_DIR / "src"
OUTPUT_PATH = TEMPLATE_DIR / "linnos_weights.rs"

NUM_DEVICES = 3
HIDDEN_INPUT = 31
HIDDEN_NEURONS = 256
OUTPUT_CLASSES = 2


def load_weights():
    """Load trained weights. Replace this with your actual model loading code.

    Returns:
        hidden_weights: list of shape [num_devices][31][256]
        output_weights: list of shape [num_devices][256][2]
    """
    # Example: load from a PyTorch model
    # import torch
    # model = torch.load("linnos_model.pt")
    # hidden_weights = [model[f"device_{i}.hidden.weight"].T.tolist() for i in range(NUM_DEVICES)]
    # output_weights = [model[f"device_{i}.output.weight"].T.tolist() for i in range(NUM_DEVICES)]

    # Placeholder: all zeros
    hidden_weights = [
        [[0.0] * HIDDEN_NEURONS for _ in range(HIDDEN_INPUT)]
        for _ in range(NUM_DEVICES)
    ]
    output_weights = [
        [[0.0] * OUTPUT_CLASSES for _ in range(HIDDEN_NEURONS)]
        for _ in range(NUM_DEVICES)
    ]
    return hidden_weights, output_weights


def main():
    hidden_weights, output_weights = load_weights()

    env = Environment(
        loader=FileSystemLoader(TEMPLATE_DIR),
        keep_trailing_newline=True,
    )
    template = env.get_template("linnos_weights.rs.j2")

    rendered = template.render(
        num_devices=NUM_DEVICES,
        hidden_weights=hidden_weights,
        output_weights=output_weights,
    )

    with open(OUTPUT_PATH, "w") as f:
        f.write(rendered)

    print(f"Generated {OUTPUT_PATH}")


if __name__ == "__main__":
    main()
