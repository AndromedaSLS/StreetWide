"""The Python world-format implementation must reproduce the Rust golden
fixture (`fixtures/format/`) byte for byte."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from tools.worldfmt import cjson, glb, ids, schemas

FORMAT_DIR = Path(__file__).resolve().parents[3] / "fixtures" / "format"
WORLD_DIR = FORMAT_DIR / "world"
BACKSLASH = chr(92)


def expected_world_id() -> str:
    return json.loads((FORMAT_DIR / "expected.json").read_text(encoding="utf-8"))["world_id"]


def json_documents() -> list[Path]:
    return sorted(WORLD_DIR.rglob("*.json"))


def test_fixture_has_expected_files() -> None:
    # 4 manifests, a tile index, 2 points documents (RFC 0004) and the world.
    assert len(json_documents()) == 8
    assert len(sorted(WORLD_DIR.rglob("*.glb"))) == 2


@pytest.mark.parametrize("path", json_documents(), ids=lambda p: p.name[:16])
def test_documents_are_canonical_and_hash_to_their_names(path: Path) -> None:
    data = path.read_bytes()
    value = cjson.loads(data)
    assert cjson.dumps(value) == data
    cid = ids.content_id(data)
    if path.name == "world.json":
        assert cid == expected_world_id()
    else:
        assert ids.hex_of(cid) == path.stem


@pytest.mark.parametrize("path", json_documents(), ids=lambda p: p.name[:16])
def test_documents_match_schemas(path: Path) -> None:
    assert schemas.errors(json.loads(path.read_bytes())) == []


def test_python_glb_writer_matches_rust() -> None:
    specs = json.loads((FORMAT_DIR / "meshes.json").read_text(encoding="utf-8"))
    committed = {p.stem: p.read_bytes() for p in WORLD_DIR.rglob("*.glb")}
    for name, spec in specs.items():
        data = glb.write(glb.nodes_from_spec(spec))
        hex_id = ids.hex_of(ids.content_id(data))
        assert hex_id in committed, f"{name}: Python glb differs from every Rust glb"
        assert committed[hex_id] == data


def test_blake3_known_vector() -> None:
    assert ids.content_id(b"") == (
        "blake3:af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
    )


def test_canonical_round_trip_with_non_ascii_and_escapes() -> None:
    value = {"b": [1, -2, None, True, False], "a": "é " + '"q" ' + BACKSLASH + " ©", "c": {"d": {}}}
    data = cjson.dumps(value)
    expected = (
        '{"a":"é ' + BACKSLASH + '"q' + BACKSLASH + '" ' + BACKSLASH * 2 + ' ©",'
        '"b":[1,-2,null,true,false],"c":{"d":{}}}'
    )
    assert data == expected.encode("utf-8")
    assert cjson.loads(data) == value


@pytest.mark.parametrize(
    "data",
    [
        b'{"a": 1}',
        b'{"b":1,"a":2}',
        b'{"a":1.0}',
        b'{"a":1e3}',
        b'{"a":-0}',
        b"\xef\xbb\xbf" + b'{"a":1}',
        b'{"a":1}' + b"\n",
        b'{"a":1,"a":1}',
        b'{"a":"' + BACKSLASH.encode() + b'u00e9"}',
        b'{"a":"' + BACKSLASH.encode() + b'/"}',
        b'{"a":"x' + BACKSLASH.encode() + b'ny"}',
        b'{"A":1}',
        b'{"a":9007199254740992}',
        b'{"a":-9007199254740992}',
    ],
    ids=[
        "whitespace", "unsorted", "float", "exponent", "negative-zero", "bom",
        "trailing-newline", "duplicate-key", "escaped-non-ascii", "escaped-slash",
        "control-escape", "uppercase-key", "too-large", "too-small",
    ],
)
def test_non_canonical_documents_are_rejected(data: bytes) -> None:
    with pytest.raises(cjson.CanonicalError):
        cjson.loads(data)


def test_milli_format() -> None:
    cases = {0: "0", 7000: "7", 12345: "12.345", -500: "-0.5", -1: "-0.001", 1230: "1.23", -1000: "-1"}
    for k, s in cases.items():
        assert glb.milli_str(k) == s
