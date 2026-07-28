"""Shared backend wiring for the two front ends (MCP server and CLI).

Both the ``server`` module and the ``cli`` module talk to the same stack —
``Config`` -> ``Transport`` -> ``Oqfs`` — and configure it from the same
``OQ_*`` environment variables. ``build_backend`` is the single place that
assembles it so the two front ends can never drift apart. Streaming is layered
on top: the CLI builds a single ``Stream`` per invocation, while the MCP server
adds a ``StreamManager`` to track many concurrent sessions.
"""

from dataclasses import dataclass

from .config import Config
from .oqfs import Oqfs
from .transport import Transport


@dataclass(frozen=True)
class Backend:
    cfg: Config
    transport: Transport
    oqfs: Oqfs


def build_backend(cfg: Config | None = None) -> Backend:
    """Assemble the backend stack, defaulting the config to the environment."""
    cfg = cfg or Config.from_env()
    transport = Transport(cfg)
    oqfs = Oqfs(cfg, transport)
    return Backend(cfg=cfg, transport=transport, oqfs=oqfs)
