"""OQueues component — host-side access to the Mariposa OQueue File System.

Provides two front ends over one shared backend:

* ``server`` — an MCP server for AI agents (``mariposa-oqueues-mcp``).
* ``cli``    — subcommands mounted under ``mariposa oqueues …``.

``server`` is not imported here so the CLI path does not pull in the MCP SDK;
import ``mariposa_cli.oqueues.server`` explicitly (as the console script does).
"""
