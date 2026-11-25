"""Fuzzing the readers of the baseline refresh (ADR-0020; roadmap M1): the
strict glb reader, change sets, the world log, JSON objects read from a
directory, and `apply` on crafted worlds and change sets; and the size
limit of each kind of JSON document (RFC 0001 section 7). Each must return
a value or raise its documented error (GlbError, ChangeError, LogError):
never another exception, never a hang, never memory out of proportion to
its input. See `fuzzing.py` for the knobs; the regression tests at the end
replay the inputs that used to crash."""

from __future__ import annotations

import copy
import json
import math
import os
import re
import shutil
import stat
import struct
from pathlib import Path
from typing import Any

import pytest

from tools.worldfmt import changes, cjson, glb, limits, log, schemas
from tools.worldfmt.changes import ChangeError, Objects
from tools.worldfmt.ids import content_id
from tools.worldfmt.log import LogError
from tools.worldfmt.store import MAX_FILE_BYTES, Store, is_link
from tools.worldfmt.tests.fuzzing import (
    INTERESTING_U32,
    MEMORY,
    Rng,
    mutate_bytes,
    mutate_json,
    mutate_json_bytes,
    mutate_json_text,
    reading,
    run_cases,
)

REPO = Path(__file__).resolve().parents[3]
FIXTURES = REPO / "fixtures"
WORLD = FIXTURES / "world"
SAMPLE = FIXTURES / "changes" / "facade-sample"
TILE = "17/68646/45903"
POSIX = os.name == "posix"
SMALL_LIMIT = 1 << 16
"""The file limit of the directory targets (MAX_FILE_BYTES is 64 MiB): a
file one byte over it is cheap to write."""
TOO_DEEP = f"not canonical JSON: arrays and objects nested deeper than {cjson.MAX_DEPTH} levels"


def regular(path: Path) -> bool:
    try:
        info = path.lstat()
    except OSError:
        return False
    return stat.S_ISREG(info.st_mode) and not is_link(info)


def is_dir_itself(path: Path) -> bool:
    """A directory, not a link to one (nor a Windows junction)."""
    try:
        info = path.lstat()
    except OSError:
        return False
    return stat.S_ISDIR(info.st_mode) and not is_link(info)


def regular_files(root: Path) -> list[Path]:
    """The regular files under `root`, in a stable order, without following
    links or junctions (rglob follows junctions on Windows)."""
    found, folders = [], [root]
    while folders:
        with os.scandir(folders.pop()) as entries:
            for entry in entries:
                path = Path(entry.path)
                if is_dir_itself(path):
                    folders.append(path)
                elif regular(path):
                    found.append(path)
    return sorted(found, key=lambda p: p.as_posix())


def files_size(root: Path) -> int:
    return sum(p.lstat().st_size for p in regular_files(root))


def clear(path: Path) -> None:
    """Removes whatever is at `path` (a file, a link, a junction, a FIFO or a
    directory) without following links."""
    try:
        info = path.lstat()
    except OSError:
        return
    if stat.S_ISDIR(info.st_mode) and not is_link(info):
        shutil.rmtree(path)
    else:
        path.unlink()  # on Windows, also a junction or a link to a directory


def put(root: Path, path: Path, data: bytes) -> None:
    """Writes `data` as a regular file at `path` under `root`, replacing what
    is there and making its folders directories: never writing through a
    link or a junction, or into a FIFO, which would block the test itself."""
    folder = root
    for part in path.relative_to(root).parts[:-1]:
        folder = folder / part
        if os.path.lexists(folder) and not is_dir_itself(folder):
            clear(folder)
        folder.mkdir(exist_ok=True)
    clear(path)
    path.write_bytes(data)


def link_directory(target: Path, link: Path) -> bool:
    """Makes `link` a link to the directory `target`: a junction on Windows,
    which needs no privilege, else a symbolic link. False if this system
    cannot."""
    try:
        if os.name == "nt":
            import _winapi

            _winapi.CreateJunction(str(target.resolve()), str(link))
        else:
            os.symlink(target, link, target_is_directory=True)
    except (OSError, NotImplementedError):
        return False
    return True


def shallow_copy_or_none(value: Any) -> Any:
    """A deep copy of `value` to compare it with later, or None when it nests
    too deeply for copy.deepcopy (mutations nest up to about 900 levels)."""
    return None if cjson.deeper_than(value, 200) else copy.deepcopy(value)


def json_size(value: Any) -> int:
    try:
        return len(json.dumps(value))
    except (TypeError, ValueError, RecursionError):
        return 1 << 20


# -------------------------------------------------------------------- glb --

_GLB_MAGIC, _CHUNK_JSON, _CHUNK_BIN = 0x46546C67, 0x4E4F534A, 0x004E4942


def glb_pack(json_chunk: bytes, bin_chunk: bytes) -> bytes:
    """A glb of these two chunks, with consistent lengths and the JSON
    padded with spaces (`glb_pack` in crates/op_world/tests/fuzzing)."""
    j = json_chunk + b" " * (-len(json_chunk) % 4)
    total = 12 + 8 + len(j) + 8 + len(bin_chunk)
    return b"".join([
        struct.pack("<III", _GLB_MAGIC, 2, total), struct.pack("<II", len(j), _CHUNK_JSON), j,
        struct.pack("<II", len(bin_chunk), _CHUNK_BIN), bin_chunk,
    ])


def glb_chunks(data: bytes) -> tuple[bytes, bytes] | None:
    """The JSON and BIN chunks of a glb whose lengths are consistent."""
    if len(data) < 20:
        return None
    json_len = struct.unpack_from("<I", data, 12)[0]
    if 28 + json_len > len(data):
        return None
    bin_len = struct.unpack_from("<I", data, 20 + json_len)[0]
    if 28 + json_len + bin_len > len(data):
        return None
    return data[20 : 20 + json_len], data[28 + json_len : 28 + json_len + bin_len]


def shared_glb(nodes: int, vertices: int, shared_mesh: bool) -> bytes:
    """`shared_glb` of crates/op_world/tests/fuzz_readers.rs: a mesh that
    is valid but for one kind of sharing. With `shared_mesh`, every node
    shows mesh 0; otherwise each node has a mesh of its own, but all read
    one POSITION accessor, as their NORMAL too."""
    floats = 12 * vertices
    vec3 = [{"bufferView": v, "componentType": 5126, "count": vertices, "type": "VEC3"} for v in range(2)]
    indices = [{"bufferView": v, "componentType": 5125, "count": 3, "type": "SCALAR"} for v in range(3)]

    def mesh(position: int, normal: int, index: int) -> dict:
        prim = {"attributes": {"NORMAL": normal, "POSITION": position}, "indices": index, "material": 0, "mode": 4}
        return {"primitives": [prim]}

    if shared_mesh:
        accessors = [vec3[0], vec3[1], indices[2]]
        views = [{"byteLength": floats, "byteOffset": 0}, {"byteLength": floats, "byteOffset": floats},
                 {"byteLength": 12, "byteOffset": 2 * floats}]
        meshes, bin_len = [mesh(0, 1, 2)], 2 * floats + 12
    else:
        accessors = [vec3[0], indices[1]]
        views = [{"byteLength": floats, "byteOffset": 0}, {"byteLength": 12, "byteOffset": floats}]
        meshes, bin_len = [mesh(0, 0, 1)] * nodes, floats + 12
    doc = {
        "accessors": accessors, "bufferViews": views, "materials": [{"name": "road"}], "meshes": meshes,
        "nodes": [{"mesh": 0 if shared_mesh else i, "name": "a:b"} for i in range(nodes)],
    }
    return glb_pack(json.dumps(doc, separators=(",", ":"), sort_keys=True).encode(), bytes(bin_len))


def _triangle(material: str, x_mm: int) -> glb.Primitive:
    return glb.Primitive(material, [(x_mm, 0, 0), (x_mm + 1000, 0, 0), (x_mm, 0, 1000)], [(0.0, 1.0, 0.0)] * 3, [0, 2, 1])


COMMITTED_MESHES = [p.read_bytes() for p in sorted(FIXTURES.rglob("*.glb"))]
HAND_MADE_MESHES = [
    glb.write([glb.Node("osm:way:1", [_triangle("road", 0)])]),
    glb.write([
        glb.Node("a:1", [_triangle("roof", -5), _triangle("wall", 7)]),
        # Far away (1,000 km and 8 km) with odd millimetres: long decimals in min and max.
        glb.Node("a:2", [glb.Primitive("water", [(1_000_000_000, 0, -987_654), (-8_000_000, 8_000_001, 5), (1, 2, 3)],
                                       [(-0.0, 0.5, 1e-40)] * 3, [0, 2, 1])]),
    ]),
    glb.write([glb.Node('a:\\"' + "[" * 150 + "{", [_triangle("terrain", 3)])]),  # brackets in a string
]
MESHES = COMMITTED_MESHES + HAND_MADE_MESHES
SMALL_MESHES = [m for m in MESHES if len(m) < 200_000]  # the 744 KB terrain takes 0.1 s to read
MESH_DOCS = [json.loads(glb_chunks(m)[0]) for m in MESHES]
NAMES = ("osm:way:1", "a:b", "tile:17/1/2", "", "A:b", "a:" + "x" * 255, "a:" + "x" * 254, "a:\u00e9", "a:\ud800", 5, None)
_ODD_F32 = (0x7FC00000, 0x7F800000, 0xFF800000, 0x80000000, 0x00800000, 0x00000001, 0x7F7FFFFF,
            struct.unpack("<I", struct.pack("<f", 1e10))[0], struct.unpack("<I", struct.pack("<f", 9e12))[0],
            struct.unpack("<I", struct.pack("<f", 0.0005))[0], struct.unpack("<I", struct.pack("<f", 1.0005))[0])


def test_the_mesh_corpus_reads() -> None:
    assert len(COMMITTED_MESHES) > 40, "fixtures are missing"
    for data in MESHES:
        assert glb.write(glb.read(data).to_nodes()) == data


