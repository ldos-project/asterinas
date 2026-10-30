# SPDX-License-Identifier: MPL-2.0

"""Build Mariposa and run one benchmark in it; shared by every subsystem's `build`/`benchmark`."""

# Guest protocol (AGENTS.md, "Benchmark"): the guest prints `<marker>begin`, then `<marker>end` on
# success or `<marker>error <reason>` on failure, and writes result files to `/host/<guest_out_dir>/`
# (the virtio-fs share).

import json
import os
import re
import shutil
import signal
import socket
import subprocess
import sys
import time
from pathlib import Path

import cbor2

from orchestrator.subsystem import BenchmarkError, OQueue

REPO_ROOT = Path(__file__).resolve().parents[2]
SHARE = REPO_ROOT / "aster-sharepoint"
QEMU_LOG = REPO_ROOT / "qemu.log"
CAPTURE_IMAGE = REPO_ROOT / "test/initramfs/build/capture.img"
VIRTIOFS_SOCKET = Path("/tmp/vhostqemu/vfs.sock")
# Shared by `build_kernel` and `run_guest` so both build the same kernel: release, benchmark binaries
# in the initramfs, kernel console logging at error level only (keeps the console quiet during runs).
MAKE_VARS = ("ENABLE_BENCHMARK_TEST=true", "RELEASE=1", "LOG_LEVEL=error")
# Wraps every guest command to record OQueues from `/oqueues` and list the exported ones
# (`test/initramfs/src/benchmark/common/oqueue_capture.sh`).
OQUEUE_WRAPPER = "/benchmark/common/oqueue_capture.sh"
# Where a run's listing of exported OQueues goes in its `out_dir`.
EXPORTS_FILE = "oqueue_exports.txt"
# The guest can wedge flushing the data capture after its results are already on the share.
SHUTDOWN_GRACE_S = 180


def log(message: str) -> None:
    """Print a progress message."""
    print(f"[mariposa] {message}", flush=True)


def build_kernel(run=subprocess.run) -> tuple[bool, str]:
    """Build the kernel without booting it; return (ok, error log)."""
    process = run(["make", "-C", str(REPO_ROOT), "kernel", *MAKE_VARS], capture_output=True, text=True)
    if process.returncode == 0:
        return True, ""
    stderr = process.stderr or ""
    # Start the log at the first compiler error; the warnings before it would bury the cause.
    first_error = re.search(r"^error(\[E\d+\])?:", stderr, re.MULTILINE)
    return False, stderr[first_error.start() :] if first_error else stderr[-8000:]


def guest_markers(text: str, marker: str) -> list[str]:
    """Extract the guest's marker lines from console output."""
    # Cut each line at the marker, dropping whatever the console prefixed (timestamps etc.).
    return [line[line.index(marker) :] for line in text.splitlines() if marker in line]


def failure(markers: list[str], marker: str) -> str | None:
    """Tell from the guest's markers why a run failed; None if it completed."""
    errors = [m for m in markers if m.startswith(marker + "error")]
    if errors:
        return "; ".join(errors)
    if not any(m.startswith(marker + "end") for m in markers):
        return "the guest did not report completion"
    return None


def _console_markers(marker: str) -> list[str]:
    """Read the guest's marker lines from the current boot's console log (qemu.log)."""
    return guest_markers(QEMU_LOG.read_text(errors="replace"), marker) if QEMU_LOG.exists() else []


def _boot(command: list[str], environment: dict, marker: str, timeout_s: float, log_path: Path) -> str | None:
    """Boot the kernel and wait for the guest to finish; return a problem description or None."""
    # Own session, so the whole QEMU process group can be killed.
    with open(log_path, "w") as log_file:
        process = subprocess.Popen(command, env=environment, stdin=subprocess.DEVNULL, stdout=log_file,
                                   stderr=subprocess.STDOUT, start_new_session=True)
    try:
        return _watch(process, marker, timeout_s)
    except BaseException:
        # Interrupted (Ctrl-C, SIGTERM): QEMU runs in its own session and would outlive us.
        os.killpg(process.pid, signal.SIGKILL)
        process.wait()
        raise


def _watch(process: subprocess.Popen, marker: str, timeout_s: float) -> str | None:
    """Wait for a boot to end, killing it if it hangs."""
    started = time.monotonic()
    finished_at = None
    while True:
        # Poll every 5 s: did QEMU exit by itself?
        try:
            status = process.wait(timeout=5)
            return f"make run_kernel exited with status {status}" if status else None
        except subprocess.TimeoutExpired:
            pass
        # Whole run too long (e.g. the policy deadlocked the kernel): kill.
        if time.monotonic() - started > timeout_s:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            return f"the run exceeded {timeout_s:.0f}s and was killed"
        # Guest reported end/error but QEMU did not power off within the grace period: kill.
        if finished_at is None:
            if any(m.startswith((marker + "end", marker + "error")) for m in _console_markers(marker)):
                finished_at = time.monotonic()
        elif time.monotonic() - finished_at > SHUTDOWN_GRACE_S:
            log(f"the guest finished but did not power off within {SHUTDOWN_GRACE_S}s; killing QEMU")
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            return None


