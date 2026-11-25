"""The strict glb reader in tools/worldfmt/glb.py, against op_world's:
every committed mesh reads, what the writer writes reads back bit for bit,
anything outside the profile is refused, and the terrain-edge rule counts
what the gate's G8 counts."""

from __future__ import annotations

import json
import math
import re
import shutil
import struct
from fractions import Fraction
from pathlib import Path

import pytest

from tools.worldfmt import changes, cjson, glb
from tools.worldfmt.ids import content_id
from tools.worldfmt.points import POINT_LAYERS
from tools.worldfmt.store import Store

FIXTURES = Path(__file__).resolve().parents[3] / "fixtures"
FORMAT_DIR = FIXTURES / "format"
WORLD = Store(FIXTURES / "world")
TERRAIN_TILE = "17/68646/45903"


def f32_bits(value: float) -> int:
    return struct.unpack("<I", struct.pack("<f", value))[0]


def committed_glbs() -> list[Path]:
    return sorted(FIXTURES.rglob("*.glb"))


def test_fixtures_hold_meshes() -> None:
    # The format fixture, the ingest fixture and the facade sample; the
    # next test reads every one of their meshes.
    folders = {"/".join(p.relative_to(FIXTURES).parts[:2]) for p in committed_glbs()}
    assert {"format/world", "world/objects", "changes/facade-sample"} <= folders


@pytest.mark.parametrize("path", committed_glbs(), ids=lambda p: p.stem[:12])
def test_every_committed_mesh_reads_and_rewrites(path: Path) -> None:
    data = path.read_bytes()
    mesh = glb.read(data)
    assert glb.write(mesh.to_nodes()) == data
    for p in mesh.primitives():
        assert len(p.positions) == len(p.position_bits) == len(p.positions_mm) == len(p.normals)
        for v, bits, mm in zip(p.positions, p.position_bits, p.positions_mm):
            assert bits == tuple(f32_bits(c) for c in v)
            assert bits == tuple(glb.mm_to_f32_bits(k) for k in mm)
        assert len(p.indices) % 3 == 0 and max(p.indices) < len(p.positions)


def test_mesh_bounds_and_sizes_match_the_fixture_manifests() -> None:
    # What `op_world validate` checks: the manifest's bounds are the mesh's.
    # The point layers hold points documents (RFC 0004), not meshes:
    # tools/ingest/tests/test_fixture_world.py checks those.
    _, world = WORLD.read_world()
    index = cjson.loads(WORLD.read_object(world["tiles"][0]["tile_id"], "json"))
    meshes = [a for a in index["assets"] if a["layer"] not in POINT_LAYERS]
    assert len(meshes) == 45
    for a in meshes:
        manifest = cjson.loads(WORLD.read_object(a["manifest_id"], "json"))
        data = WORLD.read_object(manifest["content_id"], "glb")
        mesh = glb.read(data)
        assert len(data) == manifest["size_bytes"]
        assert mesh.bounds_mm() == manifest["bounds_mm"], a["feature_id"]


def test_reads_back_what_the_writer_writes() -> None:
    specs = json.loads((FORMAT_DIR / "meshes.json").read_text(encoding="utf-8"))
    for name, spec in specs.items():
        nodes = glb.nodes_from_spec(spec)
        data = glb.write(nodes)
        mesh = glb.read(data)
        assert [n.name for n in mesh.nodes] == [n.name for n in nodes], name
        wanted = [p for n in nodes for p in n.primitives]
        for got, want in zip(mesh.primitives(), wanted, strict=True):
            assert got.material == want.material
            assert list(got.positions_mm) == [tuple(v) for v in want.positions_mm]
            assert [tuple(f32_bits(c) for c in v) for v in got.normals] == [
                tuple(f32_bits(c) for c in v) for v in want.normals
            ]
            assert list(got.indices) == want.indices
        assert glb.write(mesh.to_nodes()) == data


def two_materials() -> list[glb.Node]:
    """Negative and large coordinates, two nodes, two materials."""
    quad = [(-123456, 7, -1), (987654, -8000000, 2), (5, 6, 8000001), (-1, -1, -1)]
    return [
        glb.Node("osm:way:1", [
            glb.Primitive("road", quad, [(0.0, 1.0, 0.0)] * 4, [0, 1, 2, 2, 1, 3]),
            glb.Primitive("water", quad[:3], [(-0.0, 0.5, 1e-40)] * 3, [0, 2, 1]),
        ]),
        glb.Node("tile:17/1/2", [glb.Primitive("roof", quad[1:], [(0.25, -1.0, 3.0)] * 3, [2, 1, 0])]),
    ]


def test_synthetic_mesh_round_trips_bit_for_bit() -> None:
    nodes = two_materials()
    data = glb.write(nodes)
    mesh = glb.read(data)
    assert glb.write(mesh.to_nodes()) == data
    road, water, roof = mesh.primitives()
    assert road.positions_mm[1] == (987654, -8000000, 2)
    assert road.position_bits[1] == tuple(glb.mm_to_f32_bits(k) for k in (987654, -8000000, 2))
    assert f32_bits(water.normals[0][0]) == 0x80000000, "-0.0 normals keep their sign"
    assert f32_bits(water.normals[0][2]) == f32_bits(1e-40), "subnormal normals survive"
    assert roof.indices == (2, 1, 0)
    assert mesh.triangle_count() == 4
    assert mesh.bounds_mm() == {"max": [987654, 7, 8000001], "min": [-123456, -8000000, -1]}
    assert list(mesh.triangles_mm())[1] == ((5, 6, 8000001), (987654, -8000000, 2), (-1, -1, -1))