def edit_glb_json(rng: Rng, doc: dict, bin_len: int) -> None:
    """One edit of the fields the reader indexes and sizes with, or of the
    nodes and primitives it walks (`edit_glb_json` in
    crates/op_world/tests/fuzzing)."""

    def index() -> Any:
        k = rng.below(5)
        if k <= 1:
            return rng.choice((0, 1, 2, 3, 4, 12, 36, -1, 65_535, 2**32 - 1, 2**32, 2**53, 2**63 - 1, -(2**63)))
        if k == 2:
            return 2**64 - 1
        if k == 3:
            return bin_len + rng.below(13)
        return 1.5

    def some(key: str) -> dict | None:
        items = doc.get(key)
        item = rng.choice(items) if isinstance(items, list) and items else None
        return item if isinstance(item, dict) else None

    def primitives() -> list | None:
        mesh = some("meshes")
        prims = mesh.get("primitives") if mesh is not None else None
        return prims if isinstance(prims, list) and prims else None

    op = rng.below(8)
    nodes = doc.get("nodes")
    if op == 0 and isinstance(nodes, list):  # every node shows mesh 0
        for n in nodes:
            if isinstance(n, dict):
                n["mesh"] = 0
    elif op == 1 and isinstance(nodes, list) and nodes:  # more nodes, copies of the first
        nodes.extend(copy.deepcopy(nodes[0]) for _ in range(1 + rng.below(63)))
    elif op == 2 and (prims := primitives()) is not None:  # more primitives, copies of the first
        prims.extend(copy.deepcopy(prims[0]) for _ in range(1 + rng.below(63)))
    elif op == 3 and (acc := some("accessors")) is not None:
        acc[rng.choice(("count", "bufferView", "componentType", "type"))] = index()
    elif op == 4 and (view := some("bufferViews")) is not None:
        view[rng.choice(("byteOffset", "byteLength", "buffer", "target"))] = index()
    elif op == 5 and (prims := primitives()) is not None and isinstance(prim := rng.choice(prims), dict):
        field = rng.choice(("indices", "material", "mode", "POSITION", "NORMAL"))
        if field.isupper():
            if isinstance(prim.get("attributes"), dict):
                prim["attributes"][field] = index()
        else:
            prim[field] = index()
    elif op == 6 and (acc := some("accessors")) is not None:  # read another accessor's view
        views = doc.get("bufferViews")
        if isinstance(views, list) and views:
            acc["bufferView"] = rng.below(len(views))
    elif (node := some("nodes")) is not None:
        if rng.chance(50):
            node["mesh"] = index()
        else:
            node["name"] = rng.choice(NAMES)


def edit_glb_bin(rng: Rng, bin_chunk: bytearray) -> None:
    """Overwrites one 4-byte value of a BIN chunk: odd floats for positions
    and normals, odd integers for indices (`edit_glb_bin`)."""
    if len(bin_chunk) < 4:
        return
    at = rng.below(len(bin_chunk) // 4) * 4
    current = struct.unpack_from("<I", bin_chunk, at)[0]
    k = rng.below(3)
    if k == 0:
        value = rng.choice(_ODD_F32)
    elif k == 1:
        value = rng.choice((0, 1, 2, 3, 0xFFFFFFFF, 0xFFFFFFFE, 0x80000000, 0x7FFFFFFF))
    else:
        value = (current + rng.below(5) - 2) & 0xFFFFFFFF
    struct.pack_into("<I", bin_chunk, at, value)


def _dumps_chunk(doc: Any) -> bytes | None:
    try:
        return json.dumps(doc, separators=(",", ":")).encode("ascii")
    except (TypeError, ValueError, RecursionError):
        return None


def mutate_glb(rng: Rng, base: bytes) -> bytes:
    """A mutated glb (`mutate_glb` in crates/op_world/tests/fuzz_readers.rs):
    edits of its JSON (glb-aware, structural and textual) and BIN chunks
    repacked with consistent lengths; the header's and the chunks' length
    and type fields; or byte-level edits."""
    k = rng.below(10)
    if k < 5 and (chunks := glb_chunks(base)) is not None:
        json_chunk, bin_chunk = chunks[0], bytearray(chunks[1])
        for _ in range(1 + rng.below(3)):
            op = rng.below(5)
            if op <= 2:
                try:
                    doc = json.loads(json_chunk)
                except (ValueError, RecursionError):
                    continue
                if cjson.deeper_than(doc, 200):  # an earlier edit nested it: too deep for copy.deepcopy
                    continue
                if op <= 1 and isinstance(doc, dict):
                    edit_glb_json(rng, doc, len(bin_chunk))
                else:
                    doc = mutate_json(rng, doc, MESH_DOCS)
                json_chunk = _dumps_chunk(doc) or json_chunk
            elif op == 3:
                text = mutate_json_text(rng, json_chunk.decode("utf-8", "replace"))
                json_chunk = text.encode("utf-8", "surrogatepass")
            else:
                edit_glb_bin(rng, bin_chunk)
        return glb_pack(json_chunk, bytes(bin_chunk))
    if k < 7:
        out = bytearray(base)
        chunks = glb_chunks(base)
        json_len = len(chunks[0]) if chunks else 0
        at = rng.choice((0, 4, 8, 12, 16, 20 + json_len, 24 + json_len))
        if len(out) >= at + 4:
            current = struct.unpack_from("<I", out, at)[0]
            near = (rng.choice((len(out), current)) + rng.below(9) - 4) & 0xFFFFFFFF
            struct.pack_into("<I", out, at, rng.choice(INTERESTING_U32) if rng.chance(50) else near)
        return bytes(out)
    return mutate_bytes(rng, base, MESHES)


def crafted_glb(rng: Rng) -> bytes:
    """A container built from scratch: sharing at scale, nesting and chunk
    lengths at and past their limits, missing, empty and swapped chunks."""
    base = rng.choice(SMALL_MESHES)
    json_chunk, bin_chunk = glb_chunks(base)
    k = rng.below(7)
    if k == 0:
        return shared_glb(1 + rng.below(100), 1 + rng.below(300), rng.chance(50))
    if k == 1:
        depth = rng.choice((126, 127, 128, 129, 1000, 100_000))
        opened = rng.choice((b"[", b'{"a":', b'[{"a":'))
        body = opened * depth + rng.choice((b"", b"]" * depth, b"0"))
        return glb_pack(rng.choice((body, b'{"nodes":' + body + b"}", json_chunk.rstrip() + body)), bin_chunk)
    if k == 2:
        size = glb.MAX_JSON_BYTES + rng.choice((-4, 0, 4, 64))
        padding = b" " * max(0, size - len(json_chunk.rstrip()))
        return glb_pack(json_chunk.rstrip() + padding, bin_chunk)
    if k == 3:  # long arrays of numbers, near the limit
        n = rng.choice((1000, 100_000, 500_000, 600_000))
        return glb_pack(b'{"accessors":[' + b"0," * n + b'0],"nodes":[]}', bin_chunk)
    if k == 4:  # chunks missing, empty, swapped or doubled
        header = struct.pack("<II", len(json_chunk), _CHUNK_JSON) + json_chunk
        binary = struct.pack("<II", len(bin_chunk), _CHUNK_BIN) + bin_chunk
        body = rng.choice((header, binary + header, header + binary + binary, b"", header + struct.pack("<II", 0, _CHUNK_BIN)))
        return struct.pack("<III", _GLB_MAGIC, 2, 12 + len(body)) + body
    if k == 5:  # numbers JSON spells in ways serde_json reads differently
        text = rng.choice((b'"mesh":-0', b'"mesh":0.0', b'"mesh":0e0', b'"mesh":1e400', b'"mesh":' + b"1" * 5000))
        return glb_pack(json_chunk.replace(b'"mesh":0', text, 1), bin_chunk)
    # Node names with escapes, brackets and lone surrogates (the node's is
    # the last name: nodes come after materials).
    name = rng.choice((b'"a:\\ud800"', b'"a:\\"[[[["', b'"a:\\\\"', b'"\\u0061:b"', b'"a:' + b"[" * 300 + b'"'))
    at = json_chunk.rfind(b'"name":"')
    return glb_pack(json_chunk[:at] + b'"name":' + name + b',"x":"' + json_chunk[at + 8 :], bin_chunk)


def check_mesh(data: bytes, mesh: glb.Mesh) -> None:
    """What an accepted mesh must be (`check_glb`)."""
    assert glb.write(mesh.to_nodes()) == data
    assert mesh.triangle_count() > 0
    bounds = mesh.bounds_mm()
    assert all(bounds["min"][k] <= bounds["max"][k] for k in range(3))
    for p in mesh.primitives():
        assert len(p.positions) == len(p.position_bits) == len(p.positions_mm) == len(p.normals) > 0
        assert len(p.indices) % 3 == 0 and max(p.indices) < len(p.positions)


def test_fuzz_glb_read() -> None:
    """glb.read on mutated meshes (every .glb under fixtures/ and hand-made
    ones) and crafted containers: a mesh that re-writes to the same bytes,
    or GlbError."""

    def case(rng: Rng) -> None:
        k = rng.below(20)
        if k < 2:
            data = crafted_glb(rng)
        else:
            base = rng.choice(MESHES) if k == 2 else rng.choice(SMALL_MESHES)
            data = mutate_glb(rng, base)
        try:
            with reading(len(data)):
                mesh = glb.read(data)
        except glb.GlbError:
            return
        check_mesh(data, mesh)

    run_cases("glb.read", 400, case, memory=MEMORY)


def rust_check_depth(data: bytes) -> bool:
    """Whether `op_world::cjson::check_depth` refuses `data`: a line-by-line
    port of its loop."""
    depth, in_string, escaped = 0, False, False
    for b in data:
        if in_string:
            if escaped:
                escaped = False
            elif b == 0x5C:
                escaped = True
            elif b == 0x22:
                in_string = False
            continue
        if b == 0x22:
            in_string = True
        elif b in (0x5B, 0x7B):
            depth += 1
            if depth > cjson.MAX_DEPTH:
                return True
        elif b in (0x5D, 0x7D):
            depth = max(0, depth - 1)
    return False


def test_fuzz_the_depth_rule_is_op_worlds() -> None:
    """glb's depth rule refuses exactly the JSON text that op_world's
    check_depth refuses: runs of brackets, quotes and backslashes, and
    mutated JSON chunks."""
    alphabet = b'[]{}"\\ a\xff\n'
    chunks = [glb_chunks(m)[0] for m in SMALL_MESHES]

    def case(rng: Rng) -> None:
        if rng.chance(50):
            data = b"".join(bytes([rng.choice(alphabet)]) * rng.choice((1, 1, 2, 3, 60, 127, 128, 200))
                            for _ in range(1 + rng.below(30)))
        else:
            data = mutate_json_text(rng, rng.choice(chunks).decode()).encode("utf-8", "surrogatepass")
            data = mutate_bytes(rng, data, chunks) if rng.chance(50) else data
        refused = rust_check_depth(data)
        try:
            glb._check_depth(data)
        except glb.GlbError as e:
            assert refused and str(e) == TOO_DEEP, data[:200]
        else:
            assert not refused, data[:200]

    run_cases("glb depth rule", 2000, case)


# ------------------------------------------------------------- change sets --


def _change_set(base_world_id: str, author: str, entries: list[dict]) -> bytes:
    return changes.to_bytes({
        "format": "streetwide.change", "contract_version": "0.1.0", "base_world_id": base_world_id,
        "title": "Fuzz corpus", "author": author, "rationale": "A change set in a shape the refresh writes.",
        "changes": sorted(entries, key=changes.sort_key),
    })


def _fixture_assets() -> tuple[str, dict, dict]:
    store = Store(WORLD)
    world_id, world = store.read_world()
    index = cjson.loads(store.read_object(world["tiles"][0]["tile_id"], "json"))
    return world_id, world, index


def hand_made_change_sets() -> dict[str, bytes]:
    """A water removal by the ingest, a takedown of the sample facade, a
    shell moved to another tile and 50 additions (the cap)."""
    world_id, _, index = _fixture_assets()
    water = next(a for a in index["assets"] if a["layer"] == "water")
    shell = next(a for a in index["assets"] if a["layer"] == "shell")
    facade = changes.parse((SAMPLE / "change.json").read_bytes())["changes"][0]

    def remove(a: dict, tile: str = TILE) -> dict:
        return {"op": "remove", "tile": tile, "layer": a["layer"], "feature_id": a["feature_id"],
                "parent_version_id": a["manifest_id"], "manifest_id": None}

    adds = [{"op": "add", "tile": TILE, "layer": "shell", "feature_id": f"streetwide:building-{k:02d}",
             "parent_version_id": None, "manifest_id": content_id(b"manifest %d" % k)} for k in range(changes.MAX_CHANGES)]
    moved = dict(remove(shell), op="add", tile="17/68646/45904", parent_version_id=None, manifest_id=shell["manifest_id"])
    takedown = dict(facade, op="remove", parent_version_id=facade["manifest_id"], manifest_id=None)
    return {
        "water removal": _change_set(world_id, changes.INGEST_AUTHOR, [remove(water)]),
        "takedown": _change_set(world_id, "github:maintainer", [takedown]),
        "move": _change_set(world_id, changes.INGEST_AUTHOR, [remove(shell), moved]),
        "fifty": _change_set(world_id, changes.INGEST_AUTHOR, adds),
    }


COMMITTED_CHANGES = [p.read_bytes() for p in sorted((FIXTURES / "changes").rglob("change.json"))]
HAND_MADE_CHANGES = hand_made_change_sets()
CHANGE_BYTES = COMMITTED_CHANGES + list(HAND_MADE_CHANGES.values())
CHANGE_DOCS = [cjson.loads(b) for b in CHANGE_BYTES]
OTHER_DOCS = [cjson.loads(p.read_bytes()) for p in sorted(FIXTURES.glob("changes/*/report.json"))] + [
    cjson.loads(Store(WORLD).read_object(a["manifest_id"], "json")) for a in _fixture_assets()[2]["assets"][:3]
]
DONORS = CHANGE_DOCS + OTHER_DOCS


def test_the_change_set_corpus_parses() -> None:
    assert COMMITTED_CHANGES, "fixtures are missing"
    for data in CHANGE_BYTES:
        assert changes.to_bytes(changes.parse(data)) == data


def test_fuzz_change_set_parse() -> None:
    """changes.parse on mutated change.json bytes: a change set that
    `problems` accepts and that writes back to the same bytes, or
    ChangeError."""

    def case(rng: Rng) -> None:
        if rng.chance(75):
            data = mutate_json_bytes(rng, rng.choice(CHANGE_DOCS), DONORS, CHANGE_BYTES)
        else:
            data = mutate_bytes(rng, rng.choice(CHANGE_BYTES), CHANGE_BYTES)
        try:
            with reading(len(data)):
                change = changes.parse(data)
        except ChangeError:
            return
        assert changes.problems(change) == []
        assert changes.to_bytes(change) == data

    run_cases("changes.parse", 300, case, memory=MEMORY)


def test_fuzz_change_set_problems() -> None:
    """changes.problems on mutated documents, which may hold what JSON
    text can spell but canonical JSON refuses (floats, lone surrogates,
    huge numbers, deep nesting): a list of messages, never an exception.
    What it accepts, to_bytes writes (or refuses with ChangeError) and
    parse reads back."""

    def case(rng: Rng) -> None:
        doc = mutate_json(rng, rng.choice(CHANGE_DOCS), DONORS)
        found = changes.problems(doc)
        assert isinstance(found, list) and all(isinstance(m, str) for m in found)
        if found:
            return
        try:
            data = changes.to_bytes(doc)
        except ChangeError:
            return
        assert changes.parse(data) == doc

    run_cases("changes.problems", 400, case)


# -------------------------------------------------------------- world log --


def write_log(root: Path) -> None:
    """A world log over fixtures/world, as log.py lays it out: the baseline,
    the sample facade merged (with its report), its takedown and a refresh
    that removes the water, with every world kept."""
    store = Store(WORLD)
    world_id, world = store.read_world()
    records: list[bytes] = []

    def write(rel: str, data: bytes) -> None:
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)

    def append(decision: str, world_after: str, world_bytes: bytes, change: bytes | None = None,
               report: bytes | None = None) -> None:
        r = {"format": "streetwide.merge", "contract_version": "0.1.0", "decision": decision, "change_id": None,
             "world_before": None, "world_after": world_after, "check_report_id": None, "reviews": [],
             "reasons": "Fuzz corpus.", "previous_record_id": None}
        if change is not None:
            report = report if report is not None else b'{"format":"streetwide.check"}'
            r.update(change_id=content_id(change), world_before=cjson.loads(records[-1])["world_after"],
                     check_report_id=content_id(report), previous_record_id=content_id(records[-1]))
            if decision == "merged":
                r["reviews"] = [{"reviewer": "github:reviewer", "verdict": "better", "notes": "Checked."}]
            write(log.change_path(content_id(change)), change)
            write(log.report_path(content_id(report)), report)
        write(log.world_path(world_after), world_bytes)
        data = cjson.dumps(r)
        write(log.record_path(len(records)), data)
        records.append(data)

    append("baseline", world_id, (WORLD / "world.json").read_bytes())
    sample = (SAMPLE / "change.json").read_bytes()
    with_facade = changes.apply(world, changes.parse(sample), Objects(SAMPLE, WORLD))
    append("merged", with_facade.world_id, with_facade.world_bytes, sample, (SAMPLE / "report.json").read_bytes())
    dry = HAND_MADE_CHANGES["water removal"]
    takedown = _change_set(with_facade.world_id, "github:maintainer",
                           changes.parse(HAND_MADE_CHANGES["takedown"])["changes"])
    back = changes.apply(with_facade.world, changes.parse(takedown), Objects(with_facade.objects, SAMPLE, WORLD))
    append("takedown", back.world_id, back.world_bytes, takedown)
    without_water = changes.apply(world, changes.parse(dry), Objects(WORLD))
    append("merged", without_water.world_id, without_water.world_bytes, dry)


