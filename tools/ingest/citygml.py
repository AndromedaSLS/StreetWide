"""Streaming reader for swissBUILDINGS3D 3.0 Beta CityGML (LOD2).

The sheet is one multi-gigabyte GML file inside a zip. It is parsed as a
stream (expat via ElementTree's pull parser, which resolves no external
entities) straight out of the zip, never extracted to disk.

Each `bldg:Building` carries its geometry twice: as typed boundary
surfaces (`bldg:boundedBy` Roof/Wall/Ground surfaces, including roof
overhangs) and again as untyped closed solids (`lod2Solid`, on the
building or its `BuildingPart`s). Only the typed surfaces are used.

The sheet is a download and the derived cache a local file: a damaged or
crafted one raises CityGmlError, and the parsed tree is pruned as it is
read, so no sheet can make the reader hold more than a few buildings. A
building of more than _MAX_BUILDING_BYTES of XML is dropped as it is read
and counted, so those few buildings take a few hundred megabytes at most.
"""

from __future__ import annotations

import json
import lzma
import os
import struct
import zipfile
import zlib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Iterator
from xml.etree.ElementTree import Element, ParseError, XMLPullParser

import numpy as np

from tools.worldfmt.store import read_file

BLDG = "{http://www.opengis.net/citygml/building/2.0}"
GML = "{http://www.opengis.net/gml}"
GEN = "{http://www.opengis.net/citygml/generics/2.0}"
APP = "{http://www.opengis.net/citygml/appearance/2.0}"

SURFACE_KIND = {
    "RoofSurface": "roof",
    "WallSurface": "wall",
    "GroundSurface": "ground",
    "ClosureSurface": "wall",
    "OuterCeilingSurface": "wall",
    "OuterFloorSurface": "ground",
}
# Kinds of building that are not walkable shells above ground.
SKIP_OBJEKTART = {"Unterirdisches Gebaeude"}
_MAX_MEMBER_BYTES = 16 << 30
# Bump when the cached extraction format or the parsing rules change.
# 2: unreadable coordinates and oversized rings or buildings are left out.
CACHE_VERSION = 2
# Bounds on the XML, far above real sheets (which nest about 15 levels deep,
# with buildings and texts of kilobytes).
_MAX_DEPTH = 256
_MAX_BUILDING_BYTES = 16 << 20
"""Most XML in one building (the largest of the 11,304 in the Zurich sheet
1091-41 has 4.5 MB). The reader keeps a building's elements until it ends,
and tiny elements take about 23 times their XML in memory, so this bounds
a building at about 400 MB. A larger building is skipped and counted."""
_MAX_RUN_BYTES = 128 << 20
"""Most bytes between two tags: one text, or one tag with its attributes."""
_CHUNK = 64 << 10
_MAX_CACHE_BYTES = 4 << 30
MAX_COORDINATE_M = 1e7
"""Largest coordinate accepted (LV95 eastings are below 3e6 m); larger or
non-finite numbers make a polygon unreadable."""
MAX_RING_POINTS = 100
"""Most points in one ring; a longer ring is unreadable. swissBUILDINGS3D
3.0 faces are triangles, and triangulating a crafted ring that retraces
itself takes time growing much faster than its length."""
MAX_BUILDING_POINTS = 1_000_000
"""Most points in one building's surfaces (the largest in Zurich's old town
has about 11,000); a larger building is skipped."""
# What zipfile, its decompressors and expat raise for a damaged or crafted
# file (LookupError: an XML declaration naming an unknown encoding).
_READ_ERRORS = (
    zipfile.BadZipFile, zipfile.LargeZipFile, ParseError, zlib.error, lzma.LZMAError, EOFError,
    NotImplementedError, RuntimeError, OSError, ValueError, LookupError, struct.error,
)


class CityGmlError(ValueError):
    """A sheet or a cached extraction that cannot be read."""


