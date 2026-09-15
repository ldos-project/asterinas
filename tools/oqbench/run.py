#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0

"""Host-side driver for the OQFS round-trip microbenchmark."""

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
QEMU_LOG = ROOT / "qemu.log"
CAPTURE_IMAGE = ROOT / "test/initramfs/build/capture.img"
DECODER_DIR = ROOT / "kernel/core/comps/mariposa_data_capture/python"

CAPTURE_PATH = "oqbench.samples"
SCHEDULER_CAPTURE_PATH = "scheduler.events"
PREFIX = "MARIPOSA_BENCH|"

# Base filenames for the decoded streams; `--output-prefix` is prepended to each.
OQ_OUTPUT_NAME = "oqbench.jsonl"
SCHED_OUTPUT_NAME = "scheduler_events.jsonl"

# Each becomes the `oqbench.<name>` kernel parameter of the same name. The kernel owns their
# defaults and their validation, so neither is duplicated here.
KERNEL_PARAMS = {
    "iterations": "measured iterations",
    "peer_compute": "TSC cycles the userspace peer spins for per request",
    "timeout_ms": "per-reply timeout in ms; a timeout ends the run as failed",
    "request_capacity": "request OQueue capacity",
    "reply_capacity": "reply OQueue capacity",
    "rt_prio": "run the kernel thread real-time at this priority, in 1..=99",
    "busy_procs": "competing busy-loop processes, as scheduler contention",
}


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    for name, help_text in KERNEL_PARAMS.items():
        parser.add_argument(
            f"--{name.replace('_', '-')}", type=int, metavar="N", help=help_text
        )
    parser.add_argument(
        "--vcpus", type=int, default=1, metavar="N", help="guest vCPU count"
    )
    parser.add_argument(
        "--release", help="Flag to build in release mode", action="store_true"
    )
    parser.add_argument(
        "-n",
        "--dry-run",
        action="store_true",
        help="print the command to run without running it",
    )
    parser.add_argument(
        "-v",
        "--verbose",
        action="store_true",
        help="print each command as it is run",
    )
    parser.add_argument(
        "--output-prefix",
        default="result_",
        metavar="PREFIX",
        help=(
            "prefix for the output files: the oqbench samples go to "
            f"<PREFIX>{OQ_OUTPUT_NAME}, and with --scheduler the scheduler events go to "
            f"<PREFIX>{SCHED_OUTPUT_NAME} (default: %(default)s)"
        ),
    )
    parser.add_argument(
        "--scheduler",
        action="store_true",
        help=(
            "also capture scheduler events: adds scheduler.capture_data=true and "
            f"FEATURES=ostd/capture_scheduling, and decodes them to <PREFIX>{SCHED_OUTPUT_NAME}"
        ),
    )
    return parser.parse_args()


def boot(args):
    """Boots the guest, blocking until it powers itself off at the end of the run."""
    params = ["oqbench.enable"]
    for name in KERNEL_PARAMS:
        value = getattr(args, name)
        if value is not None:
            params.append(f"oqbench.{name}={value}")
    if args.scheduler:
        params.append("scheduler.capture_data=true")

    command = [
        "make",
        "-C",
        str(ROOT),
        "run_kernel",
        "KCMDARGS=" + " ".join(params),
        f"SMP={args.vcpus}",
        f"RELEASE={1 if args.release else 0}",
    ]
    if args.scheduler:
        command.append("FEATURES=ostd/capture_scheduling")
    if args.dry_run or args.verbose:
        print(format_cmd_line(command))
    if (
        not args.dry_run
        and subprocess.run(command, stdin=subprocess.DEVNULL).returncode != 0
    ):
        sys.exit(f"make run_kernel failed; inspect {QEMU_LOG}")


HAS_WHITE_SPACE_RE = re.compile(r"\s")


def format_cmd_line(command):
    def format_arg(s):
        if HAS_WHITE_SPACE_RE.search(s):
            return f"'{s}'"
        return s

    return " ".join(format_arg(s) for s in command)


def console_block():
    """Returns the benchmark's console lines, stripped of the log's own timestamps and colours."""
    lines = []
    for line in QEMU_LOG.read_text(errors="replace").splitlines():
        start = line.find(PREFIX)
        if start >= 0:
            lines.append(line[start:])
    return lines


def check(block):
    """Exits unless the benchmark reported that the run completed."""
    errors = [line for line in block if line.startswith(f"{PREFIX}error")]
    if errors:
        sys.exit("\n".join(errors))
    if not any(line.startswith(f"{PREFIX}end oqueue_roundtrip") for line in block):
        sys.exit(f"the run did not complete; inspect {QEMU_LOG}")


def decode(targets):
    """Writes each requested capture path to its output file as JSON Lines.

    `targets` maps a capture path (e.g. `oqbench.samples`) to the file to write.
    Returns a dict mapping each decoded path to its record count.
    """
    sys.path.insert(0, str(DECODER_DIR))
    try:
        from mariposa_data_reader import DataCaptureDevice
    except ImportError as error:
        sys.exit(f"cannot decode: {error} (see {DECODER_DIR / 'requirements.txt'})")

    counts = {}
    for capture_file in DataCaptureDevice(CAPTURE_IMAGE):
        output = targets.get(capture_file.path)
        if output is None:
            continue
        count = 0
        with open(output, "w") as out:
            for record in capture_file:
                out.write(json.dumps(record) + "\n")
                count += 1
        counts[capture_file.path] = count
    missing = sorted(set(targets) - set(counts))
    if missing:
        sys.exit(f"no {' or '.join(missing)} capture in {CAPTURE_IMAGE}")
    return counts


def main():
    args = parse_args()

    if not args.dry_run:
        # Drop the image and let the build recreate an empty one, so that a run which dies early
        # cannot decode into the previous run's samples.
        CAPTURE_IMAGE.unlink(missing_ok=True)

    boot(args)
    if args.dry_run:
        return

    block = console_block()
    check(block)
    # Echo the metadata so the run is self-describing.
    print("run metadata:", *(f"  {line}" for line in block), sep="\n", file=sys.stderr)

    targets = {CAPTURE_PATH: f"{args.output_prefix}{OQ_OUTPUT_NAME}"}
    if args.scheduler:
        targets[SCHEDULER_CAPTURE_PATH] = f"{args.output_prefix}{SCHED_OUTPUT_NAME}"
    counts = decode(targets)
    for path, output in targets.items():
        print(f"wrote {counts[path]} samples from {path} to {output}", file=sys.stderr)


if __name__ == "__main__":
    main()
