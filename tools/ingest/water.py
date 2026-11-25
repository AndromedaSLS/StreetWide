"""Water layer from OpenStreetMap water areas (RFC 0003, ADR-0014).

- Selection: areas tagged `natural=water` (rivers, canals, lakes, ponds
  and the like), `waterway=riverbank` or `landuse=reservoir`. Fountains,
  basins, reflecting pools and covered or underground water are left out.
- Assembly: ways and multipolygon relations become polygons with holes in
  LV95, repaired if invalid. The repair is bounded: an invalid ring with
  more than MAX_REPAIR_PAIRS pairs of segments that are not neighbours and
  whose bounding boxes overlap counts as broken, as a ring that does not
  close does. Rings nest as deep as they are mapped: a pond on an island
  in a lake is water. A relation that reaches the area but gives no area
  (rings that do not close or are too broken to repair, a member way
  without geometry or with a role other than outer or inner) stops the
  ingest: a river must never vanish silently. A way that cannot be
  repaired is counted as unusable.
- Level: swissALTI3D models water as its surface, so each water body gets
  one flat level per swissALTI3D tile: the median terrain height at 2 m
  grid points at least 3 m inside the outline, plus 3 cm. Only that
  terrain tile's pixels are read, so the level depends on the body and the
  terrain tile, never on the area being ingested. Water with no such grid
  point in a terrain tile has no level there and is not drawn; the report
  lists it.
- Mesh: the outline, buffered by 1 m so the shoreline tucks under the
  banks, is cut per z17 tile and to the area, triangulated in LV95 and
  converted to the region frame vertex by vertex, facing straight up. One
  visual-only asset per tile (`collider: none`): players still walk on
  the terrain underneath.

The Overpass answer is untrusted (a download, or a file in the cache): an
element without the structure Overpass gives raises OsmError, as in
`osm.parse`, and a relation too complex to assemble is a broken relation.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

import mapbox_earcut
import numpy as np
import shapely
from shapely import make_valid
from shapely.errors import GEOSException
from shapely.geometry import MultiPolygon, Polygon, box
from shapely.ops import unary_union

from tools.ingest.dem import Dem
from tools.ingest.geo import BBox, Frame, lonlat_to_lv95, to_mm
from tools.ingest.mesh import MeshBuilder
from tools.ingest.osm import MAX_WAY_NODES, OsmError, RingError, _coordinate, _tags, join_rings

# Kept: `natural=water` with no `water` tag or one of these.
KEEP_WATER = ("river", "canal", "lake", "oxbow", "lagoon", "reservoir", "pond")
# Left out, whatever else the element says.
LEAVE_OUT = (
    ("amenity", "fountain"),
    ("amenity", "drinking_water"),
    ("water", "reflecting_pool"),
    ("water", "basin"),
    ("covered", "yes"),
    ("tunnel", "culvert"),
    ("location", "underground"),
)
BUFFER_M = 1.0
INSIDE_M = 3.0
GRID_M = 2
LIFT_M = 0.03
# The report counts water as hidden where the terrain is this far above it.
HIDDEN_M = 0.1
# swissALTI3D tiles are 1 km squares on whole LV95 kilometres.
TERRAIN_TILE_M = 1000
# A body is cut to a terrain tile plus this margin before it is buffered.
# The margin is wider than INSIDE_M and BUFFER_M, so the cut never shows
# inside the tile, and the result depends on the tile alone.
MARGIN_M = 10.0
UP = np.array([0.0, 1.0, 0.0])
MAX_NEST_TESTS = 1_000_000
"""Most ring-in-ring tests to nest the rings of one relation. A real
relation needs one or two per ring; rings stacked on one another need one
per pair, and such a relation counts as one that gives no area."""
MAX_REPAIR_PAIRS = 1_000
"""Most pairs of segments that are not neighbours and whose bounding boxes
overlap in an invalid ring that the ingest repairs (ADR-0014, open
question 1, option E). GEOS's repair grows with how often a ring crosses
or touches itself, more than with its length: under this cap the slowest
repair measured took about a second, and of the real rings sampled,
Bodensee had the most such pairs, 503. A ring with more counts as broken.
It is a count, not a clock, so every machine decides the same."""
QUERY_PAIRS = 1 << 20
"""Most pairs one query of the segment tree may return, so that counting
the pairs of a long ring that retraces itself never holds more than this
many in memory."""


class WaterError(RuntimeError):
    """OpenStreetMap water data that the ingest will not guess around."""


@dataclass
class Body:
    osm_type: str  # "way" or "relation"
    id: int
    tags: dict[str, str]
    shape: Polygon | MultiPolygon  # LV95, as mapped (not buffered)

    @property
    def ref(self) -> str:
        return f"{self.osm_type}/{self.id}"


@dataclass
class Level:
    """The water level of one body in one swissALTI3D tile."""

    body: str
    terrain_tile: tuple[int, int]  # south-west corner in LV95 kilometres
    height_m: float  # the median sample plus LIFT_M (LN02, like the terrain)
    samples: int
    p10_m: float
    p90_m: float


@dataclass
class Piece:
    """One body within one swissALTI3D tile."""

    body: Body
    terrain_tile: tuple[int, int]
    level: Level | None  # None if it has no level in that tile: not drawn
    outline: Polygon | MultiPolygon | None  # as mapped, cut to the terrain tile
    surface: Polygon | MultiPolygon  # buffered, cut to the terrain tile


@dataclass
class TileWater:
    mesh: MeshBuilder
    refs: list[str]  # bodies with triangles in the mesh
    terrain_tiles: list[tuple[int, int]]  # whose levels the mesh uses
    area_m2: float  # water as mapped
    drawn_m2: float  # the buffered surface in the mesh
    hidden_m2: float  # water under terrain more than HIDDEN_M above it (at most area_m2)


def _bump(stats: dict[str, int], key: str, k: int = 1) -> None:
    stats[key] = stats.get(key, 0) + k


def exclusion(tags: dict[str, str]) -> str | None:
    """Why a water element is left out (ADR-0014), or None if it is kept."""
    for key, value in LEAVE_OUT:
        if tags.get(key) == value:
            return f"{key}={value}"
    if tags.get("waterway") == "riverbank" or tags.get("landuse") == "reservoir":
        return None
    if tags.get("natural") == "water":
        kind = tags.get("water")
        return None if kind is None or kind in KEEP_WATER else f"water={kind}"
    return "not a water area"


def _polygons(geom) -> list[Polygon]:
    """The polygons with area in any geometry, collections flattened."""
    if isinstance(geom, Polygon):
        return [geom] if geom.area > 0 else []
    return [p for g in getattr(geom, "geoms", []) for p in _polygons(g)]


def overlapping_pairs(ring: np.ndarray, cap: int) -> int:
    """The pairs of segments of the closed `ring` (points, the first equal
    to the last) that are not neighbours and whose bounding boxes overlap or
    touch, counted up to `cap + 1`: the count stops there. Repeated points
    are dropped first, so a segment of zero length makes no pair. A segment
    and the next one are neighbours, and so are the last and the first."""
    keep = np.r_[True, np.any(ring[1:] != ring[:-1], axis=1)]
    ring = ring[keep]
    m = len(ring) - 1  # segments
    if m < 4:
        return 0  # every pair of a triangle is a pair of neighbours
    segments = shapely.linestrings(np.stack([ring[:-1], ring[1:]], axis=1))
    tree = shapely.STRtree(segments)
    step = max(1, QUERY_PAIRS // m)  # a segment can meet at most m others
    count = 0
    for start in range(0, m, step):
        # Every pair (i, j) of a segment i in this batch and a segment j of
        # the ring whose boxes meet; each pair of segments is counted once,
        # from its lower index.
        i, j = tree.query(segments[start:start + step])
        i = i + start
        count += int(np.count_nonzero((j > i + 1) & ~((i == 0) & (j == m - 1))))
        if count > cap:
            return cap + 1
    return count


def _check_repair(poly: Polygon) -> None:
    """Raises RingError, completing "the relation reaches the area, but
    ...", if `poly`, an invalid polygon, is too broken to repair: its rings
    have more than MAX_REPAIR_PAIRS pairs of segments that are not
    neighbours and whose bounding boxes overlap (ADR-0014, option E)."""
    pairs = 0
    for ring in (poly.exterior, *poly.interiors):
        pairs += overlapping_pairs(shapely.get_coordinates(ring), MAX_REPAIR_PAIRS - pairs)
        if pairs > MAX_REPAIR_PAIRS:
            raise RingError(
                f"a ring of {len(ring.coords)} points crosses or touches itself too often to be repaired: more "
                f"than {MAX_REPAIR_PAIRS} pairs of its segments that are not neighbours have overlapping "
                "bounding boxes"
            )


def _area(geom) -> Polygon | MultiPolygon | None:
    """The polygonal part of `geom`, repaired if invalid; None if empty.
    An invalid polygon is repaired only if it is not too broken: raises
    RingError otherwise (`_check_repair`)."""
    if geom.is_empty:
        return None
    if not geom.is_valid:
        if isinstance(geom, Polygon):
            _check_repair(geom)
        geom = make_valid(geom)
    parts = _polygons(geom)
    if not parts:
        return None
    return parts[0] if len(parts) == 1 else unary_union(parts)


def _lv95_ring(points: list[tuple[float, float]]) -> Polygon | MultiPolygon | None:
    """The area inside a closed ring of (lon, lat) points, in LV95. Raises
    RingError for a ring too broken to repair (`_check_repair`)."""
    if len(points) < 4:
        return None
    lon, lat = np.array(points, dtype=np.float64).T
    e, n = lonlat_to_lv95(lon, lat)
    en = np.stack([np.asarray(e), np.asarray(n)], axis=1)
    if not np.isfinite(en).all():
        raise OsmError("a geometry point is outside LV95")
    return _area(Polygon(en))


def _points(geometry: object, what: str) -> list[tuple[float, float] | None] | None:
    """The (lon, lat) points of an Overpass geometry, with None where it
    gives none; None if there is no geometry. Raises OsmError for anything
    Overpass does not give."""
    if not geometry:
        return None
    if not isinstance(geometry, list):
        raise OsmError(f"{what}: a geometry is not an array")
    if len(geometry) > MAX_WAY_NODES:
        raise OsmError(f"{what}: a way of {len(geometry)} points, more than OpenStreetMap allows ({MAX_WAY_NODES})")
    return [None if g is None else (_coordinate(g, "lon", 180.0), _coordinate(g, "lat", 90.0)) for g in geometry]


def _way_area(el: dict) -> Polygon | MultiPolygon | None:
    points = _points(el.get("geometry"), f"way/{el['id']}")
    nodes = el.get("nodes", [])
    if not isinstance(nodes, list) or not all(type(n) is int for n in nodes):
        raise OsmError(f"way/{el['id']}: node ids are not integers")
    if not points or any(p is None for p in points) or len(nodes) < 4 or nodes[0] != nodes[-1]:
        return None
    return _lv95_ring(points)


def _check_relation(el: dict) -> None:
    """Raises OsmError unless the relation has the structure Overpass gives:
    members that are objects, way members with an integer ref, valid
    geometries and, if given, numeric bounds."""
    what = f"relation/{el['id']}"
    members = el.get("members", [])
    if not isinstance(members, list) or not all(isinstance(m, dict) for m in members):
        raise OsmError(f"{what}: members are not objects")
    for m in members:
        if m.get("type") == "way" and type(m.get("ref")) is not int:
            raise OsmError(f"{what}: a member way has no integer ref")
        _points(m.get("geometry"), what)
    b = el.get("bounds")
    if b is not None:
        if not isinstance(b, dict):
            raise OsmError(f"{what}: bounds are not an object")
        for key, limit in (("minlon", 180.0), ("maxlon", 180.0), ("minlat", 90.0), ("maxlat", 90.0)):
            v = b.get(key)
            if type(v) not in (int, float) or not abs(v) <= limit:
                raise OsmError(f"{what}: bounds have no valid {key}")


def _relation_area(el: dict, stats: dict[str, int]) -> Polygon | MultiPolygon:
    """The area of a multipolygon relation. Raises RingError, completing
    "the relation reaches the area, but ...", if it gives none: its rings
    do not close or one is too broken to repair, a member way has no
    geometry or a role other than outer or inner, or the rings enclose
    nothing."""
    lines: dict[str, list[list[tuple[float, float]]]] = {"outer": [], "inner": []}
    seen: set[tuple[str, int]] = set()
    for m in el.get("members", []):
        if m.get("type") != "way":
            continue
        role = m.get("role") or "outer"  # an empty role counts as outer
        if not isinstance(role, str) or role not in lines:
            # Its place in the area is unknown: guessing could drop a river.
            raise RingError(f"its member way {m.get('ref')} has the role {role!r}, not outer or inner")
        if (role, m.get("ref")) in seen:
            continue  # listed twice; once is enough
        seen.add((role, m.get("ref")))
        geometry = m.get("geometry")
        if not geometry or any(g is None for g in geometry):
            raise RingError(f"its member way {m.get('ref')} has no geometry")
        lines[role].append([(g["lon"], g["lat"]) for g in geometry])
    rings: list[tuple[str, Polygon | MultiPolygon]] = []
    for role in ("outer", "inner"):
        try:
            joined = join_rings(lines[role])
        except RingError as e:
            raise RingError(f"its member ways do not form closed rings: {e}") from e
        for r in joined:
            try:
                p = _lv95_ring(r)  # a ring too broken to repair raises RingError
            except GEOSException as e:  # a ring GEOS cannot repair
                raise RingError(f"its rings cannot be repaired ({e})") from e
            if p is not None:
                rings.append((role, p))
    try:
        shape = _nest(rings, stats)
    except GEOSException as e:
        raise RingError(f"its rings cannot be combined ({e})") from e
    if shape is None:
        raise RingError("its member ways enclose no area")
    return shape


def _nest(rings: list[tuple[str, Polygon | MultiPolygon]], stats: dict[str, int]) -> Polygon | MultiPolygon | None:
    """Outer rings minus inner rings, nested as deep as they are mapped.
    Rings are taken level by level, by how many larger rings contain them:
    outer rings are added and inner rings cut away, so a pond (outer) on an
    island (inner) in a lake (outer) is water again. With no ring inside an
    inner ring, this is the union of the outer rings minus the union of the
    inner rings."""
    shapes = [s for _, s in rings]
    for s in shapes:
        shapely.prepare(s)
    # Only rings whose bounding box holds the point can contain it; the tree
    # finds them without testing every pair.
    tree = shapely.STRtree(shapes)
    depth = []
    tests = 0
    for i, s in enumerate(shapes):
        p = s.representative_point()
        candidates = tree.query(p)
        tests += len(candidates)
        if tests > MAX_NEST_TESTS:
            raise RingError(f"its {len(shapes)} rings overlap too much to be nested")
        depth.append(sum(1 for j in candidates if j != i and shapes[j].area > s.area and shapes[j].contains(p)))
    shape = None
    for d in sorted(set(depth)):
        add = [s for (role, s), k in zip(rings, depth) if k == d and role == "outer"]
        cut = [s for (role, s), k in zip(rings, depth) if k == d and role == "inner"]
        if add:
            shape = unary_union(add) if shape is None else shape.union(unary_union(add))
        if cut and shape is not None:
            shape = shape.difference(unary_union(cut))
    # An outer ring inside an odd number of rings, or an inner ring inside
    # an even number, is mapped unusually (or wrongly): count it.
    odd = sum(1 for (role, _), k in zip(rings, depth) if (role == "outer") != (k % 2 == 0))
    if odd:
        _bump(stats, "OSM water rings whose role does not match their nesting", odd)
    return None if shape is None else _area(shape)


def _reaches(el: dict, bbox: BBox) -> bool:
    """Whether the element's bounds reach the area (lon/lat, edges included).
    Without any bounds it counts as reaching: better to stop than to guess."""
    b = el.get("bounds")
    if b is None:
        pts = [g for m in el.get("members", []) for g in (m.get("geometry") or []) if g is not None]
        if not pts:
            return True
        lons = [g["lon"] for g in pts]
        lats = [g["lat"] for g in pts]
        b = {"minlon": min(lons), "minlat": min(lats), "maxlon": max(lons), "maxlat": max(lats)}
    return (
        b["minlon"] <= bbox.maxlon and b["maxlon"] >= bbox.minlon
        and b["minlat"] <= bbox.maxlat and b["maxlat"] >= bbox.minlat
    )


def parse(doc: dict, bbox: BBox, stats: dict[str, int]) -> list[Body]:
    """The water areas of an Overpass water response, sorted by type and id.

    Raises WaterError for a multipolygon relation that reaches `bbox` but
    gives no area (see `_relation_area`), and OsmError for an element
    without the structure Overpass gives.
    """
    raw = doc.get("elements") if isinstance(doc, dict) else None
    if not isinstance(raw, list):
        raise OsmError("the response has no elements")
    if not all(isinstance(e, dict) and isinstance(e.get("type"), str) and type(e.get("id")) is int for e in raw):
        raise OsmError("an element has no type or no integer id")
    bodies: list[Body] = []
    for el in sorted(raw, key=lambda e: (e["type"], e["id"])):
        if el["type"] not in ("way", "relation"):
            continue
        tags = _tags(el)
        reason = exclusion(tags)
        if reason is not None:
            _bump(stats, f"OSM water areas left out: {reason}")
            continue
        if el["type"] == "way":
            try:
                shape = _way_area(el)
            except (GEOSException, RingError):  # a ring GEOS cannot repair, or too broken to try
                _bump(stats, "OSM water ways unusable")
                continue
            if shape is None:
                _bump(stats, "OSM water ways not closed or empty")
                continue
        elif tags.get("type") != "multipolygon":
            _bump(stats, f"OSM water relations left out: type={tags.get('type')}")
            continue
        else:
            _check_relation(el)
            try:
                shape = _relation_area(el, stats)
            except RingError as e:
                if not _reaches(el, bbox):
                    _bump(stats, "OSM water relations broken outside the area")
                    continue
                name = f" ({tags['name']})" if "name" in tags else ""
                raise WaterError(
                    f"OpenStreetMap water relation {el['id']}{name} reaches the area, but {e}. Water is "
                    f"never dropped silently: fix the relation (https://www.openstreetmap.org/relation/"
                    f"{el['id']}) in OpenStreetMap, then query again with --refresh."
                ) from e
        bodies.append(Body(el["type"], el["id"], tags, shape))
    _bump(stats, "OSM water areas", len(bodies))
    return bodies


def terrain_square(t: tuple[int, int], margin: float = 0.0) -> Polygon:
    e0, n0 = t[0] * TERRAIN_TILE_M, t[1] * TERRAIN_TILE_M
    return box(e0 - margin, n0 - margin, e0 + TERRAIN_TILE_M + margin, n0 + TERRAIN_TILE_M + margin)


def grid(t: tuple[int, int], bounds: tuple[float, float, float, float]) -> tuple[np.ndarray, np.ndarray]:
    """LV95 points on multiples of GRID_M, within `bounds` and strictly
    inside terrain tile `t`. The terrain pixels around such a point all lie
    in that tile, so no sample reads a neighbouring tile, loaded or not."""
    e0, n0 = t[0] * TERRAIN_TILE_M, t[1] * TERRAIN_TILE_M
    last = TERRAIN_TILE_M // GRID_M - 1
    x0, y0, x1, y1 = bounds
    i = np.arange(max(1, math.ceil((x0 - e0) / GRID_M)), min(last, math.floor((x1 - e0) / GRID_M)) + 1)
    j = np.arange(max(1, math.ceil((y0 - n0) / GRID_M)), min(last, math.floor((y1 - n0) / GRID_M)) + 1)
    e, n = np.meshgrid(e0 + GRID_M * i.astype(np.float64), n0 + GRID_M * j.astype(np.float64))
    return e.ravel(), n.ravel()


def _heights(dem: Dem, region, e: np.ndarray, n: np.ndarray) -> np.ndarray:
    """Terrain heights at the points inside `region` (NaN dropped)."""
    if region.is_empty or len(e) == 0:
        return np.empty(0)
    shapely.prepare(region)
    pick = shapely.contains_xy(region, e, n)
    h = dem.height(e[pick], n[pick])
    return h[np.isfinite(h)]


def level(body: Body, near: Polygon | MultiPolygon, t: tuple[int, int], dem: Dem) -> Level | None:
    """The level of `body` in terrain tile `t` (ADR-0014): the median terrain
    height at the grid points at least INSIDE_M inside it, plus LIFT_M.
    `near` is the body cut to the tile plus MARGIN_M: the samples keep
    INSIDE_M from the real shore, never from the cut. None if no grid point
    qualifies, as in water narrower than about 6 m."""
    e, n = grid(t, near.bounds)
    h = _heights(dem, near.buffer(-INSIDE_M), e, n)
    if len(h) == 0:
        return None
    p10, p90 = np.percentile(h, [10, 90])
    return Level(body.ref, t, float(np.median(h)) + LIFT_M, len(h), float(p10), float(p90))


def pieces(dem: Dem, bodies: list[Body], area: Polygon, terrain_tiles: set[tuple[int, int]],
           stats: dict[str, int]) -> list[Piece]:
    """Every body cut to every swissALTI3D tile where its surface reaches
    the area, with its level there. `terrain_tiles` are the tiles loaded
    in `dem`; elsewhere a piece has no level and is not drawn."""
    out: list[Piece] = []
    ax0, ay0, ax1, ay1 = area.bounds
    for body in bodies:
        bx0, by0, bx1, by1 = body.shape.bounds
        x0, y0 = max(ax0, bx0 - BUFFER_M), max(ay0, by0 - BUFFER_M)
        x1, y1 = min(ax1, bx1 + BUFFER_M), min(ay1, by1 + BUFFER_M)
        if x0 > x1 or y0 > y1:
            continue
        for te in range(math.floor(x0 / TERRAIN_TILE_M), math.floor(x1 / TERRAIN_TILE_M) + 1):
            for tn in range(math.floor(y0 / TERRAIN_TILE_M), math.floor(y1 / TERRAIN_TILE_M) + 1):
                t = (te, tn)
                near = _area(body.shape.intersection(terrain_square(t, MARGIN_M)))
                if near is None:
                    continue
                square = terrain_square(t)
                surface = _area(near.buffer(BUFFER_M).intersection(square))
                if surface is None or surface.intersection(area).area <= 0:
                    continue
                if t not in terrain_tiles:
                    lvl = None
                    _bump(stats, "water pieces in terrain tiles not loaded (not drawn)")
                else:
                    lvl = level(body, near, t, dem)
                    if lvl is None:
                        _bump(stats, f"water pieces with no grid point {INSIDE_M:g} m inside (not drawn)")
                out.append(Piece(body, t, lvl, _area(near.intersection(square)), surface))
    return out


def hidden_m2(dem: Dem, outline: Polygon | MultiPolygon, lvl: Level) -> float:
    """Area of `outline` where the terrain is more than HIDDEN_M above the
    water and so hides it: the share of the grid points inside the outline
    where it is, times the outline's area. A grid point stands for GRID_M
    squared, which would overstate slivers narrower than that."""
    e, n = grid(lvl.terrain_tile, outline.bounds)
    h = _heights(dem, outline, e, n)
    if len(h) == 0:
        return 0.0
    return float(np.count_nonzero(h > lvl.height_m + HIDDEN_M) / len(h) * outline.area)


def _add_polygon(mesh: MeshBuilder, frame: Frame, poly: Polygon, height_m: float) -> tuple[int, int]:
    """Adds a flat polygon with holes at `height_m`, triangulated in LV95.
    Returns the numbers of triangles kept and dropped."""
    rings = [np.asarray(r.coords)[:-1, :2] for r in (poly.exterior, *poly.interiors)]
    en = np.concatenate(rings)
    ends = np.cumsum([len(r) for r in rings]).astype(np.uint32)
    # Shifting by whole metres is exact and keeps earcut's numbers small.
    tris = np.asarray(mapbox_earcut.triangulate_float64(en - np.floor(en.min(axis=0)), ends), dtype=np.int64)
    tris = tris.reshape(-1, 3)
    a, b, c = en[tris[:, 0]], en[tris[:, 1]], en[tris[:, 2]]
    signed = (b[:, 0] - a[:, 0]) * (c[:, 1] - a[:, 1]) - (b[:, 1] - a[:, 1]) * (c[:, 0] - a[:, 0])
    # Counter-clockwise in LV95 (east, north) is counter-clockwise seen from
    # above in the region frame (X east, Z south): the triangles face up.
    tris = np.where((signed < 0)[:, None], tris[:, [0, 2, 1]], tris)
    points = frame.from_lv95(en[:, 0], en[:, 1], height_m)
    # Slivers along dense shorelines can collapse or turn over when snapped
    # to the millimetre; keep only triangles that still face up there.
    mm = to_mm(points) - mesh.origin
    d1, d2 = mm[tris[:, 1]] - mm[tris[:, 0]], mm[tris[:, 2]] - mm[tris[:, 0]]
    up = d1[:, 2] * d2[:, 0] - d1[:, 0] * d2[:, 2] > 0
    if up.any():
        mesh.add("water", points, np.tile(UP, (len(points), 1)), tris[up])
    return int(up.sum()), int((~up).sum())


def tile_water(frame: Frame, dem: Dem, cut: list[Piece], clip: Polygon, origin_mm: list[int],
               stats: dict[str, int]) -> TileWater:
    """The water of one tile: every piece within `clip`, the tile cut to
    the area."""
    mesh = MeshBuilder(origin_mm)
    refs: set[str] = set()
    used: set[tuple[int, int]] = set()
    area = drawn = hidden = 0.0
    dropped = 0
    cx0, cy0, cx1, cy1 = clip.bounds
    for p in cut:
        x0, y0, x1, y1 = p.surface.bounds
        if x1 < cx0 or x0 > cx1 or y1 < cy0 or y0 > cy1:
            continue
        outline = _area(p.outline.intersection(clip)) if p.outline is not None else None
        if outline is not None:
            area += outline.area
        if p.level is None:
            continue
        if outline is not None:
            hidden += hidden_m2(dem, outline, p.level)
        kept = 0
        for poly in _polygons(p.surface.intersection(clip)):
            drawn += poly.area
            k, d = _add_polygon(mesh, frame, poly, p.level.height_m)
            kept += k
            dropped += d
        if kept:
            refs.add(p.body.ref)
            used.add(p.terrain_tile)
    _bump(stats, "water triangles dropped (mm snap)", dropped)
    return TileWater(mesh, sorted(refs), sorted(used), area, drawn, hidden)