@dataclass
class Building:
    gml_id: str
    objektart: str | None
    egid: str | None
    # (kind, rings): rings[0] is the exterior, the rest are holes; each ring
    # is a list of [e, n, h] LV95 points without the closing repeat.
    surfaces: list[tuple[str, list[list[list[float]]]]] = field(default_factory=list)

    def footprint_points(self) -> np.ndarray:
        """LV95 (e, n) of the ground surfaces, or of everything if none."""
        pts = [p for kind, rings in self.surfaces if kind == "ground" for p in rings[0]]
        if not pts:
            pts = [p for _, rings in self.surfaces for p in rings[0]]
        return np.asarray(pts, dtype=np.float64).reshape(-1, 3)[:, :2]


def _local(tag: str) -> str:
    return tag.rsplit("}", 1)[-1]


def _numbers(text: str | None) -> list[float] | None:
    """The numbers of a gml:posList or gml:pos, or None unless all are
    plausible LV95 coordinates (finite, within MAX_COORDINATE_M)."""
    try:
        values = [float(v) for v in (text or "").split()]
    except ValueError:
        return None
    return values if all(abs(v) <= MAX_COORDINATE_M for v in values) else None


def _ring(el: Element | None) -> list[list[float]] | None:
    if el is None:
        return None
    pos_list = el.find(f"{GML}posList")
    if pos_list is not None:
        try:
            dim = int(pos_list.get("srsDimension", "3"))
        except ValueError:
            return None
        values = _numbers(pos_list.text)
    else:
        dim = 3
        values = []
        for p in el.findall(f"{GML}pos"):
            v = _numbers(p.text)
            if v is None:
                return None
            values.extend(v)
    if values is None or dim != 3 or len(values) % 3:
        return None
    pts = [values[i : i + 3] for i in range(0, len(values), 3)]
    if len(pts) > 1 and pts[0] == pts[-1]:
        pts.pop()
    return pts if 3 <= len(pts) <= MAX_RING_POINTS else None


def _polygons(surface: Element) -> Iterator[list[list[list[float]]] | None]:
    for poly in surface.iter(f"{GML}Polygon"):
        ext = _ring(poly.find(f"{GML}exterior/{GML}LinearRing"))
        if ext is None:
            yield None
            continue
        holes = [_ring(r) for r in poly.findall(f"{GML}interior/{GML}LinearRing")]
        yield [ext] + [h for h in holes if h is not None]


def _attribute(el: Element, name: str) -> str | None:
    for a in el:
        if a.tag.startswith(GEN) and a.get("name") == name:
            v = a.find(f"{GEN}value")
            return v.text if v is not None else None
    return None


def _building(el: Element, stats: dict[str, int]) -> Building | None:
    b = Building(
        gml_id=el.get(f"{GML}id", ""),
        objektart=_attribute(el, "OBJEKTART"),
        egid=_attribute(el, "EGID"),
    )
    owners = [el] + [p for cp in el.findall(f"{BLDG}consistsOfBuildingPart") for p in cp]
    for owner in owners:
        for bounded in owner.findall(f"{BLDG}boundedBy"):
            for surface in bounded:
                kind = SURFACE_KIND.get(_local(surface.tag))
                if kind is None:
                    stats["surfaces of unknown type"] = stats.get("surfaces of unknown type", 0) + 1
                    continue
                for rings in _polygons(surface):
                    if rings is None:
                        stats["polygons unreadable"] = stats.get("polygons unreadable", 0) + 1
                    else:
                        b.surfaces.append((kind, rings))
    if sum(len(r) for _, rings in b.surfaces for r in rings) > MAX_BUILDING_POINTS:
        stats["buildings too large"] = stats.get("buildings too large", 0) + 1
        return None
    return b


def _envelope_overlaps(el: Element, env: tuple[float, float, float, float]) -> bool:
    e0, n0, e1, n1 = env
    for pl in el.iter(f"{GML}posList"):
        v = (pl.text or "").split()
        try:
            es = [float(x) for x in v[0::3]]
            ns = [float(x) for x in v[1::3]]
        except ValueError:  # not numbers: the polygon is unreadable anyway
            continue
        if es and min(es) <= e1 and max(es) >= e0 and min(ns) <= n1 and max(ns) >= n0:
            return True
    return False