def exact_f32_to_mm(bits: int) -> int | None:
    """op_world's `f32_to_mm`, with the rounding done in exact fractions."""
    value = struct.unpack("<f", struct.pack("<I", bits))[0]
    if not math.isfinite(value):
        return None
    x = Fraction(value * 1000.0)  # the same double product as in Rust
    half = Fraction(1, 2)
    mm = math.floor(x + half) if x >= 0 else -math.floor(-x + half)
    if abs(mm) > 9 * 10**15:
        return None
    return mm if f32_bits(mm / 1000) == bits else None


def test_snapping_mirrors_op_world() -> None:
    # crates/op_world/src/glb.rs, test `snapping`.
    assert glb.f32_to_mm(glb.mm_to_f32_bits(12345)) == 12345
    assert glb.f32_to_mm(glb.mm_to_f32_bits(-7)) == -7
    assert glb.f32_to_mm(f32_bits(0.0005)) is None
    assert glb.f32_to_mm(0x80000000) is None  # -0.0
    assert glb.f32_to_mm(0x7FC00000) is None  # NaN
    assert glb.f32_to_mm(0x7F800000) is None  # +inf
    assert glb.f32_to_mm(0) == 0
    for mm in [0, 1, -1, 999, 1000, 1001, 123456, -654321, 8_388_608, 16_777_217, -2_147_774]:
        assert glb.f32_to_mm(glb.mm_to_f32_bits(mm)) == mm


def test_rounding_is_exact_ties_away_from_zero() -> None:
    cases = [0.5, -0.5, 1.5, -1.5, 2.5, -2.5, 0.49999999999999994, -0.49999999999999994,
             0.5 - 2**-54, -(0.5 - 2**-54), 0.25 + 2**-54, -(0.25 + 2**-54), 2.0**51 + 0.5,
             -(2.0**51 + 0.5), 2.0**53, 1e300, -1e300, 0.0, -0.0]
    for x in cases:
        q = Fraction(x)
        expected = math.floor(q + Fraction(1, 2)) if q >= 0 else -math.floor(-q + Fraction(1, 2))
        assert glb._round_half_away(x) == expected, x


def test_f32_to_mm_agrees_with_exact_rounding_on_a_spread_of_bit_patterns() -> None:
    # A fixed spread over all 2^32 patterns, and the patterns at and next
    # to every 7th millimetre between -2 m and 2 m.
    patterns = [(i * 2654435761) & 0xFFFFFFFF for i in range(20000)]
    for mm in range(-2000, 2001, 7):
        b = glb.mm_to_f32_bits(mm)
        patterns += [b - 1, b, b + 1]
    for bits in patterns:
        assert glb.f32_to_mm(bits) == exact_f32_to_mm(bits), hex(bits)
        # `read` snaps a primitive's coordinates in one batch.
        value = struct.unpack("<f", struct.pack("<I", bits))[0]
        one = glb.f32_to_mm(bits)
        assert glb._snapped_mm([value], [bits]) == (None if one is None else [one]), hex(bits)
    good = [b for b in patterns if glb.f32_to_mm(b) is not None]
    values = [struct.unpack("<f", struct.pack("<I", b))[0] for b in good]
    assert glb._snapped_mm(values, good) == [glb.f32_to_mm(b) for b in good]
    assert glb._snapped_mm(values + [0.0005], good + [f32_bits(0.0005)]) is None


# ------------------------------------------------------ outside the profile --


def house_bytes() -> bytes:
    specs = json.loads((FORMAT_DIR / "meshes.json").read_text(encoding="utf-8"))
    return glb.write(glb.nodes_from_spec(specs["house"]))


def json_len(data: bytes) -> int:
    return struct.unpack_from("<I", data, 12)[0]


def json_of(data: bytes) -> dict:
    return json.loads(data[20 : 20 + json_len(data)])


def with_json(data: bytes, doc_bytes: bytes) -> bytes:
    """`data` with its JSON chunk replaced and the lengths fixed up."""
    bin_part = data[20 + json_len(data) :]
    padded = doc_bytes + b" " * (-len(doc_bytes) % 4)
    out = bytearray(data[:12] + struct.pack("<I", len(padded)) + data[16:20] + padded + bin_part)
    out[8:12] = struct.pack("<I", len(out))
    return bytes(out)


def edited_json(data: bytes, edit) -> bytes:
    doc = json_of(data)
    edit(doc)
    return with_json(data, json.dumps(doc, separators=(",", ":")).encode())


def bin_start(data: bytes) -> int:
    return 20 + json_len(data) + 8


def flipped(data: bytes, at: int, value: bytes) -> bytes:
    out = bytearray(data)
    out[at : at + len(value)] = value
    return bytes(out)


