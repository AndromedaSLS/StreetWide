"""swisstopo STAC API: find the source files covering an area.

Items are tiled (`<product>_<year>_<tile>`, e.g. `swissalti3d_2026_2683-1247`
or `swissbuildings3d_3_0_2025_1091-41`). For every tile whose footprint
(the item geometry, not its lon/lat envelope) overlaps the area, the latest
year that has the wanted asset is used. Search responses are cached so
that re-running from cache selects exactly the same files.

Answers are untrusted (downloads, or files in the cache): an item that is
not what the API documents raises StacError, and nothing an answer says can
place a download outside the cache.
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
from tools.worldfmt.store import read_file

STAC_ROOT = "https://data.geo.admin.ch/api/stac/v1"
_ITEM_ID = re.compile(r"(?P<product>[a-z0-9_]+?)_(?P<year>\d{4})_(?P<tile>\d{4}-\d{2,4})", re.ASCII)
_MAX_SEARCH_BYTES = 16 << 20
_MAX_CACHED_SEARCH_BYTES = 64 << 20
MAX_PAGES = 100
"""Most result pages one search may have (100 items each)."""
MAX_FOOTPRINT_POINTS = 1_000
"""Most points in an item's footprint (swisstopo's tiles have 5)."""
# An asset name becomes a file name in the cache: one path component.
_ASSET_NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]*")
_URL = re.compile(r"https://[!-~]+")
_SHA256_MULTIHASH = re.compile(r"1220[0-9a-fA-F]{64}")


class StacError(RuntimeError):
    """A STAC answer that does not have the documented form."""


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
        try:
            features = _json(read_file(path, _MAX_CACHED_SEARCH_BYTES), list)
        except (StacError, ValueError) as e:
            raise StacError(f"cached {path}: {e}; delete it and retry") from e
        return features
    if offline:
        raise RuntimeError(f"STAC search for {product.collection} is not cached and --offline is set")
    features: list[dict] = []
    pages = 0
    while url:
        pages += 1
        if pages > MAX_PAGES:
            raise StacError(f"STAC search for {product.collection} has more than {MAX_PAGES} pages")
        page = _json(fetch_bytes(url, max_bytes=_MAX_SEARCH_BYTES), dict)
        page_features, links = page.get("features", []), page.get("links", [])
        if not isinstance(page_features, list) or not isinstance(links, list):
            raise StacError("a STAC page has no list of features or links")
        features.extend(page_features)
        nxt = next((link for link in links if isinstance(link, dict) and link.get("rel") == "next"), None)
        url = None if nxt is None else nxt.get("href")
        if url and not (isinstance(url, str) and url.startswith(STAC_ROOT + "/") and _URL.fullmatch(url)):
            raise StacError(f"unexpected STAC next link {_text(url)}")
    if not all(isinstance(f, dict) and isinstance(f.get("id"), str) for f in features):
        raise StacError("a STAC item has no id")
    features.sort(key=lambda f: f["id"])
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(features, sort_keys=True, indent=1), encoding="utf-8")
    return features


def _text(value: object) -> str:
    """An untrusted value, shortened, for an error message."""
    return repr(value[:200]) if isinstance(value, str) else f"(a JSON {type(value).__name__})"


def _json(data: bytes, kind: type):
    try:
        value = json.loads(data)
    except (ValueError, RecursionError) as e:  # not JSON, or nested far too deeply
        raise StacError(f"not valid JSON: {e}") from e
    if not isinstance(value, kind):
        raise StacError(f"not a JSON {'array' if kind is list else 'object'}")
    return value


def _positions(geometry: object, limit: int) -> int:
    """How many positions (arrays of numbers) a GeoJSON geometry holds,
    counted up to just past `limit`."""
    count, stack = 0, [geometry]
    while stack and count <= limit:
        v = stack.pop()
        if isinstance(v, dict):
            stack.extend(v.values())
        elif isinstance(v, list):
            if v and all(type(x) in (int, float) for x in v):
                count += 1
            else:
                stack.extend(v)
    return count


def _overlaps(f: dict, area: Polygon) -> bool:
    geometry = f.get("geometry")
    # GEOS's overlay slows down with the square of overlapping segments: a
    # footprint of 3,000 identical rings took gigabytes.
    if _positions(geometry, MAX_FOOTPRINT_POINTS) > MAX_FOOTPRINT_POINTS:
        raise StacError(f"STAC item {f['id']}: a footprint of more than {MAX_FOOTPRINT_POINTS} points")
    try:
        return shape(geometry).intersection(area).area > 0
    except Exception as e:  # shapely raises many kinds of error for malformed GeoJSON
        raise StacError(f"STAC item {f['id']}: unreadable footprint ({type(e).__name__})") from e


def select(features: list[dict], product: Product, bbox: BBox, cache: Path) -> list[SourceFile]:
    """Latest file per tile whose item footprint overlaps `bbox`."""
    area = Polygon(bbox.ring())
    best: dict[str, SourceFile] = {}
    for f in features:
        if not isinstance(f, dict) or not isinstance(f.get("id", ""), str):
            raise StacError("a STAC item is not an object with a text id")
        m = _ITEM_ID.fullmatch(f.get("id", ""))
        if not m or not _overlaps(f, area):
            continue
        assets = f.get("assets", {})
        if not isinstance(assets, dict) or not all(isinstance(a, dict) for a in assets.values()):
            raise StacError(f"STAC item {f['id']}: assets are not objects")
        for name, asset in sorted(assets.items()):
            checksum = asset.get("file:checksum", "")
            if not isinstance(checksum, str):
                raise StacError(f"STAC asset {_text(name)}: checksum is not text")
            if not name.endswith(product.asset_suffix) or not checksum.startswith("1220"):
                continue
            if not _ASSET_NAME.fullmatch(name):
                raise StacError(f"unexpected STAC asset name {_text(name)}")
            if not _SHA256_MULTIHASH.fullmatch(checksum):
                raise StacError(f"STAC asset {name}: malformed SHA-256 checksum")
            href = asset.get("href")
            if not (isinstance(href, str) and href.startswith("https://data.geo.admin.ch/") and _URL.fullmatch(href)):
                raise StacError(f"unexpected asset URL {_text(href)}")
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
