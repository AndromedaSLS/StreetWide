"""swisstopo STAC API: find the source files covering an area.

Items are tiled (`<product>_<year>_<tile>`, e.g. `swissalti3d_2026_2683-1247`
or `swissbuildings3d_3_0_2025_1091-41`). For every tile whose footprint
(the item geometry, not its lon/lat envelope) overlaps the area, the latest
year that has the wanted asset is used. Search responses are cached so
that re-running from cache selects exactly the same files.
"""

from __future__ import annotations

import hashlib
import json
import re
import urllib.parse
from dataclasses import dataclass
from pathlib import Path

from shapely.geometry import Polygon, shape

from tools.ingest.geo import BBox
from tools.ingest.http import fetch_bytes, fetch_file

STAC_ROOT = "https://data.geo.admin.ch/api/stac/v1"
_ITEM_ID = re.compile(r"(?P<product>[a-z0-9_]+?)_(?P<year>\d{4})_(?P<tile>\d{4}-\d{2,4})")
_MAX_SEARCH_BYTES = 16 << 20


@dataclass(frozen=True)
class Product:
    collection: str
    asset_suffix: str
    max_bytes: int


SWISSBUILDINGS3D = Product(
    collection="ch.swisstopo.swissbuildings3d_3_0",
    asset_suffix="_2056_5728.citygml.zip",
    max_bytes=2 << 30,
)
SWISSALTI3D = Product(
    collection="ch.swisstopo.swissalti3d",
    asset_suffix="_0.5_2056_5728.tif",
    max_bytes=256 << 20,
)


@dataclass(frozen=True)
class SourceFile:
    item_id: str
    tile: str
    year: str
    url: str
    item_url: str
    sha256: str
    path: Path


def _search(product: Product, bbox: BBox, cache: Path, offline: bool, refresh: bool) -> list[dict]:
    query = urllib.parse.urlencode({"bbox": str(bbox), "limit": 100})
    url = f"{STAC_ROOT}/collections/{product.collection}/items?{query}"
    key = hashlib.sha256(url.encode()).hexdigest()[:16]
    path = cache / "stac" / product.collection / f"search-{key}.json"
    if path.exists() and not refresh:
        return json.loads(path.read_text(encoding="utf-8"))
    if offline:
        raise RuntimeError(f"STAC search for {product.collection} is not cached and --offline is set")
    features: list[dict] = []
    while url:
        page = json.loads(fetch_bytes(url, max_bytes=_MAX_SEARCH_BYTES))
        features.extend(page.get("features", []))
        url = next((link["href"] for link in page.get("links", []) if link.get("rel") == "next"), None)
        if url and not url.startswith(STAC_ROOT + "/"):
            raise RuntimeError(f"unexpected STAC next link {url}")
    features.sort(key=lambda f: f["id"])
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(features, sort_keys=True, indent=1), encoding="utf-8")
    return features


def select(features: list[dict], product: Product, bbox: BBox, cache: Path) -> list[SourceFile]:
    """Latest file per tile whose item footprint overlaps `bbox`."""
    area = Polygon(bbox.ring())
    best: dict[str, SourceFile] = {}
    for f in features:
        m = _ITEM_ID.fullmatch(f.get("id", ""))
        if not m or not shape(f["geometry"]).intersection(area).area > 0:
            continue
        for name, asset in sorted(f.get("assets", {}).items()):
            checksum = asset.get("file:checksum", "")
            if not name.endswith(product.asset_suffix) or not checksum.startswith("1220"):
                continue
            href = asset["href"]
            if not href.startswith("https://data.geo.admin.ch/"):
                raise RuntimeError(f"unexpected asset URL {href}")
            item_id = f["id"]
            candidate = SourceFile(
                item_id=item_id,
                tile=m["tile"],
                year=m["year"],
                url=href,
                item_url=f"{STAC_ROOT}/collections/{product.collection}/items/{item_id}",
                sha256=checksum[4:].lower(),
                path=cache / "swisstopo" / item_id / name,
            )
            current = best.get(candidate.tile)
            if current is None or candidate.year > current.year:
                best[candidate.tile] = candidate
    return [best[t] for t in sorted(best)]


def find_and_fetch(
    product: Product, bbox: BBox, cache: Path, *, offline: bool, refresh: bool
) -> list[SourceFile]:
    files = select(_search(product, bbox, cache, offline, refresh), product, bbox, cache)
    if not files:
        raise RuntimeError(f"no {product.collection} data covers {bbox}")
    for sf in files:
        fetch_file(sf.url, sf.path, sha256=sf.sha256, max_bytes=product.max_bytes, offline=offline)
    return files
