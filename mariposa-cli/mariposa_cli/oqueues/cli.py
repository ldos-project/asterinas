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
import os
import sys
from pathlib import Path

from .backend import build_backend
from .serialize import jsonify, serialize
from .streams import Stream


def _emit(text: str) -> None:
    """Print output to the terminal (stdout) with exactly one trailing newline.

    Used by the one-shot commands that emit a single result: tree, list,
    metadata, and collect.
    """
    if not text:
        return
    sys.stdout.write(text if text.endswith("\n") else text + "\n")


def _cmd_tree(args) -> None:
    _emit(build_backend().oqfs.tree())


def _cmd_list(args) -> None:
    _emit(json.dumps(build_backend().oqfs.list_oqueues(), indent=2))


def _cmd_metadata(args) -> None:
    _emit(build_backend().oqfs.read_metadata(args.oqueue_path))


def _build_stream(args) -> Stream:
    backend = build_backend()
    return Stream(
        backend.transport,
        backend.oqfs,
        args.oqueue_path,
        max_records=args.max_records,
        timeout_s=args.timeout,
    )


def _cmd_collect(args) -> None:
    stream = _build_stream(args)
    records = stream.collect()
    text = serialize(records, args.format)
    if args.output:
        Path(args.output).write_text(text, encoding="utf-8")
        print(f"wrote {len(records)} records to {args.output}", file=sys.stderr)
    else:
        _emit(text)
    if stream.status == "error":
        raise RuntimeError(f"stream error: {stream.error}")


def _cmd_stream(args) -> None:
    """Live-tail an OQueue, printing each record as newline-delimited JSON.

    Runs until a bound (``--max-records`` / ``--timeout``) is hit, the stream
    closes, or the user interrupts with Ctrl-C — whichever comes first. Blocks
    on the record queue, so an idle stream costs no CPU.
    """
    stream = _build_stream(args)
    stream.start()

    def emit(record) -> None:
        sys.stdout.write(json.dumps(jsonify(record)) + "\n")
        sys.stdout.flush()

    try:
        for record in stream.iter_live():
            emit(record)
    except KeyboardInterrupt:
        stream.stop()
        # Print anything drained between the interrupt and the stop.
        for record in stream.read():
            emit(record)
    except BrokenPipeError:
        # The downstream reader (e.g. `head`) closed the pipe: stop the drain
        # and exit cleanly. Redirect stdout to /dev/null.
        stream.stop()
        os.dup2(os.open(os.devnull, os.O_WRONLY), sys.stdout.fileno())
        return
    if stream.status == "error":
        raise RuntimeError(f"stream error: {stream.error}")


def _cmd_serve(args) -> None:
    # Imported lazily so the other subcommands don't pull in the MCP SDK.
    from .server import main as serve

    serve()


def _add_bounds(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "-n", "--max-records", type=int, default=None, help="stop after N records"
    )
    parser.add_argument(
        "-t", "--timeout", type=float, default=None, help="stop after S seconds"
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
    _add_bounds(p)
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