def corrupted_cases() -> list[tuple[str, bytes, str]]:
    good = house_bytes()
    doc = json_of(good)
    first_index = bin_start(good) + doc["bufferViews"][doc["accessors"][2]["bufferView"]]["byteOffset"]
    first_normal = bin_start(good) + doc["bufferViews"][doc["accessors"][1]["bufferView"]]["byteOffset"]
    unsnapped = bytearray(good)
    unsnapped[bin_start(good)] ^= 1  # lowest mantissa bit of the first position
    return [
        # The cases of crates/op_world/tests/glb_profile.rs.
        ("truncated", good[:-4], "header length does not match file size"),
        ("trailing data", good + b"\0\0\0\0", "header length does not match file size"),
        ("bad magic", b"x" + good[1:], "not a glTF 2.0 binary file"),
        ("unsnapped position", bytes(unsnapped), "positions must be snapped to millimetres"),
        ("pretty JSON", with_json(good, json.dumps(doc, indent=2).encode()), "not in the canonical"),
        ("material outside palette",
         edited_json(good, lambda d: d["materials"][0].update(name="marble")), "material is not in the palette"),
        ("external buffer uri",
         edited_json(good, lambda d: d["buffers"][0].update(uri="data:application/octet-stream;base64,")),
         "not in the canonical"),
        ("count beyond buffer",
         edited_json(good, lambda d: d["accessors"][0].update(count=1_000_000)),
         "accessor count does not match its buffer view"),
        # More of the profile.
        ("too short", good[:10], "truncated file"),
        ("glTF version 1", good[:4] + struct.pack("<I", 1) + good[8:], "not a glTF 2.0 binary file"),
        ("second chunk not BIN", flipped(good, bin_start(good) - 4, b"XXXX"), "second chunk must be BIN"),
        ("node transform",
         edited_json(good, lambda d: d["nodes"][0].update(translation=[1, 0, 0])), "not in the canonical"),
        ("extension", edited_json(good, lambda d: d.update(extensionsUsed=["KHR_x"])), "not in the canonical"),
        ("node name not a feature id",
         edited_json(good, lambda d: d["nodes"][0].update(name="my house")), "is not a feature id"),
        ("materials out of palette order",
         edited_json(good, lambda d: d["materials"].reverse()), "distinct materials in palette order"),
        ("missing materials", edited_json(good, lambda d: d.pop("materials")), "missing materials"),
        # Node i must show mesh i (op_world checks it before the reference).
        ("bad mesh reference", edited_json(good, lambda d: d["nodes"][0].update(mesh=9)), "node 0 must show mesh 0"),
        ("float index", edited_json(good, lambda d: d["nodes"][0].update(mesh=0.0)), "node 0 must show mesh 0"),
        ("no meshes", edited_json(good, lambda d: d.update(meshes=[])), "bad reference into meshes"),
        ("indices as floats",
         edited_json(good, lambda d: d["accessors"][2].update(componentType=5126)), "accessor must be SCALAR"),
        ("NaN normal", flipped(good, first_normal, struct.pack("<I", 0x7FC00000)), "normals must be finite"),
        ("index out of range", flipped(good, first_index, struct.pack("<I", 10**6)), "out of range"),
        ("JSON not UTF-8", with_json(good, b'{"a":"\xff"}'), "not valid JSON"),
        ("JSON NaN", with_json(good, b'{"materials":NaN}'), "NaN"),
        ("JSON not an object", with_json(good, b"[]"), "missing materials"),
    ]


@pytest.mark.parametrize("name,data,message", corrupted_cases(), ids=[c[0] for c in corrupted_cases()])
def test_files_outside_the_profile_are_refused(name: str, data: bytes, message: str) -> None:
    with pytest.raises(glb.GlbError, match=re.escape(message)):
        glb.read(data)


def test_the_corrupted_cases_start_from_a_good_file() -> None:
    glb.read(house_bytes())


# ---------------------------------------- the rules of op_world's fuzzing --
#
# op_world's reader gained four rules when it was fuzzed (main, gate
# 0.1.2): a JSON chunk of at most MAX_JSON_BYTES, checked before it is
# parsed; at most cjson.MAX_DEPTH levels of nesting, also checked before
# parsing; node i shows mesh i; the accessors decode at most the BIN
# chunk's size. The Python reader applies them in the same order, with the
# same messages. op_world reports the nesting as "not canonical JSON: ..."
# (Error::NotCanonical) and the others as "invalid glb: ..." (Error::Glb).

TOO_DEEP = "not canonical JSON: arrays and objects nested deeper than 127 levels"
NOT_CANONICAL = "not in the canonical StreetWide glb form (re-writing it changes the bytes)"


def glb_pack(json_chunk: bytes, bin_chunk: bytes) -> bytes:
    """A glb of these two chunks, with consistent lengths and the JSON
    padded with spaces (`glb_pack` in crates/op_world/tests/fuzzing)."""
    j = json_chunk + b" " * (-len(json_chunk) % 4)
    total = 12 + 8 + len(j) + 8 + len(bin_chunk)
    return b"".join([
        struct.pack("<III", 0x46546C67, 2, total), struct.pack("<II", len(j), 0x4E4F534A), j,
        struct.pack("<II", len(bin_chunk), 0x004E4942), bin_chunk,
    ])


