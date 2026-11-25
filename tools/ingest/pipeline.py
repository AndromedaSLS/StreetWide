"""The ingest pipeline: sources -> meshes -> world store."""

from __future__ import annotations

import json
import math
import os
import shutil
import time
from dataclasses import dataclass
from pathlib import Path

import numpy as np
import shapely
from shapely.geometry import LineString, Polygon
from shapely.ops import unary_union

from tools.ingest import buildings, heights, overpass, point_features, stac, stadt_zuerich, terrain, transport, water
from tools.ingest.assemble import (
    CITY_POINTS_LICENSE,
    OSM_CREDIT,
    OSM_LICENSE,
    OSM_ON_TERRAIN_LICENSE,
    SWISSTOPO_CREDIT,
    SWISSTOPO_LICENSE,
    Asset,
    write_world,
)
from tools.ingest.citygml import SKIP_OBJEKTART, load_buildings
from tools.ingest.dem import Dem
from tools.ingest.geo import (
    Area,
    BBox,
    Frame,
    lonlat_to_lv95,
    lonlat_to_tilef,
    lv95_to_lonlat,
    tile_key,
    tilef_to_lonlat,
    to_mm,
)
from tools.ingest.osm import parse as parse_osm
from tools.worldfmt.store import NOTICE_FILES, Store

STAC_COLLECTIONS = "https://data.geo.admin.ch/api/stac/v1/collections/"
OSM_URL = "https://www.openstreetmap.org/"
BUILDING_MARGIN_M = 100.0
SPAWN_ABOVE_GROUND_M = 1.0
SURFACE_SEARCH_MARGIN_M = 3.0
"""How far beyond the area the surface model is searched for, so that a
tree's 1.5 m disc can read the next tile (ADR-0016); only the tiles the
discs reach are downloaded."""
NEAR_BUILDING_M = 3.0
NEAR_DECK_M = 5.0


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
    return _item_sources("swissALTI3D", stac.SWISSALTI3D, files)


def _item_sources(dataset: str, product: stac.Product, files: list[stac.SourceFile]) -> list[dict]:
    """One source entry per year of swisstopo items, listing their ids."""
    years = sorted({f.year for f in files})
    return [
        _source(dataset, y, SWISSTOPO_LICENSE, SWISSTOPO_CREDIT, STAC_COLLECTIONS + product.collection,
                [f.item_id for f in files if f.year == y])
        for y in years
    ]


