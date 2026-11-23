"""Building shells: swissBUILDINGS3D LOD2 surfaces, and the OSM fallback.

swissBUILDINGS3D is the primary source. An OSM building footprint is
extruded only where swissBUILDINGS3D has no building: when at most
`FALLBACK_MAX_OVERLAP` of its area is covered by swissBUILDINGS3D
footprints.
"""

from __future__ import annotations

import numpy as np
from shapely.geometry.polygon import orient
from shapely.geometry import MultiPoint, MultiPolygon, Polygon
from shapely.ops import unary_union

from tools.ingest.citygml import SKIP_OBJEKTART, Building
from tools.ingest.dem import Dem
from tools.ingest.geo import Frame
from tools.ingest.mesh import MeshBuilder
from tools.ingest.osm import Footprint, parse_length_m

FALLBACK_MAX_OVERLAP = 0.2
LEVEL_HEIGHT_M = 3.0
DEFAULT_HEIGHT_M = 10.0
# OSM building values that are not a closed volume standing on the ground.
SKIP_BUILDING = {"no", "roof", "construction", "proposed", "ruins", "collapsed", "demolished", "destroyed", "underground"}

UP = np.array([0.0, 1.0, 0.0])


def swiss_footprint(b: Building) -> Polygon | MultiPolygon | None:
    """2D LV95 footprint: union of the ground surfaces, else the hull."""
    grounds = []
    for kind, rings in b.surfaces:
        if kind == "ground":
            p = Polygon([q[:2] for q in rings[0]], [[q[:2] for q in r] for r in rings[1:]])
            if p.is_valid and p.area > 0:
                grounds.append(p)
    if grounds:
        return unary_union(grounds)
    hull = MultiPoint(b.footprint_points()).convex_hull
    return hull if isinstance(hull, Polygon) and hull.area > 0 else None


def swiss_shell(frame: Frame, b: Building, origin_mm, stats: dict[str, int]) -> MeshBuilder | None:
    if b.objektart in SKIP_OBJEKTART:
        stats[f"swissBUILDINGS3D skipped: {b.objektart}"] = stats.get(f"swissBUILDINGS3D skipped: {b.objektart}", 0) + 1
        return None
    flat = np.asarray([p for _, rings in b.surfaces for r in rings for p in r], dtype=np.float64)
    if len(flat) == 0:
        return None
    pts = frame.from_lv95(flat[:, 0], flat[:, 1], flat[:, 2])
    mesh = MeshBuilder(origin_mm)
    k = 0
    for kind, rings in b.surfaces:
        frame_rings = []
        for r in rings:
            frame_rings.append(pts[k : k + len(r)])
            k += len(r)
        # Walls and roofs keep the source's (outward) orientation. Ground
        # plates in swissBUILDINGS3D 3.0 face into the building; turn them
        # down so every normal points out of the shell.
        towards = -UP if kind == "ground" else None
        if not mesh.add_polygon(kind, frame_rings, towards):
            stats["swissBUILDINGS3D polygons degenerate"] = stats.get("swissBUILDINGS3D polygons degenerate", 0) + 1
    stats["swissBUILDINGS3D triangles dropped (mm snap)"] = (
        stats.get("swissBUILDINGS3D triangles dropped (mm snap)", 0) + mesh.dropped_triangles
    )
    return None if mesh.is_empty() else mesh


def fallback_candidates(footprints: list[Footprint], swiss: Polygon | MultiPolygon, stats: dict[str, int]) -> list[Footprint]:
    out = []
    for fp in footprints:
        t = fp.tags
        reason = None
        if t.get("building") in SKIP_BUILDING:
            reason = f"building={t['building']}"
        elif t.get("location") in ("underground", "underwater"):
            reason = f"location={t['location']}"
        else:
            try:
                if int(t.get("layer", "0")) < 0:
                    reason = "layer below ground"
            except ValueError:
                pass
        if reason is None:
            covered = fp.shape.intersection(swiss).area / fp.shape.area
            if covered > FALLBACK_MAX_OVERLAP:
                reason = "covered by swissBUILDINGS3D"
        if reason:
            key = f"OSM buildings not extruded: {reason}"
            stats[key] = stats.get(key, 0) + 1
        else:
            out.append(fp)
    return out


def fallback_height(tags: dict[str, str]) -> tuple[float, str]:
    h = parse_length_m(tags.get("height"))
    if h:
        return h, "height"
    levels = parse_length_m(tags.get("building:levels"))
    if levels:
        return levels * LEVEL_HEIGHT_M, "building:levels"
    return DEFAULT_HEIGHT_M, "default"


def _ring_coords(ring) -> np.ndarray:
    c = np.asarray(ring.coords)[:, :2]
    return c[:-1] if len(c) > 1 and np.array_equal(c[0], c[-1]) else c


def fallback_shell(frame: Frame, dem: Dem, fp: Footprint, origin_mm, stats: dict[str, int]) -> MeshBuilder | None:
    polys = list(fp.shape.geoms) if isinstance(fp.shape, MultiPolygon) else [fp.shape]
    polys = [orient(p, sign=1.0) for p in polys]
    rings = [_ring_coords(r) for p in polys for r in [p.exterior, *p.interiors]]
    allpts = np.concatenate(rings)
    ground = dem.height(allpts[:, 0], allpts[:, 1])
    if not np.any(np.isfinite(ground)):
        stats["OSM fallback buildings without terrain"] = stats.get("OSM fallback buildings without terrain", 0) + 1
        return None
    base = float(np.nanmin(ground))
    height, rule = fallback_height(fp.tags)
    stats[f"OSM fallback height from {rule}"] = stats.get(f"OSM fallback height from {rule}", 0) + 1
    top = base + height
    mesh = MeshBuilder(origin_mm)
    for p in polys:
        ring_list = [_ring_coords(r) for r in [p.exterior, *p.interiors]]
        for level, material, towards in ((top, "roof", UP), (base, "ground", -UP)):
            frame_rings = [frame.from_lv95(r[:, 0], r[:, 1], level) for r in ring_list]
            mesh.add_polygon(material, frame_rings, towards)
        for r in ring_list:
            q = np.roll(r, -1, axis=0)
            # Exterior rings are counter-clockwise and holes clockwise, so the
            # outside of the wall is always to the right of p -> q.
            out_en = np.stack([q[:, 1] - r[:, 1], -(q[:, 0] - r[:, 0])], axis=1)
            towards = np.stack([out_en[:, 0], np.zeros(len(r)), -out_en[:, 1]], axis=1)
            pb = frame.from_lv95(r[:, 0], r[:, 1], base)
            pt = frame.from_lv95(r[:, 0], r[:, 1], top)
            qb, qt = np.roll(pb, -1, axis=0), np.roll(pt, -1, axis=0)
            tris = np.concatenate([np.stack([pb, qb, pt], 1), np.stack([pt, qb, qt], 1)])
            mesh.add_flat("wall", tris, np.concatenate([towards, towards]))
    return None if mesh.is_empty() else mesh