def shared_glb(nodes: int, vertices: int, shared_mesh: bool) -> bytes:
    """`shared_glb` of crates/op_world/tests/fuzz_readers.rs: `nodes` nodes,
    each showing one primitive of `vertices` vertices (at the origin) and
    one triangle, valid but for one kind of sharing. With `shared_mesh`,
    every node shows mesh 0. Otherwise every node has a mesh of its own,
    but all of them read one POSITION accessor, as their NORMAL too."""
    floats = 12 * vertices

    def vec3(view: int) -> dict:
        return {"bufferView": view, "componentType": 5126, "count": vertices, "type": "VEC3"}

    def indices(view: int) -> dict:
        return {"bufferView": view, "componentType": 5125, "count": 3, "type": "SCALAR"}

    def view(offset: int, length: int) -> dict:
        return {"byteLength": length, "byteOffset": offset}

    def mesh(position: int, normal: int, index: int) -> dict:
        prim = {"attributes": {"NORMAL": normal, "POSITION": position}, "indices": index, "material": 0, "mode": 4}
        return {"primitives": [prim]}

    if shared_mesh:
        accessors, views = [vec3(0), vec3(1), indices(2)], [view(0, floats), view(floats, floats), view(2 * floats, 12)]
        meshes, bin_len = [mesh(0, 1, 2)], 2 * floats + 12
    else:
        accessors, views = [vec3(0), indices(1)], [view(0, floats), view(floats, 12)]
        meshes, bin_len = [mesh(0, 0, 1)] * nodes, floats + 12
    doc = {
        "accessors": accessors, "bufferViews": views, "materials": [{"name": "road"}], "meshes": meshes,
        "nodes": [{"mesh": 0 if shared_mesh else i, "name": "a:b"} for i in range(nodes)],
    }
    return glb_pack(json.dumps(doc, separators=(",", ":"), sort_keys=True).encode(), bytes(bin_len))


def nested_list(levels: int) -> list:
    """A list nested `levels` deep: [] is one level."""
    value: list = []
    for _ in range(levels - 1):
        value = [value]
    return value


def padded_json(data: bytes, size: int) -> bytes:
    """`data` with its JSON chunk padded with spaces to `size` bytes."""
    chunk = data[20 : 20 + json_len(data)].rstrip(b" ")
    return with_json(data, chunk + b" " * (size - len(chunk)))


def json_of_bytes(data: bytes) -> bytes:
    return data[20 : 20 + json_len(data)].rstrip(b" ")


def stricter_cases() -> list[tuple[str, bytes, str]]:
    """(id, file, message): files that break, or just keep, the four rules."""
    good = house_bytes()
    two = glb.write(two_materials())
    long_array = b'{"a":[' + b"0," * 600_000 + b"0]}"  # crates/op_world/tests/fuzz_readers.rs
    escaped_backslash = b'{"a":"\\\\","b":' + b"[" * 127 + b"]" * 127 + b"}"  # the string ends after \\
    swapped = edited_json(two, lambda d: (d["nodes"][0].update(mesh=1), d["nodes"][1].update(mesh=0)))

    def read_twice(d: dict) -> None:  # the roof's normals read the roof's positions again
        d["accessors"][1]["bufferView"] = d["accessors"][0]["bufferView"]

    return [
        ("json-too-long", glb_pack(long_array, b""), "JSON chunk longer than 1048576 bytes"),
        ("json-at-the-limit", padded_json(good, glb.MAX_JSON_BYTES), NOT_CANONICAL),
        ("json-a-word-over", padded_json(good, glb.MAX_JSON_BYTES + 4), "JSON chunk longer than 1048576 bytes"),
        # The length is checked before the nesting, which is checked before parsing.
        ("json-too-long-and-too-deep", glb_pack(b"[" * (glb.MAX_JSON_BYTES + 4), b""),
         "JSON chunk longer than 1048576 bytes"),
        ("depth-128", edited_json(good, lambda d: d.update(extras=nested_list(127))), TOO_DEEP),
        ("depth-127", edited_json(good, lambda d: d.update(extras=nested_list(126))), NOT_CANONICAL),
        ("depth-unclosed", with_json(good, b"[" * 128), TOO_DEEP),
        ("depth-before-utf8", with_json(good, b"[" * 128 + b"\xff"), TOO_DEEP),
        ("depth-closing-first", with_json(good, b"]}" * 5 + b"[" * 128), TOO_DEEP),
        ("depth-in-strings", edited_json(good, lambda d: d.update(extras='"' + "[" * 300)), NOT_CANONICAL),
        ("depth-after-escapes", with_json(good, escaped_backslash), TOO_DEEP),
        ("nodes-share-a-mesh", shared_glb(2, 1, True), "node 1 must show mesh 1"),
        ("nodes-swap-meshes", swapped, "node 0 must show mesh 0"),
        ("node-shows-mesh-minus-0", with_json(good, json_of_bytes(good).replace(b'"mesh":0', b'"mesh":-0')),
         "node 0 must show mesh 0"),
        ("accessor-read-twice", shared_glb(1, 1, False), "accessors read more data than the BIN chunk holds"),
        ("view-read-twice-within-the-chunk", edited_json(good, read_twice), NOT_CANONICAL),
        ("view-overflows", edited_json(good, lambda d: d["bufferViews"][0].update(byteOffset=2**64 - 1)),
         "buffer view overflows"),
        ("view-past-the-chunk", edited_json(good, lambda d: d["bufferViews"][0].update(byteOffset=len(good))),
         "buffer view outside the BIN chunk"),
        # An integer beyond u64 that a float can hold: serde_json reads it as
        # one, which is no index.
        ("mesh-a-301-digit-integer", with_json(good, json_of_bytes(good).replace(b'"mesh":0', b'"mesh":1' + b"0" * 300)),
         "node 0 must show mesh 0"),
        *named_cases(good),
    ]


