"""Mariposa CLI — host-side tooling for the Mariposa interlayer.

An umbrella command that dispatches to per-component subcommands. The first
component is ``oqueues`` (the OQueue File System access that also ships an MCP
server); more components mount the same way.

The entry point lives in :mod:`mariposa_cli.cli` (the ``mariposa-cli`` console
script points at ``mariposa_cli.cli:main``).
"""
