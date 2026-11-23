"""OpenStreetMap elements from an Overpass response, in LV95.

All 2D work on OSM geometry (clipping, offsets, overlaps) happens in LV95,
the metric conformal projection the swisstopo data and the terrain use.
Elements are sorted by type and id so the output never depends on the
order of the server's response.
"""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np
from shapely import make_valid
from shapely.geometry import LineString, MultiPolygon, Polygon
from shapely.ops import polygonize, unary_union

from tools.ingest.geo import lonlat_to_lv95


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


def _en(geometry: list[dict | None]) -> np.ndarray | None:
    if not geometry or any(g is None for g in geometry):
        return None
    lon = np.array([g["lon"] for g in geometry], dtype=np.float64)
    lat = np.array([g["lat"] for g in geometry], dtype=np.float64)
    e, n = lonlat_to_lv95(lon, lat)
    return np.stack([np.asarray(e), np.asarray(n)], axis=1)


def _polygonal(geom) -> Polygon | MultiPolygon | None:
    if geom.is_empty:
        return None
    if not geom.is_valid:
        geom = make_valid(geom)
    if isinstance(geom, (Polygon, MultiPolygon)):
        return geom if geom.area > 0 else None
    parts = [g for g in getattr(geom, "geoms", []) if isinstance(g, Polygon) and g.area > 0]
    return MultiPolygon(parts) if parts else None


def parse(doc: dict, stats: dict[str, int]) -> tuple[list[Way], list[Footprint]]:
    """Highway ways and building footprints."""
    elements = sorted(doc["elements"], key=lambda e: (e["type"], e["id"]))
    ways: list[Way] = []
    footprints: list[Footprint] = []
    for el in elements:
        tags = el.get("tags", {})
        if el["type"] == "way":
            en = _en(el.get("geometry"))
            if en is None or len(en) < 2:
                stats["osm ways without geometry"] = stats.get("osm ways without geometry", 0) + 1
                continue
            if "highway" in tags:
                ways.append(Way(el["id"], tags, list(el.get("nodes", [])), en))
            if "building" in tags and len(en) >= 4 and el["nodes"][0] == el["nodes"][-1]:
                shape = _polygonal(Polygon(en))
                if shape is not None:
                    footprints.append(Footprint("way", el["id"], tags, shape))
        elif el["type"] == "relation" and "building" in tags:
            lines = {"outer": [], "inner": []}
            for m in el.get("members", []):
                en = _en(m.get("geometry")) if m.get("type") == "way" else None
                if en is not None and len(en) >= 2 and m.get("role") in lines:
                    lines[m["role"]].append(LineString(en))
            outer = unary_union(list(polygonize(lines["outer"])))
            inner = unary_union(list(polygonize(lines["inner"])))
            shape = _polygonal(outer.difference(inner) if not inner.is_empty else outer)
            if shape is None:
                stats["osm building relations unusable"] = stats.get("osm building relations unusable", 0) + 1
            else:
                footprints.append(Footprint("relation", el["id"], tags, shape))
    return ways, footprints


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