QUOTED_NAMES = [
    # (id, the name as JSON escapes it, the name as op_world quotes it)
    ("control", b"a\\u0001b", '"a\\u{1}b"'),
    ("nul", b"a\\u0000b", '"a\\0b"'),
    ("backspace", b"a\\bb", '"a\\u{8}b"'),
    ("form-feed", b"a\\fb", '"a\\u{c}b"'),
    ("tab", b"a\\tb", '"a\\tb"'),
    ("quote-and-backslash", b'a\\"b\\\\c', '"a\\"b\\\\c"'),
    ("delete", "a\u007fb".encode(), '"a\\u{7f}b"'),
    ("next-line", "a\u0085b".encode(), '"a\\u{85}b"'),
    ("soft-hyphen", "a­b".encode(), '"a\\u{ad}b"'),
    ("zero-width-space", "a​b".encode(), '"a\\u{200b}b"'),
    ("byte-order-mark", "a﻿b".encode(), '"a\\u{feff}b"'),
    ("line-separator", "a b".encode(), '"a\\u{2028}b"'),
    ("leading-combining-mark", "́a".encode(), '"\\u{301}a"'),
    ("accented-letter", "été".encode(), '"été"'),
]
"""Node names that are no feature ids, which op_world quotes with Rust's
`{:?}`. Found by review: the Python reader quoted them with json.dumps, so
11 of these read differently."""


def named_cases(good: bytes) -> list[tuple[str, bytes, str]]:
    chunk = json_of_bytes(good)
    name = b'"name":"streetwide:sample-house"'
    return [(f"name-{case}", with_json(good, chunk.replace(name, b'"name":"' + escaped + b'"')),
             f"node name {quoted} is not a feature id") for case, escaped, quoted in QUOTED_NAMES]


def out_of_range_cases() -> list[tuple[str, bytes]]:
    """(id, file): numbers that no f64 holds. serde_json refuses the whole
    document as invalid JSON ("number out of range"); Python's json read
    1e400 as infinity and a 400-digit integer as an integer, and the reader
    refused the file later, for another reason. Found by review."""
    good = house_bytes()
    chunk = json_of_bytes(good)
    return [
        (case, with_json(good, edit(chunk)))
        for case, edit in (
            ("mesh-1e400", lambda c: c.replace(b'"mesh":0', b'"mesh":1e400')),
            ("mesh-minus-1e400", lambda c: c.replace(b'"mesh":0', b'"mesh":-1e400')),
            ("mesh-a-401-digit-integer", lambda c: c.replace(b'"mesh":0', b'"mesh":1' + b"0" * 400)),
            ("unknown-field-1e400", lambda c: c[:-1] + b',"zz":1e400}'),
        )
    ]


@pytest.mark.parametrize("case,data", out_of_range_cases(), ids=[c[0] for c in out_of_range_cases()])
def test_numbers_no_float_holds_are_invalid_json(case: str, data: bytes) -> None:
    with pytest.raises(glb.GlbError) as e:
        glb.read(data)
    assert str(e.value) == "the JSON chunk is not valid JSON: number out of range"


def accepted_meshes() -> list[tuple[str, bytes]]:
    """(id, file): meshes inside the profile whose JSON chunks hold
    brackets, quotes and backslashes in strings, which the depth rule must
    skip."""
    tri = [(0, 0, 0), (1000, 0, 0), (0, 0, 1000)]

    def one(name: str) -> bytes:
        return glb.write([glb.Node(name, [glb.Primitive("road", tri, [(0.0, 1.0, 0.0)] * 3, [0, 2, 1])])])

    return [
        ("brackets-in-a-name", one("a:" + "[" * 200)),
        ("quotes-and-backslashes-in-a-name", one('a:\\"' + "{" * 150 + '\\')),
    ]


def lone_surrogate_cases() -> list[tuple[str, bytes]]:
    """(id, file): node names that JSON spells with an unpaired surrogate.
    serde_json refuses them as invalid JSON; Python's json reads them, so
    the reader refuses them after parsing, as invalid JSON too."""
    good = house_bytes()
    chunk = json_of_bytes(good)
    name = b'"name":"streetwide:sample-house"'
    return [
        (case, with_json(good, chunk.replace(name, b'"name":"a:' + escape + b'"')))
        for case, escape in (("lone-leading-surrogate", b"\\ud800"), ("lone-trailing-surrogate", b"\\udc00"),
                             ("leading-surrogate-then-a-letter", b"\\ud800\\u0061"))
    ]


@pytest.mark.parametrize("case,data", lone_surrogate_cases(), ids=[c[0] for c in lone_surrogate_cases()])
def test_lone_surrogates_are_invalid_json(case: str, data: bytes) -> None:
    with pytest.raises(glb.GlbError) as e:
        glb.read(data)
    assert str(e.value) == "the JSON chunk is not valid JSON: a string holds a lone surrogate"


@pytest.mark.parametrize("case,data,message", stricter_cases(), ids=[c[0] for c in stricter_cases()])
def test_the_rules_op_world_gained_with_fuzzing(case: str, data: bytes, message: str) -> None:
    with pytest.raises(glb.GlbError) as e:
        glb.read(data)
    assert str(e.value) == message


@pytest.mark.parametrize("case,data", accepted_meshes(), ids=[c[0] for c in accepted_meshes()])
def test_brackets_in_strings_do_not_nest(case: str, data: bytes) -> None:
    assert data.count(b"[") > 127 or data.count(b"{") > 127
    assert glb.write(glb.read(data).to_nodes()) == data


