"""End-to-end tests driving the real code paths via OQ_TRANSPORT=local."""

import time

from conftest import fifo_queue

from mariposa_cli.oqueues.config import Config
from mariposa_cli.oqueues.oqfs import Oqfs
from mariposa_cli.oqueues.streams import StreamManager
from mariposa_cli.oqueues.transport import Transport


def _stack():
    cfg = Config.from_env()
    transport = Transport(cfg)
    oqfs = Oqfs(cfg, transport)
    return cfg, transport, oqfs, StreamManager(transport, oqfs)


def test_list_oqueues(fake_oqfs):
    _, _, oqfs, _ = _stack()
    queues = oqfs.list_oqueues()
    relpaths = {q["relpath"] for q in queues}
    assert relpaths == {"scheduler/events", "raid1/rebuilds"}


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
    stop, t = fifo_queue(fake_oqfs, "watchdog/q", [{"tick": 1}])

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
    stop, t = fifo_queue(fake_oqfs, "scheduler/events", [{"n": 1}, {"n": 2}])

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