def _buildings(stream, name: str) -> Iterator[Element | None]:
    """Every complete bldg:Building of a CityGML stream, in document order,
    with None in place of a building of more than _MAX_BUILDING_BYTES; the
    caller may clear each building once it has read it.

    Elements outside buildings, or inside a building found too large, are
    dropped as soon as they end, and the nesting depth, the size of a
    building and the bytes between two tags are bounded, so memory stays
    small whatever the stream holds.
    """
    building, material = f"{BLDG}Building", f"{APP}X3DMaterial"
    parser = XMLPullParser(events=("start", "end"))
    open_elements: list[Element] = []
    inside = 0  # open bldg:Building elements
    too_large = False  # the open building has more than _MAX_BUILDING_BYTES
    fed = building_start = last_event = 0  # stream offsets, to the chunk
    while True:
        chunk = stream.read(_CHUNK)
        if chunk:
            parser.feed(chunk)
        else:
            parser.close()  # raises ParseError if the document is incomplete
        start = fed
        fed += len(chunk)
        for event, el in parser.read_events():
            last_event = fed
            if event == "start":
                open_elements.append(el)
                if len(open_elements) > _MAX_DEPTH:
                    raise CityGmlError(f"{name}: elements nested deeper than {_MAX_DEPTH} levels")
                if el.tag == building:
                    if not inside:
                        building_start = start
                    inside += 1
                continue
            open_elements.pop()
            if el.tag == building:
                inside -= 1
                if not too_large:
                    yield el
                elif not inside:
                    too_large = False
                    yield None
            elif el.tag == material:
                el.clear()
            if not inside or too_large:
                el.clear()
                if open_elements:
                    open_elements[-1].remove(el)
        if fed - last_event > _MAX_RUN_BYTES:
            raise CityGmlError(f"{name}: more than {_MAX_RUN_BYTES} bytes without a tag")
        if inside and not too_large and fed - building_start > _MAX_BUILDING_BYTES:
            # Drop what the building holds so far: each open element keeps
            # only the next one, which the parser is still filling.
            too_large = True
            first = next(i for i, e in enumerate(open_elements) if e.tag == building)
            chain = open_elements[first:]
            for parent, child in zip(chain, chain[1:] + [None]):
                del parent[:]
                if child is not None:
                    parent.append(child)
        if not chunk:
            return


def _reading(items: Iterator[Element], name: str) -> Iterator[Element]:
    """`items`, with archive and XML errors raised as CityGmlError (and
    nothing else: errors in the caller's own code stay what they are)."""
    while True:
        try:
            item = next(items)
        except StopIteration:
            return
        except CityGmlError:
            raise
        except _READ_ERRORS as e:
            raise CityGmlError(f"{name}: unreadable ({type(e).__name__}: {e})") from e
        yield item


def read_buildings(zip_path: Path, envelope: tuple[float, float, float, float], stats: dict[str, int]) -> list[Building]:
    """Buildings with geometry inside the LV95 envelope, sorted by id. A
    building of more than _MAX_BUILDING_BYTES of XML, anywhere in the sheet,
    is skipped and counted as one "too large to read"."""
    out: list[Building] = []
    try:
        z = zipfile.ZipFile(zip_path)
    except _READ_ERRORS as e:
        raise CityGmlError(f"{zip_path.name}: not a readable zip ({type(e).__name__}: {e})") from e
    with z:
        members = [i for i in z.infolist() if i.filename.lower().endswith(".gml")]
        if len(members) != 1 or members[0].file_size > _MAX_MEMBER_BYTES:
            raise CityGmlError(f"{zip_path.name}: expected exactly one .gml member")
        try:
            stream = z.open(members[0])
        except _READ_ERRORS as e:
            raise CityGmlError(f"{zip_path.name}: unreadable ({type(e).__name__}: {e})") from e
        with stream:
            for el in _reading(_buildings(stream, zip_path.name), zip_path.name):
                stats["buildings in sheet"] = stats.get("buildings in sheet", 0) + 1
                if el is None:  # more than _MAX_BUILDING_BYTES of XML
                    key = "buildings in sheet too large to read"
                    stats[key] = stats.get(key, 0) + 1
                    continue
                if _envelope_overlaps(el, envelope) and (b := _building(el, stats)) is not None:
                    out.append(b)
                el.clear()
    out.sort(key=lambda b: b.gml_id)
    return out