def _terrain_tile(f: stac.SourceFile) -> tuple[int, int]:
    """A swissALTI3D item's tile ("2683-1247") as LV95 kilometres."""
    e, n = f.tile.split("-")
    return int(e), int(n)


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
    water_doc = overpass.fetch_water(bbox, cache, offline=offline, refresh=refresh)
    water_version = water_doc["osm3s"]["timestamp_osm_base"]
    # Parsed now, so a broken river relation stops the run before the slow steps.
    bodies = water.parse(water_doc, bbox, stats)
    dem = Dem([f.path for f in dem_files])
    dem_sources = _dem_sources(dem_files)
    area_poly = _lv95_polygon(_rect(area.x0, area.y0, area.x1, area.y1))
    # Each tile cut to the area, in LV95.
    clips = {(x, y): _lv95_polygon(_rect(x, y, x + 1, y + 1)).intersection(area_poly) for x, y in tiles}
    assets: list[Asset] = []

    print("terrain", flush=True)
    grounds: dict[tuple[int, int], point_features.Ground] = {}
    for t in tiles:
        mesh = terrain.build(frame, dem, area, t, origins[t], stats)
        if mesh is not None:
            assets.append(Asset(t, "terrain", f"tile:{tile_key(*t)}", mesh, SWISSTOPO_LICENSE, dem_sources, "trimesh"))
            grounds[t] = point_features.Ground(mesh)

    print("buildings (swissBUILDINGS3D)", flush=True)
    e0, n0, e1, n1 = area_poly.buffer(BUILDING_MARGIN_M).bounds
    swiss = {}
    for sheet in sheets:
        for b in load_buildings(sheet.path, (e0, n0, e1, n1), cache, stats):
            swiss.setdefault(b.gml_id, (b, sheet))
    footprints = []
    # Footprints of buildings that stand on the ground, which mask tree
    # heights (ADR-0016) and count light points inside buildings (ADR-0015).
    standing = []
    for gml_id in sorted(swiss):
        b, sheet = swiss[gml_id]
        fp = buildings.swiss_footprint(b)
        if fp is None:
            _bump(stats, "swissBUILDINGS3D without footprint")
            continue
        footprints.append(fp)
        if b.objektart not in SKIP_OBJEKTART:
            standing.append(fp)
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
        if transport.densified_count(w.en) > transport.MAX_DECK_POINTS:
            _bump(stats, "bridge decks too long")
            continue
        mesh = transport.bridge_deck(frame, w, heights, origins[tile])
        if mesh is None:
            _bump(stats, "bridge decks without heights")
            continue
        _bump(stats, "bridge decks")
        assets.append(Asset(tile, "transport", f"osm:way:{w.id}", mesh, OSM_ON_TERRAIN_LICENSE,
                            osm_sources([w.ref]), "trimesh"))
    for t in tiles:
        mesh, refs = transport.tile_ribbons(frame, dem, streets, clips[t], origins[t], stats)
        if not mesh.is_empty():
            assets.append(Asset(t, "transport", f"tile:{tile_key(*t)}", mesh, OSM_ON_TERRAIN_LICENSE,
                                osm_sources(refs), "none"))

    print("water (OpenStreetMap, ADR-0014)", flush=True)
    water_assets, water_report = build_water(frame, dem, dem_files, bodies, water_version, area_poly, clips, origins, stats)
    assets += water_assets

    print("buildings (OpenStreetMap fallback)", flush=True)
    inside = []
    for fp in osm_footprints:
        c = fp.shape.centroid
        tx, ty = _tile_of(c.x, c.y)
        if area.contains(tx, ty):
            inside.append((fp, (int(tx), int(ty))))
    candidates = {id(fp) for fp in buildings.fallback_candidates([fp for fp, _ in inside], swiss_union, stats)}
    fallback_shapes = []
    for fp, tile in inside:
        if id(fp) not in candidates:
            continue
        mesh = buildings.fallback_shell(frame, dem, fp, origins[tile], stats)
        if mesh is not None:
            _bump(stats, "OSM fallback buildings")
            assets.append(Asset(tile, "shell", fp.feature_id, mesh, OSM_ON_TERRAIN_LICENSE,
                                osm_sources([fp.ref]), "trimesh"))
            fallback_shapes.append(fp.shape)

    print("trees and lights (Stadt Zuerich, ADR-0015; heights, ADR-0016)", flush=True)
    checks = Checks(standing, fallback_shapes, bridges, [b.shape for b in bodies])
    point_assets, points_report = build_points(frame, dem, dem_files, area, area_poly, origins, grounds, checks,
                                               cache, offline, refresh, stats)
    assets += point_assets

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
            "OpenStreetMap water": water_version,
        },
        "water": water_report,
        "points": points_report,
        "stats": dict(sorted(stats.items())),
        "seconds": round(time.time() - t0, 1),
    }
    return report


def build_water(frame: Frame, dem: Dem, dem_files: list[stac.SourceFile], bodies: list[water.Body],
                version: str, area_poly: Polygon, clips: dict[tuple[int, int], Polygon],
                origins: dict[tuple[int, int], list[int]], stats: dict[str, int]) -> tuple[list[Asset], dict]:
    """One water asset per tile (RFC 0003, ADR-0014) and the report's water
    section. `version` is the water snapshot's `timestamp_osm_base`, and
    `clips` are the tiles cut to the area."""
    items = {_terrain_tile(f): f for f in dem_files}
    cut = water.pieces(dem, bodies, area_poly, set(items), stats)
    assets: list[Asset] = []
    tiles: dict[str, dict] = {}
    for t in sorted(clips):
        w = water.tile_water(frame, dem, cut, clips[t], origins[t], stats)
        if w.area_m2 > 0 or w.drawn_m2 > 0:
            tiles[tile_key(*t)] = {
                "area_m2": round(w.area_m2, 1),
                "drawn_m2": round(w.drawn_m2, 1),
                "hidden_m2": round(w.hidden_m2, 1),
            }
        if w.mesh.is_empty():
            continue
        # The OSM ids of the bodies drawn here, and the terrain tiles whose
        # heights set their levels.
        osm = _source("OpenStreetMap", version, OSM_LICENSE, OSM_CREDIT, OSM_URL, w.refs)
        assets.append(Asset(t, "water", f"tile:{tile_key(*t)}", w.mesh, OSM_ON_TERRAIN_LICENSE,
                            [osm] + _dem_sources([items[k] for k in w.terrain_tiles]), "none"))
    def piece(p: water.Piece) -> dict:
        terrain = items[p.terrain_tile].item_id if p.terrain_tile in items else "{}-{}".format(*p.terrain_tile)
        name = {"name": p.body.tags["name"]} if "name" in p.body.tags else {}
        return {"body": p.body.ref, **name, "swissALTI3D": terrain}

    levels = [
        {
            **piece(p),
            # The water surface, and the spread of the terrain samples under it.
            "level_m": round(p.level.height_m, 3),
            "p10_m": round(p.level.p10_m, 3),
            "p90_m": round(p.level.p90_m, 3),
            "samples": p.level.samples,
        }
        for p in cut
        if p.level is not None
    ]
    report = {
        "assets": len(assets),
        "area_m2": round(sum(v["area_m2"] for v in tiles.values()), 1),
        "hidden_m2": round(sum(v["hidden_m2"] for v in tiles.values()), 1),
        "tiles": tiles,
        "levels": levels,
        # Water with no level in a terrain tile (no grid point far enough
        # inside it, or no terrain loaded there) is left out.
        "not_drawn": [piece(p) for p in cut if p.level is None],
    }
    return assets, report


