"""Streaming sessions over ``strong_observe``.

One background thread per session drains ``cat strong_observe`` on the guest and
decodes CBOR into an in-memory record list. Termination is one of three modes:

* ``max_records`` — the reader stops itself after N records.
* ``timeout``     — a watchdog terminates the process after T seconds.
* ``infinite``    — runs until ``stop()`` (the agent's kill signal) or EOF.

The guest process is killed on every stop path, which closes the drain.
"""

import shlex
import threading
import time
import uuid
from dataclasses import dataclass, field
from typing import Any

from .cbor_stream import iter_records
from .oqfs import Oqfs
from .transport import Transport

# Grace period between SIGTERM and SIGKILL when stopping the guest process.
_KILL_GRACE_S = 2.0
# Cadence at which the timeout watchdog re-checks its deadline.
_WATCHDOG_POLL_S = 0.2


@dataclass
class Session:
    """State of one live drain over an OQueue's ``strong_observe`` stream.

    Fields:
        stream_id:   unique handle callers use to read/stop this session.
        oqueue_path: the OQueue being drained (as requested by the caller).
        mode:        termination mode — ``max_records`` | ``timeout`` | ``infinite``.
        max_records / timeout_s: the bound for the chosen mode (``None`` if unused).
        process:     the guest-side ``cat strong_observe`` subprocess. Its stdout
                     is the SSH data stream carrying raw CBOR bytes from the guest
                     OS to the host; killing it closes that stream.
        thread:      the host-side draining thread. It reads ``process`` stdout and
                     decodes the CBOR byte stream into records in real time,
                     appending to ``records`` as items arrive.
        watchdog:    for ``timeout`` mode, the thread that kills ``process`` at the
                     deadline (``None`` otherwise).
        records:     all records decoded so far (grows as the drain runs).
        read_cursor: index up to which the caller has already consumed ``records``,
                     so each ``read`` returns only what is new.
        status:      running | completed | stopped | error.
        error:       error message when ``status == "error"``.
        lock:        guards the mutable fields above, since the drain thread writes
                     while callers read concurrently.
    """

    stream_id: str
    oqueue_path: str
    mode: str
    max_records: int | None
    timeout_s: float | None
    process: Any = None
    thread: Any = None
    watchdog: Any = None
    records: list = field(default_factory=list)
    read_cursor: int = 0
    status: str = "running"
    error: str | None = None
    lock: threading.Lock = field(default_factory=threading.Lock)

    def finalize(self, status: str, error: str | None = None) -> bool:
        """Move a still-running session to a terminal status; the first
        terminal status wins. Returns whether this call applied the change."""
        with self.lock:
            if self.status != "running":
                return False
            self.status = status
            self.error = error
            return True

    def snapshot(self) -> dict:
        """Return a lock-safe copy of the session's public state (id, mode,
        status, record counts, error) for reporting to the caller/agent."""
        with self.lock:
            return {
                "stream_id": self.stream_id,
                "oqueue_path": self.oqueue_path,
                "mode": self.mode,
                "status": self.status,
                "records_total": len(self.records),
                "records_unread": len(self.records) - self.read_cursor,
                "error": self.error,
            }


