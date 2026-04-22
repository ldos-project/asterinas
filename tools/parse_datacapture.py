import argparse
import json
import sys
from typing import IO
from dataclasses import dataclass

MAGIC = b"MARIPOSALDOSDATA\x00"
ENDIANESS = "little"


@dataclass
class DtlbMisses:
    timestamp: int
    miss_l1_tlb: int
    miss_all_tlb: int

    @classmethod
    def read_record(cls, f: IO[bytes]) -> "DtlbMisses":
        timestamp = int.from_bytes(f.read(16), ENDIANESS)
        if timestamp == 0:
            raise EOFError(f"{f.seek(0, 1)}")
        miss_l1_tlb = int.from_bytes(f.read(8), ENDIANESS)
        miss_all_tlb = int.from_bytes(f.read(8), ENDIANESS)
        return DtlbMisses(timestamp, miss_l1_tlb, miss_all_tlb)


@dataclass
class Rss:
    rss: int
    timestamp: int

    @classmethod
    def read_record(cls, f: IO[bytes]) -> "Rss":
        rss = int.from_bytes(f.read(8), ENDIANESS)
        timestamp = int.from_bytes(f.read(16), ENDIANESS)
        if timestamp == 0:
            raise EOFError(f"{f.seek(0, 1)}")
        return Rss(rss, timestamp)


@dataclass
class PageFaultOQueueMessage:
    vm_space_id: int
    address: int
    required_perms: int
    timestamp: int

    def __repr__(self) -> str:
        perm_str = f"{self.required_perms:b}"
        return (
            f"PageFaultOQueueMessage(0x{self.vm_space_id:x}, "
            + f"0x{self.address:x}, {perm_str:0>3}, {self.timestamp})"
        )

    @classmethod
    def read_record(cls, f: IO[bytes]) -> "PageFaultOQueueMessage":
        vm_space_id = int.from_bytes(f.read(8), ENDIANESS)
        address = int.from_bytes(f.read(8), ENDIANESS)
        required_perms = int.from_bytes(f.read(4), ENDIANESS)
        timestamp = int.from_bytes(f.read(16), ENDIANESS)
        if timestamp == 0:
            raise EOFError(f"{f.seek(0, 1)}")
        return PageFaultOQueueMessage(vm_space_id, address, required_perms, timestamp)

@dataclass
class Connect:
    fd: int
    is_close: bool
    timestamp: int

    @classmethod
    def read_record(cls, f: IO[bytes]) -> "Connect":
        fd = int.from_bytes(f.read(4), ENDIANESS)
        is_close = bool(int.from_bytes(f.read(1), ENDIANESS))
        timestamp = int.from_bytes(f.read(16), ENDIANESS)
        if timestamp == 0:
            raise EOFError(f"{f.seek(0, 1)}")
        return Connect(fd, is_close, timestamp)


parser = argparse.ArgumentParser()
parser.add_argument("filename", type=str)
parser.add_argument("type", type=str)

args = parser.parse_args()

types_to_record_type = {
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
    while True:
        print(record_type.read_record(f))
