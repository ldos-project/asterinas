"""Mariposa CLI — host-side tooling for the Mariposa interlayer.

An umbrella command that dispatches to per-component subcommands. The first
component is ``oqueues`` (the OQueue File System access that also ships an MCP
server); more components mount the same way.
"""

__all__ = ["main"]


def __getattr__(name: str):
    # Lazy re-export so importing the package doesn't eagerly pull in `cli`
    # (which would trigger a double-import warning under `python -m`).
    if name == "main":
        from .cli import main

        return main
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
