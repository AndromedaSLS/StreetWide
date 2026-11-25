"""Trees and light points of the City of Zurich (ADR-0015): two CC0
datasets of the city's open-data portal, fetched from its WFS one LV95
kilometre square at a time, cached, checked and read into records.

- **Datasets:** the tree register (Baumkataster, layer
  `baumkataster_baumstandorte`) and ewz's public lighting (layer
  `ewz_brennstelle_p`). Point ids are `stadt-zuerich:baumkataster:<baumnummer>`
  and `stadt-zuerich:beleuchtung:<nisnr>`, the city's local ids byte for
  byte.
- **Fetching:** per dataset and square, the portal's metadata
  (`dateLastUpdated` and the licence it states, which must be `cc-zero`),
  a `RESULTTYPE=hits` request and one GetFeature in LV95
  (`SRSNAME=EPSG:2056`, a plain `BBOX`), then the metadata again: a date
  that changed in between stops the run. HTTPS to the city's hosts only,
  at most 32 MB per answer, JSON only.
- **Cache:** `data/cache/stadt-zuerich/<layer>/<E>-<N>.json`, the raw answer,
  and `<E>-<N>.sidecar.json` with the URLs, fetch time, publisher's date,
  licence statement, SHA-256, size, feature count and hit count. Both are
  written to `.part` files and renamed. Every run checks the answer's hash
  against its sidecar again.
- **Version** of an answer: the publisher's date and its SHA-256, as
  `2026-11-22 sha256:<hex>` (ADR-0015 section 3).
- **Validation:** the run stops (StadtZuerichError) on a coordinate system
  other than `urn:ogc:def:crs:EPSG::2056`, a feature count that differs
  from the hits, a record that differs between two squares (or appears
  twice in one with different values), a tree status outside the six of
  ADR-0015 section 5, or a licence statement other than `cc-zero`. A record
  that is not a point inside its square (plus 1 m), or whose id or
  attributes have the wrong form, is left out and counted. The GeoJSON
  `bbox` member (WGS84) is ignored.
- **Kept** (ADR-0015 section 7): for trees, genus, species, taxon,
  planting year and crown diameter (a crown of exactly 8 m, the register's
  default, and any missing value are unknown); for lights, the
  orientation, which `point_features` turns into an estimated yaw.
  Never kept: `baumtyp`, `strasse`, `quartier`, `status`, `objectid`,
  `objid`, the German name. `genauigkeit` (survey method) is counted in the
  report only.

Answers are untrusted (downloads, or files in the cache).
"""

from __future__ import annotations

import hashlib
import json
import math
import os
import re
import urllib.parse
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path

from shapely.geometry import Polygon, box

from tools.ingest.http import fetch_bytes
from tools.worldfmt.points import MAX_POINT_ID_CHARS, MAX_TAXON_CHARS, _is_genus, _is_species, _is_text
from tools.worldfmt.store import read_file

WFS_ROOT = "https://www.ogd.stadt-zuerich.ch/wfs/geoportal/"
METADATA = "https://data.stadt-zuerich.ch/api/3/action/package_show?id="
DATASET_PAGE = "https://data.stadt-zuerich.ch/dataset/"
CITY_HOSTS = ("www.ogd.stadt-zuerich.ch", "data.stadt-zuerich.ch")
CRS = "urn:ogc:def:crs:EPSG::2056"
LICENSE_STATEMENT = "cc-zero"
"""The licence the portal's metadata must state (`license_id`; ADR-0017)."""
LICENSE = "CC0-1.0"
CREDIT = "Quelle: Stadt Zürich"
NAMESPACE = "stadt-zuerich"
MAX_ANSWER_BYTES = 32 << 20
MAX_HITS_BYTES = 64 << 10
MAX_METADATA_BYTES = 4 << 20
MAX_SIDECAR_BYTES = 64 << 10
SQUARE_MARGIN_M = 1.0
"""How far outside its square a record may lie (ADR-0015, Validation)."""
CITY_SQUARES = (2676, 1241, 2690, 1254)
"""LV95 kilometre squares (east, north; the last exclusive) that hold the
city: squares outside are never asked for."""
STATUSES = ("Strassenbaum", "Strassenbaum (A)", "Grünanlage", "Obst", "Kanton", "Schulen")
"""The tree statuses ADR-0015 section 5 accepts: trees of public bodies.
Anything else (Privat, Bund, empty or new) stops the run."""
CROWN_DEFAULT_M = 8
"""The register's crown diameter where none is known (ADR-0015 section 7)."""
SPECIES_PLACEHOLDERS = ("spec.",)
"""Epithets that say the species is not known."""
_LOCAL_ID = re.compile(r"[!-9;-~]+")
_DATE = re.compile(r"(\d{2})\.(\d{2})\.(\d{4})")
_ISO_DATE = re.compile(r"\d{4}-\d{2}-\d{2}")
_HITS = re.compile(rb'\bnumberOfFeatures="(\d{1,9})"')
_SHA256 = re.compile(r"[0-9a-f]{64}")