@pytest.fixture(scope="module")
def log_template(tmp_path_factory: pytest.TempPathFactory) -> Path:
    root = tmp_path_factory.mktemp("log-template")
    write_log(root)
    world_log = log.read_log(root)
    assert [r.decision for r in world_log.records] == ["baseline", "merged", "takedown", "merged"]
    assert world_log.report(world_log.records[1].record["check_report_id"])["passed"] is True
    return root


def _json_value(data: bytes) -> Any:
    try:
        return json.loads(data)
    except (ValueError, RecursionError):
        return None


def _write_bytes(doc: Any) -> bytes:
    """`doc` as canonical JSON, or as plain JSON when canonical JSON cannot
    hold it (a float, a lone surrogate, deep nesting)."""
    try:
        return cjson.dumps(doc)
    except (cjson.CanonicalError, RecursionError):
        try:
            return json.dumps(doc).encode("utf-8", "surrogatepass")
        except (TypeError, ValueError, RecursionError):
            return b"[]"


def rechain(root: Path, start: int, worlds: bool) -> None:
    """Rewrites the records after record `start` so that each names the one
    before it and, with `worlds`, continues from its world, for as long as
    they parse: the chain then holds, and the checks after it run."""
    i = start + 1
    while regular(path := root / log.record_path(i)) and regular(before := root / log.record_path(i - 1)):
        r, previous = _json_value(path.read_bytes()), _json_value(before.read_bytes())
        if not isinstance(r, dict) or not isinstance(previous, dict):
            return
        r["previous_record_id"] = content_id(before.read_bytes())
        if worlds:
            r["world_before"] = previous.get("world_after")
        put(root, path, _write_bytes(r))
        i += 1


def special(rng: Rng, root: Path, path: Path, others: list[Path]) -> None:
    """Puts something that is not a regular file at `path`: a directory, or
    where the system allows, a link (to another file, to itself, to a
    directory or to /dev/zero) or a FIFO."""
    clear(path)
    k = rng.below(3)
    if k == 0 and POSIX:
        os.mkfifo(path)
        return
    if k == 1:
        target = rng.choice([*others, path, root, Path("/dev/zero")])
        try:
            os.symlink(target, path, target_is_directory=target.is_dir())
            return
        except OSError:  # Windows without the privilege to make links
            pass
    path.mkdir()
    (path / "inside.json").write_bytes(b"{}")


LOG_NAMES = ("README", "00000009.json", "0000000.json", "00000001.JSON", "00000004.json", "00000000.json.tmp",
             ".hidden", "0000000a.json", "log")
UNDECODABLE_NAMES = tuple(os.fsdecode(b) for b in (b"\xff\xfe.json", b"0000000\xe9.json", b"\x80")) if POSIX else (
    "\ud800.json", "0000000\udc00.json")


