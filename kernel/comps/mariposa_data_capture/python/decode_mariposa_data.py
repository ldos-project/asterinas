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
import logging
import re
from pathlib import Path

from mariposa_data_reader import DataCaptureDevice


logger = logging.getLogger(__name__)


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
            logger.error(f"[{path}] warning: could not read header: {exc}")
            type_name = "<unknown>"

        print(f"=== {path} (type: {type_name})")
        for i, record in enumerate(capture_file):
            if i >= n:
                print(f"  ... (showing first {n} records)")
                break
            try:
                print(f"  {json.dumps(record)}")
            except (ValueError, TypeError) as e:
                logger.error(f"{e}")
        print()


def _dump(device: DataCaptureDevice, output_dir: Path):
    for capture_file in device:
        path = capture_file.path
        out_path = output_dir / _output_name(path)
        count = 0
        with open(out_path, "w") as out:
            for record in capture_file:
                try:
                    out.write(json.dumps(record))
                    out.write("\n")
                    count += 1
                except (ValueError, TypeError) as e:
                    logger.error(
                        f"Failed to write record {count} as JSON: {record}\n  {e}\n"
                        "  (If this is the last record, the stream may have been truncated, e.g., by failing to flush or running out of space.)"
                    )
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
        help="Print the first N records of each file to stdout instead of writing files. "
        "This intensionally permits more errors than full decoding.",
    )
    args = parser.parse_args()

    device = DataCaptureDevice(args.device)

    if args.preview is not None:
        _preview(device, args.preview)
    else:
        _dump(device, Path(args.output_dir))


if __name__ == "__main__":
    main()
