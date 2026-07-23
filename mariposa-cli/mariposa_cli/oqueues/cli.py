"""``oqueues`` CLI component — mounted under ``mariposa-cli oqueues …``.

Exposes the same operations as the MCP server (same ``OQ_*`` configuration) as
one-shot subcommands for humans and shell scripts. The stateful MCP session
tools (``stream_start``/``read``/``stop``/``list``) collapse into a single live
``stream`` command, since a one-shot process has no session to poll.

``register(subparsers)`` attaches this component; the umbrella CLI in
``mariposa_cli.cli`` calls it and dispatches to the ``func`` each leaf sets.
"""

import argparse
import json
import sys
import time

from .backend import build_backend
from .frames import jsonify, serialize

# Cadence at which `stream` polls its background drain for new records.
_STREAM_POLL_S = 0.1


def _emit(text: str) -> None:
    """Write serialized output to stdout with exactly one trailing newline. So this function prints the result in your terminal. It's being called by tree, list, and metadata command."""
    if not text:
        return
    sys.stdout.write(text if text.endswith("\n") else text + "\n")

def _cmd_tree(args) -> None:
    _emit(build_backend().oqfs.tree())


def _cmd_list(args) -> None:
    _emit(json.dumps(build_backend().oqfs.list_oqueues(), indent=2))


def _cmd_metadata(args) -> None:
    _emit(build_backend().oqfs.read_metadata(args.oqueue_path))


def _cmd_collect(args) -> None:
    session, records = build_backend().streams.collect(
        args.oqueue_path, max_records=args.max_records, timeout_s=args.timeout
    )
    text = serialize(records, args.format)
    if args.output:
        # Large captures: write straight to a file instead of the terminal.
        with open(args.output, "w", encoding="utf-8") as f:
            f.write(text)
        print(f"wrote {len(records)} records to {args.output}", file=sys.stderr)
    else:
        # If the alt profile is not specified, then print everything to stand out.
        _emit(text)
    if session.status == "error":
        raise RuntimeError(f"stream error: {session.error}")


def _cmd_stream(args) -> None:
    """Live-tail an OQueue, printing each record as newline-delimited JSON.

    Runs until a bound (``--max-records`` / ``--timeout``) is hit, the stream
    closes, or the user interrupts with Ctrl-C — whichever comes first.
    """
    streams = build_backend().streams
    session = streams.start(
        args.oqueue_path, max_records=args.max_records, timeout_s=args.timeout
    )

    def flush() -> None:
        _, new = streams.read(session.stream_id)
        for record in new:
            sys.stdout.write(json.dumps(jsonify(record)) + "\n")
        sys.stdout.flush()

    try:
        while session.thread.is_alive():
            flush()
            time.sleep(_STREAM_POLL_S)
    except KeyboardInterrupt:
        streams.stop(session.stream_id)
    session.thread.join()
    flush()  # anything that arrived between the last poll and completion
    if session.status == "error":
        raise RuntimeError(f"stream error: {session.error}")


def _cmd_serve(args) -> None:
    # Imported lazily so the other subcommands don't pull in the MCP SDK.
    from .server import main as serve

    serve()


def _add_bounds(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--max-records", type=int, default=None, help="stop after N records"
    )
    parser.add_argument(
        "--timeout", type=float, default=None, help="stop after S seconds"
    )


def register(subparsers) -> None:
    """Attach the ``oqueues`` component and its subcommands to a parent parser."""
    oq = subparsers.add_parser(
        "oqueues", help="Inspect the Mariposa OQueue File System."
    )
    sub = oq.add_subparsers(dest="oqueues_command", required=True)

    p = sub.add_parser("tree", help="Print the /oqueues tree (human-readable).")
    p.set_defaults(func=_cmd_tree)

    p = sub.add_parser("list", help="List OQueues as JSON.")
    p.set_defaults(func=_cmd_list)

    p = sub.add_parser("metadata", help="Print an OQueue's metadata.yaml.")
    p.add_argument("oqueue_path", help="absolute or root-relative OQueue path")
    p.set_defaults(func=_cmd_metadata)

    p = sub.add_parser(
        "collect",
        help="Bounded drain to CSV/JSON (needs --max-records or --timeout).",
    )
    p.add_argument("oqueue_path", help="absolute or root-relative OQueue path")
    _add_bounds(p)  # add bounds (num of records or seconds) to the OQueue read
    p.add_argument("--format", choices=["csv", "json"], default="csv")
    p.add_argument(
        "-o",
        "--output",
        metavar="PATH",
        default=None,
        help="write to this file instead of stdout",
    )
    p.set_defaults(func=_cmd_collect)

    p = sub.add_parser(
        "stream",
        help="Live-tail an OQueue as newline-delimited JSON (Ctrl-C to stop).",
    )
    p.add_argument("oqueue_path", help="absolute or root-relative OQueue path")
    _add_bounds(p)
    p.set_defaults(func=_cmd_stream)

    p = sub.add_parser("serve", help="Run the OQueues MCP server over stdio.")
    p.set_defaults(func=_cmd_serve)