class StadtZuerichError(RuntimeError):
    """A rule of ADR-0015 that stops the run."""


@dataclass(frozen=True)
class Dataset:
    key: str
    """The dataset part of point ids: `stadt-zuerich:<key>:<local id>`."""
    name: str
    service: str
    typename: str
    package: str
    id_field: str
    kind: str

    @property
    def source(self) -> str:
        """The point ids' source, which names the asset (`tile:17/x/y/<source>`)."""
        return f"{NAMESPACE}:{self.key}"

    @property
    def url(self) -> str:
        """The dataset page, the source entries' URL."""
        return DATASET_PAGE + self.package


TREES = Dataset(
    key="baumkataster",
    name="Baumkataster Stadt Zürich",
    service="Baumkataster",
    typename="baumkataster_baumstandorte",
    package="geo_baumkataster",
    id_field="baumnummer",
    kind="tree",
)
LIGHTS = Dataset(
    key="beleuchtung",
    name="Öffentliche Beleuchtung der Stadt Zürich",
    service="Oeffentliche_Beleuchtung_der_Stadt_Zuerich",
    typename="ewz_brennstelle_p",
    package="geo_oeffentliche_beleuchtung_der_stadt_zuerich",
    id_field="nisnr",
    kind="light",
)
SOURCES = {("CH", "tree"): (TREES,), ("CH", "light"): (LIGHTS,)}
"""Where each kind of point comes from, per country (ADR-0015 section 1,
(a), and "Where the policy lives"): in Switzerland only non-OSM sources,
here the city's data inside the city. No OSM trees or street lamps
anywhere in Switzerland, and no gap-fill, in the data or in the client."""


@dataclass
class Answer:
    """One cached GetFeature answer: a dataset in one square."""

    dataset: Dataset
    square: tuple[int, int]
    sha256: str
    date: str
    hits: int
    features: list
    sidecar: dict

    @property
    def version(self) -> str:
        """The publisher's date and the answer's SHA-256 (ADR-0015 section 3)."""
        return f"{self.date} sha256:{self.sha256}"

    @property
    def key(self) -> str:
        return "{}-{}".format(*self.square)


@dataclass
class Record:
    """One record kept: a tree or a light point, at its LV95 position."""

    point_id: str
    local_id: str
    e: float
    n: float
    answer: Answer
    attributes: dict = field(default_factory=dict)
    """Trees: the kept attributes, by RFC 0004 name. Lights:
    `orientation_deg`, None where unknown or exactly 0."""
    survey: str | None = None
    crown_default: bool = False
    orientation_zero: bool = False


# --------------------------------------------------------------- fetching --


def squares(area_lv95: Polygon) -> list[tuple[int, int]]:
    """The LV95 kilometre squares (east, north, in km) that the area covers
    with positive area and that hold the city, sorted."""
    e0, n0, e1, n1 = area_lv95.bounds
    out = []
    for e in range(max(math.floor(e0 / 1000), CITY_SQUARES[0]), min(math.ceil(e1 / 1000), CITY_SQUARES[2])):
        for n in range(max(math.floor(n0 / 1000), CITY_SQUARES[1]), min(math.ceil(n1 / 1000), CITY_SQUARES[3])):
            if area_lv95.intersection(box(e * 1000, n * 1000, (e + 1) * 1000, (n + 1) * 1000)).area > 0:
                out.append((e, n))
    return out


def _wfs_url(dataset: Dataset, square: tuple[int, int], hits: bool) -> str:
    e, n = square[0] * 1000, square[1] * 1000
    query = {
        "SERVICE": "WFS",
        "VERSION": "1.1.0",
        "REQUEST": "GetFeature",
        "TYPENAME": dataset.typename,
    }
    if hits:
        query["RESULTTYPE"] = "hits"
    else:
        query["OUTPUTFORMAT"] = "application/vnd.geo+json"
        query["SRSNAME"] = "EPSG:2056"
    query["BBOX"] = f"{e},{n},{e + 1000},{n + 1000},EPSG:2056"
    return WFS_ROOT + dataset.service + "?" + urllib.parse.urlencode(query, safe=",:")


