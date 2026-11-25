"""StreetWide glb profile 0.1 writer and strict reader (RFC 0001, section 6).

`write` produces exactly the bytes of `op_world::glb::write` for the same
mesh. Positions are given as integer millimetres; each becomes the f32
nearest to the double `mm / 1000`, the same two-step rounding as the Rust
writer.

`read` mirrors `op_world::glb::read`: it parses a file, checks the profile
rules, and accepts it only if writing the parsed mesh gives the same bytes.
It returns each primitive's positions as raw f32 values with their bit
patterns, the same positions in integer millimetres (the profile snaps
every position to the millimetre), the normals and the u32 indices. The
profile has no node transforms, so these positions are final: they are in
the tile frame, as the gate's rules assume. `missing_boundary_vertices`
mirrors the gate's terrain-edge rule (G8).
"""

from __future__ import annotations

import json
import math
import re
import struct
from collections.abc import Iterator
from dataclasses import dataclass, field

from tools.worldfmt.cjson import write_string

GENERATOR = "StreetWide world format 0.1"

PALETTE = ["roof", "wall", "ground", "terrain", "road", "path", "bridge", "water"]
BASE_COLOR_MILLI = {
    "roof": (600, 250, 180, 1000),
    "wall": (850, 820, 760, 1000),
    "ground": (500, 480, 450, 1000),
    "terrain": (420, 450, 380, 1000),
    "road": (300, 300, 310, 1000),
    "path": (600, 580, 550, 1000),
    "bridge": (550, 540, 520, 1000),
    "water": (200, 350, 450, 1000),
}

_GLB_MAGIC = 0x46546C67
_CHUNK_JSON = 0x4E4F534A
_CHUNK_BIN = 0x004E4942
_ARRAY_BUFFER = 34962
_ELEMENT_ARRAY_BUFFER = 34963
_FLOAT = 5126
_UNSIGNED_INT = 5125


class GlbError(ValueError):
    """The mesh is outside the StreetWide glb profile."""


@dataclass
class Primitive:
    material: str
    positions_mm: list[tuple[int, int, int]]
    normals: list[tuple[float, float, float]]
    indices: list[int]


@dataclass
class Node:
    name: str
    primitives: list[Primitive] = field(default_factory=list)


@dataclass
class Milli:
    """A number written in shortest decimal form of thousandths."""

    k: int


def milli_str(k: int) -> str:
    whole, frac = divmod(abs(k), 1000)
    s = ("-" if k < 0 else "") + str(whole)
    if frac:
        s += "." + f"{frac:03d}".rstrip("0")
    return s


def _json(v, out: list[str]) -> None:
    if isinstance(v, Milli):
        out.append(milli_str(v.k))
    elif isinstance(v, bool):
        raise GlbError("booleans are not used in the profile")
    elif isinstance(v, int):
        out.append(str(v))
    elif isinstance(v, str):
        write_string(v, out)
    elif isinstance(v, list):
        out.append("[")
        for i, item in enumerate(v):
            if i:
                out.append(",")
            _json(item, out)
        out.append("]")
    elif isinstance(v, dict):
        out.append("{")
        for i, k in enumerate(sorted(v, key=lambda s: s.encode("utf-8"))):
            if i:
                out.append(",")
            write_string(k, out)
            out.append(":")
            _json(v[k], out)
        out.append("}")
    else:
        raise GlbError(f"cannot write {type(v).__name__}")


def f32_of_mm(mm: int) -> bytes:
    return struct.pack("<f", mm / 1000)


def _validate(nodes: list[Node]) -> None:
    if not nodes:
        raise GlbError("a mesh needs at least one node")
    for node in nodes:
        if not node.primitives:
            raise GlbError(f"node {node.name!r} has no primitives")
        order = [PALETTE.index(p.material) for p in node.primitives]
        if order != sorted(set(order)):
            raise GlbError(f"node {node.name!r}: materials must be distinct, in palette order")
        for p in node.primitives:
            n = len(p.positions_mm)
            if n == 0 or len(p.normals) != n:
                raise GlbError(f"node {node.name!r}: bad vertex or normal count")
            if not p.indices or len(p.indices) % 3:
                raise GlbError(f"node {node.name!r}: index count must be a positive multiple of 3")
            if any(not 0 <= i < n for i in p.indices):
                raise GlbError(f"node {node.name!r}: index out of range")


