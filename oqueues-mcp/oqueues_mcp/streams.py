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


@dataclass
class Session:
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
    status: str = "running"  # running | completed | stopped | error
    error: str | None = None
    lock: threading.Lock = field(default_factory=threading.Lock)

    def snapshot(self) -> dict:
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
                    with session.lock:
                        if session.status == "running":
                            session.status = "completed"
                    break
            else:
                # Iterator exhausted: the stream closed on its own.
                with session.lock:
                    if session.status == "running":
                        session.status = "completed"
        except Exception as exc:  # decode/pipe failure
            with session.lock:
                if session.status == "running":
                    session.status = "error"
                    session.error = str(exc)
        finally:
            self._kill(session.process)

    def _watchdog(self, session: Session, timeout_s: float) -> None:
        deadline = time.monotonic() + timeout_s
        while time.monotonic() < deadline:
            if session.process.poll() is not None:
                return  # already finished
            time.sleep(min(0.2, deadline - time.monotonic()))
        with session.lock:
            if session.status == "running":
                session.status = "completed"
        self._kill(session.process)

    @staticmethod
    def _kill(process) -> None:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=2.0)
            except Exception:
                process.kill()

    def read(self, stream_id: str) -> tuple[Session, list[Any]]:
        session = self._get(stream_id)
        with session.lock:
            new = session.records[session.read_cursor :]
            session.read_cursor = len(session.records)
        return session, new

    def stop(self, stream_id: str) -> Session:
        """The kill signal for a session."""
        session = self._get(stream_id)
        with session.lock:
            if session.status == "running":
                session.status = "stopped"
        self._kill(session.process)
        return session

    def list(self) -> list[dict]:
        with self._lock:
            return [s.snapshot() for s in self._sessions.values()]

    def _get(self, stream_id: str) -> Session:
        with self._lock:
            session = self._sessions.get(stream_id)
        if session is None:
            raise KeyError(f"unknown stream_id: {stream_id}")
        return session