def metadata_url(dataset: Dataset) -> str:
    return METADATA + dataset.package


def _fetch(url: str, max_bytes: int) -> bytes:
    host = urllib.parse.urlsplit(url).hostname
    if not url.startswith("https://") or host not in CITY_HOSTS:
        raise StadtZuerichError(f"refusing {url}: HTTPS to the city's hosts only")
    return fetch_bytes(url, max_bytes=max_bytes)


def parse_metadata(data: bytes, dataset: Dataset) -> tuple[str, str]:
    """The publisher's date (ISO) and the licence the portal states, from
    its CKAN `package_show` answer. Raises StadtZuerichError."""
    try:
        doc = json.loads(data)
    except (ValueError, RecursionError) as e:
        raise StadtZuerichError(f"{dataset.name}: the portal's metadata is not JSON") from e
    result = doc.get("result") if isinstance(doc, dict) else None
    if not isinstance(result, dict) or doc.get("success") is not True:
        raise StadtZuerichError(f"{dataset.name}: unexpected metadata answer")
    if result.get("name") != dataset.package:
        raise StadtZuerichError(f"{dataset.name}: the metadata describes another dataset")
    license_id = result.get("license_id")
    if license_id != LICENSE_STATEMENT:
        shown = repr(license_id[:100]) if isinstance(license_id, str) else "none"
        raise StadtZuerichError(
            f"{dataset.name}: the portal states the licence {shown}, not {LICENSE_STATEMENT!r} (CC0-1.0, "
            "as ADR-0015 and ADR-0017 record); review it before ingesting")
    date = result.get("dateLastUpdated")
    m = _DATE.fullmatch(date) if isinstance(date, str) else None
    if m is None:
        raise StadtZuerichError(f"{dataset.name}: no publisher's date (dateLastUpdated) in the metadata")
    day, month, year = m.groups()
    iso = f"{year}-{month}-{day}"
    try:
        datetime.strptime(iso, "%Y-%m-%d")
    except ValueError as e:
        raise StadtZuerichError(f"{dataset.name}: invalid dateLastUpdated {date!r}") from e
    return iso, license_id


def parse_hits(data: bytes, what: str) -> int:
    """The feature count of a WFS 1.1.0 `RESULTTYPE=hits` answer. The XML is
    only searched, never parsed."""
    found = _HITS.findall(data)
    if len(found) != 1:
        raise StadtZuerichError(f"{what}: no feature count in the hits answer")
    return int(found[0])


def parse_answer(data: bytes, what: str) -> list:
    """The features of a GetFeature answer, after the checks that stop the
    run: JSON, a FeatureCollection in LV95 (EPSG:2056) with a list of
    features. Each feature is checked later, as a record."""
    try:
        doc = json.loads(data)
    except (ValueError, RecursionError) as e:
        raise StadtZuerichError(f"{what}: the answer is not JSON") from e
    if not isinstance(doc, dict) or doc.get("type") != "FeatureCollection":
        raise StadtZuerichError(f"{what}: the answer is not a GeoJSON FeatureCollection")
    crs = doc.get("crs")
    name = crs.get("properties", {}).get("name") if isinstance(crs, dict) and isinstance(
        crs.get("properties"), dict) else None
    if not (isinstance(crs, dict) and crs.get("type") == "name" and name == CRS):
        raise StadtZuerichError(f"{what}: coordinates are not in {CRS}")
    features = doc.get("features")
    if not isinstance(features, list):
        raise StadtZuerichError(f"{what}: the answer has no list of features")
    return features


def _write(path: Path, data: bytes) -> None:
    part = path.with_name(path.name + ".part")
    part.write_bytes(data)
    os.replace(part, path)


def _read_sidecar(path: Path, what: str) -> dict:
    try:
        side = json.loads(read_file(path, MAX_SIDECAR_BYTES, follow_links=True))
    except (ValueError, RecursionError) as e:
        raise StadtZuerichError(f"cached {path}: unreadable ({e}); delete it and retry") from e
    ok = (
        isinstance(side, dict)
        and isinstance(side.get("sha256"), str) and _SHA256.fullmatch(side["sha256"])
        and isinstance(side.get("publisher_date"), str) and _ISO_DATE.fullmatch(side["publisher_date"])
        and side.get("license") == LICENSE_STATEMENT
        and all(type(side.get(k)) is int and side[k] >= 0 for k in ("size", "features", "hits"))
    )
    if not ok:
        raise StadtZuerichError(f"cached {path}: not a sidecar of {what}; delete it and retry")
    return side


