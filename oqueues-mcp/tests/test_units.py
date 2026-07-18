import io

from conftest import cbor_blob

from oqueues_mcp.cbor_stream import iter_records
from oqueues_mcp.frames import serialize, to_frame


def test_iter_records_decodes_concatenated_items():
    blob = cbor_blob([{"a": 1}, {"a": 2}, {"a": 3}])
    got = list(iter_records(io.BytesIO(blob)))
    assert got == [{"a": 1}, {"a": 2}, {"a": 3}]


def test_iter_records_empty_stream():
    assert list(iter_records(io.BytesIO(b""))) == []


def test_iter_records_drops_trailing_truncation():
    blob = cbor_blob([{"a": 1}, {"a": 2}])
    truncated = blob[:-1]  # kill signal mid-item
    got = list(iter_records(io.BytesIO(truncated)))
    assert got == [{"a": 1}]


def test_to_frame_flattens_nested():
    df = to_frame([{"x": {"y": 1}}, {"x": {"y": 2}}])
    assert df.height == 2
    assert "x.y" in df.columns


def test_serialize_csv_and_json():
    recs = [{"a": 1, "b": "z"}, {"a": 2, "b": "w"}]
    csv = serialize(recs, "csv")
    assert "a,b" in csv.splitlines()[0]
    js = serialize(recs, "json")
    assert '"a": 1' in js or '"a":1' in js


def test_serialize_bytes_become_hex():
    out = serialize([{"blob": b"\x01\x02"}], "json")
    assert "0102" in out
