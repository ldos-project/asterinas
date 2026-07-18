"""Read-only OQFS operations backed by stock guest tools."""

import posixpath
import shlex

from .config import Config
from .transport import Transport


class Oqfs:
    def __init__(self, cfg: Config, transport: Transport):
        self._cfg = cfg
        self._transport = transport

    def tree(self) -> str:
        """Human-readable ``tree`` of the OQFS root."""
        return self._transport.run(f"tree {shlex.quote(self._cfg.root)}")

    def list_oqueues(self) -> list[dict]:
        """Machine-readable enumeration of OQueues.

        An OQueue is a leaf directory containing ``strong_observe``. We find
        those (avoiding GNU-only ``find`` flags) and derive a dotted name from
        the path relative to the root, e.g. ``scheduler/events`` ->
        ``scheduler.events``. The dotted name is derived, not authoritative;
        the definitive name lives in the OQueue's metadata.
        """
        root = self._cfg.root
        out = self._transport.run(f"find {shlex.quote(root)} -name strong_observe")
        queues = []
        for line in out.splitlines():
            line = line.strip()
            if not line.endswith("/strong_observe"):
                continue
            path = line[: -len("/strong_observe")]
            relpath = posixpath.relpath(path, root)
            queues.append(
                {
                    "path": path,
                    "relpath": relpath,
                    "name": relpath.replace("/", "."),
                }
            )
        queues.sort(key=lambda q: q["relpath"])
        return queues

    def read_metadata(self, oqueue_path: str) -> str:
        """Return the OQueue's ``metadata.yaml`` contents."""
        base = self._resolve(oqueue_path)
        path = posixpath.join(base, self._cfg.metadata_file)
        return self._transport.run(f"cat {shlex.quote(path)}")

    def strong_observe_path(self, oqueue_path: str) -> str:
        return posixpath.join(self._resolve(oqueue_path), "strong_observe")

    def _resolve(self, oqueue_path: str) -> str:
        """Accept either an absolute OQFS path or one relative to the root."""
        if oqueue_path.startswith("/"):
            return posixpath.normpath(oqueue_path)
        return posixpath.normpath(posixpath.join(self._cfg.root, oqueue_path))
