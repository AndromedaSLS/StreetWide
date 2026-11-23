"""A small reader for glTF 2.0 binary files (.glb) as exported by 3D editors.

It reads what a facade needs: every mesh primitive of the default scene
drawn as triangles, with its node transforms applied (translation,
rotation, scale or matrix, through the node hierarchy), and the name of
its material. Textures, animations and other attributes are ignored.
"""

from __future__ import annotations

import json
import struct
from dataclasses import dataclass

import numpy as np

_COMPONENT = {5120: "b", 5121: "B", 5122: "h", 5123: "H", 5125: "I", 5126: "f"}
_SIZE = {"b": 1, "B": 1, "h": 2, "H": 2, "I": 4, "f": 4}
_COUNT = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT4": 16}
MAX_BYTES = 64 << 20


class GltfError(ValueError):
    """The file is not a glTF binary this reader understands."""


@dataclass
class Part:
    material: str
    triangles: np.ndarray  # (n, 3, 3) positions in metres, after node transforms


def _matrix(node: dict) -> np.ndarray:
    if "matrix" in node:
        return np.asarray(node["matrix"], dtype=np.float64).reshape(4, 4).T  # column-major
    t = np.asarray(node.get("translation", [0, 0, 0]), dtype=np.float64)
    x, y, z, w = node.get("rotation", [0, 0, 0, 1])
    r = np.array([
        [1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w)],
        [2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w)],
        [2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y)],
    ])
    s = np.asarray(node.get("scale", [1, 1, 1]), dtype=np.float64)
    m = np.eye(4)
    m[:3, :3] = r * s
    m[:3, 3] = t
    return m


def _accessor(doc: dict, binary: bytes, index: int) -> np.ndarray:
    acc = doc["accessors"][index]
    if "sparse" in acc or "bufferView" not in acc:
        raise GltfError("sparse or empty accessors are not supported")
    view = doc["bufferViews"][acc["bufferView"]]
    if view.get("buffer", 0) != 0:
        raise GltfError("only the binary chunk's buffer is supported")
    fmt = _COMPONENT[acc["componentType"]]
    n = _COUNT[acc["type"]]
    item = _SIZE[fmt] * n
    stride = view.get("byteStride", item)
    start = view.get("byteOffset", 0) + acc.get("byteOffset", 0)
    count = acc["count"]
    if start + stride * (count - 1) + item > len(binary):
        raise GltfError("accessor reaches past the binary chunk")
    rows = [struct.unpack_from(f"<{n}{fmt}", binary, start + i * stride) for i in range(count)]
    return np.asarray(rows, dtype=np.float64 if fmt == "f" else np.int64).reshape(count, n)


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
    doc = json.loads(data[20 : 20 + json_len])
    binary = b""
    at = 20 + json_len
    if at + 8 <= len(data):
        bin_len, bin_type = struct.unpack_from("<II", data, at)
        if bin_type == 0x004E4942:
            binary = data[at + 8 : at + 8 + bin_len]

    materials = [m.get("name", f"material {i}") for i, m in enumerate(doc.get("materials", []))]
    scene = doc.get("scenes", [{}])[doc.get("scene", 0)] if doc.get("scenes") else {"nodes": list(range(len(doc.get("nodes", []))))}
    parts: dict[str, list[np.ndarray]] = {}

    def visit(index: int, parent: np.ndarray, depth: int) -> None:
        if depth > 64:
            raise GltfError("node hierarchy too deep")
        node = doc["nodes"][index]
        world = parent @ _matrix(node)
        if "mesh" in node:
            for prim in doc["meshes"][node["mesh"]]["primitives"]:
                if prim.get("mode", 4) != 4:
                    continue
                pos = _accessor(doc, binary, prim["attributes"]["POSITION"])[:, :3]
                pos = (np.c_[pos, np.ones(len(pos))] @ world.T)[:, :3]
                if "indices" in prim:
                    idx = _accessor(doc, binary, prim["indices"]).reshape(-1)
                else:
                    idx = np.arange(len(pos))
                if len(idx) % 3 or (len(idx) and (idx.min() < 0 or idx.max() >= len(pos))):
                    raise GltfError("bad index buffer")
                tris = pos[idx.reshape(-1, 3)]
                name = materials[prim["material"]] if "material" in prim else "default"
                parts.setdefault(name, []).append(tris)
        for child in node.get("children", []):
            visit(child, world, depth + 1)

    for n in scene.get("nodes", []):
        visit(n, np.eye(4), 0)
    return [Part(material=k, triangles=np.concatenate(v)) for k, v in sorted(parts.items())]
