"""Streaming over a single OQueue's ``strong_observe``.

A ``Stream`` owns one background drain of ``cat strong_observe`` on the guest,
decoding CBOR items onto a ``queue.Queue`` as they arrive.
Termination is one of three modes:

* ``max_records`` — the reader stops itself after N records.
* ``timeout``     — a watchdog terminates the process after T seconds.
* ``infinite``    — runs until ``stop()`` (the kill signal) or EOF.

The terminal ``status`` is written only by the drain thread — ``stop()`` and the
watchdog just kill the guest process and let the drain observe the resulting EOF. 

The CLI drives a single ``Stream`` directly. The MCP server, which juggles many
concurrent sessions, tracks them through a ``StreamManager``.
"""

import queue
import shlex
import subprocess
import threading
import time
import uuid

from .cbor_stream import iter_records
from .oqfs import Oqfs
from .transport import Transport

# Grace period between SIGTERM and SIGKILL when stopping the guest process.
_KILL_GRACE_S = 2.0
# Cadence at which the timeout watchdog re-checks its deadline.
_WATCHDOG_POLL_S = 0.2

# Pushed onto the record queue when the drain ends, so a blocking consumer wakes
# up at end-of-stream instead of waiting forever.
_END = object()


class Stream:
    """One live drain over an OQueue's ``strong_observe`` stream.

    Construct with the bounds, call ``start()`` to launch the guest ``cat`` and
    its drain thread, then consume records with ``iter_live()`` (blocking) or
    ``read()`` (drain what is queued now). End an infinite stream with ``stop()``.
    """

    def __init__(
        self,
        transport: Transport,
        oqfs: Oqfs,
        oqueue_path: str,
        max_records: int | None = None,
        timeout_s: float | None = None,
    ):
        self._transport = transport
        self._oqfs = oqfs
        self.stream_id = uuid.uuid4().hex
        self.oqueue_path = oqueue_path
        self.max_records = max_records
        self.timeout_s = timeout_s
        if max_records is not None:
            self.mode = "max_records"
        elif timeout_s is not None:
            self.mode = "timeout"
        else:
            self.mode = "infinite"

        self.process: subprocess.Popen | None = None
        self.thread: threading.Thread | None = None
        self.watchdog: threading.Thread | None = None

        self._queue: queue.Queue = queue.Queue()
        self._produced = 0
        self._consumed = 0
        self._stop_requested = threading.Event()
        self.status = "running"
        self.error: str | None = None

    def start(self) -> "Stream":
        """Launch the guest ``cat`` and start draining it in the background.

        Returns immediately with ``self``; the caller consumes records via
        ``iter_live`` / ``read`` and ends the stream via ``stop``.
        """
        if self.max_records is not None and self.max_records <= 0:
            raise ValueError("max_records must be positive")
        if self.timeout_s is not None and self.timeout_s <= 0:
            raise ValueError("timeout_s must be positive")

        device = self._oqfs.strong_observe_path(self.oqueue_path)
        self.process = self._transport.popen(f"cat {shlex.quote(device)}")

        self.thread = threading.Thread(target=self._drain, daemon=True)
        self.thread.start()

        if self.timeout_s is not None:
            self.watchdog = threading.Thread(target=self._watchdog, daemon=True)
            self.watchdog.start()

        return self

    def _drain(self) -> None:
        """Background-thread body: decode CBOR records off the guest pipe onto
        the queue until a bound is hit, the stream closes, or a decode/pipe error
        occurs, then record the terminal status and kill the guest process.
        """
        terminal = "completed"
        try:
            for record in iter_records(self.process.stdout):
                self._queue.put(record)
                self._produced += 1
                if self.max_records is not None and self._produced >= self.max_records:
                    break
            else:
                # Loop fell through: the pipe closed on its own (EOF).
                pass
            if self._stop_requested.is_set():
                terminal = "stopped"
        except Exception as exc:
            self.error = str(exc)
            terminal = "error"
        finally:
            self._kill()
            self.status = terminal
            self._queue.put(_END)

    def _watchdog(self) -> None:
        """Timeout-mode watchdog: kill the guest process at the deadline (unless
        it already finished) so the drain unblocks and ends as ``completed``.
        """
        deadline = time.monotonic() + self.timeout_s
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                return
            time.sleep(min(_WATCHDOG_POLL_S, deadline - time.monotonic()))
        self._kill()

    def _kill(self) -> None:
        """Terminate the guest process if still running: SIGTERM, wait a grace
        period, then SIGKILL. Safe to call from several threads and when already
        dead (``Popen.wait`` is internally locked).
        """
        p = self.process
        if p is not None and p.poll() is None:
            p.terminate()
            try:
                p.wait(timeout=_KILL_GRACE_S)
            except Exception:
                p.kill()

    def read(self) -> list:
        """Return the records queued since the last read (non-blocking)."""
        out = []
        while True:
            try:
                item = self._queue.get_nowait()
            except queue.Empty:
                break
            if item is _END:
                break
            out.append(item)
            self._consumed += 1
        return out

    def iter_live(self):
        """Yield records as they arrive, blocking until the stream ends."""
        while True:
            item = self._queue.get()
            if item is _END:
                return
            self._consumed += 1
            yield item

    def collect(self) -> list:
        """Start a bounded drain, block until it finishes, and return its records.

        A bound is required: without ``max_records`` or ``timeout_s`` the drain
        would run forever. Use a live ``Stream`` (``start`` + ``iter_live``) for
        an unbounded stream.
        """
        if self.max_records is None and self.timeout_s is None:
            raise ValueError(
                "collect requires max_records or timeout_s; "
                "use a live stream for an unbounded drain"
            )
        self.start()
        self.join()
        return self.read()

    def stop(self) -> "Stream":
        """The kill signal: request a stop, kill the guest process, and wait for
        the drain to finalize (so ``status`` is settled when this returns)."""
        self._stop_requested.set()
        self._kill()
        self.join()
        return self

    def join(self, timeout: float | None = None) -> None:
        """Wait for the drain thread to finish."""
        if self.thread is not None:
            self.thread.join(timeout)

    def snapshot(self) -> dict:
        """Return a copy of the stream's public state for reporting to callers."""
        return {
            "stream_id": self.stream_id,
            "oqueue_path": self.oqueue_path,
            "mode": self.mode,
            "status": self.status,
            "records_total": self._produced,
            "records_unread": self._produced - self._consumed,
            "error": self.error,
        }