def write(nodes: list[Node]) -> bytes:
    """Writes a profile-0.1 glb."""
    _validate(nodes)
    used = [m for m in PALETTE if any(p.material == m for n in nodes for p in n.primitives)]
    bin_parts: list[bytes] = []
    bin_len = 0
    views: list[dict] = []
    accessors: list[dict] = []
    meshes: list[dict] = []
    gl_nodes: list[dict] = []

    def push_view(data: bytes, target: int) -> int:
        nonlocal bin_len
        views.append({"buffer": 0, "byteLength": len(data), "byteOffset": bin_len, "target": target})
        bin_parts.append(data)
        bin_len += len(data)
        return len(views) - 1

    for node_index, node in enumerate(nodes):
        prims = []
        for p in node.primitives:
            lo = [min(v[i] for v in p.positions_mm) for i in range(3)]
            hi = [max(v[i] for v in p.positions_mm) for i in range(3)]
            first = len(accessors)
            pos = b"".join(f32_of_mm(c) for v in p.positions_mm for c in v)
            accessors.append({
                "bufferView": push_view(pos, _ARRAY_BUFFER),
                "componentType": _FLOAT,
                "count": len(p.positions_mm),
                "max": [Milli(k) for k in hi],
                "min": [Milli(k) for k in lo],
                "type": "VEC3",
            })
            nrm = b"".join(struct.pack("<f", float(c)) for v in p.normals for c in v)
            accessors.append({
                "bufferView": push_view(nrm, _ARRAY_BUFFER),
                "componentType": _FLOAT,
                "count": len(p.normals),
                "type": "VEC3",
            })
            idx = struct.pack(f"<{len(p.indices)}I", *p.indices)
            accessors.append({
                "bufferView": push_view(idx, _ELEMENT_ARRAY_BUFFER),
                "componentType": _UNSIGNED_INT,
                "count": len(p.indices),
                "type": "SCALAR",
            })
            prims.append({
                "attributes": {"NORMAL": first + 1, "POSITION": first},
                "indices": first + 2,
                "material": used.index(p.material),
                "mode": 4,
            })
        meshes.append({"primitives": prims})
        gl_nodes.append({"mesh": node_index, "name": node.name})

    materials = [
        {
            "name": m,
            "pbrMetallicRoughness": {
                "baseColorFactor": [Milli(k) for k in BASE_COLOR_MILLI[m]],
                "metallicFactor": 0,
                "roughnessFactor": 1,
            },
        }
        for m in used
    ]
    doc = {
        "accessors": accessors,
        "asset": {"generator": GENERATOR, "version": "2.0"},
        "bufferViews": views,
        "buffers": [{"byteLength": bin_len}],
        "materials": materials,
        "meshes": meshes,
        "nodes": gl_nodes,
        "scene": 0,
        "scenes": [{"nodes": list(range(len(nodes)))}],
    }
    out: list[str] = []
    _json(doc, out)
    json_bytes = "".join(out).encode("utf-8")
    json_bytes += b" " * (-len(json_bytes) % 4)
    bin_bytes = b"".join(bin_parts)
    total = 12 + 8 + len(json_bytes) + 8 + len(bin_bytes)
    return b"".join([
        struct.pack("<III", _GLB_MAGIC, 2, total),
        struct.pack("<II", len(json_bytes), _CHUNK_JSON),
        json_bytes,
        struct.pack("<II", len(bin_bytes), _CHUNK_BIN),
        bin_bytes,
    ])


def nodes_from_spec(spec: dict) -> list[Node]:
    """Builds nodes from the `fixtures/format/meshes.json` description."""
    return [
        Node(
            name=n["name"],
            primitives=[
                Primitive(
                    material=p["material"],
                    positions_mm=[tuple(v) for v in p["positions_mm"]],
                    normals=[tuple(v) for v in p["normals"]],
                    indices=list(p["indices"]),
                )
                for p in n["primitives"]
            ],
        )
        for n in spec["nodes"]
    ]