def rust_display(message: str) -> str:
    """How op_world prints the error the Python reader names `message`."""
    return message if message.startswith("not canonical JSON: ") else f"invalid glb: {message}"


def shell_manifest() -> dict:
    """A shell manifest of the fixture world."""
    _, world = WORLD.read_world()
    index = cjson.loads(WORLD.read_object(world["tiles"][0]["tile_id"], "json"))
    entry = next(a for a in index["assets"] if a["layer"] == "shell")
    return cjson.loads(WORLD.read_object(entry["manifest_id"], "json"))


def adding_shells(bundle: Path, meshes: dict[str, bytes]) -> None:
    """Writes a bundle that adds one shell per mesh to the fixture world:
    `meshes` maps a feature id to its mesh, and each gets a copy of a
    fixture shell's manifest pointing at it."""
    store = Store(bundle)
    world_id, _ = WORLD.read_world()
    base = shell_manifest()
    entries = []
    for feature_id, data in meshes.items():
        try:
            bounds = glb.read(data).bounds_mm()
        except glb.GlbError:
            bounds = base["bounds_mm"]  # never compared: the mesh is refused first
        m = dict(base, feature_id=feature_id, content_id=store.write_object(data, "glb"), size_bytes=len(data),
                 bounds_mm=bounds, parent_version_id=None)
        entries.append({"op": "add", "tile": TERRAIN_TILE, "layer": "shell", "feature_id": feature_id,
                        "parent_version_id": None, "manifest_id": store.write_json(m)})
    change = {
        "format": "streetwide.change", "contract_version": "0.1.0", "base_world_id": world_id,
        "title": "glb reader parity", "author": "github:parity", "rationale": "Reads crafted meshes.",
        "changes": sorted(entries, key=changes.sort_key),
    }
    (bundle / "change.json").write_bytes(changes.to_bytes(change))


def test_op_world_refuses_the_same_files_with_the_same_messages(check_change, tmp_path: Path) -> None:
    # Each crafted file becomes a shell that a bundle adds; the gate reports
    # each mesh it cannot read under G1 as "<tile> shell <feature id>: <error>".
    expected = {f"streetwide:glb-{case}": (data, rust_display(message)) for case, data, message in stricter_cases()}
    expected.update({f"streetwide:glb-{case}": (data, None) for case, data in accepted_meshes()})
    invalid_json = {f"streetwide:glb-{case}": data for case, data in lone_surrogate_cases()}
    out_of_range = {f"streetwide:glb-{case}": data for case, data in out_of_range_cases()}
    adding_shells(tmp_path / "bundle", {**{feature_id: data for feature_id, (data, _) in expected.items()},
                                        **invalid_json, **out_of_range})
    (g1,) = [c["details"] for c in check_change(WORLD.root, tmp_path / "bundle")["checks"] if c["id"] == "G1"]

    def reported(feature_id: str) -> list[str]:
        what = f"{TERRAIN_TILE} shell {feature_id}: "
        return [d[len(what):] for d in g1 if d.startswith(what)]

    for feature_id, (_, message) in expected.items():
        assert reported(feature_id) == ([] if message is None else [message]), feature_id
    # serde_json's own words for these ("lone leading surrogate in hex
    # escape at line 1 column ...", "number out of range at line 1 column
    # ...") are not Python's: the verdict and the kind of error agree.
    for feature_id in invalid_json:
        (found,) = reported(feature_id)
        assert found.startswith("invalid JSON: ") and ("surrogate" in found or "hex escape" in found), found
    for feature_id in out_of_range:
        (found,) = reported(feature_id)
        assert found.startswith("invalid JSON: number out of range at line 1 column "), found


# ------------------------------------------------- open edges (gate's G8) --


def grid(n: int, lift: dict[tuple[int, int], int] | None = None, step: int = 1000) -> glb.Mesh:
    """An n x n heightfield in one primitive, as the ingest makes terrain;
    `lift` raises vertex (i, j) by some millimetres."""
    lift = lift or {}
    positions = [(i * step, 400000 + lift.get((i, j), 0), j * step) for j in range(n) for i in range(n)]
    indices = []
    for j in range(n - 1):
        for i in range(n - 1):
            a = j * n + i
            indices += [a, a + n, a + 1, a + 1, a + n, a + n + 1]
    node = glb.Node("tile:17/1/2", [glb.Primitive("terrain", positions, [(0.0, 1.0, 0.0)] * len(positions), indices)])
    return glb.read(glb.write([node]))


def test_open_edges_of_a_grid() -> None:
    mesh = grid(5)
    assert len(glb.boundary_vertices(mesh)) == 16
    interior = tuple(glb.mm_to_f32_bits(k) for k in (2000, 400000, 2000))
    assert interior not in glb.boundary_vertices(mesh)
    assert glb.boundary_vertices(mesh) == sorted(glb.boundary_vertices(mesh))


@pytest.mark.parametrize(
    "lift,missing",
    [
        ({}, 0),
        ({(2, 2): 5}, 0),  # interior
        ({(1, 1): 5, (3, 2): -5}, 0),
        ({(0, 2): 1}, 1),  # one edge vertex, by one millimetre
        ({(4, 4): 1}, 1),  # a corner
        ({(0, 1): 1, (4, 3): 1, (2, 0): -1, (2, 2): 9}, 3),
    ],
)
def test_missing_boundary_vertices_on_grids(lift: dict, missing: int) -> None:
    assert glb.missing_boundary_vertices(grid(5), grid(5, lift)) == missing


