#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0

"""Host-side driver for the OQFS round-trip microbenchmark."""

import argparse
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
QEMU_LOG = ROOT / "qemu.log"
CAPTURE_IMAGE = ROOT / "test/initramfs/build/capture.img"
DECODER_DIR = ROOT / "kernel/comps/mariposa_data_capture/python"

CAPTURE_PATH = "oqbench.samples"
PREFIX = "MARIPOSA_BENCH|"

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
        "--output",
        type=Path,
        default=Path("oqbench-samples.jsonl"),
        metavar="FILE",
        help="sample file to write (default: %(default)s)",
    )
    return parser.parse_args()


def boot(args):
    """Boots the guest, blocking until it powers itself off at the end of the run."""
    params = ["oqbench.enable"]
    for name in KERNEL_PARAMS:
        value = getattr(args, name)
        if value is not None:
            params.append(f"oqbench.{name}={value}")

    command = [
        "make",
        "-C",
        str(ROOT),
        "run_kernel",
        "KCMDARGS=" + " ".join(params),
        f"SMP={args.vcpus}",
    ]
    if subprocess.run(command, stdin=subprocess.DEVNULL).returncode != 0:
        sys.exit(f"make run_kernel failed; inspect {QEMU_LOG}")


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


def decode(output):
    """Writes the captured samples to `output` as JSON Lines, returning how many there were."""
    sys.path.insert(0, str(DECODER_DIR))
    try:
        from mariposa_data_reader import DataCaptureDevice
    except ImportError as error:
        sys.exit(f"cannot decode: {error} (see {DECODER_DIR / 'requirements.txt'})")

    for capture_file in DataCaptureDevice(CAPTURE_IMAGE):
        if capture_file.path != CAPTURE_PATH:
            continue
        with open(output, "w") as out:
            samples = 0
            for record in capture_file:
                out.write(json.dumps(record) + "\n")
                samples += 1
        return samples
    sys.exit(f"no {CAPTURE_PATH} capture in {CAPTURE_IMAGE}")


def main():
    args = parse_args()

    # Drop the image and let the build recreate an empty one, so that a run which dies early cannot
    # decode into the previous run's samples.
    CAPTURE_IMAGE.unlink(missing_ok=True)
    boot(args)

    block = console_block()
    check(block)
    # Echo the metadata so the run is self-describing.
    print("run metadata:", *(f"  {line}" for line in block), sep="\n", file=sys.stderr)

    print(f"wrote {decode(args.output)} samples to {args.output}", file=sys.stderr)


if __name__ == "__main__":
    main()