def _cached_ring(ring: object) -> bool:
    return isinstance(ring, list) and 3 <= len(ring) <= MAX_RING_POINTS and all(
        isinstance(p, list) and len(p) == 3 and all(type(v) in (int, float) and abs(v) <= MAX_COORDINATE_M for v in p)
        for p in ring
    )


def _cached_building(b: object) -> Building:
    """A Building from its cached form, checked to be what read_buildings writes."""
    ok = (
        isinstance(b, dict)
        and isinstance(b.get("gml_id"), str)
        and all(k in b and isinstance(b[k], (str, type(None))) for k in ("objektart", "egid"))
        and isinstance(b.get("surfaces"), list)
        and all(
            isinstance(s, list) and len(s) == 2 and s[0] in ("roof", "wall", "ground")
            and isinstance(s[1], list) and len(s[1]) >= 1 and all(_cached_ring(r) for r in s[1])
            for s in b["surfaces"]
        )
        and sum(len(r) for s in b["surfaces"] for r in s[1]) <= MAX_BUILDING_POINTS
    )
    if not ok:
        raise CityGmlError("a building is not in the cached form")
    return Building(b["gml_id"], b["objektart"], b["egid"], [(k, r) for k, r in b["surfaces"]])


def load_buildings(
    zip_path: Path, envelope: tuple[float, float, float, float], cache: Path, stats: dict[str, int]
) -> list[Building]:
    """`read_buildings`, cached as JSON under `cache/derived/` (parsing the
    whole sheet takes minutes; the cache is keyed by the sheet file name,
    whose content is checksum-verified, and the envelope)."""
    key = "_".join(str(round(v)) for v in envelope)
    path = cache / "derived" / f"{zip_path.stem}-v{CACHE_VERSION}-{key}.json"
    if path.exists():
        try:
            doc = json.loads(read_file(path, _MAX_CACHE_BYTES, follow_links=True))
            if not isinstance(doc, dict) or not isinstance(doc.get("buildings"), list):
                raise CityGmlError("not a cached extraction")
            cached_stats = doc.get("stats")
            if not isinstance(cached_stats, dict) or not all(type(v) is int for v in cached_stats.values()):
                raise CityGmlError("not a cached extraction")
            buildings = [_cached_building(b) for b in doc["buildings"]]
        except (ValueError, RecursionError) as e:  # CityGmlError and invalid JSON are ValueErrors
            raise CityGmlError(f"{path}: {e}; delete it and retry") from e
        stats.update(cached_stats)
        return buildings
    local: dict[str, int] = {}
    buildings = read_buildings(zip_path, envelope, local)
    stats.update(local)
    path.parent.mkdir(parents=True, exist_ok=True)
    doc = {
        "stats": local,
        "buildings": [
            {"gml_id": b.gml_id, "objektart": b.objektart, "egid": b.egid, "surfaces": b.surfaces}
            for b in buildings
        ],
    }
    # Through a part file of this process's own: ingests sharing the cache
    # may extract the same sheet at once, and each writes the same bytes.
    part = path.with_name(f"{path.name}.{os.getpid()}.part")
    try:
        part.write_text(json.dumps(doc, separators=(",", ":")), encoding="utf-8")
        try:
            os.replace(part, path)  # the last writer wins
        except PermissionError:
            # Windows refuses to replace a file that another process has
            # open: one that has just written the same extraction.
            if not path.exists():
                raise
    finally:
        part.unlink(missing_ok=True)
    return buildings
