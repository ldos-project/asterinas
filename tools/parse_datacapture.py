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


with open(sys.argv[1], "rb") as f:
    assert (b := f.read(len(MAGIC))) == MAGIC, b

    json_bytes = b""
    while (b := f.read(1)) != b"\x00":
        json_bytes += b
    metadata = json.loads(json_bytes)

    f.read(64 - f.seek(0, 1) % 64)
    while True:
        print(DtlbMisses.read_record(f))
