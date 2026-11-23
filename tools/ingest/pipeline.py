"""The ingest pipeline: sources -> meshes -> world store."""

from __future__ import annotations

import json
import os
import shutil
import time
from dataclasses import dataclass
from pathlib import Path

import numpy as np
from shapely.geometry import Polygon
from shapely.ops import unary_union

from tools.ingest import buildings, overpass, stac, terrain, transport
from tools.ingest.assemble import (
    OSM_CREDIT,
    OSM_LICENSE,
    OSM_ON_TERRAIN_LICENSE,
    SWISSTOPO_CREDIT,
    SWISSTOPO_LICENSE,
    Asset,
    write_world,
)
from tools.ingest.citygml import load_buildings
from tools.ingest.dem import Dem
from tools.ingest.geo import Area, Frame, lonlat_to_lv95, lonlat_to_tilef, lv95_to_lonlat, tile_key, tilef_to_lonlat, to_mm
from tools.ingest.osm import parse as parse_osm
from tools.worldfmt.store import Store

STAC_COLLECTIONS = "https://data.geo.admin.ch/api/stac/v1/collections/"
OSM_URL = "https://www.openstreetmap.org/"
BUILDING_MARGIN_M = 100.0
SPAWN_ABOVE_GROUND_M = 1.0


@dataclass
class Spawn:
    lon: float
    lat: float
    yaw_deg: float = 0.0


def _source(dataset, version, license, credit, url, ids) -> dict:
    return {
        "dataset": dataset,
        "version": version,
        "license": license,
        "attribution": credit,
        "url": url,
        "source_ids": sorted(set(ids)),
    }


def _dem_sources(files: list[stac.SourceFile]) -> list[dict]:
    years = sorted({f.year for f in files})
    return [
        _source("swissALTI3D", y, SWISSTOPO_LICENSE, SWISSTOPO_CREDIT,
                STAC_COLLECTIONS + stac.SWISSALTI3D.collection,
                [f.item_id for f in files if f.year == y])
        for y in years
    ]


def _lv95_polygon(corners_tilef: list[tuple[float, float]], steps: int = 8) -> Polygon:
    """A polygon in LV95 from a rectangle in tile units (edges densified)."""
    pts = []
    for (x0, y0), (x1, y1) in zip(corners_tilef, corners_tilef[1:] + corners_tilef[:1]):
        for s in range(steps):
            t = s / steps
            pts.append(tilef_to_lonlat(x0 + (x1 - x0) * t, y0 + (y1 - y0) * t))
    lon, lat = np.array(pts).T
    e, n = lonlat_to_lv95(lon, lat)
    return Polygon(np.stack([e, n], axis=1))


def _rect(x0, y0, x1, y1) -> list[tuple[float, float]]:
    return [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]


def _tile_of(e: float, n: float) -> tuple[float, float]:
    lon, lat = lv95_to_lonlat(e, n)
    return lonlat_to_tilef(float(lon), float(lat))


def _bump(stats: dict[str, int], key: str, k: int = 1) -> None:
    stats[key] = stats.get(key, 0) + k


