import argparse
import json
import subprocess
import struct
from abc import abstractmethod
from typing import IO, Union
from dataclasses import dataclass, asdict

import polars as pl

MAGIC = b"MARIPOSALDOSDATA\x00"
ENDIANESS = "little"


def eof(f: IO[bytes]):
    pos = f.seek(0, 1)
    pos = subprocess.check_output(
        ["numfmt", "--to=iec-i", "--suffix=B", str(pos)]
    ).decode()
    raise EOFError(pos)


def timestamp_invalid(ts: int) -> bool:
    return ts == 0 or ts > 99999999999999


class RecordType:
    @classmethod
    @abstractmethod
    def read_record(cls, f: IO[bytes]) -> Union["RecordType", None]: ...

    def asdict(self) -> dict:
        return asdict(self)

    @classmethod
    def to_df(cls, values: list["RecordType"]) -> pl.DataFrame:
        return pl.DataFrame([v.asdict() for v in values])


@dataclass
class DtlbMisses(RecordType):
    timestamp: int
    miss_l1_tlb: int
    miss_all_tlb: int

    @classmethod
    def read_record(cls, f: IO[bytes]) -> "DtlbMisses":
        timestamp = int.from_bytes(f.read(16), ENDIANESS)
        if timestamp_invalid(timestamp):
            eof(f)
        miss_l1_tlb = int.from_bytes(f.read(8), ENDIANESS)
        miss_all_tlb = int.from_bytes(f.read(8), ENDIANESS)
        return DtlbMisses(timestamp, miss_l1_tlb, miss_all_tlb)


@dataclass
class Rss(RecordType):
    rss: int
    timestamp: int

    @classmethod
    def read_record(cls, f: IO[bytes]) -> "Rss":
        # rss = int.from_bytes(f.read(8), ENDIANESS)
        rss = struct.unpack("q", f.read(8))[0]
        timestamp = int.from_bytes(f.read(16), ENDIANESS)
        if timestamp_invalid(timestamp):
            eof(f)
        return Rss(rss, timestamp)


@dataclass
class PageFaultOQueueMessage(RecordType):
    vm_space_id: str
    address: int
    required_perms: int
    timestamp: int

    def __repr__(self) -> str:
        perm_str = f"{self.required_perms:b}"
        return (
            f"PageFaultOQueueMessage(0x{int(self.vm_space_id):x}, "
            + f"0x{self.address:x}, {perm_str:0>3}, {self.timestamp})"
        )

    @classmethod
    def read_record(cls, f: IO[bytes]) -> "PageFaultOQueueMessage":
        vm_space_id = str(int.from_bytes(f.read(8), ENDIANESS))
        address = int.from_bytes(f.read(8), ENDIANESS)
        required_perms = int.from_bytes(f.read(4), ENDIANESS)
        timestamp = int.from_bytes(f.read(16), ENDIANESS)
        if timestamp_invalid(timestamp):
            eof(f)
        return PageFaultOQueueMessage(vm_space_id, address, required_perms, timestamp)


@dataclass
class Connect(RecordType):
    fd: int
    is_close: bool
    timestamp: int

    @classmethod
    def read_record(cls, f: IO[bytes]) -> Union["Connect", None]:
        fd = int.from_bytes(f.read(4), ENDIANESS)
        is_close = bool(int.from_bytes(f.read(1), ENDIANESS))
        timestamp = int.from_bytes(f.read(16), ENDIANESS)
        if timestamp_invalid(timestamp):
            eof(f)

        # We only care about fd=22 as a proxy for YCSB starting/stopping
        is_load_start = fd == 7 and is_close == False
        is_run_end = fd == 22 and is_close == True
        if not (is_load_start or is_run_end):
            return None
        return Connect(fd, is_close, timestamp)

    @classmethod
    def to_df(cls, values: list["RecordType"]) -> pl.DataFrame:
        return pl.DataFrame([v.asdict() for i, v in enumerate(values) if i % 3 != 1])


parser = argparse.ArgumentParser()
parser.add_argument("filename", type=str)
parser.add_argument("type", type=str)
parser.add_argument("-o", "--output", type=str)

args = parser.parse_args()

types_to_record_type: dict[str, type[RecordType]] = {
    "connect": Connect,
    "pagefault": PageFaultOQueueMessage,
    "pmu": DtlbMisses,
    "rss": Rss,
}

record_type = types_to_record_type[args.type]

with open(args.filename, "rb") as f:
    assert (b := f.read(len(MAGIC))) == MAGIC, b

    json_bytes = b""
    while (b := f.read(1)) != b"\x00":
        json_bytes += b
    metadata = json.loads(json_bytes)

    f.read(64 - f.seek(0, 1) % 64)
    values: list[RecordType] = []
    while True:
        try:
            v = record_type.read_record(f)
            if v:
                values.append(v)
        except EOFError:
            break
    df = record_type.to_df(values)
    print(df)
    if args.output:
        df.write_parquet(args.output)
