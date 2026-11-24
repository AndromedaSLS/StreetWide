"""A small reader for glTF 2.0 binary files (.glb) as exported by 3D editors.

It reads what a facade needs: every mesh primitive of the default scene
(in a file without scenes, of every node that is nobody's child) drawn as
triangles, with its node transforms applied (translation, rotation, scale
or matrix, through the node hierarchy), and the name of its material.
Textures, animations and other attributes are ignored.

The file comes from a contributor, so it is untrusted: every index, count,
offset and number the reader uses is checked, the nodes must form trees (as
glTF requires) and the work a file can ask for is bounded. Whatever the
bytes, `read` returns parts or raises GltfError.
"""

from __future__ import annotations

import json
import math
import struct
from dataclasses import dataclass

import numpy as np

_COMPONENT = {5120: "b", 5121: "B", 5122: "h", 5123: "H", 5125: "I", 5126: "f"}
_SIZE = {"b": 1, "B": 1, "h": 2, "H": 2, "I": 4, "f": 4}
_COUNT = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT4": 16}
MAX_BYTES = 64 << 20
MAX_DEPTH = 64
"""Deepest node hierarchy."""
LIMITS = {
    "nodes visited": 100_000,
    "primitives": 50_000,
    "accessor elements": 8_000_000,
    "triangles": 1_000_000,
}
"""Most work a file may ask for: instancing (many nodes drawing one mesh) can
otherwise multiply a small file into any amount of geometry. A facade may
have at most 50,000 triangles (RFC 0002 G7), so no usable model comes near."""


class GltfError(ValueError):
    """The file is not a glTF binary this reader understands."""


@dataclass
class Part:
    material: str
    triangles: np.ndarray  # (n, 3, 3) positions in metres, after node transforms


class _Budget:
    def __init__(self) -> None:
        self.used = dict.fromkeys(LIMITS, 0)

    def spend(self, what: str, n: int) -> None:
        self.used[what] += n
        if self.used[what] > LIMITS[what]:
            raise GltfError(f"the model is too large: more than {LIMITS[what]:,} {what}")


def _short(value: object) -> str:
    """A value for an error message (not the repr of a deeply nested one)."""
    if isinstance(value, (list, dict)):
        return "an array" if isinstance(value, list) else "an object"
    text = repr(value)
    return text if len(text) <= 40 else text[:37] + "..."


def _list(value: object, what: str) -> list:
    if not isinstance(value, list):
        raise GltfError(f"{what} is not an array")
    return value


def _dict(value: object, what: str) -> dict:
    if not isinstance(value, dict):
        raise GltfError(f"{what} is not an object")
    return value


def _index(value: object, size: int, what: str) -> int:
    """`value` as an index into an array of `size` items."""
    if type(value) is not int or not 0 <= value < size:
        raise GltfError(f"{what} index {_short(value)} is not below {size}")
    return value


def _natural(value: object, what: str) -> int:
    if type(value) is not int or value < 0:
        raise GltfError(f"{what} {_short(value)} is not a non-negative integer")
    return value


def _lookup(table: dict, key: object, what: str):
    # JSON arrays and objects cannot be dictionary keys.
    found = None if isinstance(key, (list, dict)) else table.get(key)
    if found is None:
        raise GltfError(f"unsupported {what} {_short(key)}")
    return found


def _numbers(value: object, n: int, what: str) -> list[float]:
    """Exactly `n` finite numbers."""
    if not isinstance(value, list) or len(value) != n:
        raise GltfError(f"{what} must be {n} numbers")
    out = []
    for v in value:
        if type(v) not in (int, float):
            raise GltfError(f"{what} must be {n} numbers")
        try:
            f = float(v)
        except OverflowError:
            f = math.inf
        if not math.isfinite(f):
            raise GltfError(f"{what} must be finite")
        out.append(f)
    return out


