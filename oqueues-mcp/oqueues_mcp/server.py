"""MCP server exposing the OQueue File System to AI agents.

Runs on the host (the agent's domain). The Mariposa guest stays passive: every
tool shells out to a stock guest command over SSH, and all CBOR decoding and
dataframe construction happen here.
"""

import json

import anyio
from mcp.server.fastmcp import FastMCP

from .config import Config
from .frames import serialize
from .oqfs import Oqfs
from .streams import StreamManager
from .transport import Transport

_cfg: Config
_transport: Transport
_oqfs: Oqfs
_streams: StreamManager


def _build() -> None:
    """(Re)build the backend singletons from the current environment.

    Called once at import; re-callable so tests can point the server at a fake
    OQFS after setting env vars.
    """
    global _cfg, _transport, _oqfs, _streams
    _cfg = Config.from_env()
    _transport = Transport(_cfg)
    _oqfs = Oqfs(_cfg, _transport)
    _streams = StreamManager(_transport, _oqfs)


_build()

mcp = FastMCP("oqueues")


# All tools are async: FastMCP runs sync tool functions on the event loop, so
# any blocking work (SSH subprocess, waiting on a drain) would freeze the whole
# server. Each tool offloads its blocking part to a worker thread, keeping the
# loop free to service concurrent streams and other calls.


@mcp.tool()
async def list_tree() -> str:
    """Recursively display the /oqueues hierarchy (human-readable `tree`)."""
    return await anyio.to_thread.run_sync(_oqfs.tree)


@mcp.tool()
async def list_oqueues() -> str:
    """Enumerate OQueues in a machine-readable form.

    Returns JSON: a list of {path, relpath, name} for every OQueue (leaf
    directory containing `strong_observe`). Prefer this over `list_tree` when
    programmatically selecting an OQueue.
    """
    queues = await anyio.to_thread.run_sync(_oqfs.list_oqueues)
    return json.dumps(queues, indent=2)


@mcp.tool()
async def read_metadata(oqueue_path: str) -> str:
    """Read an OQueue's metadata.

    `oqueue_path` may be absolute (e.g. /oqueues/scheduler/events) or relative
    to the OQFS root (e.g. scheduler/events).
    """
    return await anyio.to_thread.run_sync(_oqfs.read_metadata, oqueue_path)


@mcp.tool()
async def stream_collect(
    oqueue_path: str,
    max_records: int | None = None,
    timeout_s: float | None = None,
    fmt: str = "csv",
) -> str:
    """Drain an OQueue's `strong_observe` stream and return a dataframe.

    For bounded modes. Supply `max_records` (mode 1) and/or `timeout_s` (mode 2);
    whichever bound is hit first stops the drain. At least one bound is required
    — for an unbounded stream use `stream_start`.

    The drain runs on its own thread; this call awaits it without blocking the
    server, so other tools (including reads of other streams) stay responsive.
    If the call is cancelled, the underlying stream is stopped.

    Returns the decoded records as CSV (default) or JSON (`fmt="json"`).
    """
    if max_records is None and timeout_s is None:
        raise ValueError(
            "stream_collect needs max_records or timeout_s; "
            "use stream_start for an infinite stream"
        )
    session = _streams.start(oqueue_path, max_records=max_records, timeout_s=timeout_s)
    try:
        while session.thread.is_alive():
            await anyio.sleep(0.1)
    finally:
        # On cancellation (or any early exit) make sure the guest `cat` dies.
        if session.snapshot()["status"] == "running":
            await anyio.to_thread.run_sync(_streams.stop, session.stream_id)
    _, records = _streams.read(session.stream_id)
    return await anyio.to_thread.run_sync(serialize, records, fmt)


@mcp.tool()
async def stream_start(
    oqueue_path: str,
    max_records: int | None = None,
    timeout_s: float | None = None,
) -> str:
    """Begin a streaming session and return its `stream_id` (JSON).

    Returns immediately; the drain runs on its own background thread. Mode is
    inferred: `max_records` -> mode 1, else `timeout_s` -> mode 2, else infinite
    -> mode 3. Poll with `stream_read`; end an infinite stream with `stream_stop`.
    Start as many sessions as you like — they drain concurrently.
    """
    session = await anyio.to_thread.run_sync(
        lambda: _streams.start(
            oqueue_path, max_records=max_records, timeout_s=timeout_s
        )
    )
    return json.dumps(session.snapshot(), indent=2)


@mcp.tool()
async def stream_read(stream_id: str, fmt: str = "csv") -> str:
    """Return records accumulated since the last read for a session (JSON envelope).

    The envelope carries `status`, `active`, `count`, and `data` (CSV or JSON
    records). Keep polling while `active` is true.
    """
    session, records = _streams.read(stream_id)
    snap = session.snapshot()
    data = await anyio.to_thread.run_sync(serialize, records, fmt)
    return json.dumps(
        {
            "stream_id": stream_id,
            "status": snap["status"],
            "active": snap["status"] == "running",
            "count": len(records),
            "records_total": snap["records_total"],
            "error": snap["error"],
            "format": fmt,
            "data": data,
        },
        indent=2,
    )


@mcp.tool()
async def stream_stop(stream_id: str) -> str:
    """Send the kill signal to a streaming session (JSON status).

    Read any final records with `stream_read` afterward.
    """
    session = await anyio.to_thread.run_sync(_streams.stop, stream_id)
    return json.dumps(session.snapshot(), indent=2)


@mcp.tool()
async def stream_list() -> str:
    """List all streaming sessions and their status (JSON)."""
    return json.dumps(_streams.list(), indent=2)


def main() -> None:
    # Speaks MCP over stdio; the MCP client (e.g. Claude CLI) spawns and manages
    # this process.
    mcp.run()


if __name__ == "__main__":
    main()