class StreamManager:
    """Owns the set of live streaming sessions and their lifecycle.

    Each session drains one ``strong_observe`` stream on its own background
    thread; the manager tracks them by ``stream_id`` and exposes start / read /
    stop / list operations plus a blocking ``collect`` convenience.
    """

    def __init__(self, transport: Transport, oqfs: Oqfs):
        self._transport = transport
        self._oqfs = oqfs
        self._sessions: dict[str, Session] = {}
        self._lock = threading.Lock()

    def start(
        self,
        oqueue_path: str,
        max_records: int | None = None,
        timeout_s: float | None = None,
    ) -> Session:
        """Open a streaming session and start draining it in the background.

        Infers the termination mode from the bounds (``max_records`` -> mode 1,
        else ``timeout_s`` -> mode 2, else infinite), launches
        ``cat strong_observe`` on the guest as a subprocess pipe, and spawns a
        drain thread (plus a watchdog thread for the timeout mode). Returns
        immediately with the registered ``Session``; the caller reads records
        via ``read`` and ends the session via ``stop``.
        """
        if max_records is not None and max_records <= 0:
            raise ValueError("max_records must be positive")
        if timeout_s is not None and timeout_s <= 0:
            raise ValueError("timeout_s must be positive")

        if max_records is not None:
            mode = "max_records"
        elif timeout_s is not None:
            mode = "timeout"
        else:
            mode = "infinite"

        device = self._oqfs.strong_observe_path(oqueue_path)
        process = self._transport.popen(f"cat {shlex.quote(device)}")

        session = Session(
            stream_id=uuid.uuid4().hex,
            oqueue_path=oqueue_path,
            mode=mode,
            max_records=max_records,
            timeout_s=timeout_s,
            process=process,
        )
        with self._lock:
            # Register under the lock so concurrent starts/reads stay consistent.
            self._sessions[session.stream_id] = session

        session.thread = threading.Thread(
            target=self._drain, args=(session,), daemon=True
        )
        session.thread.start()

        if timeout_s is not None:
            session.watchdog = threading.Thread(
                target=self._watchdog, args=(session, timeout_s), daemon=True
            )
            session.watchdog.start()

        return session

    def _drain(self, session: Session) -> None:
        """Background-thread body: decode CBOR records off the guest pipe into
        the session's record list until a bound is hit, the stream closes, or a
        decode/pipe error occurs, then mark the terminal status and kill the
        guest process. In ``max_records`` mode the reader stops itself at N.
        """
        try:
            for record in iter_records(session.process.stdout):
                with session.lock:
                    session.records.append(record)
                    reached = (
                        session.max_records is not None
                        and len(session.records) >= session.max_records
                    )
                if reached:
                    self._kill(session.process)
                    session.finalize("completed")
                    break
            else:
                # Iterator exhausted: the stream closed on its own.
                session.finalize("completed")
        # A decode or pipe failure ends the drain as an error.
        except Exception as exc:
            session.finalize("error", str(exc))
        finally:
            self._kill(session.process)

    def _watchdog(self, session: Session, timeout_s: float) -> None:
        """Background-thread body for the timeout mode: sleep until the deadline
        (returning early if the process already finished), then mark the session
        completed and kill the guest process so the drain unblocks and ends.
        """
        deadline = time.monotonic() + timeout_s
        while time.monotonic() < deadline:
            if session.process.poll() is not None:
                # The guest process already finished on its own.
                return
            time.sleep(min(_WATCHDOG_POLL_S, deadline - time.monotonic()))
        session.finalize("completed")
        self._kill(session.process)

    @staticmethod
    def _kill(process) -> None:
        """Terminate the guest process if still running: send SIGTERM, wait a
        grace period, then SIGKILL if it hasn't exited. No-op if already dead.
        """
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=_KILL_GRACE_S)
            except Exception:
                process.kill()

    def collect(
        self,
        oqueue_path: str,
        max_records: int | None = None,
        timeout_s: float | None = None,
    ) -> tuple[Session, list[Any]]:
        """Start a bounded drain, block until it finishes, and return
        ``(session, records)``.

        A bound is required: without ``max_records`` or ``timeout_s`` the drain
        would run forever and this call would never return — use ``start`` for
        an unbounded session. This is the blocking counterpart the CLI uses; the
        MCP server drives the same session non-blockingly so it can cancel.
        """
        if max_records is None and timeout_s is None:
            raise ValueError(
                "collect requires max_records or timeout_s; "
                "use start for an unbounded stream"
            )
        session = self.start(
            oqueue_path, max_records=max_records, timeout_s=timeout_s
        )
        session.thread.join()
        return self.read(session.stream_id)

    def read(self, stream_id: str) -> tuple[Session, list[Any]]:
        """Return records decoded since the last read for this session, and the
        session itself. Advances a per-session cursor under the lock, so each
        record is returned exactly once across repeated polls.
        """
        session = self._get(stream_id)
        with session.lock:
            new = session.records[session.read_cursor :]
            session.read_cursor = len(session.records)
        return session, new

    def stop(self, stream_id: str) -> Session:
        """The kill signal for a session."""
        session = self._get(stream_id)
        session.finalize("stopped")
        self._kill(session.process)
        return session

    def list(self) -> list[dict]:
        """Return a snapshot of every session created this process lifetime."""
        with self._lock:
            return [s.snapshot() for s in self._sessions.values()]

    def _get(self, stream_id: str) -> Session:
        """Look up a session by id, raising ``KeyError`` if it is unknown."""
        with self._lock:
            session = self._sessions.get(stream_id)
        if session is None:
            raise KeyError(f"unknown stream_id: {stream_id}")
        return session