def _matrix(node: dict) -> np.ndarray:
    if "matrix" in node:
        return np.asarray(_numbers(node["matrix"], 16, "node matrix"), dtype=np.float64).reshape(4, 4).T  # column-major
    t = np.asarray(_numbers(node.get("translation", [0, 0, 0]), 3, "node translation"), dtype=np.float64)
    x, y, z, w = _numbers(node.get("rotation", [0, 0, 0, 1]), 4, "node rotation")
    r = np.array([
        [1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w)],
        [2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w)],
        [2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y)],
    ])
    s = np.asarray(_numbers(node.get("scale", [1, 1, 1]), 3, "node scale"), dtype=np.float64)
    m = np.eye(4)
    m[:3, :3] = r * s
    m[:3, 3] = t
    return m


def _accessor(doc: dict, binary: bytes, index: object, budget: _Budget) -> np.ndarray:
    accessors = _list(doc.get("accessors"), "accessors")
    acc = _dict(accessors[_index(index, len(accessors), "accessor")], "accessor")
    if "sparse" in acc or "bufferView" not in acc:
        raise GltfError("sparse or empty accessors are not supported")
    views = _list(doc.get("bufferViews"), "bufferViews")
    view = _dict(views[_index(acc["bufferView"], len(views), "buffer view")], "buffer view")
    if view.get("buffer", 0) != 0:
        raise GltfError("only the binary chunk's buffer is supported")
    fmt = _lookup(_COMPONENT, acc.get("componentType"), "accessor componentType")
    n = _lookup(_COUNT, acc.get("type"), "accessor type")
    size = _SIZE[fmt]
    item = size * n
    stride = view.get("byteStride", item)
    if type(stride) is not int or not item <= stride <= max(item, 252):  # glTF: at most 252
        raise GltfError(f"byteStride {_short(stride)} is not between an element ({item} bytes) and 252")
    start = _natural(view.get("byteOffset", 0), "byteOffset") + _natural(acc.get("byteOffset", 0), "byteOffset")
    count = _natural(acc.get("count"), "accessor count")
    if start + stride * (count - 1) + item > len(binary):
        raise GltfError("accessor reaches past the binary chunk")
    budget.spend("accessor elements", count)
    out = np.float64 if fmt == "f" else np.int64
    if count == 0:
        return np.zeros((0, n), dtype=out)
    raw = np.ndarray((count, n), dtype=np.dtype("<" + fmt), buffer=binary, offset=start, strides=(stride, size))
    return raw.astype(out)


def _parse_json(chunk: bytes) -> dict:
    try:
        doc = json.loads(chunk)
    except ValueError as e:  # not UTF-8 or not JSON (JSONDecodeError is a ValueError)
        raise GltfError(f"the JSON chunk is not valid JSON: {e}") from e
    except RecursionError as e:
        raise GltfError("the JSON chunk is nested too deeply") from e
    return _dict(doc, "the JSON chunk")


def _check_trees(nodes: list) -> list[bool]:
    """glTF nodes form disjoint trees: no node is the child of two nodes
    (otherwise a small file could describe exponentially many paths) and
    none is its own ancestor. Returns whether each node has a parent."""
    has_parent = [False] * len(nodes)
    children = []
    for node in nodes:
        kids = [_index(c, len(nodes), "child node") for c in _list(_dict(node, "node").get("children", []), "node children")]
        for c in kids:
            if has_parent[c]:
                raise GltfError(f"node {c} is the child of more than one node")
            has_parent[c] = True
        children.append(kids)
    # With one parent at most, a node that no root reaches is in a loop.
    reached, stack = 0, [i for i, p in enumerate(has_parent) if not p]
    while stack:
        reached += 1
        stack.extend(children[stack.pop()])
    if reached < len(nodes):
        raise GltfError("the nodes form a loop: a node is its own ancestor")
    return has_parent


