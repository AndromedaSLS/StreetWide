"""Fuzzing the world-format readers (roadmap M1): canonical JSON, schema
validation and store reads, on mutated fixtures. Each reader must accept its
input or raise its documented error. See `fuzzing.py` for the knobs; the
regression tests below replay the inputs that used to crash or hang."""

from __future__ import annotations

import json
import os
import time
from pathlib import Path

import pytest
from jsonschema import Draft202012Validator

from tools.worldfmt import cjson, ids, schemas
from tools.worldfmt.store import Store, read_file
from tools.worldfmt.tests.fuzzing import (
    MEMORY,
    Rng,
    mutate_bytes,
    mutate_json,
    mutate_json_bytes,
    reading,
    run_cases,
)

FIXTURES = Path(__file__).resolve().parents[3] / "fixtures"
JSON_FILES = sorted(
    p for p in FIXTURES.rglob("*.json") if "replays" not in p.parts
)
HAND_MADE = [
    b"{}", b"[]", b"null", b"true", b"0", b"-1", b'"a"', b'"\xc3\xa9\\\\\\""',
    b'{"a":[1,-2,null,true,false],"b":{"c":{}}}', b"9007199254740991", b"-9007199254740991",
    b"[" * 127 + b"]" * 127, b'{"a":' * 126 + b"{}" + b"}" * 126,
]
CORPUS = [p.read_bytes() for p in JSON_FILES] + HAND_MADE
VALUES = [json.loads(d) for d in CORPUS]
DOCUMENTS = [v for v in VALUES if isinstance(v, dict) and "format" in v]


def test_corpus_covers_every_document_kind():
    formats = {d["format"] for d in DOCUMENTS}
    assert {"streetwide.asset", "streetwide.tile", "streetwide.world", "streetwide.change", "streetwide.check"} <= formats


def test_fuzz_canonical_json():
    """cjson.loads returns a value only for canonical bytes, else raises
    CanonicalError; what it returns passes through schema validation."""

    def case(rng: Rng) -> None:
        if rng.chance(60):
            data = mutate_json_bytes(rng, rng.choice(VALUES), VALUES, CORPUS)
        else:
            data = mutate_bytes(rng, rng.choice(CORPUS), CORPUS)
        try:
            with reading(len(data)):
                value = cjson.loads(data)
        except cjson.CanonicalError:
            return
        assert cjson.dumps(value) == data
        assert all(isinstance(e, str) for e in schemas.errors(value))

    run_cases("cjson.loads", 2000, case, memory=MEMORY)


def test_the_memory_check_catches_what_a_length_field_reserves():
    # A reader that reserves what a 4-byte length field claims (100 MiB):
    # the harness itself must notice (found by review: it could not).
    def case(rng: Rng) -> None:
        data = (100 << 20).to_bytes(4, "little")
        with reading(len(data)):
            bytearray(int.from_bytes(data, "little"))

    with pytest.raises(AssertionError, match="for an input of 4 bytes"):
        run_cases("memory check", 1, case, memory=MEMORY)


def test_fuzz_schema_validation():
    """schemas.errors returns a list of messages for any parsed JSON value,
    canonical or not."""

    def case(rng: Rng) -> None:
        value = mutate_json(rng, rng.choice(DOCUMENTS), VALUES)
        errors = schemas.errors(value)
        assert isinstance(errors, list) and all(isinstance(e, str) for e in errors)

    run_cases("schemas.errors", 600, case)


def test_fuzz_store_reads(tmp_path):
    """Store reads of crafted ids and crafted world.json files raise
    ValueError (bad id, bad bytes) or OSError (missing), and never touch a
    path outside the store."""
    root = tmp_path / "store"
    store = Store(root)
    good = [store.write_object(d, "json") for d in CORPUS[:5]]
    world = CORPUS[JSON_FILES.index(FIXTURES / "format" / "world" / "world.json")]

    def case(rng: Rng) -> None:
        cid = mutate_json(rng, rng.choice(good), [good, VALUES], edits=1) if rng.chance(50) else rng.choice(good)
        if isinstance(cid, str) and rng.chance(50):
            cid = cid[: rng.below(len(cid) + 1)] + rng.choice(("/", "..", "\\", "0", "g", "\n", "")) + cid[rng.below(len(cid) + 1) :]
        ext = rng.choice(("json", "glb"))
        try:
            path = store.object_path(cid, ext)
        except ValueError:
            pass
        else:
            assert path.parent.parent.parent == root and path.name.startswith(path.parent.name)
        try:
            store.read_object(cid, ext)
        except (ValueError, OSError):
            pass
        if rng.chance(30):
            (root / "world.json").write_bytes(mutate_bytes(rng, world, CORPUS))
            try:
                store.read_world()
            except (ValueError, OSError):
                pass

    run_cases("Store reads", 300, case)


# Regressions: inputs that crashed, hung or disagreed with op_world --------


@pytest.mark.parametrize(
    "data",
    [b"[" * 100_000, b'{"a":' * 3_000 + b"1" + b"}" * 3_000, b"[" * 5_000 + b"]" * 5_000],
    ids=["unclosed", "objects", "closed"],
)
def test_deep_nesting_is_not_canonical(data):
    # Used to raise RecursionError from the json module.
    with pytest.raises(cjson.CanonicalError, match="nested deeper than 127"):
        cjson.loads(data)