def fetch_answer(dataset: Dataset, square: tuple[int, int], cache: Path, *, offline: bool,
                 refresh: bool) -> Answer:
    """The answer for `dataset` in `square`: from the cache, checked against
    its sidecar, or fetched (unless `offline`) and cached."""
    key = "{}-{}".format(*square)
    what = f"{dataset.name}, square {key}"
    folder = cache / "stadt-zuerich" / dataset.typename
    path, side_path = folder / f"{key}.json", folder / f"{key}.sidecar.json"
    if path.exists() and side_path.exists() and not refresh:
        side = _read_sidecar(side_path, what)
        try:
            data = read_file(path, MAX_ANSWER_BYTES, follow_links=True)
        except ValueError as e:
            raise StadtZuerichError(f"cached {path}: {e}") from e
        digest = hashlib.sha256(data).hexdigest()
        if digest != side["sha256"] or len(data) != side["size"]:
            raise StadtZuerichError(f"cached {path} does not match its sidecar; delete both and retry")
        features = parse_answer(data, f"cached {what}")
        if not len(features) == side["features"] == side["hits"]:
            raise StadtZuerichError(f"cached {what}: {len(features)} features, {side['hits']} hits")
        return Answer(dataset, square, digest, side["publisher_date"], side["hits"], features, side)
    if offline:
        raise StadtZuerichError(f"{what} is not cached and --offline is set")
    date, license_id = parse_metadata(_fetch(metadata_url(dataset), MAX_METADATA_BYTES), dataset)
    hits_url, url = _wfs_url(dataset, square, True), _wfs_url(dataset, square, False)
    print(f"  querying {url}", flush=True)
    hits = parse_hits(_fetch(hits_url, MAX_HITS_BYTES), what)
    data = _fetch(url, MAX_ANSWER_BYTES)
    fetched = datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")
    again, _ = parse_metadata(_fetch(metadata_url(dataset), MAX_METADATA_BYTES), dataset)
    if again != date:
        raise StadtZuerichError(f"{what}: the city's data was updated while it was fetched ({date}, then {again}); "
                                "run again")
    features = parse_answer(data, what)
    if len(features) != hits:
        raise StadtZuerichError(f"{what}: {len(features)} features, but the service counts {hits}")
    digest = hashlib.sha256(data).hexdigest()
    side = {
        "url": url,
        "hits_url": hits_url,
        "metadata_url": metadata_url(dataset),
        "fetched": fetched,
        "publisher_date": date,
        "license": license_id,
        "sha256": digest,
        "size": len(data),
        "features": len(features),
        "hits": hits,
    }
    folder.mkdir(parents=True, exist_ok=True)
    _write(path, data)
    _write(side_path, (json.dumps(side, indent=1, sort_keys=True) + "\n").encode())
    print(f"  saved {path} ({len(data) / 1e6:.1f} MB, {len(features)} features)", flush=True)
    return Answer(dataset, square, digest, date, hits, features, side)


# ---------------------------------------------------------------- records --


def _bump(stats: dict[str, int], key: str) -> None:
    stats[key] = stats.get(key, 0) + 1


def _number(v: object) -> bool:
    return type(v) in (int, float) and math.isfinite(v)


def _text(p: dict, key: str) -> str | None:
    """A text attribute, None where missing; raises ValueError where it is
    not text."""
    v = p.get(key)
    if v is None or v == "":
        return None
    if not isinstance(v, str):
        raise ValueError(key)
    return v


def tree_attributes(p: dict) -> dict:
    """RFC 0004's tree attributes from a register record (ADR-0015
    section 7); `height_mm` stays None here (ADR-0016). Raises ValueError
    for an attribute of the wrong form."""
    genus = _text(p, "baumgattunglat")
    if genus is not None and not _is_genus(genus):
        raise ValueError("baumgattunglat")
    epithet = _text(p, "baumartlat")
    species = None
    if genus is not None and epithet is not None and epithet not in SPECIES_PLACEHOLDERS:
        species = f"{genus} {epithet}"
        if not _is_species(species):
            raise ValueError("baumartlat")
    taxon = _text(p, "baumnamelat")
    if taxon is not None and not (_is_text(taxon) and len(taxon) <= MAX_TAXON_CHARS):
        raise ValueError("baumnamelat")
    planted = p.get("pflanzjahr")
    if planted is not None and not (type(planted) is int and 1000 <= planted <= 2999):
        raise ValueError("pflanzjahr")
    crown = p.get("kronendurchmesser")
    if crown is not None and not (type(crown) is int and 1 <= crown <= 100):
        raise ValueError("kronendurchmesser")
    return {
        "crown_diameter_mm": None if crown in (None, CROWN_DEFAULT_M) else crown * 1000,
        "genus": genus,
        "height_mm": None,
        "leaf_cycle": None,
        "leaf_type": None,
        "planted_year": planted,
        "species": species,
        "taxon": taxon,
        "trunk_circumference_mm": None,
    }


