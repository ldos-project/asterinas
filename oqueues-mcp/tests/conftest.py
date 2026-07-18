import os

import cbor2
import pytest


def cbor_blob(records):
    """Concatenate records as back-to-back CBOR items (like strong_observe)."""
    return b"".join(cbor2.dumps(r) for r in records)


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
