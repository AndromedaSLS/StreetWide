"""OpenStreetMap data from the Overpass API.

One query per area: highways (roads, paths, steps, bridges) and building
footprints (for the fallback where swissBUILDINGS3D has no building). The
raw response is cached; its `timestamp_osm_base` is the OSM data version
recorded in every OSM-derived manifest.
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


def query(bbox: BBox) -> str:
    # Overpass bboxes are south,west,north,east.
    b = f"{bbox.minlat!r},{bbox.minlon!r},{bbox.maxlat!r},{bbox.maxlon!r}"
    return (
        f"[out:json][timeout:180][bbox:{b}];"
        '(way["highway"];way["building"];relation["building"]["type"="multipolygon"];);'
        "out body geom;"
    )


def fetch(bbox: BBox, cache: Path, *, offline: bool, refresh: bool) -> dict:
    q = query(bbox)
    key = hashlib.sha256(q.encode()).hexdigest()[:16]
    path = cache / "osm" / f"overpass-{key}.json"
    if path.exists() and not refresh:
        return _check(json.loads(path.read_text(encoding="utf-8")))
    if offline:
        raise RuntimeError("the Overpass response is not cached and --offline is set")
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
