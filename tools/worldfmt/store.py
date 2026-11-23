"""World store layout (RFC 0001, section 7), byte-compatible with
`op_world::Store`:

    world.json                  the world manifest
    objects/<aa>/<hex>.json     tile indexes and asset manifests
    objects/<aa>/<hex>.glb      meshes
"""

from __future__ import annotations

from pathlib import Path

from tools.worldfmt import cjson
from tools.worldfmt.ids import content_id, hex_of


class Store:
    def __init__(self, root: Path):
        self.root = Path(root)

    def object_path(self, cid: str, ext: str) -> Path:
        h = hex_of(cid)
        return self.root / "objects" / h[:2] / f"{h}.{ext}"

    def write_object(self, data: bytes, ext: str) -> str:
        if ext not in ("json", "glb"):
            raise ValueError(f"unknown object kind {ext}")
        cid = content_id(data)
        path = self.object_path(cid, ext)
        if not path.exists():
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        return cid

    def write_json(self, doc: dict) -> str:
        return self.write_object(cjson.dumps(doc), "json")

    def read_object(self, cid: str, ext: str) -> bytes:
        data = self.object_path(cid, ext).read_bytes()
        if content_id(data) != cid:
            raise ValueError(f"object {cid} does not match its hash")
        return data

    def write_world(self, doc: dict) -> str:
        data = cjson.dumps(doc)
        self.root.mkdir(parents=True, exist_ok=True)
        (self.root / "world.json").write_bytes(data)
        return content_id(data)

    def read_world(self) -> tuple[str, dict]:
        data = (self.root / "world.json").read_bytes()
        return content_id(data), cjson.loads(data)
