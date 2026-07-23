#!/usr/bin/env python3
"""Mariposa CLI umbrella — ``mariposa-cli <component> <command> …``.

Each component registers its own subparser (see ``mariposa_cli.oqueues.cli``);
the leaf sets a ``func(args)`` that this module dispatches to, with component
errors turned into a clean nonzero exit rather than a traceback.
"""

import argparse
import sys

from mariposa_cli.oqueues import cli as oqueues_cli


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="mariposa-cli",
        description="Host-side tooling for the Mariposa interlayer.",
    )
    components = parser.add_subparsers(dest="component", required=True)
    oqueues_cli.register(components)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        args.func(args)
    except (RuntimeError, ValueError, KeyError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
