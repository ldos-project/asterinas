#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0
"""Minimal OQueue stream receiver."""

import argparse
import datetime
import pathlib
import socket
import sys

import cbor2


def decode_report(path: pathlib.Path) -> None:
    try:
        with path.open("rb") as stream:
            decoder = cbor2.CBORDecoder(stream)
            header = decoder.decode()
            count = 0
            while True:
                try:
                    decoder.decode()
                    count += 1
                except cbor2.CBORDecodeEOF:
                    break
        print(f"  decoded header={header} records={count}", flush=True)
    except Exception as err:
        print(f"  decode failed: {err}", file=sys.stderr, flush=True)


def serve(port: int, out_root: pathlib.Path) -> None:
    out_root.mkdir(parents=True, exist_ok=True)
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("0.0.0.0", port))
    listener.listen(1)
    print(f"listening on :{port}", flush=True)

    while True:
        conn, peer = listener.accept()
        timestamp = datetime.datetime.now().strftime("%Y%m%dT%H%M%S")
        out = out_root / f"stream-{timestamp}-{peer[0]}-{peer[1]}.cbor"
        print(f"connection from {peer} -> {out}", flush=True)
        with conn, out.open("wb") as stream:
            while True:
                chunk = conn.recv(65536)
                if not chunk:
                    break
                stream.write(chunk)
                stream.flush()
        decode_report(out)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=9311)
    parser.add_argument("--out", default="oqueue_dump")
    args = parser.parse_args()
    serve(args.port, pathlib.Path(args.out))


if __name__ == "__main__":
    main()
