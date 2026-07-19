"""Verify tools are async and non-blocking: concurrent streams, live progress."""

import asyncio
import os
import threading
import time

import cbor2

from oqueues_mcp import server


def _make_fifo_queue(root, name, records, hold_s):
    """Create <root>/<name>/strong_observe as a fifo fed by a writer that holds
    it open for hold_s seconds (so timeout/kill paths are exercised)."""
    qdir = root / name
    qdir.mkdir(parents=True)
    so = qdir / "strong_observe"
    os.mkfifo(so)
    stop = threading.Event()

    def writer():
        with open(so, "wb") as f:
            for r in records:
                f.write(cbor2.dumps(r))
            f.flush()
            deadline = time.monotonic() + hold_s
            while not stop.is_set() and time.monotonic() < deadline:
                time.sleep(0.05)

    t = threading.Thread(target=writer, daemon=True)
    t.start()
    return name, stop, t


def test_async_smoke(fake_oqfs):
    server._build()

    async def go():
        queues = await server.list_oqueues()
        assert "scheduler.events" in queues
        meta = await server.read_metadata("scheduler/events")
        assert "scheduler.events" in meta

    asyncio.run(go())


def test_concurrent_stream_collect(fake_oqfs):
    # Two independent fifo-backed queues, each drained for ~1s. Running them
    # concurrently must finish in ~1s, not ~2s — proof the loop isn't blocked.
    server._build()
    a = _make_fifo_queue(fake_oqfs, "modA/q", [{"n": 1}, {"n": 2}], hold_s=2)
    b = _make_fifo_queue(fake_oqfs, "modB/q", [{"n": 9}], hold_s=2)

    async def go():
        start = time.monotonic()
        ra, rb = await asyncio.gather(
            server.stream_collect("modA/q", timeout_s=1.0, fmt="json"),
            server.stream_collect("modB/q", timeout_s=1.0, fmt="json"),
        )
        elapsed = time.monotonic() - start
        return ra, rb, elapsed

    ra, rb, elapsed = asyncio.run(go())
    for _, stop, t in (a, b):
        stop.set()
        t.join(timeout=2)

    assert "1" in ra and "2" in ra
    assert "9" in rb
    assert elapsed < 1.8, f"streams did not run concurrently (elapsed={elapsed:.2f}s)"


def test_stream_collect_cancel_stops_stream(fake_oqfs):
    # A fifo that stays open: cancelling the collect must stop the drain.
    server._build()
    name, stop, t = _make_fifo_queue(fake_oqfs, "modC/q", [{"n": 1}], hold_s=5)

    async def go():
        task = asyncio.create_task(server.stream_collect("modC/q", timeout_s=30.0))
        await asyncio.sleep(0.5)
        task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass

    asyncio.run(go())
    stop.set()
    t.join(timeout=2)

    # The session created by the cancelled collect must be stopped, not running.
    statuses = [s["status"] for s in server._streams.list()]
    assert "stopped" in statuses
    assert "running" not in statuses
