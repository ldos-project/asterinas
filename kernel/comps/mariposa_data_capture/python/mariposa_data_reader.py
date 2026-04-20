# SPDX-License-Identifier: MPL-2.0

"""
A library to parse data written using the `mariposa_data_capture` crate.

This reads the data using the headers in the format defined in the crate and then decodes the
records.
"""

import logging
from pathlib import Path

import cbor2

# This must match the DIRECTORY_BLOCKS constant in `data_capture_device.rs`
BLOCK_SIZE = 4096
DIRECTORY_BLOCKS = 1
DIRECTORY_SIZE = DIRECTORY_BLOCKS * BLOCK_SIZE

# This must match the MAGIC constant in `data_buffering.rs`
MAGIC = b"MARIPOSALDOSDATA\x00"

logger = logging.getLogger(__name__)


class InvalidHeaderError(ValueError):
    pass


def _decode_cbor_values(decoder: cbor2.CBORDecoder, end_pos: int):
    """
    Yield decoded CBOR values from `decoder` until file offset `end_pos`, a `0xff` (the CBOR "break"
    command) is encountered after a message, or an invalid record is encountered.
    """
    fp = decoder.fp
    i = 0
    # Breaks when the stream is over as defined above.
    while True:
        peek_pos = fp.tell()
        peek = fp.read(1)
        fp.seek(peek_pos)
        if peek == b"\xff":
            break
        try:
            record = decoder.decode()
            # Check if we have overrun the file here, since we can't know the record length before
            # reading it.
            if fp.tell() < end_pos:
                yield record
            else:
                break
        except cbor2.CBORDecodeValueError as e:
            # Only print an error if the record completed before the end of the file.
            if fp.tell() < end_pos:
                logger.error(
                    f"CBOR decoding failed at element {i} (at offset {peek_pos}), assuming end of event stream: {e}\n"
                    "(The stream may have been truncated, e.g., by failing to flush or running out of space.)"
                )
            break
        i += 1


class DataCaptureDevice:
    """
    A whole device containing data captured in Mariposa using the `mariposa_data_capture` crate.
    """

    def __init__(self, filename: Path):
        self._filename = filename
        with open(self._filename, "rb") as f:
            decoder = cbor2.CBORDecoder(f)
            self._file_records = list(_decode_cbor_values(decoder, DIRECTORY_SIZE))

    def __len__(self):
        return len(self._file_records)

    def __iter__(self):
        """Iterate over all the files on the device."""
        for record in self._file_records:
            yield DataCaptureFile(self._filename, record)

    def __getitem__(self, index):
        """Get a specific file by its index."""
        return DataCaptureFile(self._filename, self._file_records[index])


class DataCaptureFile:
    """
    A single capture file in a `DataCaptureDevice`.
    """

    def __init__(self, device_filename: Path, record: dict):
        self._device_filename = device_filename
        try:
            if not isinstance(record, dict):
                raise KeyError()
            self._offset = record["offset"]
            self._length = record["length"]
            self._path = record["path"]
        except KeyError:
            raise ValueError(f"Invalid file record: {record}")
        self._header = None

    def _new_decoder(self, f) -> cbor2.CBORDecoder:
        """
        Construct a new decoder on this file. It starts at the beginning of the file header;
        immediately after the magic number.
        """
        f.seek(self._offset)
        magic = f.read(len(MAGIC))
        if magic != MAGIC:
            raise InvalidHeaderError(
                f"Expected file magic at offset {self._offset}, got {magic!r}. "
                f"This generally means the data capture file was not sync()'ed."
            )
        return cbor2.CBORDecoder(f)

    def _get_header(self):
        """
        Get the header of this file.
        """
        if self._header is None:
            with open(self._device_filename, "rb") as f:
                decoder = self._new_decoder(f)
                header = decoder.decode()
            self._header = header
        return self._header

    @property
    def path(self) -> str:
        """The path provided when creating the file in the kernel capture code."""
        return self._path

    @property
    def type_name(self) -> str:
        """The Rust type which was serialized into the file."""
        return self._get_header()["type_name"]

    def __iter__(self):
        """
        Iterate over the records in the capture file.

        The end is either when the file length is reached, the decoder encounters `0xff` instead of
        a record, or an invalid CBOR record is found. `0xff` is the CBOR "break" command, so it can
        never appear as the start of a correct record. An invalid record is treated as a terminator,
        since a truncated file should still be readable. Note that a truncated file can produce an
        oddly formatted record (or records), if the following bytes happen to be valid CBOR.
        """
        end_offset = self._offset + self._length
        with open(self._device_filename, "rb") as f:
            decoder = self._new_decoder(f)
            # skip the header
            decoder.decode()
            yield from _decode_cbor_values(decoder, end_offset)
