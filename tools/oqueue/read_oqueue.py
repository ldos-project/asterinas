#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0
"""Decode and display an OQueue filesystem (`/oqueues`) CBOR stream.

Each observation file under `/oqueues` (e.g. `.../strong_observe`) emits a
self-delimiting CBOR byte stream of one record map per observed value,
``{"seq", "value"}``. Records are concatenated with no framing, so they decode
back-to-back regardless of ``read()`` boundaries. Reading a live file blocks for
new records (like ``tail -f``) and ends when the queue is unregistered or this
reader is dropped for falling too far behind. Descriptive information about the
queue lives in the sibling ``metadata.yaml``, not in this stream.

Examples
--------
    # Live-tail the scheduler's events (the MVP demo):
    read_oqueue.py /oqueues/scheduler/events/strong_observe

    # Decode a previously captured dump, stopping after 20 records:
    read_oqueue.py -n 20 capture.cbor
"""

import argparse
import sys

try:
    import cbor2
except ImportError:
    sys.exit("This tool requires the 'cbor2' package: pip install cbor2")


def format_value(value):
    """Render one record's projected value.

    Scheduler events project to ``{"task", "kind"}``; other queues may project
    to any value, so fall back to a plain repr.
    """
    if isinstance(value, dict) and "kind" in value and "task" in value:
        return f"{value['kind']:<10} task=0x{value['task']:x}"
    return repr(value)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "path",
        nargs="?",
        default="/oqueues/scheduler/events/strong_observe",
        help="observation file to read (default: %(default)s)",
    )
    parser.add_argument(
        "-n",
        "--count",
        type=int,
        default=None,
        help="stop after N records (default: run until EOF or interrupt)",
    )
    args = parser.parse_args()

    with open(args.path, "rb") as stream:
        decoder = cbor2.CBORDecoder(stream)

        count = 0
        try:
            while True:
                try:
                    record = decoder.decode()
                except EOFError:
                    break
                print(f"{record.get('seq')}: {format_value(record.get('value'))}", flush=True)
                count += 1
                if args.count is not None and count >= args.count:
                    break
        except KeyboardInterrupt:
            pass


if __name__ == "__main__":
    main()