@dataclass
class Checks:
    """What the points report measures points against (ADR-0015, ADR-0016):
    swissBUILDINGS3D footprints of buildings standing on the ground (which
    also mask tree heights), and, as checks only that never change a point,
    OSM fallback buildings, OSM bridge ways and OSM water areas."""

    footprints: list
    fallback: list
    bridges: list
    water: list


def _expanded(bbox: BBox, metres: float) -> BBox:
    """`bbox` grown by at least `metres` on every side."""
    dlat = metres / 110_000.0
    dlon = metres / (111_320.0 * math.cos(math.radians(max(abs(bbox.minlat), abs(bbox.maxlat)))))
    return BBox(bbox.minlon - dlon, bbox.minlat - dlat, bbox.maxlon + dlon, bbox.maxlat + dlat)


def _percentiles(values: list[float]) -> dict | None:
    if not values:
        return None
    return {f"p{q}": round(float(np.percentile(values, q)), 1) for q in (5, 25, 50, 75, 95)}


def _point_sources(dataset: stadt_zuerich.Dataset, placed: list[point_features.Placed]) -> list[dict]:
    """One source entry per answer the points came from (ADR-0015)."""
    by_answer: dict[str, tuple[stadt_zuerich.Answer, list[str]]] = {}
    for p in placed:
        by_answer.setdefault(p.record.answer.key, (p.record.answer, []))[1].append(p.record.local_id)
    return [
        _source(dataset.name, answer.version, stadt_zuerich.LICENSE, stadt_zuerich.CREDIT, dataset.url, ids)
        for _, (answer, ids) in sorted(by_answer.items())
    ]


def tree_heights(placed: list[point_features.Placed], bbox: BBox, dem: Dem, dem_files: list[stac.SourceFile],
                 footprints: list, cache: Path, offline: bool, refresh: bool
                 ) -> tuple[list[heights.Height], dict[tuple[int, int], stac.SourceFile],
                            dict[tuple[int, int], stac.SourceFile]]:
    """ADR-0016's heights of the placed trees, and the swissSURFACE3D Raster
    and swissALTI3D items of each kilometre tile the discs reach."""
    needed: set[tuple[int, int]] = set()
    for p in placed:
        needed |= heights.disc_tiles(p.record.e, p.record.n)
    wider = _expanded(bbox, SURFACE_SEARCH_MARGIN_M)
    found = stac.find(stac.SWISSSURFACE3D_RASTER, wider, cache, offline=offline, refresh=refresh)
    surface_files = {_terrain_tile(f): f for f in found if _terrain_tile(f) in needed}
    stac.fetch(stac.SWISSSURFACE3D_RASTER, list(surface_files.values()), offline=offline)
    area_terrain = {_terrain_tile(f): f for f in dem_files}
    terrain_dem: Dem | None
    if needed <= set(area_terrain):
        terrain_files = {t: area_terrain[t] for t in needed}
        terrain_dem = dem
    else:
        found = stac.find(stac.SWISSALTI3D, wider, cache, offline=offline, refresh=refresh)
        terrain_files = {_terrain_tile(f): f for f in found if _terrain_tile(f) in needed}
        stac.fetch(stac.SWISSALTI3D, list(terrain_files.values()), offline=offline)
        terrain_dem = Dem([terrain_files[t].path for t in sorted(terrain_files)]) if terrain_files else None
    surface = (Dem([surface_files[t].path for t in sorted(surface_files)])
               if surface_files and terrain_dem is not None else None)
    years = {t: int(f.year) for t, f in surface_files.items()}
    trees = [(p.record.e, p.record.n, p.ground_m, p.record.attributes["planted_year"]) for p in placed]
    return heights.measure(trees, surface, terrain_dem, years, footprints), surface_files, terrain_files