def _start_virtiofsd(log_path: Path) -> subprocess.Popen:
    """Start the virtio-fs server that shares SHARE with the guest as /host."""
    # A virtiofsd serves exactly one boot, so every run starts its own.
    VIRTIOFS_SOCKET.parent.mkdir(parents=True, exist_ok=True)
    VIRTIOFS_SOCKET.unlink(missing_ok=True)
    with open(log_path, "w") as log_file:
        process = subprocess.Popen(
            ["/usr/libexec/virtiofsd", f"--socket-path={VIRTIOFS_SOCKET}", f"--shared-dir={SHARE}"],
            stdin=subprocess.DEVNULL, stdout=log_file, stderr=subprocess.STDOUT,
        )
    # Wait up to 10 s for its socket.
    deadline = time.monotonic() + 10
    while not VIRTIOFS_SOCKET.exists():
        if process.poll() is not None or time.monotonic() > deadline:
            process.kill()
            raise BenchmarkError("virtiofsd did not start; is another one still running?")
        time.sleep(0.05)
    return process


def _fields(record, prefix: str = "") -> set[str]:
    """The dotted names of a nested record's fields."""
    if not isinstance(record, dict):
        return {prefix}
    return set().union(*(_fields(value, f"{prefix}.{key}" if prefix else str(key)) for key, value in record.items()))


def _write_records(records, out_path: Path, name: str) -> int:
    """Write decoded records to `out_path` as JSON lines; return how many."""
    # The last record can be cut short when the guest stops mid-write: stop at a decode error, and
    # drop a final record whose fields no earlier record had (a partial decode).
    count, seen_fields, pending = 0, set(), None  # written one record behind, so the last can be checked
    with open(out_path, "w") as out:
        try:
            for record in records:
                if pending is not None:
                    out.write(json.dumps(pending, default=str) + "\n")
                    count += 1
                    seen_fields |= _fields(pending)
                pending = record
        except Exception as error:  # a truncated tail; keep what decoded
            log(f"{name}: stopped decoding after {count} records ({error})")
        if pending is not None and (not seen_fields or _fields(pending) <= seen_fields):
            out.write(json.dumps(pending, default=str) + "\n")
            count += 1
    return count


def decode_captures(capture: list[OQueue], out_dir: Path) -> dict[str, int]:
    """Decode OQueues recorded by the data-capture device into `out_dir/<name>.jsonl`."""
    sys.path.insert(0, str(REPO_ROOT / "kernel/core/comps/mariposa_data_capture/python"))
    from mariposa_data_reader import DataCaptureDevice

    by_path = {oqueue.capture_path: oqueue for oqueue in capture}
    counts = {oqueue.name: 0 for oqueue in capture}
    for capture_file in DataCaptureDevice(CAPTURE_IMAGE):
        oqueue = by_path.get(capture_file.path)
        if oqueue is not None:
            counts[oqueue.name] = _write_records(capture_file, out_dir / f"{oqueue.name}.jsonl", oqueue.name)
    for name in counts:
        (out_dir / f"{name}.jsonl").touch()
    log("decoded " + ", ".join(f"{n} {name} records" for name, n in counts.items()))
    return counts


def _cbor_stream(path: Path):
    """Yield the records of a CBOR stream file (an OQFS `strong_observe` recording)."""
    with open(path, "rb") as f:
        decoder = cbor2.CBORDecoder(f)
        while f.peek(1):
            yield decoder.decode()


def decode_oqfs_captures(capture: list[OQueue], capture_dir: Path, out_dir: Path) -> dict[str, int]:
    """Decode OQueues recorded from `/oqueues` by the guest wrapper into `out_dir/<name>.jsonl`."""
    counts = {}
    for oqueue in capture:
        src = capture_dir / f"{oqueue.name}.cbor"
        counts[oqueue.name] = _write_records(_cbor_stream(src), out_dir / f"{oqueue.name}.jsonl", oqueue.name) if src.exists() else 0
        if not src.exists():
            (out_dir / f"{oqueue.name}.jsonl").touch()
    log("decoded " + ", ".join(f"{n} {name} records" for name, n in counts.items()))
    return counts


