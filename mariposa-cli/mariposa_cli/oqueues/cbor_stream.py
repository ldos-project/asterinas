"""Incremental CBOR decoding of a ``strong_observe`` byte stream.

``strong_observe`` yields back-to-back CBOR items with no framing. ``cbor2``'s
decoder reads exactly the bytes each item needs, so we just call ``decode()`` in
a loop and stop at end-of-stream.

The decoder is driven from a buffered file object. That matters for pipes: a
buffered reader's ``read(n)`` loops internally on a non-interactive stream until
it has ``n`` bytes or hits a real EOF, so a short pipe read never truncates a
CBOR item mid-decode. ``subprocess`` stdout (bufsize=-1) and ``io.BytesIO`` are
both already buffered; anything else is wrapped.
"""

import io
import sys
from collections.abc import Iterator
from typing import BinaryIO

import cbor2


def _as_buffered(fp: BinaryIO) -> BinaryIO:
    if isinstance(fp, (io.BufferedIOBase, io.BytesIO)):
        return fp
    return io.BufferedReader(fp)


class _CountingReader:
    """Wraps a binary reader and counts bytes consumed, so a mid-item EOF
    (truncation) can be told apart from a clean end at an item boundary."""

    def __init__(self, fp: BinaryIO):
        self._fp = fp
        self.count = 0

    def read(self, size: int = -1) -> bytes:
        data = self._fp.read(size)
        self.count += len(data)
        return data

    def readable(self) -> bool:
        return True

    def __getattr__(self, name):
        # Delegate any other file-like probe (seekable, closed, …) to the
        # wrapped reader.
        return getattr(self._fp, name)


def iter_records(fp: BinaryIO) -> Iterator[object]:
    """Yields decoded CBOR items until the stream ends.

    An end-of-stream at an item boundary is a clean end and stops silently. If
    it fires mid-item (truncation, e.g. the process was killed between an item's
    bytes), the partial trailing bytes are dropped rather than raised — so a
    kill signal never surfaces as an error — but a warning is printed to stderr
    so the truncation isn't mistaken for a clean end.
    """
    reader = _CountingReader(_as_buffered(fp))
    decoder = cbor2.CBORDecoder(reader)
    boundary = 0
    while True:
        try:
            item = decoder.decode()
        except (cbor2.CBORDecodeEOF, EOFError):
            if reader.count > boundary:
                # Bytes were consumed past the last complete item: the stream
                # was cut mid-record rather than ending cleanly.
                print(
                    "warning: stream truncated mid-record; "
                    "dropped trailing partial bytes",
                    file=sys.stderr,
                )
            return
        boundary = reader.count
        yield item
