"""OpenStreetMap data from the Overpass API.

Two queries per area, each with its own cache file:
- highways (roads, paths, steps, bridges) and building footprints (for the
  fallback where swissBUILDINGS3D has no building);
- water areas (ADR-0014).

The raw responses are cached; their `timestamp_osm_base` is the OSM data
version recorded in every OSM-derived manifest.
"""

from __future__ import annotations

import hashlib
import json
import urllib.parse
from pathlib import Path

from tools.ingest.geo import BBox
from tools.ingest.http import fetch_bytes

ENDPOINT = "https://overpass-api.de/api/interpreter"
_MAX_BYTES = 256 << 20
# Tags of the water areas asked for (ADR-0014); the ingest selects among them.
WATER_TAGS = (("natural", "water"), ("waterway", "riverbank"), ("landuse", "reservoir"))


def _swne(bbox: BBox) -> str:
    # Overpass bboxes are south,west,north,east.
    return f"{bbox.minlat!r},{bbox.minlon!r},{bbox.maxlat!r},{bbox.maxlon!r}"


def query(bbox: BBox) -> str:
    return (
        f"[out:json][timeout:180][bbox:{_swne(bbox)}];"
        '(way["highway"];way["building"];relation["building"]["type"="multipolygon"];);'
        "out body geom;"
    )


def water_query(bbox: BBox) -> str:
    """Water areas. Each statement has its own bounding box and the query has
    no global one, so relations come back with every member: a river's level
    must not depend on how much of it the area covers."""
    b = _swne(bbox)
    statements = "".join(
        f'{kind}["{key}"="{value}"]({b});' for key, value in WATER_TAGS for kind in ("way", "relation")
    )
    return f"[out:json][timeout:180];({statements});out body geom;"


def fetch(bbox: BBox, cache: Path, *, offline: bool, refresh: bool) -> dict:
    q = query(bbox)
    return _fetch(q, cache / "osm" / f"overpass-{_key(q)}.json", "the Overpass response", offline, refresh)


def fetch_water(bbox: BBox, cache: Path, *, offline: bool, refresh: bool) -> dict:
    q = water_query(bbox)
    return _fetch(q, cache / "osm" / f"overpass-water-{_key(q)}.json", "the Overpass water response", offline, refresh)


def _key(q: str) -> str:
    return hashlib.sha256(q.encode()).hexdigest()[:16]


def _fetch(q: str, path: Path, what: str, offline: bool, refresh: bool) -> dict:
    if path.exists() and not refresh:
        return _check(json.loads(path.read_text(encoding="utf-8")))
    if offline:
        raise RuntimeError(f"{what} is not cached and --offline is set")
    print(f"  querying {ENDPOINT}", flush=True)
    body = fetch_bytes(ENDPOINT, data=urllib.parse.urlencode({"data": q}).encode(), max_bytes=_MAX_BYTES)
    doc = _check(json.loads(body))
    path.parent.mkdir(parents=True, exist_ok=True)
    part = path.with_name(path.name + ".part")
    part.write_bytes(body)
    part.replace(path)
    print(f"  saved {path} ({len(body) / 1e6:.1f} MB)", flush=True)
    return doc


def _check(doc: dict) -> dict:
    if not isinstance(doc, dict) or not isinstance(doc.get("elements"), list):
        raise RuntimeError("unexpected Overpass response")
    if "remark" in doc:
        # Overpass reports timeouts and errors as a remark next to partial data.
        raise RuntimeError(f"Overpass error: {doc['remark']}")
    if not doc.get("osm3s", {}).get("timestamp_osm_base"):
        raise RuntimeError("Overpass response has no timestamp_osm_base")
    return doc