def read(data: bytes) -> list[Part]:
    """Triangles per material of the default scene."""
    if len(data) > MAX_BYTES:
        raise GltfError("file too large")
    if len(data) < 20 or data[:4] != b"glTF":
        raise GltfError("not a glTF binary (.glb) file")
    version, total = struct.unpack_from("<II", data, 4)
    if version != 2 or total > len(data):
        raise GltfError("unsupported glTF version or truncated file")
    json_len, json_type = struct.unpack_from("<II", data, 12)
    if json_type != 0x4E4F534A:
        raise GltfError("first chunk is not JSON")
    doc = _parse_json(data[20 : 20 + json_len])
    binary = b""
    at = 20 + json_len
    if at + 8 <= len(data):
        bin_len, bin_type = struct.unpack_from("<II", data, at)
        if bin_type == 0x004E4942:
            binary = data[at + 8 : at + 8 + bin_len]

    nodes = _list(doc.get("nodes", []), "nodes")
    has_parent = _check_trees(nodes)
    materials = [_dict(m, "material") for m in _list(doc.get("materials", []), "materials")]
    if doc.get("scenes"):
        scenes = _list(doc["scenes"], "scenes")
        scene = _dict(scenes[_index(doc.get("scene", 0), len(scenes), "scene")], "scene")
        roots = [_index(n, len(nodes), "scene node") for n in _list(scene.get("nodes", []), "scene nodes")]
        if len(set(roots)) != len(roots):
            raise GltfError("a scene lists a node twice")
        # Its parent draws it already (the glTF validator's SCENE_NON_ROOT_NODE).
        for n in roots:
            if has_parent[n]:
                raise GltfError(f"scene node {n} is the child of another node")
    else:
        # No scene: every root, that is every node that is nobody's child.
        roots = [i for i, p in enumerate(has_parent) if not p]
    parts: dict[str, list[np.ndarray]] = {}
    budget = _Budget()
    read_accessors: dict[int, np.ndarray] = {}

    def accessor(index: object) -> np.ndarray:
        # Instanced meshes read the same accessors again; the budget still
        # counts every use, as each one adds geometry.
        if type(index) is int and index in read_accessors:
            budget.spend("accessor elements", len(read_accessors[index]))
            return read_accessors[index]
        values = _accessor(doc, binary, index, budget)
        read_accessors[index] = values
        return values

    def material_name(index: object) -> str:
        i = _index(index, len(materials), "material")
        name = materials[i].get("name", f"material {i}")
        if not isinstance(name, str):
            raise GltfError(f"material {i} has a name that is not a string")
        return name

    def visit(index: int, parent: np.ndarray, depth: int) -> None:
        if depth > MAX_DEPTH:
            raise GltfError("node hierarchy too deep")
        budget.spend("nodes visited", 1)
        node = nodes[index]
        world = parent @ _matrix(node)
        if "mesh" in node:
            meshes = _list(doc.get("meshes"), "meshes")
            mesh = _dict(meshes[_index(node["mesh"], len(meshes), "mesh")], "mesh")
            for prim in _list(mesh.get("primitives"), "mesh primitives"):
                prim = _dict(prim, "mesh primitive")
                budget.spend("primitives", 1)
                if prim.get("mode", 4) != 4:
                    continue
                attributes = _dict(prim.get("attributes"), "primitive attributes")
                if "POSITION" not in attributes:
                    raise GltfError("a primitive has no POSITION")
                pos = accessor(attributes["POSITION"])
                if pos.shape[1] < 3:
                    raise GltfError("POSITION is not a VEC3 accessor")
                pos = (np.column_stack((pos[:, :3], np.ones(len(pos)))) @ world.T)[:, :3]
                if "indices" in prim:
                    idx = accessor(prim["indices"])
                    if idx.dtype.kind == "f":
                        raise GltfError("indices are not integers")
                    idx = idx.reshape(-1)
                else:
                    idx = np.arange(len(pos))
                if len(idx) % 3 or (len(idx) and (idx.min() < 0 or idx.max() >= len(pos))):
                    raise GltfError("bad index buffer")
                budget.spend("triangles", len(idx) // 3)
                tris = pos[idx.reshape(-1, 3)]
                if not np.isfinite(tris).all():
                    raise GltfError("vertex positions are not finite")
                name = material_name(prim["material"]) if "material" in prim else "default"
                parts.setdefault(name, []).append(tris)
        for child in node.get("children", []):
            visit(child, world, depth + 1)

    # Overflowing transforms give infinities, which `visit` refuses.
    with np.errstate(over="ignore", invalid="ignore"):
        for n in roots:
            visit(n, np.eye(4), 0)
    return [Part(material=k, triangles=np.concatenate(v)) for k, v in sorted(parts.items())]