# ---------------------------------------------------------------- reader --

MAX_GLB_BYTES = 64 * 1024 * 1024
"""Largest file `read` accepts, as `op_world::glb::MAX_GLB_BYTES`."""

_GLB_VERSION = 2
_FEATURE_ID = re.compile(r"[a-z][a-z0-9_-]*:[!-~]+")
_MAX_FEATURE_ID_BYTES = 256
# `f32_to_mm` gives up above this many millimetres, as op_world does.
_MAX_SNAPPED_MM = 9_000_000_000_000_000
_MAX_U64 = (1 << 64) - 1


@dataclass(frozen=True)
class MeshPrimitive:
    """One primitive of a mesh read by `read`.

    Vertex `i` has `positions[i]` (its f32 coordinates, exact as Python
    floats), `position_bits[i]` (the same f32 values as bit patterns, the
    gate's `f32::to_bits`), `positions_mm[i]` (the same position in integer
    millimetres in the tile frame) and `normals[i]` (f32 values).
    `indices` holds the u32 vertex indices, three per triangle.
    """

    material: str
    positions: tuple[tuple[float, float, float], ...]
    position_bits: tuple[tuple[int, int, int], ...]
    positions_mm: tuple[tuple[int, int, int], ...]
    normals: tuple[tuple[float, float, float], ...]
    indices: tuple[int, ...]


@dataclass(frozen=True)
class MeshNode:
    """A named part of a mesh: `name` is the feature id of what it shows."""

    name: str
    primitives: tuple[MeshPrimitive, ...]