def test_new_vertices_anywhere_do_not_matter() -> None:
    # A finer grid has every old position, plus new ones.
    assert glb.missing_boundary_vertices(grid(5), grid(9, step=500)) == 0
    # The other way round, the coarse grid lacks half of the fine edge.
    assert glb.missing_boundary_vertices(grid(9, step=500), grid(5)) == 16


def test_edges_are_counted_per_primitive_by_index_pair() -> None:
    # A 3 x 3 grid split in two primitives along its middle column: that
    # column is open in each primitive, though the surface is closed.
    pos = [(i * 1000, 0, j * 1000) for j in range(3) for i in range(3)]

    def half(columns: range, material: str) -> glb.Primitive:
        keep = [k for k, p in enumerate(pos) if p[0] // 1000 in columns]
        local = {k: n for n, k in enumerate(keep)}
        idx = []
        for j in range(2):
            for i in columns[:-1]:
                a = j * 3 + i
                idx += [local[a], local[a + 3], local[a + 1], local[a + 1], local[a + 3], local[a + 4]]
        return glb.Primitive(material, [pos[k] for k in keep], [(0.0, 1.0, 0.0)] * len(keep), idx)

    split = glb.read(glb.write([glb.Node("tile:17/1/2", [half(range(0, 2), "ground"), half(range(1, 3), "terrain")])]))
    whole = glb.read(glb.write([glb.Node("tile:17/1/2", [half(range(0, 3), "terrain")])]))
    centre = tuple(glb.mm_to_f32_bits(k) for k in (1000, 0, 1000))
    assert centre in glb.boundary_vertices(split)
    assert centre not in glb.boundary_vertices(whole)
    moved = glb.read(glb.write([glb.Node("tile:17/1/2", [
        glb.Primitive("terrain", [(1000, 5, 1000) if p == (1000, 0, 1000) else p for p in pos],
                      [(0.0, 1.0, 0.0)] * 9, list(whole.nodes[0].primitives[0].indices)),
    ])]))
    assert glb.missing_boundary_vertices(split, moved) == 1
    assert glb.missing_boundary_vertices(whole, moved) == 0


def test_a_triangle_listed_twice_closes_its_edges() -> None:
    pos = [(0, 0, 0), (1000, 0, 0), (0, 0, 1000)]
    once = glb.read(glb.write([glb.Node("osm:way:1", [glb.Primitive("road", pos, [(0.0, 1.0, 0.0)] * 3, [0, 2, 1])])]))
    twice = glb.read(glb.write([glb.Node("osm:way:1", [
        glb.Primitive("road", pos, [(0.0, 1.0, 0.0)] * 3, [0, 2, 1, 1, 2, 0])])]))
    assert len(glb.boundary_vertices(once)) == 3
    assert glb.boundary_vertices(twice) == []


# The fixture's terrain with some vertices lifted: what the gate's G8 says
# about replacing the terrain by it. `op_world check-change` on a bundle
# with this replacement (author streetwide:ingest) reports
# "17/68646/45903 terrain tile:17/68646/45903: 11 edge vertices differ from
# the old terrain; tiles would crack": the 10 lifted edge vertices, and
# vertex 0, a corner, which the every-997th rule lifts as well.
# test_op_world_counts_the_same_open_edge_vertices runs that check again
# when STREETWIDE_GATE=1.
G8_FIXTURE_MISSING = 11


def fixture_terrain() -> tuple[dict, bytes]:
    _, world = WORLD.read_world()
    index = cjson.loads(WORLD.read_object(world["tiles"][0]["tile_id"], "json"))
    (entry,) = [a for a in index["assets"] if a["layer"] == "terrain"]
    return entry, WORLD.read_object(entry["content_id"], "glb")


def lifted_fixture_terrain() -> tuple[glb.Mesh, glb.Mesh, bytes]:
    """The old terrain, the new one and its bytes: every 50th open-edge
    vertex in bit order is lifted by 1 mm, and every 997th vertex by 3 mm,
    whichever it is."""
    _, data = fixture_terrain()
    old = glb.read(data)
    edge = glb.boundary_vertices(old)
    lifted_edge = set(edge[::50])
    (node,) = old.to_nodes()
    (prim,) = node.primitives
    bits = old.nodes[0].primitives[0].position_bits
    prim.positions_mm = [
        (x, y + (1 if bits[k] in lifted_edge else 0) + (3 if k % 997 == 0 else 0), z)
        for k, (x, y, z) in enumerate(prim.positions_mm)
    ]
    new_bytes = glb.write([node])
    return old, glb.read(new_bytes), new_bytes


def test_lifted_fixture_terrain_misses_what_the_gate_counts() -> None:
    old, new, _ = lifted_fixture_terrain()
    assert len(glb.boundary_vertices(old)) == 496  # a 125 x 125 grid has 4 x 124
    assert glb.missing_boundary_vertices(old, new) == G8_FIXTURE_MISSING
    assert glb.missing_boundary_vertices(old, old) == 0


# The fixture's terrain cut into two primitives after half of its
# triangles: the seam is open in each primitive. With that terrain in the
# world, `op_world check-change` on a replacement by the whole terrain with
# the seam lifted by 1 mm reports "127 edge vertices differ from the old
# terrain" (checked again with STREETWIDE_GATE=1, as above).
G8_SEAM_MISSING = 127


def split_fixture_terrain() -> tuple[glb.Mesh, bytes]:
    """The fixture terrain as two primitives, `ground` with the first half
    of its triangles and `terrain` with the rest."""
    _, data = fixture_terrain()
    whole = glb.read(data)
    (prim,) = whole.nodes[0].primitives
    cut = len(prim.indices) // 6 * 3

    def part(idx: tuple[int, ...], material: str) -> glb.Primitive:
        used = sorted(set(idx))
        local = {k: n for n, k in enumerate(used)}
        return glb.Primitive(
            material, [prim.positions_mm[k] for k in used], [prim.normals[k] for k in used], [local[k] for k in idx]
        )

    halves = [part(prim.indices[:cut], "ground"), part(prim.indices[cut:], "terrain")]
    out = glb.write([glb.Node(whole.nodes[0].name, halves)])
    return glb.read(out), out


def seam_lifted_terrain() -> tuple[glb.Mesh, bytes, int]:
    """The whole fixture terrain with every vertex on the seam of
    `split_fixture_terrain` lifted by 1 mm, its bytes and the seam's size."""
    _, data = fixture_terrain()
    whole = glb.read(data)
    split, _ = split_fixture_terrain()
    seam = set(glb.boundary_vertices(split)) - set(glb.boundary_vertices(whole))
    (node,) = whole.to_nodes()
    (prim,) = node.primitives
    bits = whole.nodes[0].primitives[0].position_bits
    prim.positions_mm = [(x, y + (1 if bits[k] in seam else 0), z) for k, (x, y, z) in enumerate(prim.positions_mm)]
    out = glb.write([node])
    return glb.read(out), out, len(seam)


def test_a_seam_between_primitives_is_an_open_edge() -> None:
    _, data = fixture_terrain()
    whole = glb.read(data)
    split, _ = split_fixture_terrain()
    new, _, seam = seam_lifted_terrain()
    assert seam == G8_SEAM_MISSING
    assert glb.missing_boundary_vertices(split, new) == G8_SEAM_MISSING
    assert glb.missing_boundary_vertices(whole, new) == 0
    assert glb.missing_boundary_vertices(whole, split) == glb.missing_boundary_vertices(split, whole) == 0


def terrain_replacement(bundle: Path, world_id: str, current_id: str, current: dict, mesh_bytes: bytes) -> dict:
    """Writes a bundle in which the ingest replaces the terrain manifest
    `current` (id `current_id`) by one for `mesh_bytes`; returns its change
    set."""
    new = dict(current, content_id=content_id(mesh_bytes), size_bytes=len(mesh_bytes),
               bounds_mm=glb.read(mesh_bytes).bounds_mm(), parent_version_id=current_id)
    store = Store(bundle)
    store.write_object(mesh_bytes, "glb")
    change = {
        "format": "streetwide.change", "contract_version": "0.1.0", "base_world_id": world_id,
        "title": "Terrain", "author": changes.INGEST_AUTHOR, "rationale": "Checks the terrain-edge rule.",
        "changes": [{"op": "replace", "tile": TERRAIN_TILE, "layer": "terrain", "feature_id": current["feature_id"],
                     "parent_version_id": current_id, "manifest_id": store.write_json(new)}],
    }
    (bundle / "change.json").write_bytes(changes.to_bytes(change))
    return change


def test_op_world_counts_the_same_open_edge_vertices(check_change, tmp_path: Path) -> None:
    def g8(report: dict) -> list[str]:
        (check,) = [c for c in report["checks"] if c["id"] == "G8"]
        return check["details"]

    world_id, world = WORLD.read_world()
    entry, _ = fixture_terrain()
    current = cjson.loads(WORLD.read_object(entry["manifest_id"], "json"))
    what = f"{TERRAIN_TILE} terrain tile:{TERRAIN_TILE}"

    _, _, lifted = lifted_fixture_terrain()
    terrain_replacement(tmp_path / "lifted", world_id, entry["manifest_id"], current, lifted)
    report = check_change(WORLD.root, tmp_path / "lifted")
    assert g8(report) == [f"{what}: {G8_FIXTURE_MISSING} edge vertices differ from the old terrain; tiles would crack"]

    # A world whose terrain is the split one, made as a merge makes it.
    root = tmp_path / "split-world"
    shutil.copytree(WORLD.root / "objects", root / "objects")
    _, split = split_fixture_terrain()
    to_split = terrain_replacement(tmp_path / "to-split", world_id, entry["manifest_id"], current, split)
    shutil.copytree(tmp_path / "to-split" / "objects", root / "objects", dirs_exist_ok=True)
    split_world_id = changes.apply(world, to_split, changes.Objects(root)).write(Store(root))
    split_id = to_split["changes"][0]["manifest_id"]
    _, seam_lifted, _ = seam_lifted_terrain()
    terrain_replacement(tmp_path / "seam", split_world_id, split_id,
                        cjson.loads(Store(root).read_object(split_id, "json")), seam_lifted)
    report = check_change(root, tmp_path / "seam")
    assert g8(report) == [f"{what}: {G8_SEAM_MISSING} edge vertices differ from the old terrain; tiles would crack"]
