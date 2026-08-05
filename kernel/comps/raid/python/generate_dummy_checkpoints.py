#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0

"""Generate throwaway "dummy" checkpoints so the weight-generation pipeline can be
exercised end to end WITHOUT real trained models.

This does NOT produce meaningful weights; it produces checkpoints of the right
*shape* so that ``generate_linnos_weights.py``, ``generate_linnos_plus_weights.py``
and ``generate_decision_tree.py`` run to completion and emit Rust that compiles.
Regenerating from these dummies still yields non-committable output (see the
placeholder headers), so the committed placeholders remain all-zero.

Usage (run from the repository root)::

    python kernel/comps/raid/python/generate_dummy_checkpoints.py --out /tmp/dummy_ckpts

produces, for ``--devices 3`` (the default)::

    /tmp/dummy_ckpts/linnos_device{0,1,2}.pt
    /tmp/dummy_ckpts/linnos_plus_device{0,1,2}.pt
    /tmp/dummy_ckpts/dt_device{0,1,2}.pkl

which are the ``--models`` / ``--model`` inputs to the generator scripts.
"""

import argparse
import pickle
from pathlib import Path

import numpy as np
import torch
from sklearn.tree import DecisionTreeClassifier

# The feature vector is always 31 elements (see the kernel selection policies).
INPUT_SIZE = 31


def linnos_state_dict() -> dict:
    """A LinnOS state dict: Linear(31, 256) -> ReLU -> Linear(256, 2)."""
    return {
        "net.0.weight": torch.zeros(256, INPUT_SIZE),
        "net.0.bias": torch.zeros(256),
        "net.2.weight": torch.zeros(2, 256),
        "net.2.bias": torch.zeros(2),
    }


def linnos_plus_state_dict() -> dict:
    """A LinnOSPlus state dict: Linear(31, 8) -> ReLU -> Linear(8, 8) -> ReLU -> Linear(8, 2)."""
    return {
        "net.0.weight": torch.zeros(8, INPUT_SIZE),
        "net.0.bias": torch.zeros(8),
        "net.2.weight": torch.zeros(8, 8),
        "net.2.bias": torch.zeros(8),
        "net.4.weight": torch.zeros(2, 8),
        "net.4.bias": torch.zeros(2),
    }


def dummy_decision_tree() -> DecisionTreeClassifier:
    """A DecisionTreeClassifier fit on a tiny synthetic dataset (both classes present)."""
    x = np.zeros((4, INPUT_SIZE), dtype=np.float64)
    x[2:] = 1.0
    y = np.array([0, 0, 1, 1])
    return DecisionTreeClassifier(max_depth=2).fit(x, y)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--out",
        required=True,
        help="Output directory for the dummy checkpoints",
    )
    parser.add_argument(
        "--devices",
        type=int,
        default=3,
        help="Number of per-device checkpoints to emit (default: 3)",
    )
    args = parser.parse_args()

    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)

    for device in range(args.devices):
        linnos_path = out_dir / f"linnos_device{device}.pt"
        torch.save(linnos_state_dict(), linnos_path)

        linnos_plus_path = out_dir / f"linnos_plus_device{device}.pt"
        torch.save(linnos_plus_state_dict(), linnos_plus_path)

        dt_path = out_dir / f"dt_device{device}.pkl"
        with open(dt_path, "wb") as f:
            pickle.dump(dummy_decision_tree(), f)

    print(f"Wrote dummy checkpoints for {args.devices} device(s) to {out_dir}")


if __name__ == "__main__":
    main()