def build_points(frame: Frame, dem: Dem, dem_files: list[stac.SourceFile], area: Area, area_poly: Polygon,
                 origins: dict[tuple[int, int], list[int]], grounds: dict[tuple[int, int], point_features.Ground],
                 checks: Checks, cache: Path, offline: bool, refresh: bool, stats: dict[str, int]
                 ) -> tuple[list[Asset], dict]:
    """The trees and light points of the area (RFC 0004, ADR-0015,
    ADR-0016): one asset per tile and source, and the report's points
    section."""
    assets: list[Asset] = []
    squares = stadt_zuerich.squares(area_poly)
    report: dict = {"squares": ["{}-{}".format(*s) for s in squares], "answers": []}
    for dataset in (stadt_zuerich.TREES, stadt_zuerich.LIGHTS):
        answers = [stadt_zuerich.fetch_answer(dataset, s, cache, offline=offline, refresh=refresh) for s in squares]
        report["answers"] += [{"dataset": dataset.name, "square": a.key, "date": a.date, "sha256": a.sha256,
                               "features": len(a.features), "hits": a.hits} for a in answers]
        records = stadt_zuerich.records(dataset, answers, stats)
        placed = point_features.place(records, frame, dem, area, origins, grounds, stats, dataset.name)
        layer = "vegetation" if dataset.kind == "tree" else "street_objects"
        section: dict = {"records": len(records), "points": len(placed)}
        prefix = f"{dataset.name} left out: "
        section["left_out"] = {k[len(prefix):]: v for k, v in sorted(stats.items()) if k.startswith(prefix)}
        measured: dict[str, int | None] = {}
        tree_sources: dict[tuple[int, int], list[dict]] = {}
        if dataset.kind == "tree" and placed:
            found, surface_files, terrain_files = tree_heights(placed, area.bbox(), dem, dem_files, checks.footprints,
                                                               cache, offline, refresh)
            by_tile: dict[tuple[int, int], tuple[set, set]] = {}
            reasons: dict[str, int] = {}
            for p, h in zip(placed, found):
                measured[p.record.point_id] = h.mm
                if h.reason:
                    reasons[h.reason] = reasons.get(h.reason, 0) + 1
                read = by_tile.setdefault(p.tile, (set(), set()))
                read[0].update(t for t in h.tiles if t in surface_files)
                read[1].update(t for t in h.tiles if t in terrain_files)
            for tile, (surface_read, terrain_read) in by_tile.items():
                # The anchors stand on the area's terrain, which lists every
                # swissALTI3D item of the area; heights may read more.
                terrain_items = {f.item_id: f for f in dem_files}
                terrain_items.update({terrain_files[t].item_id: terrain_files[t] for t in terrain_read})
                tree_sources[tile] = (
                    _dem_sources([terrain_items[k] for k in sorted(terrain_items)])
                    + _item_sources("swissSURFACE3D Raster", stac.SWISSSURFACE3D_RASTER,
                                    [surface_files[t] for t in sorted(surface_read)]))
            values = [h.mm / 1000.0 for h in found if h.mm is not None]
            section["heights"] = {
                "measured": len(values),
                "null": dict(sorted(reasons.items())),
                "percentiles_m": _percentiles(values),
                "swissSURFACE3D Raster": sorted(f.item_id for f in surface_files.values()),
            }
        if dataset.kind == "tree":
            section["crowns_at_the_8_m_default"] = sum(p.record.crown_default for p in placed)
            surveys: dict[str, int] = {}
            for p in placed:
                key = p.record.survey or "(none)"
                surveys[key] = surveys.get(key, 0) + 1
            section["survey_methods"] = dict(sorted(surveys.items()))
        else:
            section["orientations_of_0"] = sum(p.record.orientation_zero for p in placed)
            section["orientations_unknown"] = sum(p.yaw is None for p in placed)
            positions: dict[tuple[float, float], int] = {}
            for p in placed:
                positions[(p.record.e, p.record.n)] = positions.get((p.record.e, p.record.n), 0) + 1
            stacks = [k for k in positions.values() if k > 1]
            section["positions"] = len(positions)
            section["stacks"] = {"positions": len(stacks), "records": sum(stacks), "largest": max(stacks, default=0)}
        section.update(_point_checks(dataset.kind, placed, checks))

        by_tile_placed: dict[tuple[int, int], list[point_features.Placed]] = {}
        for p in placed:
            by_tile_placed.setdefault(p.tile, []).append(p)
        tiles_report = {}
        for tile in sorted(by_tile_placed):
            group = by_tile_placed[tile]
            doc = point_features.document(dataset.kind, group, measured)
            sources = _point_sources(dataset, group) + tree_sources.get(tile, _dem_sources(dem_files))
            assets.append(Asset(tile, layer, f"tile:{tile_key(*tile)}/{dataset.source}", None, CITY_POINTS_LICENSE,
                                sources, "none", points=doc))
            tiles_report[tile_key(*tile)] = len(group)
        section["assets"] = len(tiles_report)
        section["tiles"] = tiles_report
        report[layer] = section
    return assets, report


