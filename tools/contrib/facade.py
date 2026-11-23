"""Facade contributions (RFC 0002): find a building, export its shell to
model on, and turn the finished model into a contribution bundle."""

from __future__ import annotations

import shutil
from pathlib import Path

import numpy as np

from tools.contrib import GENERATOR, VERSION
from tools.contrib.gltf_in import Part, read
from tools.worldfmt import CONTRACT_VERSION, cjson, glb, schemas
from tools.worldfmt.ids import content_id
from tools.worldfmt.store import Store

CONTRIBUTION_CONTRACT = "0.1.0"
CONTRIBUTION_LICENSE = "CC-BY-SA-4.0"
COLLECTIVE_CREDIT = "The StreetWide contributors"
DATASET = "StreetWide contributions"
REPO_URL = "https://github.com/AndromedaSLS/StreetWide"


class ContributionError(ValueError):
    """The contribution cannot be built; the message says why."""


class World:
    """A world store opened for reading."""

    def __init__(self, directory: Path):
        self.store = Store(directory)
        self.world_id, self.doc = self.store.read_world()
        self.tiles = [
            (wt, cjson.loads(self.store.read_object(wt["tile_id"], "json"))) for wt in self.doc["tiles"]
        ]

    def find(self, layer: str, feature_id: str):
        for wt, index in self.tiles:
            for a in index["assets"]:
                if a["layer"] == layer and a["feature_id"] == feature_id:
                    return wt, a
        return None

    def manifest(self, entry: dict) -> dict:
        return cjson.loads(self.store.read_object(entry["manifest_id"], "json"))


def find_building(world_dir: Path, lon: float, lat: float) -> str:
    """The feature id of the building at (or within 30 m of) a WGS84 point."""
    from tools.ingest.geo import Frame

    world = World(world_dir)
    _, rx, ry = (int(v) for v in world.doc["region"]["key"].split("/"))
    frame = Frame((rx, ry))
    if frame.anchor_e9 != world.doc["region"]["anchor_e9"]:
        raise ContributionError("the world's region anchor does not match its key")
    p = frame.from_lonlat(lon, lat, 0.0)[0]
    x, z = p[0] * 1000, p[2] * 1000
    best = None
    for wt, index in world.tiles:
        o = wt["origin_mm"]
        for a in index["assets"]:
            if a["layer"] != "shell":
                continue
            lo = [a["bounds_mm"]["min"][k] + o[k] for k in (0, 2)]
            hi = [a["bounds_mm"]["max"][k] + o[k] for k in (0, 2)]
            dx = max(lo[0] - x, 0, x - hi[0])
            dz = max(lo[1] - z, 0, z - hi[1])
            distance = (dx * dx + dz * dz) ** 0.5
            area = (hi[0] - lo[0]) * (hi[1] - lo[1])
            key = (distance, area, a["feature_id"])
            if best is None or key < best:
                best = key
    if best is None or best[0] > 30_000:
        raise ContributionError("no building within 30 m of that point")
    return best[2]


def export_reference(world_dir: Path, feature_id: str, out_dir: Path) -> Path:
    """Copies the building's shell mesh (a standard .glb, in its tile's
    frame, metres, Y up) to model on."""
    world = World(world_dir)
    found = world.find("shell", feature_id)
    if found is None:
        raise ContributionError(f"no building {feature_id} in this world")
    _, shell = found
    out_dir.mkdir(parents=True, exist_ok=True)
    path = out_dir / f"{feature_id.rsplit(':', 1)[-1]}-shell.glb"
    shutil.copyfile(world.store.object_path(shell["content_id"], "glb"), path)
    return path


def palette_material(name: str) -> str:
    """The palette material whose name appears in a material name, else wall."""
    lowered = name.lower()
    return next((m for m in glb.PALETTE if m in lowered), "wall")