def run(name: str, area: Area, spawn: Spawn | None, out: Path, cache: Path, *,
        offline: bool = False, refresh: bool = False) -> dict:
    t0 = time.time()
    stats: dict[str, int] = {}
    bbox = area.bbox()
    frame = Frame(area.region())
    tiles = area.tiles()
    origins = {t: frame.tile_origin_mm(*t) for t in tiles}
    print(f"{name}: {len(tiles)} tiles in region {frame.key}, bbox {bbox}", flush=True)

    print("sources", flush=True)
    sheets = stac.find_and_fetch(stac.SWISSBUILDINGS3D, bbox, cache, offline=offline, refresh=refresh)
    dem_files = stac.find_and_fetch(stac.SWISSALTI3D, bbox, cache, offline=offline, refresh=refresh)
    osm_doc = overpass.fetch(bbox, cache, offline=offline, refresh=refresh)
    osm_version = osm_doc["osm3s"]["timestamp_osm_base"]
    dem = Dem([f.path for f in dem_files])
    dem_sources = _dem_sources(dem_files)
    area_poly = _lv95_polygon(_rect(area.x0, area.y0, area.x1, area.y1))
    assets: list[Asset] = []

    print("terrain", flush=True)
    for t in tiles:
        mesh = terrain.build(frame, dem, area, t, origins[t], stats)
        if mesh is not None:
            assets.append(Asset(t, "terrain", f"tile:{tile_key(*t)}", mesh, SWISSTOPO_LICENSE, dem_sources, "trimesh"))

    print("buildings (swissBUILDINGS3D)", flush=True)
    e0, n0, e1, n1 = area_poly.buffer(BUILDING_MARGIN_M).bounds
    swiss = {}
    for sheet in sheets:
        for b in load_buildings(sheet.path, (e0, n0, e1, n1), cache, stats):
            swiss.setdefault(b.gml_id, (b, sheet))
    footprints = []
    for gml_id in sorted(swiss):
        b, sheet = swiss[gml_id]
        fp = buildings.swiss_footprint(b)
        if fp is None:
            _bump(stats, "swissBUILDINGS3D without footprint")
            continue
        footprints.append(fp)
        c = fp.centroid
        tx, ty = _tile_of(c.x, c.y)
        if not area.contains(tx, ty):
            continue
        tile = (int(tx), int(ty))
        mesh = buildings.swiss_shell(frame, b, origins[tile], stats)
        if mesh is None:
            continue
        uuid = gml_id[3:] if gml_id.startswith("ID_") else gml_id
        src = _source("swissBUILDINGS3D 3.0 Beta", sheet.year, SWISSTOPO_LICENSE, SWISSTOPO_CREDIT,
                      STAC_COLLECTIONS + stac.SWISSBUILDINGS3D.collection, [gml_id])
        assets.append(Asset(tile, "shell", f"swisstopo:swissbuildings3d:{uuid.lower()}", mesh,
                            SWISSTOPO_LICENSE, [src], "trimesh"))
    swiss_union = unary_union(footprints)

    print("OpenStreetMap", flush=True)
    ways, osm_footprints = parse_osm(osm_doc, stats)
    used = []
    for w in ways:
        reason = transport.exclusion(w)
        if reason:
            _bump(stats, f"OSM ways not drawn: {reason}")
        else:
            used.append(w)
    bridges = [w for w in used if transport.is_bridge(w)]
    streets = [w for w in used if not transport.is_bridge(w)]
    heights = transport.bridge_heights(bridges, [w for w in ways if not transport.is_bridge(w)], dem, stats)

    def osm_sources(refs: list[str]) -> list[dict]:
        return [_source("OpenStreetMap", osm_version, OSM_LICENSE, OSM_CREDIT, OSM_URL, refs)] + dem_sources

    for w in bridges:
        mid = w.en[len(w.en) // 2] if len(w.en) % 2 else (w.en[len(w.en) // 2 - 1] + w.en[len(w.en) // 2]) / 2
        tx, ty = _tile_of(float(mid[0]), float(mid[1]))
        if not area.contains(tx, ty):
            continue
        tile = (int(tx), int(ty))
        mesh = transport.bridge_deck(frame, w, heights, origins[tile])
        if mesh is None:
            _bump(stats, "bridge decks without heights")
            continue
        _bump(stats, "bridge decks")
        assets.append(Asset(tile, "transport", f"osm:way:{w.id}", mesh, OSM_ON_TERRAIN_LICENSE,
                            osm_sources([w.ref]), "trimesh"))
    for t in tiles:
        x, y = t
        clip = _lv95_polygon(_rect(x, y, x + 1, y + 1)).intersection(area_poly)
        mesh, refs = transport.tile_ribbons(frame, dem, streets, clip, origins[t], stats)
        if not mesh.is_empty():
            assets.append(Asset(t, "transport", f"tile:{tile_key(*t)}", mesh, OSM_ON_TERRAIN_LICENSE,
                                osm_sources(refs), "none"))

    print("buildings (OpenStreetMap fallback)", flush=True)
    inside = []
    for fp in osm_footprints:
        c = fp.shape.centroid
        tx, ty = _tile_of(c.x, c.y)
        if area.contains(tx, ty):
            inside.append((fp, (int(tx), int(ty))))
    candidates = {id(fp) for fp in buildings.fallback_candidates([fp for fp, _ in inside], swiss_union, stats)}
    for fp, tile in inside:
        if id(fp) not in candidates:
            continue
        mesh = buildings.fallback_shell(frame, dem, fp, origins[tile], stats)
        if mesh is not None:
            _bump(stats, "OSM fallback buildings")
            assets.append(Asset(tile, "shell", fp.feature_id, mesh, OSM_ON_TERRAIN_LICENSE,
                                osm_sources([fp.ref]), "trimesh"))

    print("spawn", flush=True)
    if spawn is None:
        lon, lat = tilef_to_lonlat((area.x0 + area.x1) / 2, (area.y0 + area.y1) / 2)
        spawn = Spawn(lon, lat, 0.0)
    se, sn = lonlat_to_lv95(spawn.lon, spawn.lat)
    ground = float(dem.height(se, sn))
    if not np.isfinite(ground):
        raise RuntimeError("no terrain at the spawn point")
    sp = to_mm(frame.from_lonlat(spawn.lon, spawn.lat, ground + SPAWN_ABOVE_GROUND_M))[0]
    yaw = round(spawn.yaw_deg / 360.0 * 65536) % 65536

    print(f"writing {len(assets)} assets", flush=True)
    tmp = out.with_name(out.name + ".tmp")
    if tmp.exists():
        shutil.rmtree(tmp)
    world_id, counts = write_world(Store(tmp), frame, name, assets, origins, [int(v) for v in sp], yaw)
    _replace_dir(tmp, out)

    sizes = [p.stat().st_size for p in out.rglob("*") if p.is_file()]
    report = {
        "world": name,
        "world_id": world_id,
        "output": str(out),
        "bbox": str(bbox),
        "region": frame.key,
        "tiles": len({a.tile for a in assets}),
        "assets": counts,
        "files": len(sizes),
        "bytes": sum(sizes),
        "sources": {
            "swissBUILDINGS3D": [s.item_id for s in sheets],
            "swissALTI3D": [f.item_id for f in dem_files],
            "OpenStreetMap": osm_version,
        },
        "stats": dict(sorted(stats.items())),
        "seconds": round(time.time() - t0, 1),
    }
    return report


def _replace_dir(tmp: Path, out: Path) -> None:
    """Moves the freshly written store into place, replacing an older store
    but never a directory that does not look like one."""
    if out.exists():
        entries = {p.name for p in out.iterdir()}
        if entries and not entries <= {"world.json", "objects"}:
            raise RuntimeError(f"{out} exists and is not a world store; not replacing it")
        shutil.rmtree(out)
    out.parent.mkdir(parents=True, exist_ok=True)
    os.replace(tmp, out)


def write_report(report: dict, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
