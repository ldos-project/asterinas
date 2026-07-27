"""OQueues component — host-side access to the Mariposa OQueue File System.

Provides two front ends over one shared backend:

* ``server`` — an MCP server for AI agents (``mariposa-oqueues-mcp``).
* ``cli``    — subcommands mounted under ``mariposa-cli oqueues …``.

When invoked through the ``mariposa-cli oqueues`` command-line interface, the
``server`` submodule is never imported, so the CLI entry path does not pull in
the MCP SDK. To use the server, import ``mariposa_cli.oqueues.server`` explicitly.
"""
