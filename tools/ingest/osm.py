"""OpenStreetMap elements from an Overpass response, in LV95.

All 2D work on OSM geometry (clipping, offsets, overlaps) happens in LV95,
the metric conformal projection the swisstopo data and the terrain use.
Elements are sorted by type and id so the output never depends on the
order of the server's response.

The response is untrusted (a download, or a file in the cache): an element
without the structure Overpass gives (an integer id, text tags, numeric
coordinates on the globe) raises OsmError.
"""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np
from shapely import make_valid
from shapely.errors import GEOSException
from shapely.geometry import LineString, MultiPolygon, Polygon
from shapely.ops import polygonize, unary_union

from tools.ingest.geo import lonlat_to_lv95

MAX_WAY_NODES = 2_000
"""OpenStreetMap's own limit on the nodes of a way (API 0.6). It also bounds
GEOS's work on a way, which grows faster than linearly when a crafted way
retraces itself."""
MAX_RELATION_POINTS = 20_000
"""Most points in the member ways of one building relation (real ones have
hundreds); more is counted as unusable."""


class OsmError(ValueError):
    """An Overpass element that does not have the structure of OSM data."""


@dataclass
class Way:
    id: int
    tags: dict[str, str]
    nodes: list[int]
    en: np.ndarray  # (n, 2) LV95

    @property
    def ref(self) -> str:
        return f"way/{self.id}"


@dataclass
class Footprint:
    osm_type: str  # "way" or "relation"
    id: int
    tags: dict[str, str]
    shape: Polygon | MultiPolygon  # LV95

    @property
    def ref(self) -> str:
        return f"{self.osm_type}/{self.id}"

    @property
    def feature_id(self) -> str:
        return f"osm:{self.osm_type}:{self.id}"


def _coordinate(g: object, key: str, limit: float) -> float:
    v = g.get(key) if isinstance(g, dict) else None
    # Compared, not converted: a JSON integer can have hundreds of digits;
    # NaN fails the comparison.
    if type(v) not in (int, float) or not abs(v) <= limit:
        raise OsmError(f"a geometry point has no valid {key}")
    return v


def _en(geometry: object) -> np.ndarray | None:
    if not geometry:
        return None
    if not isinstance(geometry, list):
        raise OsmError("a geometry is not an array")
    if len(geometry) > MAX_WAY_NODES:
        raise OsmError(f"a way of {len(geometry)} points, more than OpenStreetMap allows ({MAX_WAY_NODES})")
    if any(g is None for g in geometry):
        return None
    lon = np.array([_coordinate(g, "lon", 180.0) for g in geometry], dtype=np.float64)
    lat = np.array([_coordinate(g, "lat", 90.0) for g in geometry], dtype=np.float64)
    e, n = lonlat_to_lv95(lon, lat)
    en = np.stack([np.asarray(e), np.asarray(n)], axis=1)
    if not np.isfinite(en).all():
        raise OsmError("a geometry point is outside LV95")
    return en


def _tags(el: dict) -> dict[str, str]:
    tags = el.get("tags", {})
    if not isinstance(tags, dict) or not all(isinstance(v, str) for v in tags.values()):
        raise OsmError(f"{el['type']}/{el['id']}: tags are not text")
    return tags


def _nodes(el: dict, count: int) -> list[int]:
    """The node ids of a way, one per point of its geometry (as Overpass
    gives them; bridges take their heights per node)."""
    nodes = el.get("nodes")
    if not isinstance(nodes, list) or not all(type(n) is int for n in nodes):
        raise OsmError(f"{el['type']}/{el['id']}: node ids are not integers")
    if len(nodes) != count:
        raise OsmError(f"{el['type']}/{el['id']}: {len(nodes)} nodes for {count} points")
    return list(nodes)


def _polygonal(geom) -> Polygon | MultiPolygon | None:
    if geom.is_empty:
        return None
    if not geom.is_valid:
        geom = make_valid(geom)
    if isinstance(geom, (Polygon, MultiPolygon)):
        return geom if geom.area > 0 else None
    parts = [g for g in getattr(geom, "geoms", []) if isinstance(g, Polygon) and g.area > 0]
    return MultiPolygon(parts) if parts else None


def _relation_shape(lines: dict[str, list[LineString]]) -> Polygon | MultiPolygon | None:
    outer = unary_union(list(polygonize(lines["outer"])))
    inner = unary_union(list(polygonize(lines["inner"])))
    return _polygonal(outer.difference(inner) if not inner.is_empty else outer)


