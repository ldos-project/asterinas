"""Turn decoded CBOR records into a serialized dataframe.

Polars is the dataframe engine (Arrow-backed, fast on record streams).
``pl.json_normalize`` flattens nested maps into columns, which CBOR views tend
to produce. Records with bytes/undefined values are coerced to a JSON-safe form
first so serialization never fails on an exotic CBOR type.
"""

import json
from typing import Any

import polars as pl


def _jsonify(value: Any) -> Any:
    """Coerce CBOR-decoded values into JSON/Arrow-friendly Python types."""
    if isinstance(value, dict):
        return {str(k): _jsonify(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [_jsonify(v) for v in value]
    if isinstance(value, (bytes, bytearray)):
        return value.hex()
    return value


def to_frame(records: list[Any]) -> pl.DataFrame:
    rows = [_jsonify(r) for r in records]
    # Wrap non-map records so they still land in a column.
    rows = [r if isinstance(r, dict) else {"value": r} for r in rows]
    if not rows:
        return pl.DataFrame()
    try:
        return pl.json_normalize(rows)
    except Exception:
        # Fallback: keep each record as a JSON string in one column.
        return pl.DataFrame({"record_json": [json.dumps(r) for r in rows]})


def serialize(records: list[Any], fmt: str = "csv") -> str:
    """Serialize records to ``csv`` or ``json`` (list-of-records) text."""
    if fmt == "json":
        return json.dumps([_jsonify(r) for r in records], default=str)
    df = to_frame(records)
    if df.is_empty():
        return ""
    return df.write_csv()
