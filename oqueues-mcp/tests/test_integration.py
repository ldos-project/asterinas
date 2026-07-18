"""End-to-end tests driving the real code paths via OQ_TRANSPORT=local."""

import os
import threading
import time

import cbor2

from oqueues_mcp.config import Config
from oqueues_mcp.oqfs import Oqfs
from oqueues_mcp.streams import StreamManager
from oqueues_mcp.transport import Transport


def _stack():
    cfg = Config.from_env()
    transport = Transport(cfg)
    oqfs = Oqfs(cfg, transport)
    return cfg, transport, oqfs, StreamManager(transport, oqfs)


def test_list_oqueues(fake_oqfs):
    _, _, oqfs, _ = _stack()
    queues = oqfs.list_oqueues()
    names = {q["name"] for q in queues}
    assert names == {"scheduler.events", "raid1.rebuilds"}


def test_read_metadata(fake_oqfs):
    _, _, oqfs, _ = _stack()
    assert "scheduler.events" in oqfs.read_metadata("scheduler/events")
    assert "raid1.rebuilds" in oqfs.read_metadata("raid1/rebuilds")


def test_tree_runs(fake_oqfs):
    _, _, oqfs, _ = _stack()
    # `tree` may not be installed in the test env; tolerate that.
    try:
        out = oqfs.tree()
    except RuntimeError:
        return
    assert "scheduler" in out


def test_stream_max_records(fake_oqfs):
    _, _, _, mgr = _stack()
    session = mgr.start("scheduler/events", max_records=2)
    session.thread.join(timeout=5)
    _, records = mgr.read(session.stream_id)
    assert len(records) == 2
    assert session.status == "completed"


def test_stream_completes_on_eof(fake_oqfs):
    _, _, _, mgr = _stack()
    session = mgr.start("scheduler/events", max_records=100)
    session.thread.join(timeout=5)
    _, records = mgr.read(session.stream_id)
    assert len(records) == 3  # file only has 3 records
    assert session.status == "completed"


def test_stream_timeout(fake_oqfs):
    # An OQueue whose strong_observe is a fifo that never closes: the timeout
    # watchdog must terminate the drain.
    q = fake_oqfs / "watchdog" / "q"
    q.mkdir(parents=True)
    so = q / "strong_observe"
    os.mkfifo(so)

    stop = threading.Event()

    def writer():
        with open(so, "wb") as f:
            f.write(cbor2.dumps({"tick": 1}))
            f.flush()
            while not stop.is_set():
                time.sleep(0.05)

    t = threading.Thread(target=writer, daemon=True)
    t.start()

    _, _, _, mgr = _stack()
    started = time.monotonic()
    session = mgr.start("watchdog/q", timeout_s=1.0)
    session.thread.join(timeout=5)
    elapsed = time.monotonic() - started

    assert session.status == "completed"
    assert elapsed < 4  # watchdog fired, did not hang on the open fifo
    _, records = mgr.read(session.stream_id)
    assert len(records) == 1

    stop.set()
    t.join(timeout=2)


def test_stream_infinite_kill(fake_oqfs):
    # Replace strong_observe with a fifo for an unbounded stream.
    so = fake_oqfs / "scheduler" / "events" / "strong_observe"
    so.unlink()
    os.mkfifo(so)

    stop = threading.Event()

    def writer():
        with open(so, "wb") as f:
            f.write(cbor2.dumps({"n": 1}))
            f.write(cbor2.dumps({"n": 2}))
            f.flush()
            while not stop.is_set():
                time.sleep(0.05)

    t = threading.Thread(target=writer, daemon=True)
    t.start()

    _, _, _, mgr = _stack()
    session = mgr.start("scheduler/events")  # infinite
    assert session.mode == "infinite"

    # Give the reader a moment to drain the two records.
    time.sleep(0.5)
    _, records = mgr.read(session.stream_id)
    assert len(records) == 2

    mgr.stop(session.stream_id)  # the kill signal
    session.thread.join(timeout=5)
    assert session.status == "stopped"

    stop.set()
    t.join(timeout=2)
