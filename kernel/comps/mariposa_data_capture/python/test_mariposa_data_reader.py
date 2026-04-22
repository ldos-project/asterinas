# SPDX-License-Identifier: MPL-2.0

"""Tests for mariposa_data_reader."""

from itertools import zip_longest
import tempfile
import unittest

from contextlib import contextmanager
from pathlib import Path

import cbor2

from mariposa_data_reader import DataCaptureDevice


class TempImage:
    """Helper class for creating a data capture device image for testing."""

    def __init__(self):
        """Initialize the builder with an empty temporary file."""
        self._file = tempfile.NamedTemporaryFile(delete=False)

    def write_bytes(self, data: bytes) -> None:
        """Write raw bytes at the current position in the file."""
        self._file.write(data)

    def write_cbor(self, value) -> None:
        """Write CBOR-encoded values at the current position in the file."""
        cbor2.dump(value, self._file)

    def write_padding(self, target_offset: int) -> None:
        """Write 0xff bytes until reaching the specified offset."""
        current_offset = self._file.tell()
        if target_offset < current_offset:
            raise ValueError("Target offset is before current position")

        remaining = target_offset - current_offset
        self.write_bytes(bytes([0xFF]) * remaining)

    def finalize(self) -> Path:
        """Close the file and return its path."""
        self._file.close()
        return Path(self._file.name)

    def __enter__(self):
        self._file.__enter__()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        Path(self._file.name).unlink()
        return self._file.__exit__(exc_type, exc_val, exc_tb)


class TestDataCaptureDevice(unittest.TestCase):
    @contextmanager
    @staticmethod
    def _test_file():
        with TempImage() as img:
            img.write_cbor({"offset": 0x1000, "length": 0x1000, "path": "test.name"})
            img.write_cbor({"offset": 0x4000, "length": 0x1000, "path": "test.name[2]"})
            img.write_padding(0x1000)
            img.write_bytes(b"MARIPOSALDOSDATA\0")
            img.write_cbor({"name": "test.name", "type_name": "Test", "oqueues": []})
            img.write_cbor({"a": 1, "b": 2})
            img.write_cbor({"a": 1, "b": 2})
            img.write_cbor({"a": 1, "b": 2})
            img.write_padding(0x4000)
            img.write_bytes(b"MARIPOSALDOSDATA\0")
            img.write_cbor(
                {"name": "test.name[2]", "type_name": "Test2", "oqueues": []}
            )
            img.write_cbor({"x": 1, "y": 2})
            img.write_cbor({"x": 1, "y": 2})
            img.write_cbor({"x": 1, "y": 2})
            img.write_padding(0x5000)
            yield img.finalize()

    def test_files(self):
        with TestDataCaptureDevice._test_file() as test_filename:
            device = DataCaptureDevice(test_filename)
            self.assertEqual(len(device), 2)
            self.assertEqual(device[0].path, "test.name")
            self.assertEqual(device[1].path, "test.name[2]")

    def test_no_files(self):
        with TempImage() as img:
            img.write_padding(0x1000)
            device = DataCaptureDevice(img.finalize())
            self.assertEqual(len(device), 0)

    def test_first_file_records(self):
        with TestDataCaptureDevice._test_file() as test_filename:
            device = DataCaptureDevice(test_filename)
            file = device[0]
            self.assertEqual(file.path, "test.name")
            self.assertEqual(file.type_name, "Test")

            expected_records = [{"a": 1, "b": 2}, {"a": 1, "b": 2}, {"a": 1, "b": 2}]

            for record, expected_record in zip_longest(file, expected_records):
                self.assertDictEqual(record, expected_record)

    def test_second_file_records(self):
        with TestDataCaptureDevice._test_file() as test_filename:
            device = DataCaptureDevice(test_filename)
            file = device[1]
            self.assertEqual(file.path, "test.name[2]")
            self.assertEqual(file.type_name, "Test2")

            expected_records = [{"x": 1, "y": 2}, {"x": 1, "y": 2}, {"x": 1, "y": 2}]

            for record, expected_record in zip_longest(file, expected_records):
                self.assertDictEqual(record, expected_record)


if __name__ == "__main__":
    unittest.main()