def mutate_log(rng: Rng, root: Path) -> None:
    """One edit of a world log: a file rewritten with mutated bytes, a record
    edited and the chain mended, a change set replaced under its new id,
    files renamed, removed, added, truncated or made too large, files
    replaced by directories, links or FIFOs, a directory replaced by a link
    (a junction on Windows) to a copy of it, and records repeated so that
    many name one change set."""
    files = regular_files(root)
    records = [p for p in files if p.parent == root / "log" and p.stem.isdigit()]
    op = rng.below(14)
    if op == 12:  # a directory made a link to a copy of itself: only a reader that follows links takes it
        folders = sorted({p.parent for p in files if p.parent != root}, key=lambda p: p.as_posix())
        if folders:
            folder = rng.choice(folders)
            copy_of = root / f"alt{rng.below(4)}"
            clear(copy_of)
            os.replace(folder, copy_of)
            if not link_directory(copy_of, folder):
                os.replace(copy_of, folder)
        return
    if op == 13 and records:  # records repeated after the last: many name one change set
        last = max(records, key=lambda p: p.name)
        named = [p for p in records
                 if isinstance(r := _json_value(p.read_bytes()), dict) and isinstance(r.get("change_id"), str)]
        if named:
            data = rng.choice(named).read_bytes()
            for i in range(1 + rng.below(40)):
                put(root, root / log.record_path(int(last.stem) + 1 + i), data)
            rechain(root, int(last.stem), True)
        return
    if op <= 1 and files:  # rewrite a file
        path = rng.choice(files)
        data = path.read_bytes()
        value = _json_value(data)
        put(root, path, mutate_json_bytes(rng, value, DONORS, CHANGE_BYTES) if value is not None and rng.chance(70)
            else mutate_bytes(rng, data, CHANGE_BYTES))
    elif op <= 3 and records:  # edit a record, then mend the chain after it
        path = rng.choice(records)
        r = _json_value(path.read_bytes())
        if not isinstance(r, dict):
            return
        ids = [content_id(p.read_bytes()) for p in files] + [None, content_id(b"elsewhere")]
        k = rng.below(6)
        if k == 0:
            r = mutate_json(rng, r, DONORS, edits=1)
        elif k == 1:
            r["decision"] = rng.choice(("baseline", "merged", "takedown", "approved"))
        elif k in (2, 3):
            r[rng.choice(("change_id", "world_after", "world_before", "check_report_id"))] = rng.choice(ids)
        elif k == 4:
            better = {"reviewer": "github:reviewer", "verdict": "better", "notes": "Checked."}
            blocked = {"reviewer": "github:zed", "verdict": "blocked", "notes": "No."}
            r["reviews"] = rng.choice(([], [better], [blocked], [better, blocked], [blocked, better], [better, better]))
        else:
            r["previous_record_id"] = rng.choice(ids)
        put(root, path, _write_bytes(r))
        rechain(root, int(path.stem), rng.chance(70))
    elif op == 4 and records:  # replace a record's change set, under its new id
        path = rng.choice(records)
        r = _json_value(path.read_bytes())
        if not isinstance(r, dict) or not isinstance(r.get("change_id"), str) or len(r["change_id"]) != 71:
            return
        old = root / "changes" / r["change_id"][7:] / "change.json"
        doc = _json_value(old.read_bytes()) if regular(old) else rng.choice(CHANGE_DOCS)
        data = _write_bytes(mutate_json(rng, doc, DONORS) if rng.chance(80) else rng.choice(CHANGE_DOCS))
        put(root, root / log.change_path(content_id(data)), data)
        r["change_id"] = content_id(data)
        put(root, path, _write_bytes(r))
        rechain(root, int(path.stem), True)
    elif op == 5 and files:  # rename
        path = rng.choice(files)
        if path.parent == root / "log":
            target = path.parent / rng.choice(LOG_NAMES)
        else:
            target = root / rng.choice(("changes", "worlds", "log")) / rng.choice(("0" * 8, "x")) / path.name
        if target != path:
            put(root, target, b"")
            os.replace(path, target)
    elif op == 6:  # remove a file or a directory
        victims = files + [root / d for d in ("log", "changes", "worlds", "reports")]
        clear(rng.choice(victims))
    elif op == 7:  # add a file
        name = rng.choice(LOG_NAMES)
        if rng.chance(20):  # a name that is not text: bytes on POSIX, an unpaired surrogate on Windows
            name = rng.choice(UNDECODABLE_NAMES)
        folder = root / rng.choice(("log", "log", "changes", "worlds"))
        try:
            put(root, folder / name, rng.choice(CHANGE_BYTES) if rng.chance(50) else rng.bytes(rng.below(64)))
        except OSError:  # a file system that only takes UTF-8 names (macOS)
            pass
    elif op == 8 and files:  # truncate
        path = rng.choice(files)
        data = path.read_bytes()
        put(root, path, data[: rng.below(len(data) + 1)])
    elif op == 9 and files:  # one byte over the limit
        path = rng.choice(files)
        data = path.read_bytes()
        put(root, path, data + b" " * max(0, SMALL_LIMIT + 1 - len(data)))
    elif files:  # a directory, a link or a FIFO in its place
        path = rng.choice(files)
        special(rng, root, path, [p for p in files if p != path])