def _point_checks(kind: str, placed: list[point_features.Placed], checks: Checks) -> dict:
    """Counts against buildings, bridges and water for the report; nothing
    here changes a point (ADR-0015, "No OSM in these points")."""
    if not placed:
        return {}
    e = np.array([p.record.e for p in placed])
    n = np.array([p.record.n for p in placed])
    pts = shapely.points(e, n)
    none = np.zeros(len(placed), dtype=bool)
    out: dict = {}
    swiss = unary_union(checks.footprints) if checks.footprints else None
    if kind == "tree":
        near = shapely.distance(swiss, pts) <= NEAR_BUILDING_M if swiss is not None else none
        out["within_3_m_of_a_swissBUILDINGS3D_building"] = int(near.sum())
        fallback = unary_union(checks.fallback) if checks.fallback else None
        near = shapely.distance(fallback, pts) <= NEAR_BUILDING_M if fallback is not None else none
        out["check_within_3_m_of_an_OSM_fallback_building"] = int(near.sum())
        return out
    inside = shapely.contains_xy(swiss, e, n) if swiss is not None else none
    deep = inside & (shapely.distance(swiss.boundary, pts) > 1.0) if swiss is not None else none
    out["inside_swissBUILDINGS3D_footprints"] = int(inside.sum())
    out["more_than_1_m_inside"] = int(deep.sum())
    gap = np.full(len(placed), np.inf)
    for w in checks.bridges:
        gap = np.minimum(gap, shapely.distance(LineString(w.en), pts) - transport.width_m(w) / 2.0)
    out["check_on_an_OSM_bridge_deck"] = int((gap <= 0).sum())
    out["check_within_5_m_of_an_OSM_bridge_deck"] = int((gap <= NEAR_DECK_M).sum())
    water_union = unary_union(checks.water) if checks.water else None
    out["check_inside_OSM_water"] = int(shapely.contains_xy(water_union, e, n).sum()) if water_union is not None else 0
    return out


STORE_ENTRIES = {"world.json", "objects", *NOTICE_FILES}
"""What a store the ingest wrote holds: the world, and its licence notices
(ADR-0019). Anything else, such as the `CANONICAL` marker of a canonical
store (docs/contributing/baseline-refresh.md), stops the ingest from
replacing the directory."""


def _replace_dir(tmp: Path, out: Path) -> None:
    """Moves the freshly written store into place, replacing an older store
    but never a directory that does not look like one."""
    if out.exists():
        entries = {p.name for p in out.iterdir()}
        if entries and not entries <= STORE_ENTRIES:
            raise RuntimeError(f"{out} exists and is not a world store; not replacing it")
        shutil.rmtree(out)
    out.parent.mkdir(parents=True, exist_ok=True)
    os.replace(tmp, out)


def write_report(report: dict, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