def facade_node(parts: list[Part], name: str) -> glb.Node:
    """Flat-shaded triangles snapped to the millimetre, one primitive per
    palette material. Triangles that collapse when snapped are dropped."""
    by_material: dict[str, list[np.ndarray]] = {}
    for part in parts:
        by_material.setdefault(palette_material(part.material), []).append(part.triangles)
    primitives = []
    for material in glb.PALETTE:
        if material not in by_material:
            continue
        tris_mm = np.floor(np.concatenate(by_material[material]) * 1000.0 + 0.5).astype(np.int64)
        normals = np.cross(tris_mm[:, 1] - tris_mm[:, 0], tris_mm[:, 2] - tris_mm[:, 0])
        keep = np.any(normals != 0, axis=1)
        tris_mm, normals = tris_mm[keep], normals[keep].astype(np.float64)
        if len(tris_mm) == 0:
            continue
        normals /= np.linalg.norm(normals, axis=1, keepdims=True)
        positions = [tuple(v) for v in tris_mm.reshape(-1, 3).tolist()]
        normal_rows = [tuple(n) for n in np.repeat(normals, 3, axis=0).tolist()]
        primitives.append(
            glb.Primitive(material=material, positions_mm=positions, normals=normal_rows, indices=list(range(len(positions))))
        )
    if not primitives:
        raise ContributionError("the model has no triangles left after snapping to the millimetre")
    return glb.Node(name=name, primitives=primitives)


def make_bundle(
    world_dir: Path,
    feature_id: str,
    model: bytes,
    out_dir: Path,
    *,
    author: str,
    title: str,
    rationale: str,
    derived_from_shell: bool = False,
    fictional: bool = False,
) -> str:
    """Writes a contribution bundle for a facade; returns the change id."""
    world = World(world_dir)
    found = world.find("shell", feature_id)
    if found is None:
        raise ContributionError(f"no building {feature_id} in this world")
    wt, shell = found
    sources = [{
        "dataset": DATASET,
        "version": CONTRIBUTION_CONTRACT,
        "license": CONTRIBUTION_LICENSE,
        "attribution": COLLECTIVE_CREDIT,
        "url": REPO_URL,
        "source_ids": [author],
    }]
    if derived_from_shell:
        shell_sources = world.manifest(shell)["provenance"]["sources"]
        if any("ODbL" in s["license"] for s in shell_sources):
            raise ContributionError(
                "this building's shell comes from OpenStreetMap (ODbL); a facade derived from it "
                "cannot be CC BY-SA. Model it from your own observations instead."
            )
        sources += shell_sources

    node = facade_node(read(model), feature_id)
    mesh = glb.write([node])
    out_dir.mkdir(parents=True, exist_ok=True)
    bundle = Store(out_dir)
    mesh_id = bundle.write_object(mesh, "glb")
    points = [v for p in node.primitives for v in p.positions_mm]
    current = world.find("facade", feature_id)
    parent = current[1]["manifest_id"] if current else None
    manifest = {
        "format": "streetwide.asset",
        "contract_version": CONTRACT_VERSION,
        "content_id": mesh_id,
        "size_bytes": len(mesh),
        "layer": "facade",
        "feature_id": feature_id,
        "tile": wt["tile"],
        "bounds_mm": {
            "max": [max(v[k] for v in points) for k in range(3)],
            "min": [min(v[k] for v in points) for k in range(3)],
        },
        "physics": {"collider": "none"},
        "fictional": fictional,
        "parent_version_id": parent,
        "provenance": {
            "license": " AND ".join(sorted({s["license"] for s in sources})),
            "attribution": sorted({s["attribution"] for s in sources}),
            "sources": sources,
            "generator": {"name": GENERATOR, "version": VERSION},
        },
    }
    errors = schemas.errors(manifest)
    if errors:
        raise ContributionError(f"manifest: {errors[:3]}")
    manifest_id = bundle.write_object(cjson.dumps(manifest), "json")
    change = {
        "format": "streetwide.change",
        "contract_version": CONTRIBUTION_CONTRACT,
        "base_world_id": world.world_id,
        "title": title,
        "author": author,
        "rationale": rationale,
        "changes": [{
            "op": "replace" if parent else "add",
            "tile": wt["tile"],
            "layer": "facade",
            "feature_id": feature_id,
            "parent_version_id": parent,
            "manifest_id": manifest_id,
        }],
    }
    errors = schemas.errors(change)
    if errors:
        raise ContributionError(f"change set: {errors[:3]}")
    data = cjson.dumps(change)
    (out_dir / "change.json").write_bytes(data)
    return content_id(data)
