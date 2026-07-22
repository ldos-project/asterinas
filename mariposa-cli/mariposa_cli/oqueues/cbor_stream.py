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
from collections.abc import Iterator
from typing import BinaryIO

import cbor2


def _as_buffered(fp: BinaryIO) -> BinaryIO:
    if isinstance(fp, (io.BufferedIOBase, io.BytesIO)):
        return fp
    return io.BufferedReader(fp)  # type: ignore[arg-type]


def iter_records(fp: BinaryIO) -> Iterator[object]:
    """Yields decoded CBOR items until the stream ends.

    A ``CBORDecodeEOF`` at an item boundary is a clean end. If it fires
    mid-item (truncation, e.g. the process was killed between items' bytes),
    that is also treated as end-of-stream — the partial trailing bytes are
    dropped rather than raised, so a kill signal never surfaces as an error.
    """
    decoder = cbor2.CBORDecoder(_as_buffered(fp))
    while True:
        try:
            yield decoder.decode()
        except (cbor2.CBORDecodeEOF, EOFError):
            return
