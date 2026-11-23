"""Assembles meshes into a world store: asset manifests, tile indexes and
the world manifest (RFC 0001, sections 3 and 4). Every document is checked
against the JSON Schemas before it is written."""

from __future__ import annotations

from dataclasses import dataclass

from tools.ingest import GENERATOR, VERSION
from tools.ingest.geo import Frame, region_key, tile_key
from tools.ingest.mesh import MeshBuilder
from tools.worldfmt import CONTRACT_VERSION, glb, schemas
from tools.worldfmt.store import Store

SWISSTOPO_LICENSE = "LicenseRef-swisstopo-OGD"
SWISSTOPO_CREDIT = "© swisstopo"
OSM_LICENSE = "ODbL-1.0"
OSM_CREDIT = "© OpenStreetMap contributors"
# OSM geometry placed on swisstopo terrain heights (ADR-0006).
OSM_ON_TERRAIN_LICENSE = f"{OSM_LICENSE} AND {SWISSTOPO_LICENSE}"


@dataclass
class Asset:
    tile: tuple[int, int]
    layer: str
    feature_id: str
    mesh: MeshBuilder
    license: str
    sources: list[dict]
    collider: str

    @property
    def attribution(self) -> list[str]:
        return sorted({s["attribution"] for s in self.sources})


class SchemaError(ValueError):
    pass


def _checked(doc: dict) -> dict:
    errs = schemas.errors(doc)
    if errs:
        raise SchemaError(f"{doc.get('format')} {doc.get('feature_id', doc.get('tile', ''))}: {errs[:3]}")
    return doc


def _bounds(node: glb.Node) -> dict:
    pts = [v for p in node.primitives for v in p.positions_mm]
    return {
        "max": [max(v[i] for v in pts) for i in range(3)],
        "min": [min(v[i] for v in pts) for i in range(3)],
    }


def _union(boxes: list[dict]) -> dict:
    return {
        "max": [max(b["max"][i] for b in boxes) for i in range(3)],
        "min": [min(b["min"][i] for b in boxes) for i in range(3)],
    }


def write_world(
    store: Store,
    frame: Frame,
    name: str,
    assets: list[Asset],
    origins: dict[tuple[int, int], list[int]],
    spawn_mm: list[int],
    yaw: int,
) -> tuple[str, dict[str, int]]:
    """Writes every asset, tile index and the world manifest. Returns the
    world id and per-layer asset counts."""
    seen: set[tuple[str, str]] = set()
    by_tile: dict[tuple[int, int], list[dict]] = {}
    counts: dict[str, int] = {}
    attribution: set[str] = set()
    for a in assets:
        if (a.layer, a.feature_id) in seen:
            raise ValueError(f"{a.layer} {a.feature_id} appears twice")
        seen.add((a.layer, a.feature_id))
        node = a.mesh.node(a.feature_id)
        data = glb.write([node])
        content_id = store.write_object(data, "glb")
        bounds = _bounds(node)
        manifest = _checked({
            "format": "streetwide.asset",
            "contract_version": CONTRACT_VERSION,
            "content_id": content_id,
            "size_bytes": len(data),
            "layer": a.layer,
            "feature_id": a.feature_id,
            "tile": tile_key(*a.tile),
            "bounds_mm": bounds,
            "physics": {"collider": a.collider},
            "fictional": False,
            "parent_version_id": None,
            "provenance": {
                "license": a.license,
                "attribution": a.attribution,
                "sources": a.sources,
                "generator": {"name": GENERATOR, "version": VERSION},
            },
        })
        manifest_id = store.write_json(manifest)
        by_tile.setdefault(a.tile, []).append({
            "layer": a.layer,
            "feature_id": a.feature_id,
            "manifest_id": manifest_id,
            "content_id": content_id,
            "license": a.license,
            "collider": a.collider,
            "bounds_mm": bounds,
        })
        counts[a.layer] = counts.get(a.layer, 0) + 1
        attribution.update(a.attribution)

    tiles = []
    for tile in sorted(by_tile):
        entries = sorted(by_tile[tile], key=lambda e: (e["layer"].encode(), e["feature_id"].encode()))
        index = _checked({
            "format": "streetwide.tile",
            "contract_version": CONTRACT_VERSION,
            "tile": tile_key(*tile),
            "region": region_key(*frame.region),
            "origin_mm": origins[tile],
            "assets": entries,
        })
        tiles.append({
            "tile": tile_key(*tile),
            "tile_id": store.write_json(index),
            "origin_mm": origins[tile],
            "bounds_mm": _union([e["bounds_mm"] for e in entries]),
        })
    world = _checked({
        "format": "streetwide.world",
        "contract_version": CONTRACT_VERSION,
        "name": name,
        "region": {"key": frame.key, "anchor_e9": frame.anchor_e9},
        "attribution": sorted(attribution),
        "spawn": {"position_mm": spawn_mm, "yaw": yaw},
        "tiles": tiles,
    })
    return store.write_world(world), counts
