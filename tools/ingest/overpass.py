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
from tools.worldfmt.store import read_file

ENDPOINT = "https://overpass-api.de/api/interpreter"
_MAX_BYTES = 256 << 20
# Tags of the water areas asked for (ADR-0014); the ingest selects among them.
WATER_TAGS = (("natural", "water"), ("waterway", "riverbank"), ("landuse", "reservoir"))


class OverpassError(RuntimeError):
    """An Overpass response that is malformed or reports an error."""


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
        try:
            return parse_answer(read_file(path, _MAX_BYTES, follow_links=True))
        except ValueError as e:  # read_file refusing the file
            raise OverpassError(f"cached {path}: {e}") from e
        except OverpassError as e:
            raise OverpassError(f"cached {path}: {e}; delete it and retry") from e
    if offline:
        raise RuntimeError(f"{what} is not cached and --offline is set")
    print(f"  querying {ENDPOINT}", flush=True)
    body = fetch_bytes(ENDPOINT, data=urllib.parse.urlencode({"data": q}).encode(), max_bytes=_MAX_BYTES)
    doc = parse_answer(body)
    path.parent.mkdir(parents=True, exist_ok=True)
    part = path.with_name(path.name + ".part")
    part.write_bytes(body)
    part.replace(path)
    print(f"  saved {path} ({len(body) / 1e6:.1f} MB)", flush=True)
    return doc


def parse_answer(data: bytes) -> dict:
    """An Overpass answer from untrusted bytes (a download or the cache),
    checked as below. Raises OverpassError for anything else."""
    try:
        doc = json.loads(data)
    except (ValueError, RecursionError) as e:  # not JSON, or nested far too deeply
        raise OverpassError(f"the Overpass response is not valid JSON: {e}") from e
    return _check(doc)


def _check(doc: object) -> dict:
    if not isinstance(doc, dict) or not isinstance(doc.get("elements"), list):
        raise OverpassError("unexpected Overpass response")
    if "remark" in doc:
        # Overpass reports timeouts and errors as a remark next to partial data.
        remark = doc["remark"]
        raise OverpassError(f"Overpass error: {remark[:500] if isinstance(remark, str) else '(not text)'}")
    osm3s = doc.get("osm3s")
    base = osm3s.get("timestamp_osm_base") if isinstance(osm3s, dict) else None
    if not base:
        raise OverpassError("Overpass response has no timestamp_osm_base")
    # It becomes the OSM source version in every manifest (schema "text").
    if not isinstance(base, str) or len(base) > 64 or not base.isprintable():
        raise OverpassError("Overpass response has an invalid timestamp_osm_base")
    return doc