def parse(doc: dict, stats: dict[str, int]) -> tuple[list[Way], list[Footprint]]:
    """Highway ways and building footprints."""
    raw = doc.get("elements") if isinstance(doc, dict) else None
    if not isinstance(raw, list):
        raise OsmError("the response has no elements")
    if not all(isinstance(e, dict) and isinstance(e.get("type"), str) and type(e.get("id")) is int for e in raw):
        raise OsmError("an element has no type or no integer id")
    elements = sorted(raw, key=lambda e: (e["type"], e["id"]))
    ways: list[Way] = []
    footprints: list[Footprint] = []
    for el in elements:
        tags = _tags(el)
        if el["type"] == "way":
            en = _en(el.get("geometry"))
            if en is None or len(en) < 2:
                stats["osm ways without geometry"] = stats.get("osm ways without geometry", 0) + 1
                continue
            nodes = _nodes(el, len(en))
            if "highway" in tags:
                ways.append(Way(el["id"], tags, nodes, en))
            if "building" in tags and len(en) >= 4 and nodes[0] == nodes[-1]:
                try:
                    shape = _polygonal(Polygon(en))
                except GEOSException:  # a ring GEOS cannot repair
                    shape = None
                if shape is not None:
                    footprints.append(Footprint("way", el["id"], tags, shape))
        elif el["type"] == "relation" and "building" in tags:
            lines = {"outer": [], "inner": []}
            members = el.get("members", [])
            if not isinstance(members, list) or not all(isinstance(m, dict) for m in members):
                raise OsmError(f"relation/{el['id']}: members are not objects")
            for m in members:
                en = _en(m.get("geometry")) if m.get("type") == "way" else None
                role = m.get("role")
                if en is not None and len(en) >= 2 and isinstance(role, str) and role in lines:
                    lines[role].append(LineString(en))
            points = sum(len(line.coords) for role in lines.values() for line in role)
            try:
                shape = _relation_shape(lines) if points <= MAX_RELATION_POINTS else None
            except GEOSException:  # rings GEOS cannot combine
                shape = None
            if shape is None:
                stats["osm building relations unusable"] = stats.get("osm building relations unusable", 0) + 1
            else:
                footprints.append(Footprint("relation", el["id"], tags, shape))
    return ways, footprints


class RingError(ValueError):
    """The member ways of a multipolygon do not join into closed rings."""


def join_rings(lines: list[list[tuple[float, float]]]) -> list[list[tuple[float, float]]]:
    """Joins ways that meet end to end into closed rings, as OSM multipolygon
    relations are built. Points are compared exactly, so pass them as the
    response gave them (lon, lat). Every ring comes back closed. Unlike the
    building footprints above, which `polygonize` and so drop open chains,
    this raises RingError if a chain of ways does not close."""
    rings = [line for line in lines if len(line) >= 2 and line[0] == line[-1]]
    chains = [line for line in lines if len(line) >= 2 and line[0] != line[-1]]
    ends: dict[tuple[float, float], list[int]] = {}
    for i, line in enumerate(chains):
        ends.setdefault(line[0], []).append(i)
        ends.setdefault(line[-1], []).append(i)
    used = [False] * len(chains)
    # Where the first unused chain of each end may be: every chain before it
    # is used, so finding the next one never looks at a chain twice (many
    # chains meeting at one point made this quadratic).
    first = dict.fromkeys(ends, 0)
    for start in range(len(chains)):
        if used[start]:
            continue
        used[start] = True
        ring = list(chains[start])
        while ring[0] != ring[-1]:
            at = ring[-1]
            candidates, k = ends[at], first[at]
            while k < len(candidates) and used[candidates[k]]:
                k += 1
            first[at] = k
            nxt = candidates[k] if k < len(candidates) else None
            if nxt is None:
                raise RingError(f"a chain of ways from {ring[0]} to {ring[-1]} (lon, lat) does not close")
            used[nxt] = True
            line = chains[nxt]
            ring.extend(line[1:] if line[0] == ring[-1] else line[-2::-1])
        rings.append(ring)
    return rings


def parse_length_m(value: str | None) -> float | None:
    """Parses OSM lengths such as "3", "3.5", "3 m", "3,5"; None if unclear."""
    if not value:
        return None
    v = value.strip().lower().replace(",", ".")
    for suffix in (" m", "m"):
        if v.endswith(suffix):
            v = v[: -len(suffix)].strip()
            break
    try:
        x = float(v)
    except ValueError:
        return None
    return x if np.isfinite(x) and x > 0 else None
