# SPDX-License-Identifier: MPL-2.0

"""
A library to parse data written using the `mariposa_data_capture` crate.

This reads the data using the headers in the format defined in the crate and then decodes and
returns the records directly.
"""

from pathlib import Path

import cbor2

BLOCK_SIZE = 4096
DIRECTORY_BLOCKS = 1
DIRECTORY_SIZE = DIRECTORY_BLOCKS * BLOCK_SIZE

MAGIC = b"MARIPOSALDOSDATA\x00"


class InvalidHeaderError(Exception):
    pass


def _decode_cbor_values(decoder: cbor2.CBORDecoder, end_pos: int):
    """
    Yield decoded CBOR values from *decoder* until *end_pos* is reached in the underlying stream or
    a 0xff (the CBOR "break" command) is encountered after a message.
    """
    fp = decoder.fp
    while fp.tell() < end_pos:
        peek_pos = fp.tell()
        peek = fp.read(1)
        fp.seek(peek_pos)
        if peek == b"\xff":
            break
        yield decoder.decode()


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
    A single capture file in a DataCaptureDevice.
    """

    def __init__(self, device_filename: Path, record: dict):
        self._device_filename = device_filename
        self._offset = record["offset"]
        self._length = record["length"]
        self._path = record["path"]
        self._header = None

    def _new_decoder(self, f) -> cbor2.CBORDecoder:
        """
        Construct a new decoder on this file. It starts at the beginning of the file header.
        """
        f.seek(self._offset)
        magic = f.read(len(MAGIC))
        if magic != MAGIC:
            raise InvalidHeaderError(
                f"Expected file magic at offset {self._offset:#x}, got {magic!r}. "
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

        This stops when either the file is empty or the decoder encounters 0xff instead of a record.
        This is the CBOR2 "break" command, so it can never appear as the start of a correct record.
        """
        end_offset = self._offset + self._length
        with open(self._device_filename, "rb") as f:
            decoder = self._new_decoder(f)
            decoder.decode()  # skip the header
            yield from _decode_cbor_values(decoder, end_offset)