@dataclass(frozen=True)
class Mesh:
    """The content of a profile glb, as `op_world::glb::GlbMesh`."""

    nodes: tuple[MeshNode, ...]

    def primitives(self) -> Iterator[MeshPrimitive]:
        """Every primitive, node by node, in file order."""
        for node in self.nodes:
            yield from node.primitives

    def triangle_count(self) -> int:
        return sum(len(p.indices) // 3 for p in self.primitives())

    def bounds_mm(self) -> dict | None:
        """The bounds of every position in millimetres, in the form of a
        manifest's `bounds_mm` (`{"max": [x, y, z], "min": [x, y, z]}`), or
        None for a mesh without vertices."""
        points = [v for p in self.primitives() for v in p.positions_mm]
        if not points:
            return None
        return {
            "max": [max(v[k] for v in points) for k in range(3)],
            "min": [min(v[k] for v in points) for k in range(3)],
        }

    def triangles_mm(
        self,
    ) -> Iterator[tuple[tuple[int, int, int], tuple[int, int, int], tuple[int, int, int]]]:
        """Every triangle as its three corners in integer millimetres (tile
        frame), primitive by primitive, in index order."""
        for p in self.primitives():
            idx, pos = p.indices, p.positions_mm
            for k in range(0, len(idx) - 2, 3):
                yield pos[idx[k]], pos[idx[k + 1]], pos[idx[k + 2]]

    def to_nodes(self) -> list[Node]:
        """The mesh as writer input: `write(mesh.to_nodes())` gives back the
        exact bytes the mesh was read from."""
        return [
            Node(
                name=n.name,
                primitives=[
                    Primitive(
                        material=p.material,
                        positions_mm=list(p.positions_mm),
                        normals=list(p.normals),
                        indices=list(p.indices),
                    )
                    for p in n.primitives
                ],
            )
            for n in self.nodes
        ]


def mm_to_f32_bits(mm: int) -> int:
    """The bit pattern of the f32 the profile stores for `mm` millimetres:
    the f32 nearest to the double `mm / 1000` (`op_world::glb::mm_to_f32`)."""
    return struct.unpack("<I", f32_of_mm(mm))[0]


def f32_to_mm(bits: int) -> int | None:
    """The millimetres of a snapped coordinate given by its f32 bit pattern,
    or None when the f32 is not `mm_to_f32_bits(mm)` for any integer `mm`
    (-0.0, NaN and the infinities included). Mirrors
    `op_world::glb::f32_to_mm`."""
    value = struct.unpack("<f", struct.pack("<I", bits))[0]
    if not math.isfinite(value):
        return None
    mm = _round_half_away(value * 1000.0)
    if abs(mm) > _MAX_SNAPPED_MM:
        return None
    return mm if mm_to_f32_bits(mm) == bits else None


def _round_half_away(x: float) -> int:
    """`f64::round` on a finite float: the nearest integer, ties away from
    zero, computed exactly."""
    a = abs(x)
    whole = math.floor(a)
    # Exact: below 1 the difference is `a` itself, and above it `a` and its
    # integer part are within a factor of two of each other.
    if a - whole >= 0.5:
        whole += 1
    return -whole if x < 0 else whole


def read(data: bytes) -> Mesh:
    """Reads a profile-0.1 glb, strictly, as `op_world::glb::read` does.

    Raises GlbError, with the reason, for anything outside the profile: a
    broken container, references or accessors, a material outside the
    palette, positions not snapped to the millimetre, and any byte that
    differs from what `write` produces for the parsed mesh (node
    transforms, extensions, URIs, padding, JSON layout).
    """
    data = bytes(data)
    nodes = _parse(data)
    mesh = _checked(nodes)
    if write(mesh.to_nodes()) != data:
        raise GlbError("not in the canonical StreetWide glb form (re-writing it changes the bytes)")
    return mesh


def boundary_vertices(mesh: Mesh) -> list[tuple[int, int, int]]:
    """The positions on the open edges of `mesh`, as f32 bit patterns,
    sorted. As in the gate: edges are counted per primitive by their pair of
    vertex indices, and an edge used by exactly one triangle is open."""
    found: set[tuple[int, int, int]] = set()
    for p in mesh.primitives():
        edges: dict[tuple[int, int], int] = {}
        idx = p.indices
        for k in range(0, len(idx) - 2, 3):
            a, b, c = idx[k], idx[k + 1], idx[k + 2]
            for u, v in ((a, b), (b, c), (c, a)):
                edge = (u, v) if u <= v else (v, u)
                edges[edge] = edges.get(edge, 0) + 1
        for (u, v), uses in edges.items():
            if uses == 1:
                found.add(p.position_bits[u])
                found.add(p.position_bits[v])
    return sorted(found)


def missing_boundary_vertices(old: Mesh, new: Mesh) -> int:
    """How many positions on the open edges of `old` the new mesh lacks,
    compared bit for bit: the count the gate's G8 reports for a terrain
    replacement ("<n> edge vertices differ from the old terrain"). Mirrors
    `missing_boundary_vertices` in crates/op_world/src/gate.rs."""
    have = {bits for p in new.primitives() for bits in p.position_bits}
    return sum(1 for v in boundary_vertices(old) if v not in have)


def _is_feature_id(s: str) -> bool:
    # `op_world::rules::is_feature_id`.
    return len(s.encode("utf-8")) <= _MAX_FEATURE_ID_BYTES and _FEATURE_ID.fullmatch(s) is not None


def _quote(s: str) -> str:
    return json.dumps(s, ensure_ascii=False)


def _is_int(v: object) -> bool:
    return isinstance(v, int) and not isinstance(v, bool)


def _as_index(v: object) -> int | None:
    # serde_json's `as_u64`: a non-negative integer, not a float.
    return v if _is_int(v) and 0 <= v <= _MAX_U64 else None


def _u32_at(data: bytes, at: int) -> int:
    if at + 4 > len(data):
        raise GlbError("truncated file")
    return struct.unpack_from("<I", data, at)[0]


def _reject_constant(token: str) -> object:
    raise GlbError(f"the JSON chunk contains {token}")


@dataclass
class _Parsed:
    material: str
    positions: list[tuple[float, float, float]]
    bits: list[tuple[int, int, int]]
    normals: list[tuple[float, float, float]]
    indices: list[int]


def _parse(data: bytes) -> list[tuple[str, list[_Parsed]]]:
    """The container and the glTF references, as `op_world::glb::parse`."""
    if len(data) > MAX_GLB_BYTES:
        raise GlbError(f"file larger than {MAX_GLB_BYTES} bytes")
    if _u32_at(data, 0) != _GLB_MAGIC or _u32_at(data, 4) != _GLB_VERSION:
        raise GlbError("not a glTF 2.0 binary file")
    if _u32_at(data, 8) != len(data):
        raise GlbError("header length does not match file size")
    json_len = _u32_at(data, 12)
    if _u32_at(data, 16) != _CHUNK_JSON:
        raise GlbError("first chunk must be JSON")
    if 20 + json_len > len(data):
        raise GlbError("truncated JSON chunk")
    json_chunk = data[20 : 20 + json_len]
    bin_header = 20 + json_len
    bin_len = _u32_at(data, bin_header)
    if _u32_at(data, bin_header + 4) != _CHUNK_BIN:
        raise GlbError("second chunk must be BIN")
    if bin_header + 8 + bin_len > len(data):
        raise GlbError("truncated BIN chunk")
    if bin_header + 8 + bin_len != len(data):
        raise GlbError("unexpected data after the BIN chunk")
    try:
        doc = json.loads(json_chunk.decode("utf-8"), parse_constant=_reject_constant)
    except ValueError as e:
        if isinstance(e, GlbError):
            raise
        raise GlbError(f"the JSON chunk is not valid JSON: {e}") from e
    view = _Doc(doc if isinstance(doc, dict) else {}, data[bin_header + 8 :])

    materials = []
    for m in view.array("materials"):
        name = m.get("name") if isinstance(m, dict) else None
        if not isinstance(name, str) or name not in PALETTE:
            raise GlbError("material is not in the palette")
        materials.append(name)

    nodes = []
    for node in view.array("nodes"):
        name = node.get("name") if isinstance(node, dict) else None
        if not isinstance(name, str):
            raise GlbError("node without a name")
        mesh = view.index_into("meshes", node.get("mesh"))
        prims = mesh.get("primitives") if isinstance(mesh, dict) else None
        if not isinstance(prims, list):
            raise GlbError("mesh without primitives")
        primitives = []
        for prim in prims:
            if not isinstance(prim, dict) or "attributes" not in prim:
                raise GlbError("primitive without attributes")
            attrs = prim["attributes"] if isinstance(prim["attributes"], dict) else {}
            k = _as_index(prim.get("material"))
            if k is None or k >= len(materials):
                raise GlbError("primitive with a bad material")
            positions, bits = view.f32x3(attrs.get("POSITION"))
            normals, _ = view.f32x3(attrs.get("NORMAL"))
            indices = view.u32s(prim.get("indices"))
            primitives.append(_Parsed(materials[k], positions, bits, normals, indices))
        nodes.append((name, primitives))
    return nodes


class _Doc:
    """The glTF JSON and the BIN chunk, with op_world's accessor rules."""

    def __init__(self, doc: dict, bin_chunk: bytes):
        self.doc = doc
        self.bin = bin_chunk

    def array(self, key: str) -> list:
        items = self.doc.get(key)
        if not isinstance(items, list):
            raise GlbError(f"missing {key}")
        return items

    def index_into(self, key: str, index: object) -> object:
        k = _as_index(index)
        items = self.doc.get(key)
        if k is None or not isinstance(items, list) or k >= len(items):
            raise GlbError(f"bad reference into {key}")
        return items[k]

    def accessor_bytes(self, accessor: object, component: int, ty: str, size: int) -> tuple[int, bytes]:
        acc = self.index_into("accessors", accessor)
        acc = acc if isinstance(acc, dict) else {}
        ct = acc.get("componentType")
        if not _is_int(ct) or ct != component or acc.get("type") != ty:
            raise GlbError(f"accessor must be {ty} of component type {component}")
        count = _as_index(acc.get("count"))
        if count is None:
            raise GlbError("accessor without count")
        view = self.index_into("bufferViews", acc.get("bufferView"))
        view = view if isinstance(view, dict) else {}
        offset = _as_index(view.get("byteOffset"))
        if offset is None:
            raise GlbError("buffer view without byteOffset")
        length = _as_index(view.get("byteLength"))
        if length is None:
            raise GlbError("buffer view without byteLength")
        if count * size != length:
            raise GlbError("accessor count does not match its buffer view")
        if offset + length > len(self.bin):
            raise GlbError("buffer view outside the BIN chunk")
        return count, self.bin[offset : offset + length]

    def f32x3(self, accessor: object) -> tuple[list[tuple[float, float, float]], list[tuple[int, int, int]]]:
        count, data = self.accessor_bytes(accessor, _FLOAT, "VEC3", 12)
        values = struct.unpack(f"<{3 * count}f", data)
        bits = struct.unpack(f"<{3 * count}I", data)
        return (
            list(zip(values[0::3], values[1::3], values[2::3])),
            list(zip(bits[0::3], bits[1::3], bits[2::3])),
        )

    def u32s(self, accessor: object) -> list[int]:
        count, data = self.accessor_bytes(accessor, _UNSIGNED_INT, "SCALAR", 4)
        return list(struct.unpack(f"<{count}I", data))


def _checked(nodes: list[tuple[str, list[_Parsed]]]) -> Mesh:
    """The profile rules of `op_world::glb::GlbMesh::validate`, with its
    messages; returns the mesh with its millimetre positions."""
    if not nodes:
        raise GlbError("a mesh needs at least one node")
    out = []
    for name, prims in nodes:
        if not _is_feature_id(name):
            raise GlbError(f"node name {_quote(name)} is not a feature id")
        if not prims:
            raise GlbError(f"node {_quote(name)} has no primitives")
        order = [PALETTE.index(p.material) for p in prims]
        if any(a >= b for a, b in zip(order, order[1:])):
            raise GlbError(f"node {_quote(name)}: primitives must have distinct materials in palette order")
        checked = []
        for p in prims:
            n = len(p.positions)
            if n == 0 or n >= 1 << 32:
                raise GlbError(f"node {_quote(name)}: bad vertex count {n}")
            if len(p.normals) != n:
                raise GlbError(f"node {_quote(name)}: normals and positions differ in count")
            if not p.indices or len(p.indices) % 3:
                raise GlbError(f"node {_quote(name)}: index count must be a positive multiple of 3")
            bad = next((i for i in p.indices if i >= n), None)
            if bad is not None:
                raise GlbError(f"node {_quote(name)}: index {bad} out of range ({n} vertices)")
            mm = _snapped_mm([c for v in p.positions for c in v], [b for v in p.bits for b in v])
            if mm is None:
                raise GlbError(f"node {_quote(name)}: positions must be snapped to millimetres")
            if not all(math.isfinite(c) for v in p.normals for c in v):
                raise GlbError(f"node {_quote(name)}: normals must be finite")
            checked.append(
                MeshPrimitive(
                    material=p.material,
                    positions=tuple(p.positions),
                    position_bits=tuple(p.bits),
                    positions_mm=tuple(zip(mm[0::3], mm[1::3], mm[2::3])),
                    normals=tuple(p.normals),
                    indices=tuple(p.indices),
                )
            )
        out.append(MeshNode(name=name, primitives=tuple(checked)))
    return Mesh(nodes=tuple(out))


def _snapped_mm(values: list[float], bits: list[int]) -> list[int] | None:
    """`f32_to_mm` of many coordinates at once (their f32 values and bit
    patterns): their millimetres, or None if any one is not snapped."""
    if not all(map(math.isfinite, values)):
        return None
    mm = [_round_half_away(v * 1000.0) for v in values]
    if max(map(abs, mm), default=0) > _MAX_SNAPPED_MM:
        return None
    count = len(mm)
    if struct.pack(f"<{count}f", *[k / 1000 for k in mm]) != struct.pack(f"<{count}I", *bits):
        return None
    return mm
