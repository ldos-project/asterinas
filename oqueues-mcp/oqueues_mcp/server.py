"""MCP server exposing the OQueue File System to AI agents.

Runs on the host (the agent's domain). The Mariposa guest stays passive: every
tool shells out to a stock guest command over SSH, and all CBOR decoding and
dataframe construction happen here.
"""

import json
import os

from mcp.server.fastmcp import FastMCP

from .config import Config
from .frames import serialize
from .oqfs import Oqfs
from .streams import StreamManager
from .transport import Transport

_cfg = Config.from_env()
_transport = Transport(_cfg)
_oqfs = Oqfs(_cfg, _transport)
_streams = StreamManager(_transport, _oqfs)

mcp = FastMCP(
    "oqueues",
    host=os.environ.get("OQ_MCP_HOST", "127.0.0.1"),
    port=int(os.environ.get("OQ_MCP_PORT", "8765")),
)


@mcp.tool()
def list_tree() -> str:
    """Recursively display the /oqueues hierarchy (human-readable `tree`)."""
    return _oqfs.tree()


@mcp.tool()
def list_oqueues() -> str:
    """Enumerate OQueues in a machine-readable form.

    Returns JSON: a list of {path, relpath, name} for every OQueue (leaf
    directory containing `strong_observe`). Prefer this over `list_tree` when
    programmatically selecting an OQueue.
    """
    return json.dumps(_oqfs.list_oqueues(), indent=2)


@mcp.tool()
def read_metadata(oqueue_path: str) -> str:
    """Read an OQueue's metadata.

    `oqueue_path` may be absolute (e.g. /oqueues/scheduler/events) or relative
    to the OQFS root (e.g. scheduler/events).
    """
    return _oqfs.read_metadata(oqueue_path)


@mcp.tool()
def stream_collect(
    oqueue_path: str,
    max_records: int | None = None,
    timeout_s: float | None = None,
    fmt: str = "csv",
) -> str:
    """Drain an OQueue's `strong_observe` stream and return a dataframe.

    Blocking, for bounded modes. Supply `max_records` (mode 1) and/or
    `timeout_s` (mode 2); whichever bound is hit first stops the drain. At least
    one bound is required — for an unbounded stream use `stream_start`.

    Returns the decoded records as CSV (default) or JSON (`fmt="json"`).
    """
    if max_records is None and timeout_s is None:
        raise ValueError(
            "stream_collect needs max_records or timeout_s; "
            "use stream_start for an infinite stream"
        )
    session = _streams.start(oqueue_path, max_records=max_records, timeout_s=timeout_s)
    session.thread.join()
    _, records = _streams.read(session.stream_id)
    return serialize(records, fmt)


@mcp.tool()
def stream_start(
    oqueue_path: str,
    max_records: int | None = None,
    timeout_s: float | None = None,
) -> str:
    """Begin a streaming session and return its `stream_id` (JSON).

    Mode is inferred: `max_records` -> mode 1, else `timeout_s` -> mode 2, else
    infinite -> mode 3. Poll with `stream_read`; end an infinite stream with
    `stream_stop`.
    """
    session = _streams.start(oqueue_path, max_records=max_records, timeout_s=timeout_s)
    return json.dumps(session.snapshot(), indent=2)


@mcp.tool()
def stream_read(stream_id: str, fmt: str = "csv") -> str:
    """Return records accumulated since the last read for a session (JSON envelope).

    The envelope carries `status`, `active`, `count`, and `data` (CSV or JSON
    records). Keep polling while `active` is true.
    """
    session, records = _streams.read(stream_id)
    snap = session.snapshot()
    return json.dumps(
        {
            "stream_id": stream_id,
            "status": snap["status"],
            "active": snap["status"] == "running",
            "count": len(records),
            "records_total": snap["records_total"],
            "error": snap["error"],
            "format": fmt,
            "data": serialize(records, fmt),
        },
        indent=2,
    )


@mcp.tool()
def stream_stop(stream_id: str) -> str:
    """Send the kill signal to a streaming session (JSON status).

    Read any final records with `stream_read` afterward.
    """
    session = _streams.stop(stream_id)
    return json.dumps(session.snapshot(), indent=2)


@mcp.tool()
def stream_list() -> str:
    """List all streaming sessions and their status (JSON)."""
    return json.dumps(_streams.list(), indent=2)


def main() -> None:
    # OQ_MCP_TRANSPORT: "stdio" (default, spawned by the MCP client) or
    # "streamable-http" (a long-running background server on OQ_MCP_HOST:PORT).
    transport = os.environ.get("OQ_MCP_TRANSPORT", "stdio")
    mcp.run(transport=transport)


if __name__ == "__main__":
    main()