def _free_port() -> int:
    """Find a free TCP port on the host."""
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def run_guest(
    out_dir: Path,
    *,
    marker: str,
    init_args: list[str],
    kernel_args: list[str] = (),
    share_files: dict[str, Path] | None = None,
    env: dict[str, str] | None = None,
    capture: list[OQueue] = (),
    smp: int = 4,
    mem: str = "16G",
    guest_out_dir: str = "run",
    timeout_s: float = 3600,
) -> None:
    """Run one benchmark in a fresh boot and collect its results into `out_dir`."""
    # Raises BenchmarkError if the guest did not report completion.
    # `share_files`: {path under /host in the guest: host file to copy there}.
    # `env`: extra environment for `make run_kernel` (e.g. variables `tools/qemu_args.sh` reads).

    # Fresh output dir and share; stage the inputs.
    shutil.rmtree(out_dir, ignore_errors=True)
    out_dir.mkdir(parents=True)
    staging = SHARE / guest_out_dir
    shutil.rmtree(staging, ignore_errors=True)
    for guest_path, host_file in (share_files or {}).items():
        (SHARE / guest_path).parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(host_file, SHARE / guest_path)
    SHARE.mkdir(exist_ok=True)
    QEMU_LOG.unlink(missing_ok=True)

    # Turn on the requested captures: device ones through the data-capture device, the rest by the
    # guest wrapper reading `/oqueues`. The wrapper runs on every boot, so each run also lists the
    # OQueues the kernel exports.
    kernel_args = list(kernel_args)
    for oqueue in capture:
        kernel_args += [a for a in oqueue.kernel_args if a not in kernel_args]
    device = [o for o in capture if o.capture_path]
    oqfs = [o for o in capture if not o.capture_path]
    if device:
        CAPTURE_IMAGE.unlink(missing_ok=True)  # the initramfs build recreates it empty
        kernel_args.append("data_capture.sync_period=500")
    shutil.rmtree(SHARE / "oqueue_capture", ignore_errors=True)
    init_args = [OQUEUE_WRAPPER, ",".join(o.name for o in oqfs) or "-", "--", *init_args]
    command = [
        "make", "-C", str(REPO_ROOT), "run_kernel", *MAKE_VARS, f"SMP={smp}", f"MEM={mem}",
        "KCMDARGS=" + " ".join(kernel_args), "INITARGS=" + " ".join(init_args),
    ]
    # `qemu_args.sh` forwards the guest's SSH to host port 22 by default, which the host's sshd holds.
    environment = dict(os.environ, VIRTIOFS="on", SSH_PORT=str(_free_port()), **(env or {}))
    log(" ".join(command))
    # Boot; make's and virtiofsd's output go to logs in out_dir.
    virtiofsd = _start_virtiofsd(out_dir / "virtiofsd.log")
    try:
        problem = _boot(command, environment, marker, timeout_s, out_dir / "run_kernel.log")
    finally:
        virtiofsd.terminate()
        virtiofsd.wait()

    # Collect the guest's result files, the console log and the captures, then check the markers.
    for produced in staging.glob("*"):
        shutil.move(str(produced), out_dir)
    if QEMU_LOG.exists():
        shutil.copy(QEMU_LOG, out_dir / "qemu.log")
    if device and CAPTURE_IMAGE.exists():
        decode_captures(device, out_dir)
    capture_dir = SHARE / "oqueue_capture"
    if (capture_dir / "exports.txt").exists():
        shutil.copy(capture_dir / "exports.txt", out_dir / EXPORTS_FILE)
    if (capture_dir / "errors.txt").exists():
        shutil.copy(capture_dir / "errors.txt", out_dir / "oqueue_capture_errors.txt")
    if oqfs:
        decode_oqfs_captures(oqfs, capture_dir, out_dir)
    problem = failure(_console_markers(marker), marker) or problem
    if problem:
        raise BenchmarkError(f"{problem}; see {out_dir}")


def read_exports(path: Path) -> list[dict]:
    """Parse a run's listing of exported OQueues into [{"path", "type_name", "observable"}]."""
    # The listing is `metadata.yaml` blocks (`name: P`, `type_name: T`), then `strong_observe:` and
    # the paths of the streams that can be read (`/oqueues/a/b/3/c/strong_observe` for `a.b[3].c`).
    text = path.read_text(errors="replace")
    head, _, streams = text.partition("strong_observe:")
    names = re.findall(r"^name: (\S+)$", head, re.MULTILINE)
    types = re.findall(r"^type_name: (.+)$", head, re.MULTILINE)
    observable = {line.strip() for line in streams.splitlines() if line.strip()}

    def stream_of(name: str) -> str:
        """`/oqueues` stream file of an OQueue path."""
        return "/oqueues/" + re.sub(r"\[(\w+)\]", r"/\1", name).replace(".", "/") + "/strong_observe"

    return [
        {"path": n, "type_name": t, "observable": stream_of(n) in observable}
        for n, t in sorted(zip(names, types))
    ]
