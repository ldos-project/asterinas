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
        """Enumerate OQueues as ``{path, relpath}`` records.

        An OQueue is a directory containing the metadata file
        (``self._cfg.metadata_file``); every OQueue has one, whereas
        ``strong_observe`` is optional, so matching on metadata future-proofs
        enumeration against OQueues without a strong observer. We find those
        (avoiding GNU-only ``find`` flags) and report each OQueue's directory
        both absolutely (``path``) and relative to the root (``relpath``). Both
        are usable directly as an ``oqueue_path`` argument. The OQueue's
        authoritative name lives in its metadata, not here.
        """
        root = self._cfg.root
        metadata_file = self._cfg.metadata_file
        out = self._transport.run(
            f"find {shlex.quote(root)} -name {shlex.quote(metadata_file)}"
        )
        queues = []
        for line in out.splitlines():
            line = line.strip()
            if posixpath.basename(line) != metadata_file:
                continue
            path = posixpath.dirname(line)
            queues.append(
                {
                    "path": path,
                    "relpath": posixpath.relpath(path, root),
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
