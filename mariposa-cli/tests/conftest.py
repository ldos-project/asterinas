import os
import threading
import time

import cbor2
import pytest

# Cadence at which a fifo writer re-checks whether it should stop holding the
# pipe open.
_WRITER_POLL_S = 0.05


def cbor_blob(records):
    """Concatenate records as back-to-back CBOR items (like strong_observe)."""
    return b"".join(cbor2.dumps(r) for r in records)


def fifo_queue(root, name, records, hold_s=None):
    """Back an OQueue's ``strong_observe`` with a fifo fed by a writer thread.

    Creates ``<root>/<name>/strong_observe`` as a fifo (replacing any existing
    file), writes ``records`` as CBOR, then holds the pipe open until the
    returned stop event is set (or ``hold_s`` seconds elapse, if given) — so the
    timeout, kill, and infinite-stream paths can all be exercised.

    Returns ``(stop_event, thread)``; set the event and join the thread to tear
    the writer down.
    """
    so = root / name / "strong_observe"
    so.parent.mkdir(parents=True, exist_ok=True)
    if so.exists():
        so.unlink()
    os.mkfifo(so)

    stop = threading.Event()

    def writer():
        with open(so, "wb") as f:
            for r in records:
                f.write(cbor2.dumps(r))
            f.flush()
            deadline = None if hold_s is None else time.monotonic() + hold_s
            while not stop.is_set() and (
                deadline is None or time.monotonic() < deadline
            ):
                time.sleep(_WRITER_POLL_S)

    t = threading.Thread(target=writer, daemon=True)
    t.start()
    return stop, t


@pytest.fixture
def fake_oqfs(tmp_path):
    """Build a fake /oqueues tree on the host, driven via OQ_TRANSPORT=local.

    Layout:
        <root>/scheduler/events/{metadata.yaml, strong_observe}
        <root>/raid1/rebuilds/{metadata.yaml, strong_observe}
    strong_observe is a plain file of CBOR bytes (cat drains it like the device).
    """
    root = tmp_path / "oqueues"

    events = root / "scheduler" / "events"
    events.mkdir(parents=True)
    (events / "metadata.yaml").write_text(
        "name: scheduler.events\n"
        "oqueues:\n- scheduler.events\n"
        "type_name: aster_kernel::...::SchedulingEventView\n"
    )
    (events / "strong_observe").write_bytes(
        cbor_blob(
            [
                {"task": 1, "cpu": 0, "state": "run"},
                {"task": 2, "cpu": 1, "state": "wait"},
                {"task": 3, "cpu": 0, "state": "run"},
            ]
        )
    )

    rebuilds = root / "raid1" / "rebuilds"
    rebuilds.mkdir(parents=True)
    (rebuilds / "metadata.yaml").write_text("name: raid1.rebuilds\n")
    (rebuilds / "strong_observe").write_bytes(cbor_blob([{"disk": "sda", "pct": 42}]))

    env = {
        "OQ_TRANSPORT": "local",
        "OQ_ROOT": str(root),
        "OQ_METADATA_FILE": "metadata.yaml",
    }
    old = {k: os.environ.get(k) for k in env}
    os.environ.update(env)
    yield root
    for k, v in old.items():
        if v is None:
            os.environ.pop(k, None)
        else:
            os.environ[k] = v