class StreamManager:
    """Registry of live ``Stream``s, used only by the MCP server.

    The MCP server can hold many concurrent sessions and must look them up by
    ``stream_id`` across separate tool calls, so it tracks them here.
    """

    def __init__(self, transport: Transport, oqfs: Oqfs):
        self._transport = transport
        self._oqfs = oqfs
        self._streams: dict[str, Stream] = {}
        self._lock = threading.Lock()

    def start(
        self,
        oqueue_path: str,
        max_records: int | None = None,
        timeout_s: float | None = None,
    ) -> Stream:
        """Create a ``Stream``, start draining it, and register it by id."""
        stream = Stream(
            self._transport,
            self._oqfs,
            oqueue_path,
            max_records=max_records,
            timeout_s=timeout_s,
        ).start()
        with self._lock:
            self._streams[stream.stream_id] = stream
        return stream

    def read(self, stream_id: str) -> tuple[Stream, list]:
        """Return ``(stream, records-since-last-read)`` for a session."""
        stream = self._get(stream_id)
        return stream, stream.read()

    def stop(self, stream_id: str) -> Stream:
        """The kill signal for a session."""
        return self._get(stream_id).stop()

    def list(self) -> list[dict]:
        """Return a snapshot of every session created this process lifetime."""
        with self._lock:
            return [s.snapshot() for s in self._streams.values()]

    def _get(self, stream_id: str) -> Stream:
        """Look up a session by id, raising ``KeyError`` if it is unknown."""
        with self._lock:
            stream = self._streams.get(stream_id)
        if stream is None:
            raise KeyError(f"unknown stream_id: {stream_id}")
        return stream
