#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0

"""
Decode a Mariposa data-capture device image and write one JSONL file per
capture file in the image.

Usage:
    dump_capture.py <device_file> [--output-dir DIR] [--preview N]

Each captured file produces a JSONL output file named:
    <path>.jsonl
where <path> has path-separator characters replaced with underscores.

With --preview, the first N records of every captured file are printed to
stdout instead of writing any files.
"""

import argparse
import json
import re
import sys
from pathlib import Path

import cbor2

from mariposa_data_reader import DataCaptureDevice


class _CborEncoder(json.JSONEncoder):
    """Extend the default JSON encoder to handle types cbor2 may return."""

    def default(self, o):
        if isinstance(o, bytes):
            return {"$bytes": o.hex()}
        if isinstance(o, set) or isinstance(o, frozenset):
            return list(o)
        if isinstance(o, cbor2.CBORTag):
            return o.value
        return super().default(o)


def _to_json(record) -> str:
    return json.dumps(record, cls=_CborEncoder)


def _output_name(capture_path: str) -> str:
    """Build a safe filename from the capture file's path."""
    safe_path = re.sub(r"[^A-Za-z0-9_\-]", "_", capture_path)
    return f"{safe_path}.jsonl"


def _preview(device: DataCaptureDevice, n: int):
    for capture_file in device:
        path = capture_file.path
        try:
            type_name = capture_file.type_name
        except Exception as exc:
            print(f"[{path}] warning: could not read header: {exc}", file=sys.stderr)
            type_name = "<unknown>"

        print(f"=== {path} (type: {type_name})")
        for i, record in enumerate(capture_file):
            if i >= n:
                print(f"  ... (showing first {n} records)")
                break
            print(f"  {_to_json(record)}")
        print()


def _dump(device: DataCaptureDevice, output_dir: Path):
    for capture_file in device:
        path = capture_file.path
        out_path = output_dir / _output_name(path)
        count = 0
        with open(out_path, "w") as out:
            for record in capture_file:
                out.write(_to_json(record))
                out.write("\n")
                count += 1
        print(
            f"Wrote {count} records (of type {capture_file.type_name}) to {out_path}."
        )


def main():
    parser = argparse.ArgumentParser(
        description="Decode a Mariposa data-capture device image into JSONL."
    )
    parser.add_argument("device", help="Path to the device image file.")
    parser.add_argument(
        "--output-dir",
        default=".",
        metavar="DIR",
        help="Directory to write JSONL files into (default: current directory).",
    )
    parser.add_argument(
        "--preview",
        "-p",
        type=int,
        metavar="N",
        help="Print the first N records of each file to stdout instead of writing files.",
    )
    args = parser.parse_args()

    device = DataCaptureDevice(args.device)

    if args.preview is not None:
        _preview(device, args.preview)
    else:
        _dump(device, Path(args.output_dir))


if __name__ == "__main__":
    main()
