"""Streaming reader for swissBUILDINGS3D 3.0 Beta CityGML (LOD2).

The sheet is one multi-gigabyte GML file inside a zip. It is parsed as a
stream (expat via ElementTree.iterparse, which resolves no external
entities) straight out of the zip, never extracted to disk.

Each `bldg:Building` carries its geometry twice: as typed boundary
surfaces (`bldg:boundedBy` Roof/Wall/Ground surfaces, including roof
overhangs) and again as untyped closed solids (`lod2Solid`, on the
building or its `BuildingPart`s). Only the typed surfaces are used.
"""

from __future__ import annotations

import json
import zipfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Iterator
from xml.etree.ElementTree import Element, iterparse

import numpy as np

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
CACHE_VERSION = 1


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
        return np.asarray(pts, dtype=np.float64)[:, :2]


def _local(tag: str) -> str:
    return tag.rsplit("}", 1)[-1]


def _ring(el: Element | None) -> list[list[float]] | None:
    if el is None:
        return None
    pos_list = el.find(f"{GML}posList")
    if pos_list is not None:
        dim = int(pos_list.get("srsDimension", "3"))
        values = [float(v) for v in (pos_list.text or "").split()]
    else:
        dim = 3
        values = [float(v) for p in el.findall(f"{GML}pos") for v in (p.text or "").split()]
    if dim != 3 or len(values) % 3:
        return None
    pts = [values[i : i + 3] for i in range(0, len(values), 3)]
    if len(pts) > 1 and pts[0] == pts[-1]:
        pts.pop()
    return pts if len(pts) >= 3 else None


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


def _building(el: Element, stats: dict[str, int]) -> Building:
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
    return b


def _envelope_overlaps(el: Element, env: tuple[float, float, float, float]) -> bool:
    e0, n0, e1, n1 = env
    for pl in el.iter(f"{GML}posList"):
        v = (pl.text or "").split()
        es = [float(x) for x in v[0::3]]
        ns = [float(x) for x in v[1::3]]
        if es and min(es) <= e1 and max(es) >= e0 and min(ns) <= n1 and max(ns) >= n0:
            return True
    return False


def read_buildings(zip_path: Path, envelope: tuple[float, float, float, float], stats: dict[str, int]) -> list[Building]:
    """Buildings with geometry inside the LV95 envelope, sorted by id."""
    out: list[Building] = []
    with zipfile.ZipFile(zip_path) as z:
        members = sorted(i for i in z.infolist() if i.filename.lower().endswith(".gml"))
        if len(members) != 1 or members[0].file_size > _MAX_MEMBER_BYTES:
            raise ValueError(f"{zip_path.name}: expected exactly one .gml member")
        with z.open(members[0]) as stream:
            for _, el in iterparse(stream, events=("end",)):
                tag = el.tag
                if tag == f"{APP}X3DMaterial":
                    el.clear()
                elif tag == f"{BLDG}Building":
                    stats["buildings in sheet"] = stats.get("buildings in sheet", 0) + 1
                    if _envelope_overlaps(el, envelope):
                        out.append(_building(el, stats))
                    el.clear()
    out.sort(key=lambda b: b.gml_id)
    return out


def load_buildings(
    zip_path: Path, envelope: tuple[float, float, float, float], cache: Path, stats: dict[str, int]
) -> list[Building]:
    """`read_buildings`, cached as JSON under `cache/derived/` (parsing the
    whole sheet takes minutes; the cache is keyed by the sheet file name,
    whose content is checksum-verified, and the envelope)."""
    key = "_".join(str(round(v)) for v in envelope)
    path = cache / "derived" / f"{zip_path.stem}-v{CACHE_VERSION}-{key}.json"
    if path.exists():
        doc = json.loads(path.read_text(encoding="utf-8"))
        stats.update(doc["stats"])
        return [Building(b["gml_id"], b["objektart"], b["egid"], [(k, r) for k, r in b["surfaces"]]) for b in doc["buildings"]]
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
    part = path.with_name(path.name + ".part")
    part.write_text(json.dumps(doc, separators=(",", ":")), encoding="utf-8")
    part.replace(path)
    return buildings