def test_fuzz_world_log(log_template: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """log.read_log on crafted log directories (a valid log, then files
    mutated, renamed, removed, added, truncated, made too large, replaced
    by directories and, where the system allows, links and FIFOs, and
    directories replaced by links or junctions): a WorldLog whose records,
    touches, worlds and reports can be read, or LogError. A log it takes
    has no link on the way to a file it read, and each change set is read
    once, however many records name it."""
    monkeypatch.setattr(log, "MAX_FILE_BYTES", SMALL_LIMIT)
    counter = iter(range(10**9))

    def case(rng: Rng) -> None:
        root = tmp_path / f"log{next(counter)}"
        shutil.copytree(log_template, root)
        for _ in range(1 + rng.below(3)):
            mutate_log(rng, root)
        try:
            with reading(files_size(root)):
                world_log = log.read_log(root)
                for r in world_log.records:
                    for get, cid in ((world_log.world, r.world_before), (world_log.world, r.world_after),
                                     (world_log.report, r.record["check_report_id"])):
                        try:
                            doc = get(cid) if cid is not None else None
                        except LogError:
                            continue
                        assert doc is None or (isinstance(doc, dict) and schemas.errors(doc) == [])
            folders = {"log"} | {f"changes/{r.change_id[7:]}" for r in world_log.records[1:]}
            folders |= {"changes"} if len(world_log.records) > 1 else set()
            linked = sorted(f for f in folders if not is_dir_itself(root / f))
        except LogError:
            return
        finally:
            shutil.rmtree(root)
        assert linked == [], f"read_log took a log with these directories as links: {linked}"
        assert world_log.baseline.decision == "baseline" and world_log.records[0].change is None
        assert [r.index for r in world_log.records] == list(range(len(world_log.records)))
        for r in world_log.records[1:]:
            assert r.change is not None and changes.problems(r.change) == []
            assert len(r.touches()) == len(r.change["changes"])
        named = world_log.records[1:]
        assert len({id(r.change) for r in named}) == len({r.change_id for r in named}), "a change set read twice"
        assert all(ts for ts in world_log.last_touches().values())

    run_cases("log.read_log", 100, case, memory=MEMORY)


# ---------------------------------------------------------------- objects --


def test_fuzz_objects_from_a_crafted_directory(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """changes.Objects reading JSON objects from a crafted store directory,
    alone or over the fixture world: the checked document, or ChangeError.
    A link, a directory or a FIFO where an object should be never hides the
    object in the next source, as in op_world's layered store."""
    monkeypatch.setattr(changes, "MAX_FILE_BYTES", SMALL_LIMIT)
    fixture = Store(WORLD)
    _, world, index = _fixture_assets()
    known = {world["tiles"][0]["tile_id"]: fixture.read_object(world["tiles"][0]["tile_id"], "json")}
    for a in index["assets"]:
        known[a["manifest_id"]] = fixture.read_object(a["manifest_id"], "json")
    for path in (SAMPLE / "objects").rglob("*.json"):
        known[content_id(path.read_bytes())] = path.read_bytes()
    ids = sorted(known)
    odd_ids = (5, None, [1], {"a": 1}, "blake3:abc", ids[0].upper(), ids[0] + "\n", "blake3:" + "g" * 64, "")
    counter = iter(range(10**9))

    def case(rng: Rng) -> None:
        root = tmp_path / f"store{next(counter)}"
        root.mkdir()
        crafted = Store(root)
        asked = [rng.choice(ids) for _ in range(1 + rng.below(4))]
        for cid in list(asked):
            data, path = known[cid], crafted.object_path(cid, "json")
            k = rng.below(10)
            if k <= 2:  # mutated, under its own new id
                new = mutate_json_bytes(rng, cjson.loads(data), DONORS, list(known.values()))
                put(root, crafted.object_path(content_id(new), "json"), new)
                asked.append(content_id(new))
            elif k == 3:  # mutated, under the old id
                put(root, path, mutate_bytes(rng, data, [data]))
            elif k == 4:  # one byte over the limit
                put(root, path, data + b" " * (SMALL_LIMIT + 1 - len(data)))
            elif k == 5:
                put(root, path, b"")
                special(rng, root, path, [p for p in regular_files(root) if p != path])
            elif k == 6:  # a file where the folder of objects should be
                put(root, path.parent, b"{}")
            elif k == 7:
                put(root, path, data)
        if rng.chance(30):
            asked.append(rng.choice(odd_ids))
        layered = rng.chance(70)
        objects = Objects(root, WORLD) if layered else Objects(root)
        with reading(files_size(root) + sum(len(v) for v in known.values())):
            for cid in asked:
                for fetch in (objects.read, objects.doc, objects.tile_index, objects.manifest):
                    try:
                        value = fetch(cid)
                    except ChangeError:
                        continue
                    if fetch == objects.read:
                        assert content_id(value) == cid
                    elif fetch != objects.doc:
                        assert isinstance(value, dict) and schemas.errors(value) == []
                if layered and isinstance(cid, str) and cid in known and fixture.object_path(cid, "json").is_file():
                    here = crafted.object_path(cid, "json")
                    if not regular(here):  # nothing readable in front: the fixture's copy
                        assert objects.read(cid) == known[cid]
        shutil.rmtree(root)

    run_cases("changes.Objects", 120, case, memory=MEMORY)


# ------------------------------------------------------------------ apply --


class WarmObjects(Objects):
    """Objects that share, between cases, the checked documents of the
    pristine base stores (`cache`, by (id, format)), so that a case does
    not check the fixture's 45 manifests again. Ids name bytes, and a
    case's crafted source holds only objects under their own ids, so a
    document checked once is the same in every case; crafted objects are
    read and checked every time."""

    def __init__(self, *sources: Any, cache: dict) -> None:
        super().__init__(*sources)
        self.cache = cache

    def tile_index(self, cid: str) -> dict:
        found = self.cache.get((cid, "streetwide.tile")) if isinstance(cid, str) else None
        return found if found is not None else super().tile_index(cid)

    def manifest(self, cid: str) -> dict:
        found = self.cache.get((cid, "streetwide.asset")) if isinstance(cid, str) else None
        return found if found is not None else super().manifest(cid)


def _manifest(layer: str, feature_id: str, tile: str, lo: list[int], hi: list[int], credit: str) -> dict:
    lic = "CC-BY-4.0"
    return {
        "format": "streetwide.asset", "contract_version": "0.1.0",
        "content_id": content_id(f"mesh {layer} {feature_id} {lo} {hi}".encode()), "size_bytes": 1000,
        "layer": layer, "feature_id": feature_id, "tile": tile, "bounds_mm": {"max": hi, "min": lo},
        "physics": {"collider": "none" if layer == "water" else "trimesh"}, "fictional": False,
        "parent_version_id": None,
        "provenance": {
            "license": lic, "attribution": [credit],
            "sources": [{"dataset": "Test", "version": "1", "license": lic, "attribution": credit,
                         "url": "https://example.org/", "source_ids": []}],
            "generator": {"name": "streetwide-ingest", "version": "0.1.0"},
        },
    }


def build_small_world(root: Path) -> None:
    """A world of two tiles and five assets, built without `apply`."""
    store = Store(root)
    assets = [
        _manifest("shell", "osm:way:9", "17/9/5", [0, 0, 0], [10, 10, 10], "© Test"),
        _manifest("shell", "osm:way:10", "17/9/5", [-5, 0, 2], [3, 4, 5], "© Test"),
        _manifest("terrain", "tile:17/9/5", "17/9/5", [-100, -1, -100], [100, 1, 100], "© Terrain"),
        _manifest("terrain", "tile:17/10/5", "17/10/5", [-100, -2, -100], [100, 2, 100], "© Terrain"),
        _manifest("water", "tile:17/10/5", "17/10/5", [-50, 0, -50], [50, 0, 50], "© Water"),
    ]
    tiles = []
    for key, (x, y) in (("17/9/5", (9, 5)), ("17/10/5", (10, 5))):
        entries = sorted(({"layer": m["layer"], "feature_id": m["feature_id"], "manifest_id": store.write_json(m),
                           "content_id": m["content_id"], "license": m["provenance"]["license"],
                           "collider": m["physics"]["collider"], "bounds_mm": m["bounds_mm"]}
                          for m in assets if m["tile"] == key), key=lambda a: (a["layer"], a["feature_id"]))
        index = {"format": "streetwide.tile", "contract_version": "0.1.0", "tile": key, "region": "12/0/0",
                 "origin_mm": [x * 100000, 0, y * 100000], "assets": entries}
        tiles.append({"tile": key, "tile_id": store.write_json(index), "origin_mm": index["origin_mm"],
                      "bounds_mm": changes.union_bounds(a["bounds_mm"] for a in entries)})
    store.write_world({
        "format": "streetwide.world", "contract_version": "0.1.0", "name": "fuzz-world",
        "region": {"key": "12/0/0", "anchor_e9": {"lat": 0, "lon": 0}},
        "attribution": sorted({m["provenance"]["attribution"][0] for m in assets}, key=str.encode),
        "spawn": {"position_mm": [0, 1000, 0], "yaw": 0}, "tiles": tiles,
    })


class Base:
    """A base world for `apply`: its manifest, sources, and assets by key."""

    def __init__(self, sources: list[Path], cache: dict) -> None:
        self.sources = sources
        store = Store(sources[-1])
        self.world_id, self.world = store.read_world()
        self.world_bytes = (sources[-1] / "world.json").read_bytes()
        objects = Objects(*sources)
        self.assets: dict[tuple[str, str, str], tuple[str, dict]] = {}
        for wt in self.world["tiles"]:
            cache[(wt["tile_id"], "streetwide.tile")] = objects.tile_index(wt["tile_id"])
            for a in objects.tile_index(wt["tile_id"])["assets"]:
                m = objects.manifest(a["manifest_id"])
                cache[(a["manifest_id"], "streetwide.asset")] = m
                self.assets[(wt["tile"], a["layer"], a["feature_id"])] = (a["manifest_id"], m)
        self.tiles = [wt["tile"] for wt in self.world["tiles"]]


@pytest.fixture(scope="module")
def apply_bases(tmp_path_factory: pytest.TempPathFactory) -> tuple[list[Base], dict]:
    small = tmp_path_factory.mktemp("small-world")
    build_small_world(small)
    cache: dict = {}
    return [Base([small], cache), Base([SAMPLE, WORLD], cache)], cache


def random_change(rng: Rng, base: Base) -> tuple[dict, dict[str, bytes]]:
    """A change set on `base` and its new manifests by id: removals,
    replacements, additions and moves, as a refresh makes them, with now
    and then a key twice, a tile outside the world or a missing manifest."""
    entries: list[dict] = []
    new: dict[str, bytes] = {}
    keys = list(base.assets)

    def manifest_for(tile: str, layer: str, feature_id: str, parent: str | None, like: dict) -> str:
        m = copy.deepcopy(like)
        m.update(tile=tile, layer=layer, feature_id=feature_id, parent_version_id=parent)
        if rng.chance(50):
            m["bounds_mm"]["max"][1] += rng.below(1000)
        if rng.chance(30):
            m["provenance"]["attribution"] = sorted({*m["provenance"]["attribution"], f"Credit {rng.below(3)}"},
                                                    key=str.encode)
        if rng.chance(20):
            m = mutate_json(rng, m, DONORS, edits=1)
        data = _write_bytes(m)
        new[content_id(data)] = data
        return content_id(data)

    for _ in range(1 + rng.below(4)):
        tile, layer, fid = rng.choice(keys)
        mid, m = base.assets[(tile, layer, fid)]
        k = rng.below(6)
        other = rng.choice(base.tiles) if rng.chance(85) else "17/11/5"  # or a tile outside the world
        if k == 0:
            entries.append({"op": "remove", "tile": tile, "layer": layer, "feature_id": fid,
                            "parent_version_id": mid, "manifest_id": None})
        elif k == 1:
            entries.append({"op": "replace", "tile": tile, "layer": layer, "feature_id": fid, "parent_version_id": mid,
                            "manifest_id": manifest_for(tile, layer, fid, mid, m)})
        elif k == 2:
            fresh = f"streetwide:fuzz-{rng.below(5)}"
            entries.append({"op": "add", "tile": other, "layer": "shell", "feature_id": fresh, "parent_version_id": None,
                            "manifest_id": manifest_for(other, "shell", fresh, None, m)})
        elif k == 3:  # a move to another tile
            entries.append({"op": "remove", "tile": tile, "layer": layer, "feature_id": fid,
                            "parent_version_id": mid, "manifest_id": None})
            entries.append({"op": "add", "tile": other, "layer": layer, "feature_id": fid, "parent_version_id": None,
                            "manifest_id": manifest_for(other, layer, fid, None, m)})
        elif k == 4:  # an add over the current key, or a manifest nobody has
            entries.append({"op": "add", "tile": tile, "layer": layer, "feature_id": fid, "parent_version_id": None,
                            "manifest_id": mid if rng.chance(50) else content_id(b"missing")})
        else:  # a removal of a key that is not there
            entries.append({"op": "remove", "tile": tile, "layer": layer, "feature_id": fid + "x",
                            "parent_version_id": mid, "manifest_id": None})
    change = {"format": "streetwide.change", "contract_version": "0.1.0", "base_world_id": base.world_id,
              "title": "Fuzz", "author": changes.INGEST_AUTHOR, "rationale": "Fuzz.",
              "changes": sorted(entries, key=changes.sort_key)}
    return change, new


def test_fuzz_apply(apply_bases: tuple[list[Base], dict]) -> None:
    """changes.apply on crafted worlds (mutated documents, and mutated bytes
    read as canonical JSON), crafted change sets and crafted manifests and
    tile indexes: a candidate world that is a valid, canonical world
    manifest, with its inputs left as they were, or ChangeError."""
    bases, cache = apply_bases

    def case(rng: Rng) -> None:
        base = bases[0] if rng.chance(75) else bases[1]
        change, new = random_change(rng, base)
        world: Any = base.world
        k = rng.below(10)
        if k == 0:
            world = mutate_json(rng, base.world, DONORS)
        elif k == 1:  # mutated bytes, when they are still canonical JSON
            try:
                world = cjson.loads(mutate_bytes(rng, base.world_bytes, CHANGE_BYTES))
            except cjson.CanonicalError:
                pass
        elif k == 2:  # a world whose tile index is crafted
            world = copy.deepcopy(base.world)
            wt = rng.choice(world["tiles"])
            index = copy.deepcopy(cache[(wt["tile_id"], "streetwide.tile")])
            data = _write_bytes(mutate_json(rng, index, DONORS, edits=1))
            new[content_id(data)] = data
            wt["tile_id"] = content_id(data)
        if rng.chance(30):
            change = mutate_json(rng, change, DONORS)
        objects = WarmObjects(new, *base.sources, cache=cache)
        world_before, change_before = shallow_copy_or_none(world), shallow_copy_or_none(change)
        try:
            with reading(json_size(world) + json_size(change) + sum(map(len, new.values()))):
                candidate = changes.apply(world, change, objects)
        except ChangeError:
            candidate = None
        for value, before in ((world, world_before), (change, change_before)):
            assert before is None or value == before, "apply changed its inputs"
        if candidate is None:
            return
        assert candidate.world_id == content_id(candidate.world_bytes)
        assert cjson.loads(candidate.world_bytes) == candidate.world and schemas.errors(candidate.world) == []
        assert list(candidate.tile_ids) == sorted(candidate.tile_ids, key=changes.tile_xy)
        assert sorted(candidate.objects) == list(candidate.objects) == sorted(candidate.tile_ids.values())
        for tile_id, data in candidate.objects.items():
            index = cjson.loads(data)
            assert content_id(data) == tile_id and schemas.errors(index) == [] and index["assets"]
        attribution = candidate.world["attribution"]
        assert attribution == sorted(set(attribution), key=lambda s: s.encode())

    run_cases("changes.apply", 250, case, memory=MEMORY)


# Regressions: inputs that crashed, hung or disagreed with op_world --------


def nested(levels: int) -> list:
    """A list nested `levels` deep: [] is one level."""
    value: list = []
    for _ in range(levels - 1):
        value = [value]
    return value


def house() -> bytes:
    specs = json.loads((FIXTURES / "format" / "meshes.json").read_text(encoding="utf-8"))
    return glb.write(glb.nodes_from_spec(specs["house"]))


def test_a_lone_surrogate_in_a_glb_is_invalid_json() -> None:
    # A node named "a:\ud800" (a JSON escape) raised UnicodeEncodeError in
    # the feature-id rule. serde_json refuses an unpaired surrogate as
    # invalid JSON; a pair is one character, in both readers.
    json_chunk, bin_chunk = glb_chunks(house())
    name = b'"name":"streetwide:sample-house"'
    for lone in (b'"name":"a:\\ud800"', b'"name":"a:\\udc00\\ud800"'):
        with pytest.raises(glb.GlbError, match="not valid JSON: a string holds a lone surrogate"):
            glb.read(glb_pack(json_chunk.replace(name, lone), bin_chunk))
    with pytest.raises(glb.GlbError, match='node name "a:\U0001F600" is not a feature id'):
        glb.read(glb_pack(json_chunk.replace(name, b'"name":"a:\\ud83d\\ude00"'), bin_chunk))
    # An escaped backslash followed by "ud800" is text: a backslash and five letters.
    mesh = glb.read(glb_pack(json_chunk.replace(name, b'"name":"a:\\\\ud800"'), bin_chunk))
    assert mesh.nodes[0].name == "a:\\ud800"


def test_a_deep_glb_json_chunk_is_refused_before_it_is_parsed() -> None:
    # 100,000 nested arrays raised RecursionError from Python's json.
    with pytest.raises(glb.GlbError) as e:
        glb.read(glb_pack(b"[" * 100_000, b""))
    assert str(e.value) == TOO_DEEP


@pytest.mark.parametrize(
    "shared_mesh,message",
    [(True, "node 1 must show mesh 1"), (False, "accessors read more data than the BIN chunk holds")],
    ids=["meshes", "accessors"],
)
def test_sharing_at_scale_is_refused_with_little_memory(shared_mesh: bool, message: str) -> None:
    # op_world's regression_glb_shared_*_at_scale: 5,000 nodes on 1,000
    # vertices, in under 1 MiB. The Python reader decoded the shared data
    # again for every node before its canonical check refused the file:
    # with 200 nodes, 111 MB and 7 s for a 29 KB file.
    data = shared_glb(5000, 1000, shared_mesh)
    assert len(data) < 1 << 20

    def case(rng: Rng) -> None:
        with reading(len(data)):
            with pytest.raises(glb.GlbError) as e:
                glb.read(data)
        assert str(e.value) == message

    run_cases(f"glb shared {'meshes' if shared_mesh else 'accessors'} at scale", 1, case, memory=(64, 1 << 20))


def test_a_long_json_chunk_is_refused_before_it_is_parsed() -> None:
    # op_world's regression_glb_json_chunk_is_bounded: 600,000 zeros.
    data = glb_pack(b'{"a":[' + b"0," * 600_000 + b"0]}", b"")
    assert len(data) == 1_200_040

    def case(rng: Rng) -> None:
        with reading(len(data)):
            with pytest.raises(glb.GlbError, match="JSON chunk longer than 1048576 bytes"):
                glb.read(data)

    run_cases("glb long JSON chunk", 1, case, memory=(1, 1 << 20))


@pytest.mark.parametrize(
    "edit",
    [
        lambda d: d.update(author="github:\ud800"),
        lambda d: d["changes"][0].update(feature_id="a:\ud800"),
        lambda d: d["changes"][0].update(tile="17/68646/\ud800"),
        lambda d: d["changes"][0].update(layer="\udc00"),
    ],
    ids=["author", "feature-id", "tile", "layer"],
)
def test_lone_surrogates_in_a_change_set_are_problems(edit) -> None:
    # JSON can spell a lone surrogate (\ud800); the field rules and the
    # order of changes encoded such strings as UTF-8: UnicodeEncodeError.
    doc = copy.deepcopy(CHANGE_DOCS[0])
    edit(doc)
    assert changes.problems(doc)
    with pytest.raises(ChangeError):
        changes.to_bytes(doc)


def test_a_change_set_far_over_the_cap_is_checked_as_its_first_changes() -> None:
    # jsonschema takes about a millisecond per change: a change.json of
    # 3,000 changes (600 KB) took 1.5 s to refuse, past the harness's 10 s
    # under tracemalloc on a busy machine. Found by the long campaigns. The
    # set breaks the cap whatever its changes are, so the schema checks the
    # first 51; the other layers still read every change.
    doc = cjson.loads(HAND_MADE_CHANGES["fifty"])
    doc["changes"] = doc["changes"] * 60
    doc["changes"][2000] = dict(doc["changes"][2000], op="rename")
    found = changes.problems(doc)
    (too_long,) = [m for m in found if m.startswith("schema: $.changes: ")]
    assert too_long.endswith(" is too long") and too_long.count("'op'") == changes.MAX_CHANGES + 1
    assert not any("changes[2000]" in m for m in found)
    assert found[-2:] == [f"a change set has 1..={changes.MAX_CHANGES} changes",
                          "changes must be sorted by (tile, layer, feature_id) with no duplicates"]
    with pytest.raises(ChangeError, match=f"a change set has 1..={changes.MAX_CHANGES} changes"):
        changes.parse(cjson.dumps(doc))
    # One over the cap is checked whole.
    doc["changes"] = doc["changes"][: changes.MAX_CHANGES + 1]
    assert [m for m in changes.problems(doc) if m.startswith("schema: $.changes: ")]


def test_lone_surrogates_in_text_are_left_to_canonical_json() -> None:
    # is_text still accepts them (the planner's overrides report them as
    # not canonical); to_bytes raised CanonicalError, not ChangeError.
    doc = dict(CHANGE_DOCS[0], title="Title \ud800")
    assert changes.problems(doc) == []
    with pytest.raises(ChangeError, match="not canonical JSON: string contains a lone surrogate"):
        changes.to_bytes(doc)
    manifest = copy.deepcopy(OTHER_DOCS[-1])
    manifest["provenance"]["attribution"] = ["a\ud800", "b"]  # sorting the credits: UnicodeEncodeError
    assert isinstance(changes.manifest_problems(manifest), list)
    record = {"format": "streetwide.merge", "contract_version": "0.1.0", "decision": "baseline", "change_id": None,
              "world_before": None, "world_after": content_id(b"w"), "check_report_id": None, "reasons": "x",
              "previous_record_id": None,
              "reviews": [{"reviewer": "github:\ud800", "verdict": "better", "notes": "x"},
                          {"reviewer": "github:b", "verdict": "better", "notes": "x"}]}
    assert log.record_problems(record)  # sorting the reviewers: UnicodeEncodeError


@pytest.mark.parametrize("field", ["title", "contract_version", "changes"])
def test_documents_nested_too_deeply(field: str) -> None:
    # Only a document built in memory nests 5,000 levels deep (JSON text
    # that deep is refused when it is parsed). problems, record_problems
    # and apply quoted such values in messages: RecursionError.
    too_deep = ["arrays and objects nested deeper than 127 levels"]
    doc = dict(CHANGE_DOCS[0], **{field: nested(5000)})
    assert changes.problems(doc) == too_deep
    assert changes.schema_problems(changes.SCHEMA_FILE, doc) == ["schema: $: " + too_deep[0]]
    assert log.record_problems({"format": "streetwide.merge", field: nested(5000)}) == too_deep
    _, world, _ = _fixture_assets()
    with pytest.raises(ChangeError, match="the change set: arrays and objects nested deeper than 127 levels"):
        changes.apply(world, {"changes": [nested(5000)]}, Objects(WORLD))
    # At the limit, the rules run as usual.
    assert changes.problems(dict(CHANGE_DOCS[0], **{field: nested(126)})) != too_deep
    assert changes.problems(dict(CHANGE_DOCS[0], **{field: nested(127)})) == too_deep


def test_apply_refuses_what_is_not_a_canonical_world() -> None:
    # A valid tile index given as the world passed its own schema
    # (schemas.errors picks the schema by format), then raised KeyError; a
    # float, which jsonschema counts as an integer, raised CanonicalError.
    _, world, index = _fixture_assets()
    sample = changes.parse((SAMPLE / "change.json").read_bytes())
    with pytest.raises(ChangeError, match="the base world: not a streetwide.world document"):
        changes.apply(index, sample, Objects(SAMPLE, WORLD))
    floating = dict(world, spawn=dict(world["spawn"], yaw=1.0))
    with pytest.raises(ChangeError, match="the base world: not canonical JSON: number 1.0 is not an integer"):
        changes.apply(floating, sample, Objects(SAMPLE, WORLD))


@pytest.mark.parametrize("cid", [[1], {"a": 1}, None, 5, "blake3:abc"], ids=["list", "object", "null", "number", "short"])
def test_objects_refuse_ids_of_any_json_type(cid: Any) -> None:
    # A list or an object as an id raised TypeError (unhashable) from the
    # document cache. Found by the Objects fuzz target.
    objects = Objects(WORLD)
    for fetch in (objects.read, objects.doc, objects.tile_index, objects.manifest):
        with pytest.raises(ChangeError, match="is not a content id"):
            fetch(cid)


def test_objects_take_only_regular_files_within_the_limit(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    # Objects read whatever a path named, of any size, through links, where
    # op_world's layered store takes only regular files of at most
    # MAX_FILE_BYTES. Anything else in one source leaves the object to the
    # next source, as in op_world.
    _, world, _ = _fixture_assets()
    tile_id = world["tiles"][0]["tile_id"]
    data = Store(WORLD).read_object(tile_id, "json")
    monkeypatch.setattr(changes, "MAX_FILE_BYTES", len(data))
    front = Store(tmp_path / "front")
    path = front.object_path(tile_id, "json")
    path.mkdir(parents=True)  # a directory where the object should be
    assert Objects(front.root, WORLD).read(tile_id) == data
    path.rmdir()
    path.write_bytes(data + b" ")  # a regular file decides, and this one is too large
    with pytest.raises(ChangeError, match=f"object {tile_id}: .* is larger than {len(data)} bytes"):
        Objects(front.root, WORLD).read(tile_id)
    path.unlink()
    try:
        os.symlink(Store(WORLD).object_path(tile_id, "json").resolve(), path)
    except OSError:
        pass  # Windows without the privilege to make links
    else:
        with pytest.raises(ChangeError, match="not found"):
            Objects(front.root).read(tile_id)
        assert Objects(front.root, WORLD).read(tile_id) == data
        path.unlink()
    if POSIX:
        os.mkfifo(path)  # reading it would block
        assert Objects(front.root, WORLD).read(tile_id) == data


def test_the_log_reads_only_regular_files_within_the_limit(log_template: Path, tmp_path: Path,
                                                           monkeypatch: pytest.MonkeyPatch) -> None:
    # read_log read whatever a path named: files over the limit were read
    # whole, a link was followed, and a FIFO blocked it for ever.
    root = tmp_path / "log"
    shutil.copytree(log_template, root)
    world_log = log.read_log(root)
    record = root / world_log.records[1].path
    monkeypatch.setattr(log, "MAX_FILE_BYTES", record.stat().st_size - 1)
    with pytest.raises(LogError, match=f"cannot read {world_log.records[1].path}: .* is larger than"):
        log.read_log(root)
    world_id = world_log.records[1].world_after
    monkeypatch.setattr(log, "MAX_FILE_BYTES", (root / log.world_path(world_id)).stat().st_size - 1)
    with pytest.raises(LogError, match=f"cannot read {log.world_path(world_id)}: .* is larger than"):
        world_log.world(world_id)
    monkeypatch.undo()
    rel = log.change_path(world_log.records[1].change_id)
    path = root / rel
    data = path.read_bytes()
    path.unlink()
    path.mkdir()
    with pytest.raises(LogError, match=f"cannot read {rel}: .* is not a regular file"):
        log.read_log(root)
    path.rmdir()
    (tmp_path / "elsewhere.json").write_bytes(data)
    try:
        os.symlink(tmp_path / "elsewhere.json", path)
    except OSError:
        pass  # Windows without the privilege to make links
    else:
        with pytest.raises(LogError, match=f"cannot read {rel}: .* is not a regular file"):
            log.read_log(root)
        path.unlink()
    if POSIX:
        os.mkfifo(path)
        with pytest.raises(LogError, match=f"cannot read {rel}: .* is not a regular file"):
            log.read_log(root)


def test_a_log_keeps_worlds_and_reports_of_their_own_kind(log_template: Path, tmp_path: Path) -> None:
    # world() returned any document stored under the id it was given, such
    # as a change set: a record may name any file's id as a world.
    root = tmp_path / "log"
    shutil.copytree(log_template, root)
    world_log = log.read_log(root)
    change_id = world_log.records[1].change_id
    change = (root / log.change_path(change_id)).read_bytes()
    (root / log.world_path(change_id)).write_bytes(change)
    with pytest.raises(LogError, match=f"{log.world_path(change_id)} is not a streetwide.world document"):
        world_log.world(change_id)
    (root / log.report_path(change_id)).write_bytes(change)
    with pytest.raises(LogError, match=f"{log.report_path(change_id)} is not a streetwide.check document"):
        world_log.report(change_id)
    assert world_log.world(content_id(b"not kept")) is None


def test_a_log_file_name_that_is_not_text_is_named(log_template: Path, tmp_path: Path) -> None:
    # Undecodable bytes (POSIX) or an unpaired surrogate (Windows) in a file
    # name raised UnicodeEncodeError when the names were sorted.
    root = tmp_path / "log"
    shutil.copytree(log_template, root)
    try:
        (root / "log" / UNDECODABLE_NAMES[0]).write_bytes(b"{}")
    except OSError:
        pytest.skip("this file system takes only UTF-8 names")
    with pytest.raises(LogError, match="the world log has log/\ufffd+.json where log/00000004.json should be"):
        log.read_log(root)


def test_a_change_set_named_by_many_records_is_read_once(tmp_path: Path) -> None:
    # read_log read, hashed and parsed a change set again for every record
    # that named it: 400 takedown records naming one 55 KB change set (a
    # 277 KB log) took 45 s and kept 400 parsed copies. op_world reads each
    # change set once; so does read_log now. Found by review.
    big = {
        "format": "streetwide.change", "contract_version": "0.1.0", "base_world_id": content_id(b"world 0"),
        "title": "\U0001F600" * 4096, "author": "github:" + "a" * 249, "rationale": "\U0001F600" * 4096,
        "changes": sorted(({"op": "add", "tile": TILE, "layer": "shell", "feature_id": f"a:{k:03d}" + "x" * 249,
                            "parent_version_id": None, "manifest_id": content_id(b"m%d" % k)} for k in range(50)),
                          key=changes.sort_key),
    }
    change = changes.to_bytes(big)
    change_id = content_id(change)
    put(tmp_path, tmp_path / log.change_path(change_id), change)
    record = {"format": "streetwide.merge", "contract_version": "0.1.0", "decision": "baseline", "change_id": None,
              "world_before": None, "world_after": content_id(b"world 0"), "check_report_id": None, "reviews": [],
              "reasons": "x", "previous_record_id": None}
    previous = cjson.dumps(record)
    put(tmp_path, tmp_path / log.record_path(0), previous)
    for i in range(1, 301):
        record = dict(record, decision="takedown", change_id=change_id, world_before=record["world_after"],
                      world_after=content_id(b"world %d" % i), check_report_id=content_id(b"report"),
                      previous_record_id=content_id(previous))
        previous = cjson.dumps(record)
        put(tmp_path, tmp_path / log.record_path(i), previous)
    size = files_size(tmp_path)
    assert len(change) > 50_000 and size < 250_000

    def case(rng: Rng) -> None:
        with reading(size):
            world_log = log.read_log(tmp_path)
        assert len(world_log.records) == 301
        assert len({id(r.change) for r in world_log.records[1:]}) == 1

    run_cases("log naming one change set 300 times", 1, case, memory=MEMORY)


@pytest.mark.parametrize("folder", ["log", "changes", "change set", "worlds", "reports"])
def test_the_log_follows_no_link_to_a_directory(log_template: Path, tmp_path: Path, folder: str) -> None:
    # read_log checked only the last part of a path, so a link (on Windows, a
    # junction, which needs no privilege) at `log`, `changes`,
    # `changes/<hex>`, `worlds` or `reports` was followed, to a copy here or
    # anywhere else. op_world refuses every link inside a log, so the
    # planner took logs that `op_world merge --log` refuses. Found by review.
    root = tmp_path / "log"
    shutil.copytree(log_template, root)
    world_log = log.read_log(root)
    merged = world_log.records[1]
    rel = {"change set": f"changes/{merged.change_id[7:]}"}.get(folder, folder)
    os.replace(root / rel, tmp_path / "copy")
    if not link_directory(tmp_path / "copy", root / rel):
        pytest.skip("this system cannot make a link to a directory")
    if folder == "log":
        with pytest.raises(LogError, match="^the world log's log is not a directory; "):
            log.read_log(root)
    elif folder in ("changes", "change set"):
        with pytest.raises(LogError, match=f"^cannot read {log.change_path(merged.change_id)}: {rel} is a link$"):
            log.read_log(root)
    elif folder == "worlds":
        with pytest.raises(LogError, match=f"^cannot read {log.world_path(merged.world_after)}: worlds is a link$"):
            log.read_log(root).world(merged.world_after)
    else:
        report = merged.record["check_report_id"]
        with pytest.raises(LogError, match=f"^cannot read {log.report_path(report)}: reports is a link$"):
            log.read_log(root).report(report)
    clear(root / rel)
    os.replace(tmp_path / "copy", root / rel)
    assert len(log.read_log(root).records) == len(world_log.records)


# ------------------------------------------------------- JSON size limits --
#
# RFC 0001 section 7: every kind of JSON document has a size limit, checked
# before it is parsed, in op_world and here alike. Found by the fuzzing
# round: a crafted 64 MiB document, the largest file a store read takes,
# made serde_json take about 7 GB.


def test_the_limits_are_op_worlds() -> None:
    rust = (REPO / "crates" / "op_world" / "src" / "limits.rs").read_text(encoding="utf-8")
    names = {"ASSET_MANIFEST": limits.ASSET_MANIFEST, "MERGE_RECORD": limits.MERGE_RECORD,
             "CHANGE_SET": limits.CHANGE_SET, "CHECK_REPORT": limits.CHECK_REPORT,
             "TILE_INDEX": limits.TILE_INDEX, "WORLD_MANIFEST": limits.WORLD_MANIFEST, "POINTS": limits.POINTS}
    values = {"MIB": limits.MIB, "MAX_FILE_BYTES": MAX_FILE_BYTES}
    consts = dict(re.findall(r"pub const MAX_(\w+)_BYTES: u64 = ([^;]+);", rust))
    assert consts.keys() == names.keys()
    for name, expression in consts.items():
        value = math.prod(values[t] if t in values else int(t) for t in map(str.strip, expression.split("*")))
        assert value == names[name].max_bytes, name
    words = dict(re.findall(r'Document::(\w+) => "(an? [a-z ]+)",', rust))
    assert {re.sub(r"(?<!^)([A-Z])", r"_\1", k).upper(): v for k, v in words.items()} == {
        k: kind.name for k, kind in names.items()}
    assert [kind.format for kind in limits.KINDS] == [
        "streetwide.asset", "streetwide.tile", "streetwide.world", "streetwide.change", "streetwide.check",
        "streetwide.merge", "streetwide.points"]
    assert '"larger than {limit} bytes ({} MiB), the limit for {}"' in rust
    assert limits.CHANGE_SET.too_large() == "larger than 8388608 bytes (8 MiB), the limit for a change set"


def padding(i: int, n: int) -> str:
    """Text number `i` of `n` characters: its number, then x. Texts sort by
    their numbers, and `{i:07d}x` appears nowhere else in a document: not
    in a content id, whose hex has no x."""
    return f"{i:07d}" + "x" * (n - 7)


def lengths_for(size: int, low: int, high: int, doc) -> list[int]:
    """Lengths of items for `doc` whose bytes are exactly `size` long, as in
    op_world's fuzz_readers.rs: `doc` writes one item per length, and an
    item's bytes grow by one for each unit of its length, from `low` to
    `high`, so only small documents are written to find them."""
    mid = (low + high) // 2
    one = len(doc([mid]))
    per_item = len(doc([mid, mid])) - one
    count = (size - one) // per_item + 1
    lengths = [mid] * count
    left = size - (one + (count - 1) * per_item)
    for i, n in enumerate(lengths):
        grow = min(left, high - n)
        lengths[i] += grow
        left -= grow
    assert left == 0, f"cannot make {size} bytes"
    return lengths


def one_byte_more(data: bytes, lengths: list[int], high: int) -> bytes:
    """`data`, written by a builder below from `lengths`, with one more
    character in the first item that can take one."""
    k = next(i for i, n in enumerate(lengths) if n < high)
    mark = f"{k:07d}x".encode()
    assert data.count(mark) == 1
    return data.replace(mark, mark + b"x")


def dumps(doc: Any) -> bytes:
    """`cjson.dumps(doc)` at the speed of the json module, for documents
    whose keys are ASCII and whose texts need no escape (checked below)."""
    return json.dumps(doc, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def manifest_of(lengths: list[int]) -> bytes:
    m = copy.deepcopy(OTHER_DOCS[-1])
    m["provenance"]["sources"][0]["source_ids"] = [padding(i, n) for i, n in enumerate(lengths)]
    return dumps(m)


def tile_index_of(lengths: list[int]) -> bytes:
    index = copy.deepcopy(_fixture_assets()[2])
    first = index["assets"][0]
    index["assets"] = [dict(first, layer="facade", feature_id="t:" + padding(i, n)) for i, n in enumerate(lengths)]
    return dumps(index)


def record_of(lengths: list[int]) -> bytes:
    return dumps({
        "format": "streetwide.merge", "contract_version": "0.1.0", "decision": "baseline", "change_id": None,
        "world_before": None, "world_after": content_id(b"world"), "check_report_id": None, "reasons": "x",
        "previous_record_id": None,
        "reviews": [{"reviewer": f"github:r{i:07d}", "verdict": "better", "notes": padding(i, n)}
                    for i, n in enumerate(lengths)],
    })


def read_kind(kind: limits.Kind, data: bytes, tmp_path: Path) -> Any:
    """`data` read as a document of `kind`: change sets and merge records by
    their parsers, check reports from a world log, manifests as objects
    from memory, tile indexes too but unparsed (tens of thousands of assets
    take jsonschema too long; objects in a directory are refused below).
    Raises what each raises."""
    cid = content_id(data)
    if kind is limits.CHANGE_SET:
        return changes.parse(data)
    if kind is limits.MERGE_RECORD:
        return log.parse_record(data)
    if kind is limits.CHECK_REPORT:
        root = tmp_path / "log"
        path = root / log.report_path(cid)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        try:
            return log.WorldLog(root=root, records=()).report(cid)
        finally:
            shutil.rmtree(root)
    if kind is limits.TILE_INDEX:
        return Objects({cid: data}).read(cid, kind)
    return Objects({cid: data}).manifest(cid)


# (kind, builder, longest item): a builder writes a valid document of the
# kind from item lengths. Change sets cannot reach their limit (50 changes
# of a few hundred bytes), and check reports take long to write and parse
# at 16 MiB here, so for them a document of spaces stands in: at the limit
# the parser refuses it, not its size.
AT_THE_LIMIT = {
    "asset manifest": (limits.ASSET_MANIFEST, manifest_of, changes.MAX_TEXT_CHARS),
    "merge record": (limits.MERGE_RECORD, record_of, changes.MAX_TEXT_CHARS),
    "tile index": (limits.TILE_INDEX, tile_index_of, 254),
    "change set": (limits.CHANGE_SET, None, 0),
    "check report": (limits.CHECK_REPORT, None, 0),
}


@pytest.mark.parametrize("what", list(AT_THE_LIMIT))
def test_documents_read_up_to_the_limit_for_their_kind(what: str, tmp_path: Path) -> None:
    # A document at the limit for its kind reads (or goes on to the parser);
    # one byte more is refused before it is parsed, with op_world's message.
    kind, doc, longest = AT_THE_LIMIT[what]
    if doc is None:
        at = b" " * kind.max_bytes
        with pytest.raises(ValueError, match="not canonical JSON: invalid JSON: Expecting value"):
            read_kind(kind, at, tmp_path)
        over = at + b" "
    else:
        assert doc([8, 9]) == cjson.dumps(cjson.loads(doc([8, 9]))), "the builder writes canonical JSON"
        lengths = lengths_for(kind.max_bytes, 8, longest, doc)
        at = doc(lengths)
        assert len(at) == kind.max_bytes
        assert read_kind(kind, at, tmp_path) == (at if kind is limits.TILE_INDEX else cjson.loads(at))
        over = one_byte_more(at, lengths, longest)
    assert len(over) == kind.max_bytes + 1
    with pytest.raises(ValueError) as refused:
        read_kind(kind, over, tmp_path)
    assert str(refused.value).endswith(kind.too_large())


def worst_case(kind: limits.Kind) -> bytes:
    """One byte over the limit of `kind`: one-key objects, the input that
    takes a JSON parser the most memory per byte."""
    data = b"[" + b'{"a":0},' * (kind.max_bytes // 8)
    return data[: kind.max_bytes].ljust(kind.max_bytes, b" ") + b"]"


@pytest.mark.parametrize("kind", [limits.CHANGE_SET, limits.MERGE_RECORD, limits.CHECK_REPORT,
                                  limits.TILE_INDEX, limits.ASSET_MANIFEST], ids=lambda k: k.format)
def test_documents_over_their_limit_are_refused_unparsed(kind: limits.Kind, tmp_path: Path) -> None:
    # The worst case for the parser, one byte over the limit: refused before
    # it is parsed, with the memory of the input alone (parsing it would
    # take hundreds of megabytes, gigabytes at 16 MiB), and an object in a
    # directory before it is read.
    data = worst_case(kind)
    assert len(data) == kind.max_bytes + 1
    cid = content_id(data)
    store = Store(tmp_path / "store")
    in_store = kind in (limits.TILE_INDEX, limits.ASSET_MANIFEST)
    if in_store:
        path = store.object_path(cid, "json")
        path.parent.mkdir(parents=True)
        with open(path, "wb") as f:
            f.truncate(len(data))  # sparse where the file system allows

    def case(rng: Rng) -> None:
        with reading(len(data)):
            with pytest.raises(ValueError) as refused:
                read_kind(kind, data, tmp_path)
        assert str(refused.value).endswith(kind.too_large())
        if in_store:
            with reading(0):
                with pytest.raises(ChangeError, match=f"^object {cid}: {re.escape(kind.too_large())}$"):
                    Objects(store.root).doc(cid, kind)

    try:
        run_cases(f"{kind.format} over its limit", 1, case, memory=(1, 1 << 20))
    finally:
        shutil.rmtree(store.root, ignore_errors=True)


def test_a_log_refuses_records_and_change_sets_over_their_limit(log_template: Path, tmp_path: Path) -> None:
    # As op_world's WorldLog::open, with its messages after the file name.
    root = tmp_path / "log"
    shutil.copytree(log_template, root)
    world_log = log.read_log(root)
    record = world_log.records[1].path
    (root / record).write_bytes(worst_case(limits.MERGE_RECORD))
    with pytest.raises(LogError, match=f"^{record}: {re.escape(limits.MERGE_RECORD.too_large())}$"):
        log.read_log(root)
    shutil.rmtree(root)
    shutil.copytree(log_template, root)
    rel = log.change_path(world_log.records[1].change_id)
    (root / rel).write_bytes(worst_case(limits.CHANGE_SET))
    with pytest.raises(LogError, match=f"^{rel}: {re.escape(limits.CHANGE_SET.too_large())}$"):
        log.read_log(root)
    shutil.rmtree(root)


def test_writers_refuse_what_readers_would(monkeypatch: pytest.MonkeyPatch) -> None:
    # As op_world's to_bytes: a change set, or a tile index or world that
    # `apply` builds, over the limit for its kind is not written. Smaller
    # limits stand in for the real ones here.
    sample = changes.parse((SAMPLE / "change.json").read_bytes())
    small = len(changes.to_bytes(sample)) - 1
    monkeypatch.setattr(changes, "CHANGE_SET", limits.Kind("streetwide.change", "a change set", small))
    with pytest.raises(ChangeError, match=f"^larger than {small} bytes \\(0 MiB\\), the limit for a change set$"):
        changes.to_bytes(sample)
    with pytest.raises(ChangeError, match="the limit for a change set$"):
        changes.parse(cjson.dumps(sample))
    monkeypatch.undo()
    _, world, _ = _fixture_assets()
    candidate = changes.apply(world, sample, Objects(SAMPLE, WORLD))
    ((key, tile_id),) = candidate.tile_ids.items()
    tile_limit = len(candidate.objects[tile_id]) - 1
    monkeypatch.setattr(changes, "TILE_INDEX", limits.Kind("streetwide.tile", "a tile index", tile_limit))
    with pytest.raises(ChangeError, match=f"^tile index {key}: larger than {tile_limit} bytes"):
        changes.apply(world, sample, Objects(SAMPLE, WORLD))
    monkeypatch.undo()
    world_limit = len(candidate.world_bytes) - 1
    assert len((WORLD / "world.json").read_bytes()) <= world_limit, "the base world fits"
    monkeypatch.setattr(changes, "WORLD_MANIFEST", limits.Kind("streetwide.world", "a world manifest", world_limit))
    with pytest.raises(ChangeError, match=f"^the candidate world: larger than {world_limit} bytes"):
        changes.apply(world, sample, Objects(SAMPLE, WORLD))
