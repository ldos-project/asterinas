"""Shared backend wiring for the two front ends (MCP server and CLI).

Both the ``server`` module and the ``cli`` module talk to the same stack —
``Config`` -> ``Transport`` -> ``Oqfs`` / ``StreamManager`` — and configure it
from the same ``OQ_*`` environment variables. ``build_backend`` is the single
place that assembles it so the two front ends can never drift apart.
"""

from dataclasses import dataclass

from .config import Config
from .oqfs import Oqfs
from .streams import StreamManager
from .transport import Transport


@dataclass(frozen=True)
class Backend:
    cfg: Config
    transport: Transport
    oqfs: Oqfs
    streams: StreamManager


def build_backend(cfg: Config | None = None) -> Backend:
    """Assemble the backend stack, defaulting the config to the environment."""
    # Fall back to the environment variables when the caller didn't pass a config.
    cfg = cfg or Config.from_env()
    # Build the transport layer (ssh or local).
    transport = Transport(cfg)
    # Build the OQFS proxy over that transport.
    oqfs = Oqfs(cfg, transport)
    streams = StreamManager(transport, oqfs)
    return Backend(cfg=cfg, transport=transport, oqfs=oqfs, streams=streams)