def test_nesting_limit_is_serde_jsons():
    # op_world (serde_json, recursion limit 128) accepts 127 levels and
    # refuses 128; Python used to accept up to about 1000.
    for open_, close in ((b"[", b"]"), (b'{"a":', b"}")):
        assert cjson.loads(open_ * 126 + b"[]" + close * 126) is not None
        with pytest.raises(cjson.CanonicalError):
            cjson.loads(open_ * 127 + b"[]" + close * 127)


@pytest.mark.parametrize("data", [b"1" * 4301, b"[-" + b"9" * 5000 + b"]", b'{"a":' + b"1" * 18 + b"}"])
def test_huge_integers_are_not_canonical(data):
    # Thousands of digits used to raise ValueError (int's digit limit).
    with pytest.raises(cjson.CanonicalError, match="outside"):
        cjson.loads(data)


@pytest.mark.parametrize("value", [[], 5, "x", None, True, {"format": []}, {"format": {"a": 1}}, {"format": 1.5}])
def test_schema_errors_for_any_json_value(value):
    # Used to raise AttributeError (not an object) or TypeError (unhashable format).
    errors = schemas.errors(value)
    assert errors and all(isinstance(e, str) for e in errors)


def test_schema_errors_for_deeply_nested_values():
    # jsonschema put the repr of a 5,000-level value in its message:
    # RecursionError. No document may be deeper than canonical JSON allows.
    deep: list = []
    for _ in range(5_000):
        deep = [deep]
    assert schemas.errors({"format": "streetwide.asset", "provenance": deep}) == [
        "arrays and objects nested deeper than 127 levels"
    ]


def test_unique_items_takes_linear_time():
    # jsonschema compared every pair of unsortable items: 60,000 mixed
    # attribution entries took about five minutes.
    items = [0] + [f"credit {i}" for i in range(60_000)] + [[0], {"a": 1}]
    start = time.perf_counter()
    assert list(schemas._unique_items(Draft202012Validator({}), True, items, {})) == []
    assert len(list(schemas._unique_items(Draft202012Validator({}), True, items + [{"a": 1}], {}))) == 1
    assert time.perf_counter() - start < 5


def test_schema_errors_check_unique_items_in_linear_time():
    # The same input through the public path: schemas.errors on a manifest.
    # Testing _unique_items alone did not notice validator() going back to
    # jsonschema's own check (found by review).
    manifest = next(d for d in DOCUMENTS if d["format"] == "streetwide.asset")
    manifest = json.loads(json.dumps(manifest))
    manifest["provenance"]["attribution"] = [0] + [f"credit {i}" for i in range(60_000)] + [[0], {"a": 1}]
    start = time.perf_counter()
    errors = schemas.errors(manifest)
    elapsed = time.perf_counter() - start
    assert errors and not any("non-unique" in e for e in errors)
    assert elapsed < 30, f"{elapsed:.1f} s; jsonschema's own check takes minutes"
    for name in schemas._BY_FORMAT.values():
        assert schemas.validator(name).VALIDATORS["uniqueItems"] is schemas._unique_items


def test_unique_items_agrees_with_jsonschema():
    original = Draft202012Validator({"uniqueItems": True})
    ours = schemas._Validator({"uniqueItems": True})
    rng = Rng(1)
    pool = [0, 1, 1.0, True, False, None, "a", "1", [], [1], [True], {}, {"a": 1}, {"a": 1.0}, {"a": True}, [[0]], [[False]]]
    for _ in range(3000):
        items = [rng.choice(pool) for _ in range(rng.below(5))]
        assert [e.message for e in ours.iter_errors(items)] == [e.message for e in original.iter_errors(items)], items


@pytest.mark.parametrize("value", [5, None, [1], {"a": 1}, b"blake3:" + b"0" * 64])
def test_content_ids_are_checked_for_any_value(value):
    # is_content_id(5) used to raise TypeError from the regex.
    assert not ids.is_content_id(value)
    with pytest.raises(ValueError):
        ids.hex_of(value)


def test_store_reads_only_regular_files_within_a_limit(tmp_path):
    store = Store(tmp_path)
    data = b"{}"
    cid = ids.content_id(data)
    path = store.object_path(cid, "json")
    path.mkdir(parents=True)  # a directory where the object should be
    with pytest.raises(ValueError, match="not a regular file"):
        store.read_object(cid, "json")
    (tmp_path / "big").write_bytes(b"x" * 11)
    with pytest.raises(ValueError, match="larger than 10 bytes"):
        read_file(tmp_path / "big", limit=10)


def test_store_reads_refuse_symbolic_links(tmp_path):
    # A link in a crafted store (git keeps them) could make a read block
    # (/dev/zero), or read and quote a file outside the store.
    (tmp_path / "target.json").write_bytes(b"{}")
    try:
        os.symlink(tmp_path / "target.json", tmp_path / "world.json")
    except (OSError, NotImplementedError):
        pytest.skip("cannot create symbolic links here")
    with pytest.raises(ValueError, match="not a regular file"):
        Store(tmp_path).read_world()


@pytest.mark.skipif(not hasattr(os, "mkfifo"), reason="needs FIFOs")
def test_store_does_not_block_on_a_fifo(tmp_path):
    # A FIFO (or a link to /dev/zero) in a crafted store used to block the
    # read forever (or read without end).
    os.mkfifo(tmp_path / "world.json")
    with pytest.raises(ValueError, match="not a regular file"):
        Store(tmp_path).read_world()
