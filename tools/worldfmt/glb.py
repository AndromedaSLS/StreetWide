"""StreetWide glb profile 0.1 writer (RFC 0001, section 6).

Produces exactly the bytes of `op_world::glb::write` for the same mesh.
Positions are given as integer millimetres; each becomes the f32 nearest to
the double `mm / 1000`, the same two-step rounding as the Rust writer.
"""

from __future__ import annotations

import struct
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

MAX_JSON_BYTES = 1 << 20
"""Longest JSON chunk the profile allows (`op_world`'s MAX_GLB_JSON_BYTES);
an asset needs about a kilobyte per primitive."""

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
    if len(json_bytes) > MAX_JSON_BYTES:
        raise GlbError(f"mesh too large: its JSON chunk would be longer than {MAX_JSON_BYTES} bytes")
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