def light_orientation(p: dict) -> float | None:
    """A light's `orientierung` in degrees, None where missing or exactly 0
    (unknown, ADR-0015's rule). Raises ValueError for the wrong form."""
    o = p.get("orientierung")
    if o is None:
        return None
    if not (_number(o) and 0 <= o < 360):
        raise ValueError("orientierung")
    return None if o == 0 else float(o)


def _position(feature: dict) -> tuple[float, float] | None:
    g = feature.get("geometry")
    if not isinstance(g, dict) or g.get("type") != "Point":
        return None
    c = g.get("coordinates")
    if not (isinstance(c, list) and len(c) == 2 and all(_number(v) for v in c)):
        return None
    return float(c[0]), float(c[1])


def _listed(ids: list[str]) -> str:
    shown = ", ".join(ids[:20])
    return shown + (f" and {len(ids) - 20} more" if len(ids) > 20 else "")


def records(dataset: Dataset, answers: list[Answer], stats: dict[str, int]) -> list[Record]:
    """The records of `answers` (one dataset), sorted by point id, after
    ADR-0015's validation. Records left out are counted in `stats` as
    "<dataset> left out: <reason>"."""
    left_out = f"{dataset.name} left out"
    kept: dict[str, Record] = {}
    first: dict[str, tuple[Answer, dict]] = {}
    statuses: list[str] = []
    for answer in answers:
        e0, n0 = answer.square[0] * 1000 - SQUARE_MARGIN_M, answer.square[1] * 1000 - SQUARE_MARGIN_M
        e1, n1 = e0 + 1000 + 2 * SQUARE_MARGIN_M, n0 + 1000 + 2 * SQUARE_MARGIN_M
        for feature in answer.features:
            p = feature.get("properties") if isinstance(feature, dict) else None
            if not isinstance(p, dict):
                _bump(stats, f"{left_out}: not a feature")
                continue
            local = p.get(dataset.id_field)
            if dataset.kind == "tree" and p.get("status") not in STATUSES:
                name = local if isinstance(local, str) else repr(feature.get("id"))[:80]
                status = p.get("status")
                shown = status[:40] if isinstance(status, str) else repr(status)[:40]
                statuses.append(f"{name} ({shown})")
                continue
            pos = _position(feature)
            if pos is None:
                _bump(stats, f"{left_out}: not a point")
                continue
            if not (e0 <= pos[0] <= e1 and n0 <= pos[1] <= n1):
                _bump(stats, f"{left_out}: outside its square")
                continue
            point_id = f"{dataset.source}:{local}" if isinstance(local, str) else ""
            if not (isinstance(local, str) and _LOCAL_ID.fullmatch(local) and len(point_id) <= MAX_POINT_ID_CHARS):
                _bump(stats, f"{left_out}: id of the wrong form")
                continue
            if point_id in first:
                other, before = first[point_id]
                if before != feature:
                    where = (f"in square {answer.key}" if other is answer
                             else f"between squares {other.key} and {answer.key}")
                    raise StadtZuerichError(f"{dataset.name}: record {local} differs {where}")
                _bump(stats, f"{dataset.name} records in two squares or listed twice")
                continue
            first[point_id] = (answer, feature)
            try:
                if dataset.kind == "tree":
                    attributes = tree_attributes(p)
                    record = Record(point_id, local, pos[0], pos[1], answer, attributes,
                                    survey=p.get("genauigkeit") if isinstance(p.get("genauigkeit"), str) else None,
                                    crown_default=p.get("kronendurchmesser") == CROWN_DEFAULT_M)
                else:
                    o = light_orientation(p)
                    record = Record(point_id, local, pos[0], pos[1], answer, {"orientation_deg": o},
                                    orientation_zero=p.get("orientierung") == 0)
            except ValueError as e:
                _bump(stats, f"{left_out}: {e.args[0]} of the wrong form")
                continue
            kept[point_id] = record
    if statuses:
        raise StadtZuerichError(
            f"{dataset.name}: {len(statuses)} trees with a status outside {', '.join(STATUSES)} "
            f"(ADR-0015 section 5, nothing on private ground enters unseen): {_listed(statuses)}")
    return [kept[k] for k in sorted(kept, key=lambda s: s.encode())]
