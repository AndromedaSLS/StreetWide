"""Fuzzing the ingest's readers of downloaded files (roadmap M1): Overpass
answers (roads and buildings, and water), STAC search answers, swissALTI3D
GeoTIFFs, swissBUILDINGS3D CityGML sheets and the derived building cache. Each must accept its input
or raise its documented error, and what it accepts must not crash or hang
the steps that use it. Regression tests replay the inputs that used to."""

from __future__ import annotations

import io
import json
import struct
import time
import tracemalloc
import zipfile
from pathlib import Path

import numpy as np
import pytest
import tifffile
from shapely.geometry import Polygon, box

from tools.ingest import buildings, citygml, overpass, stac, transport, water
from tools.ingest.citygml import CACHE_VERSION, CityGmlError, _buildings, load_buildings, read_buildings
from tools.ingest.dem import Dem, DemError
from tools.ingest.geo import BBox, Frame, lv95_to_lonlat
from tools.ingest.osm import OsmError, RingError, join_rings, parse
from tools.ingest.overpass import OverpassError, parse_answer
from tools.ingest.stac import StacError
from tools.ingest.tests.helpers import PlaneDem, way
from tools.ingest.tests.test_citygml import HEADER, box_building
from tools.ingest.tests.test_osm_water import AREA as WATER_AREA
from tools.ingest.tests.test_osm_water import FRAME as WATER_FRAME
from tools.ingest.tests.test_osm_water import _area_poly, _clips, _relation, _square, _way, pond, river
from tools.ingest.water import WaterError
from tools.worldfmt.tests.fuzzing import (
    MEMORY,
    Rng,
    mutate_bytes,
    mutate_json,
    mutate_json_bytes,
    reading,
    run_cases,
)

FRAME = Frame((2145, 1434))
E, N = 2683500.0, 1247500.0
DEM = PlaneDem()
ENVELOPE = (2683000.0, 1247000.0, 2684000.0, 1248000.0)


# Overpass ------------------------------------------------------------------


def geometry(*en):
    lon, lat = lv95_to_lonlat(np.array([p[0] for p in en]), np.array([p[1] for p in en]))
    return [{"lat": float(b), "lon": float(a)} for a, b in zip(lon, lat)]


def square(e, n, size):
    return [(e, n), (e + size, n), (e + size, n + size), (e, n + size), (e, n)]


ANSWER = {
    "version": 0.6,
    "generator": "Overpass API 0.7.62",
    "osm3s": {"timestamp_osm_base": "2026-11-20T12:00:00Z", "copyright": "OpenStreetMap contributors (ODbL)"},
    "elements": [
        {"type": "way", "id": 1, "nodes": [1, 2, 3], "tags": {"highway": "residential", "width": "6"},
         "geometry": geometry((E - 60, N), (E, N), (E + 10, N + 5))},
        {"type": "way", "id": 2, "nodes": [3, 4], "tags": {"highway": "footway", "width": "2,5"},
         "geometry": geometry((E + 10, N + 5), (E + 20, N + 5))},
        {"type": "way", "id": 3, "nodes": [4, 5, 6], "tags": {"highway": "residential", "bridge": "yes", "layer": "1"},
         "geometry": geometry((E + 20, N + 5), (E + 30, N + 5), (E + 40, N + 5))},
        {"type": "way", "id": 4, "nodes": [6, 7], "tags": {"highway": "footway", "bridge": "yes"},
         "geometry": geometry((E + 40, N + 5), (E + 50, N + 5))},
        {"type": "way", "id": 5, "nodes": [8, 9], "tags": {"highway": "service", "tunnel": "yes"},
         "geometry": geometry((E, N - 20), (E + 10, N - 20))},
        {"type": "way", "id": 6, "nodes": [9, 10], "tags": {"highway": "pedestrian", "area": "yes"},
         "geometry": [None, {"lat": 47.37, "lon": 8.54}]},
        {"type": "way", "id": 10, "nodes": [20, 21, 22, 23, 20], "tags": {"building": "yes", "height": "12.5"},
         "geometry": geometry(*square(E + 30, N + 30, 12))},
        {"type": "way", "id": 11, "nodes": [24, 25, 26, 27, 24], "tags": {"building": "house", "building:levels": "3"},
         "geometry": geometry(*square(E - 30, N + 30, 8))},
        {"type": "relation", "id": 30, "tags": {"building": "yes", "type": "multipolygon"}, "members": [
            {"type": "way", "ref": 40, "role": "outer", "geometry": geometry(*square(E + 60, N - 40, 20))},
            {"type": "way", "ref": 41, "role": "inner", "geometry": geometry(*square(E + 65, N - 35, 5))},
            {"type": "node", "ref": 50, "role": "entrance"},
        ]},
    ],
}
ANSWER_BYTES = json.dumps(ANSWER).encode()


def use_osm(doc: dict, stats: dict) -> None:
    """What the pipeline does with a parsed answer, on a synthetic terrain."""
    ways, footprints = parse(doc, stats)
    used = [w for w in ways[:50] if transport.exclusion(w) is None]
    bridges = [w for w in used if transport.is_bridge(w)]
    heights = transport.bridge_heights(bridges, [w for w in ways if not transport.is_bridge(w)], DEM, stats)
    for w in bridges[:5]:
        transport.width_m(w)
        mesh = transport.bridge_deck(FRAME, w, heights, [0, 0, 0])
        if mesh is not None:
            mesh.node(f"osm:way:{w.id}")
    streets = [w for w in used if not transport.is_bridge(w)][:20]
    transport.tile_ribbons(FRAME, DEM, streets, box(E - 70, N - 70, E + 70, N + 70), [0, 0, 0], stats)
    for fp in buildings.fallback_candidates(footprints[:20], box(E, N, E + 1, N + 1), stats)[:3]:
        buildings.fallback_shell(FRAME, DEM, fp, [0, 0, 0], stats)


def test_synthetic_answer_is_used():
    stats = {}
    use_osm(parse_answer(ANSWER_BYTES), stats)
    assert stats["OSM fallback height from height"] == 1


def test_fuzz_overpass_answers():
    """parse_answer and osm.parse raise OverpassError or OsmError; what they
    accept goes through the transport and building steps."""

    def case(rng: Rng) -> None:
        data = mutate_json_bytes(rng, ANSWER, [ANSWER], [ANSWER_BYTES]) if rng.chance(85) else mutate_bytes(rng, ANSWER_BYTES, [ANSWER_BYTES])
        try:
            with reading(len(data)):
                use_osm(parse_answer(data), {})
        except (OverpassError, OsmError):
            pass

    run_cases("Overpass answers", 250, case, memory=MEMORY)


# Overpass water answers (ADR-0014) ---------------------------------------------

WATER_ANSWER = {
    "version": 0.6,
    "osm3s": {"timestamp_osm_base": "2026-11-24T12:00:00Z"},
    "elements": [
        river(),
        pond(),
        _way(8, _square(2683000.5, 1247350.5, 30.0), natural="water", amenity="fountain"),
        _way(9, _square(2683100.5, 1247400.5, 40.0), natural="water"),
    ],
}
WATER_ANSWER_BYTES = json.dumps(WATER_ANSWER).encode()
WATER_TERRAIN = {(2682, 1247), (2683, 1247)}


def use_water(doc: dict) -> int:
    """What the pipeline does with a parsed water answer: parse, cut per
    terrain tile with levels, and build every z17 tile's mesh. Returns how
    many tiles got water drawn."""
    stats: dict[str, int] = {}
    bodies = water.parse(doc, WATER_AREA.bbox(), stats)
    cut = water.pieces(DEM, bodies[:20], _area_poly(WATER_AREA), WATER_TERRAIN, stats)
    clips = _clips(WATER_AREA)
    tiles = [water.tile_water(WATER_FRAME, DEM, cut, clips[t], WATER_FRAME.tile_origin_mm(*t), stats) for t in sorted(clips)]
    return sum(not w.mesh.is_empty() for w in tiles)


def test_synthetic_water_answer_is_used():
    doc = parse_answer(WATER_ANSWER_BYTES)
    assert [b.ref for b in water.parse(doc, WATER_AREA.bbox(), {})] == ["relation/10", "way/7", "way/9"]
    assert use_water(doc) == 2


def test_fuzz_overpass_water_answers():
    """parse_answer and water.parse raise OverpassError, OsmError or
    WaterError; what they accept goes through the levels and the meshes."""

    def case(rng: Rng) -> None:
        if rng.chance(85):
            data = mutate_json_bytes(rng, WATER_ANSWER, [WATER_ANSWER, ANSWER], [WATER_ANSWER_BYTES])
        else:
            data = mutate_bytes(rng, WATER_ANSWER_BYTES, [WATER_ANSWER_BYTES])
        try:
            with reading(len(data)):
                use_water(parse_answer(data))
        except (OverpassError, OsmError, WaterError):
            pass

    run_cases("Overpass water answers", 600, case, memory=MEMORY)


# STAC ------------------------------------------------------------------------

BBOX = BBox(8.5377, 47.3687, 8.5507, 47.3774)
INSIDE = [[8.5375049, 47.3686138], [8.5507426, 47.3684871], [8.5509303, 47.3774805], [8.5376903, 47.3776072], [8.5375049, 47.3686138]]
SUFFIX = stac.SWISSALTI3D.asset_suffix


def item(id, ring=INSIDE):
    return {
        "id": id,
        "type": "Feature",
        "geometry": {"type": "Polygon", "coordinates": [ring]},
        "assets": {
            f"{id}{SUFFIX}": {"href": f"https://data.geo.admin.ch/ch.swisstopo.swissalti3d/{id}/{id}{SUFFIX}",
                              "file:checksum": "1220" + "ab" * 32},
            f"{id}_2_2056_5728.tif": {"href": "https://data.geo.admin.ch/other", "file:checksum": "1220" + "cd" * 32},
        },
    }


FEATURES = [item("swissalti3d_2019_2683-1247"), item("swissalti3d_2026_2683-1247"), item("swissalti3d_2026_2684-1247")]
PAGE = {"type": "FeatureCollection", "features": FEATURES,
        "links": [{"rel": "next", "href": stac.STAC_ROOT + "/collections/x/items?page=2"}, {"rel": "self", "href": "x"}]}


def check_selection(files: list[stac.SourceFile], cache: Path) -> None:
    for f in files:
        assert f.path.parent.parent == cache / "swisstopo" and f.path.parent.name == f.item_id
        assert f.url.startswith("https://data.geo.admin.ch/") and len(f.sha256) == 64


def test_fuzz_stac_answers(tmp_path, monkeypatch):
    """STAC pages and cached searches raise StacError or select files whose
    paths stay in the cache, whatever the answer says."""
    cache = tmp_path / "cache"
    pages = iter(())
    monkeypatch.setattr(stac, "fetch_bytes", lambda url, max_bytes: next(pages, b'{"features":[]}'))

    def case(rng: Rng) -> None:
        nonlocal pages
        k = rng.below(3)
        try:
            if k == 0:
                files = stac.select(mutate_json(rng, FEATURES, [PAGE]), stac.SWISSALTI3D, BBOX, cache)
            else:
                data = [mutate_json_bytes(rng, PAGE, [PAGE], []) for _ in range(1 + rng.below(3))]
                if k == 1:
                    pages = iter(data)
                    features = stac._search(stac.SWISSALTI3D, BBOX, cache, offline=False, refresh=True)
                else:
                    search = next((cache / "stac").rglob("search-*.json"), None)
                    if search is None:
                        return
                    search.write_bytes(data[0])
                    features = stac._search(stac.SWISSALTI3D, BBOX, cache, offline=True, refresh=False)
                files = stac.select(features, stac.SWISSALTI3D, BBOX, cache)
        except StacError:
            return
        check_selection(files, cache)

    run_cases("STAC answers", 300, case)


# GeoTIFF ---------------------------------------------------------------------

S, E0, N1 = 0.5, 2683000.0, 1248000.0


def geotiff(z, *, e0=E0, n1=N1, dtype=np.float32, **options) -> bytes:
    keys = [1, 1, 0, 3, 1024, 0, 1, 1, 1025, 0, 1, 1, 3072, 0, 1, 2056]
    tags = [
        (33550, "d", 3, (S, S, 0.0), True),
        (33922, "d", 6, (0.0, 0.0, 0.0, e0, n1, 0.0), True),
        (34735, "H", len(keys), keys, True),
        (42113, "s", 0, "-9999", True),
    ]
    out = io.BytesIO()
    tifffile.imwrite(out, np.asarray(z, dtype=dtype), extratags=tags, **options)
    return out.getvalue()


Z = 400 + np.arange(24 * 32, dtype=np.float64).reshape(24, 32) / 10
TIFFS = [
    geotiff(Z),
    geotiff(Z, compression="zlib"),
    geotiff(Z, compression="lzw", predictor=True),
    geotiff(np.pad(Z, ((0, 8), (0, 0))), compression="zlib", tile=(16, 16)),
    geotiff(Z, e0=E0 + 16),  # the eastern neighbour of the first
]


def mutate_tiff(rng: Rng, data: bytes) -> bytes:
    """Edits one field of one entry of the first IFD: tag, type, count or
    value/offset (sizes, strip offsets and counts, compression, ...)."""
    out = bytearray(data)
    if len(out) < 8:
        return bytes(out)
    ifd = struct.unpack_from("<I", out, 4)[0]
    if ifd + 2 > len(out):
        return bytes(out)
    entries = struct.unpack_from("<H", out, ifd)[0]
    if not entries:
        return bytes(out)
    at = ifd + 2 + 12 * rng.below(entries)
    field, size = rng.choice(((0, 2), (2, 2), (4, 4), (8, 4)))
    if at + field + size > len(out):
        return bytes(out)
    value = rng.choice((0, 1, 2, 3, 5, 7, 8, 16, 255, 256, 0x7FFF, 0xFFFF, 0x10000, 0x7FFFFFFF, 0xFFFFFFFF,
                        len(data), len(data) - 4, 32946, 34887, 50000, 33003, 34925))
    out[at + field : at + field + size] = (value & ((1 << (8 * size)) - 1)).to_bytes(size, "little")
    return bytes(out)


def test_fuzz_geotiffs(tmp_path):
    """Dem raises DemError for any malformed tile; a mosaic it builds gives
    heights or NaN."""
    paths = [tmp_path / "a.tif", tmp_path / "b.tif"]

    def case(rng: Rng) -> None:
        count = 1 + rng.below(2)
        for p in paths[:count]:
            data = rng.choice(TIFFS)
            for _ in range(1 + rng.below(2)):
                data = mutate_tiff(rng, data) if rng.chance(50) else mutate_bytes(rng, data, TIFFS)
            p.write_bytes(data)
        try:
            dem = Dem(paths[:count])
        except DemError:
            return
        h = dem.height(np.array([E0 + 1.0, E0 + 7.3, E0 - 5.0]), np.array([N1 - 1.0, N1 - 4.1, N1 + 5.0]))
        assert h.shape == (3,) and not np.isinf(h).any()

    run_cases("GeoTIFF tiles", 400, case)


# CityGML ---------------------------------------------------------------------

APPEARANCE = (
    "<app:appearanceMember><app:Appearance><app:surfaceDataMember><app:X3DMaterial><app:target>#x</app:target>"
    "</app:X3DMaterial></app:surfaceDataMember></app:Appearance></app:appearanceMember>"
)
SHEET_XML = HEADER + box_building("ID_A", 2683200, 1247200) + APPEARANCE + box_building("ID_B", 2683300, 1247300, "Unterirdisches Gebaeude") + "</core:CityModel>"
XML_EDITS = (
    "nan", "inf", "-1e400", "1e300", "abc", "", "1 2", "<", "&", "&amp;", "&lt;", "&x;", "<![CDATA[1 2 3]]>", "<!-- c -->",
    '"', "\x00", "\udcff", "</gml:posList>", "<bldg:Building>", "</bldg:Building>", 'srsDimension="2"',
    'srsDimension="x"', "9" * 5000,
)


def mutate_xml(rng: Rng, text: str) -> str:
    k = rng.below(6)
    if k == 0:  # an edit inside the text, attributes or markup
        at = rng.below(len(text) + 1)
        return text[:at] + rng.choice(XML_EDITS) + text[at + rng.below(8) :]
    if k == 1:  # many buildings
        member = text[len(HEADER) : text.index("<app:appearanceMember>")]
        return HEADER + member * rng.choice((2, 50)) + text[len(HEADER) :]
    if k == 2:  # deep nesting, outside or inside a building
        depth = rng.choice((10, 255, 300, 5000))
        at = text.index("<bldg:boundedBy>") if rng.chance(50) else len(HEADER)
        return text[:at] + "<a>" * depth + "</a>" * depth + text[at:]
    if k == 3:  # entities, including an expanding one
        doctype = '<!DOCTYPE m [<!ENTITY a "aaaaaaaaaa"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;">'
        doctype += '<!ENTITY c "&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;"><!ENTITY d "&c;&c;&c;&c;&c;&c;&c;&c;&c;&c;">]>'
        return text.replace("?>", "?>" + doctype, 1).replace("#x", rng.choice(("&a;", "&d;", "&e;")), 1)
    if k == 4:  # truncated
        return text[: rng.below(len(text))]
    return text.replace("UTF-8", rng.choice(("UTF-16", "latin-1", "bogus", "ascii")), 1)


def _entry(name: str) -> zipfile.ZipInfo:
    # A fixed time: writestr(name, ...) stamps the current time, and every
    # case must depend on the seed, the test and the case number alone.
    return zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))


def zipped(xml: bytes, rng: Rng, names=("sheet.gml",)) -> bytes:
    method = rng.choice((zipfile.ZIP_DEFLATED, zipfile.ZIP_STORED, zipfile.ZIP_BZIP2, zipfile.ZIP_LZMA))
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w") as z:
        for name in names:
            z.writestr(_entry(name), xml, compress_type=method)
    return out.getvalue()


SHEETS = [zipped(SHEET_XML.encode(), Rng(i)) for i in range(4)]


def test_fuzz_citygml_sheets(tmp_path):
    """read_buildings raises CityGmlError for a damaged sheet; the buildings
    it reads give footprints and shells."""
    path = tmp_path / "sheet.citygml.zip"

    def case(rng: Rng) -> None:
        if rng.chance(60):
            xml = mutate_xml(rng, SHEET_XML)
            data = zipped(xml.encode("utf-8", "surrogatepass"), rng, rng.choice((("sheet.gml",), ("a.gml", "b.GML"), ("x.txt",))))
        else:
            data = mutate_bytes(rng, rng.choice(SHEETS), SHEETS)
        path.write_bytes(data)
        stats: dict[str, int] = {}
        try:
            found = read_buildings(path, ENVELOPE, stats)
        except CityGmlError:
            return
        for b in found[:5]:
            buildings.swiss_footprint(b)
            buildings.swiss_shell(FRAME, b, [0, 0, 0], stats)

    run_cases("CityGML sheets", 400, case)


def test_fuzz_derived_cache(tmp_path):
    """A damaged cached extraction raises CityGmlError, never a KeyError."""
    sheet = tmp_path / "sheet.citygml.zip"
    sheet.write_bytes(SHEETS[0])
    cache = tmp_path / "cache"
    good = load_buildings(sheet, ENVELOPE, cache, {})
    (cached,) = (cache / "derived").glob(f"sheet.citygml-v{CACHE_VERSION}-*.json")
    doc = json.loads(cached.read_bytes())
    original = cached.read_bytes()

    def case(rng: Rng) -> None:
        data = mutate_json_bytes(rng, doc, [doc], [original])
        cached.write_bytes(data)
        try:
            with reading(len(data)):
                for b in load_buildings(sheet, ENVELOPE, cache, {}):
                    buildings.swiss_footprint(b)
        except CityGmlError:
            pass

    assert [b.gml_id for b in good] == ["ID_A", "ID_B"]
    run_cases("derived building cache", 300, case, memory=MEMORY)


# Regressions: inputs that crashed or hung ------------------------------------


@pytest.mark.parametrize(
    "data",
    [
        b"[" * 100_000,  # RecursionError
        b'{"elements":[],"osm3s":5}',  # AttributeError: osm3s is not an object
        b'{"elements":[],"osm3s":{"timestamp_osm_base":5}}',  # a number as the OSM version
        b'{"elements":[],"remark":[[[1]]],"osm3s":{}}',
        b'{"elements":[],"osm3s":{"timestamp_osm_base":"' + b"1" * 5000 + b'"}}',
        b"\xff\xfe",
    ],
)
def test_malformed_overpass_answers(data):
    with pytest.raises(OverpassError):
        parse_answer(data)


@pytest.mark.parametrize(
    "element",
    [
        {"type": "way"},  # KeyError: no id
        {"type": "way", "id": "1"},  # TypeError when sorted with integer ids
        {"type": "way", "id": 2, "tags": ["highway"]},
        {"type": "way", "id": 2, "tags": {"highway": "residential", "width": 5}},  # AttributeError in parse_length_m
        {"type": "way", "id": 2, "tags": {"highway": "residential"}, "geometry": [{"lat": 47.4}, {"lat": 47.4, "lon": 8.5}]},  # KeyError
        {"type": "way", "id": 2, "tags": {"highway": "residential"}, "geometry": [{"lat": "47", "lon": 8.5}, {"lat": 47.4, "lon": 8.5}]},
        {"type": "way", "id": 2, "tags": {"highway": "residential"}, "geometry": [{"lat": 1e308, "lon": 8.5}, {"lat": 47.4, "lon": 8.5}]},
        # A 400-digit integer: OverflowError when converted to float.
        {"type": "way", "id": 2, "tags": {"highway": "residential"}, "geometry": [{"lat": 10**400, "lon": 8.5}, {"lat": 47.4, "lon": 8.5}]},
        {"type": "way", "id": 2, "tags": {"highway": "residential"}, "geometry": {"lat": 47.4, "lon": 8.5}},
        {"type": "way", "id": 2, "tags": {"highway": "residential"}, "nodes": "ab", "geometry": geometry((E, N), (E + 1, N))},
        # KeyError: a building without nodes.
        {"type": "way", "id": 9, "tags": {"building": "yes"}, "geometry": geometry(*square(E, N, 10))},
        # A bridge without nodes: np.interp raised ValueError in bridge_deck.
        {"type": "way", "id": 9, "tags": {"highway": "footway", "bridge": "yes"}, "geometry": geometry((E, N), (E + 9, N))},
        {"type": "way", "id": 9, "tags": {"highway": "footway"}, "nodes": [1, 2, 3], "geometry": geometry((E, N), (E + 9, N))},
        {"type": "relation", "id": 3, "tags": {"building": "yes"}, "members": [5]},  # AttributeError
    ],
)
def test_malformed_osm_elements(element):
    with pytest.raises(OsmError):
        parse({"elements": [element, ANSWER["elements"][0]]}, {})


@pytest.mark.parametrize(
    "elements",
    [
        [{"type": "way", "id": "1"}, {"type": "way", "id": 2}],  # TypeError when sorted
        [{"type": "way"}],  # KeyError: no id
        [{"type": "way", "id": 2, "tags": ["natural", "water"]}],  # AttributeError
        [{"type": "way", "id": 2, "tags": {"natural": "water"}, "nodes": [1, 2, 3, 1],
          "geometry": [{"lat": 47.37}, {"lat": 47.37, "lon": 8.54}, {"lat": 47.38, "lon": 8.54}, {"lat": 47.37}]}],
        [{"type": "way", "id": 2, "tags": {"natural": "water"}, "nodes": 5, "geometry": [{"lat": 47.37, "lon": 8.54}]}],
        [{"type": "way", "id": 2, "tags": {"natural": "water"}, "nodes": [1, 2, 3, 1], "geometry": {"lat": 47.37}}],
        [{"type": "relation", "id": 3, "tags": {"natural": "water", "type": "multipolygon"}, "members": [5]}],
        [{"type": "relation", "id": 3, "tags": {"natural": "water", "type": "multipolygon"},
          "members": [{"type": "way", "ref": [1], "role": "outer", "geometry": []}]}],
        [{"type": "relation", "id": 3, "tags": {"natural": "water", "type": "multipolygon"},
          "members": [{"type": "way", "ref": 1, "role": "outer", "geometry": [{"lat": "47", "lon": 8.5}]}]}],
        [{"type": "relation", "id": 3, "tags": {"natural": "water", "type": "multipolygon"}, "members": [],
          "bounds": {"minlat": "x"}}],
    ],
    ids=["mixed-ids", "no-id", "tags-list", "no-lon", "nodes-number", "geometry-object", "member-number",
         "unhashable-ref", "text-coordinate", "bad-bounds"],
)
def test_malformed_water_elements(elements):
    # Each raised TypeError, KeyError or AttributeError (found by review:
    # water.parse never had the checks osm.parse got).
    with pytest.raises(OsmError):
        water.parse({"elements": elements}, WATER_AREA.bbox(), {})


def test_a_water_member_with_an_unhashable_role_is_a_broken_relation():
    # TypeError (unhashable role) before; a role other than outer or inner
    # stops the ingest when the relation reaches the area (ADR-0014).
    rel = river()
    rel["members"][0]["role"] = ["outer"]
    with pytest.raises(WaterError, match="not outer or inner"):
        water.parse({"elements": [rel]}, WATER_AREA.bbox(), {})


def test_joining_rings_is_linear_in_the_ways():
    # 20,000 ways between the same two points: finding each next way looked
    # at every way before it (about 2e8 steps).
    a, b = (8.5, 47.3), (8.6, 47.4)
    lines = [[a, (8.55, 47.35 + i * 1e-9), b] for i in range(20_000)]
    start = time.perf_counter()
    rings = join_rings(lines)
    assert len(rings) == 10_000 and all(r[0] == r[-1] for r in rings)
    assert time.perf_counter() - start < 5
    assert rings[0] == lines[0] + lines[1][-2::-1]


def _nest_by_pairs(rings, stats):
    """water._nest as first written: every ring tested against every other."""
    shapes = [s for _, s in rings]
    depth = [sum(1 for o in shapes if o is not s and o.area > s.area and o.contains(s.representative_point()))
             for s in shapes]
    shape = None
    for d in sorted(set(depth)):
        add = [s for (role, s), k in zip(rings, depth) if k == d and role == "outer"]
        cut = [s for (role, s), k in zip(rings, depth) if k == d and role == "inner"]
        if add:
            shape = water.unary_union(add) if shape is None else shape.union(water.unary_union(add))
        if cut and shape is not None:
            shape = shape.difference(water.unary_union(cut))
    odd = sum(1 for (role, _), k in zip(rings, depth) if (role == "outer") != (k % 2 == 0))
    if odd:
        stats["odd"] = odd
    return None if shape is None else water._area(shape)


def test_nesting_finds_the_same_depths_as_testing_every_pair():
    rng = Rng(14)
    for _ in range(200):
        rings = []
        for _ in range(1 + rng.below(12)):
            e, n, size = rng.below(60), rng.below(60), 1 + rng.below(50)
            rings.append(("outer" if rng.chance(60) else "inner", Polygon(_square(e, n, size)[:-1])))
        new_stats, old_stats = {}, {}
        new, old = water._nest(rings, new_stats), _nest_by_pairs(rings, old_stats)
        assert (new is None) == (old is None)
        assert new is None or new.equals_exact(old, 0)
        assert new_stats.get("OSM water rings whose role does not match their nesting") == old_stats.get("odd")


def test_stacked_water_rings_are_a_broken_relation():
    # 1,500 copies of one ring: nesting them tested every pair (2.25 million
    # tests, minutes), and the relation then gave its area anyway.
    ring = _square(2683000.5, 1247300.5, 20.0)
    rel = _relation(3, [(k, "outer", ring) for k in range(1, 1501)], natural="water")
    start = time.perf_counter()
    with pytest.raises(WaterError, match="overlap too much"):
        water.parse({"elements": [rel]}, WATER_AREA.bbox(), {})
    assert time.perf_counter() - start < 10


def test_overlong_ways_are_refused():
    # A building way retracing its outline 6,000 times (24,001 points) took
    # gigabytes in GEOS (make_valid); OpenStreetMap ways have at most 2,000 nodes.
    pts = square(E, N, 10)[:-1] * 6000 + [(E, N)]
    long_way = {"type": "way", "id": 1, "tags": {"building": "yes"}, "nodes": list(range(len(pts) - 1)) + [0],
                "geometry": geometry(*pts)}
    with pytest.raises(OsmError, match="more than OpenStreetMap allows"):
        parse({"elements": [long_way]}, {})


def test_huge_building_relations_are_unusable():
    # 60 outer members, each a 1,997-point way retracing one square: GEOS's
    # polygon building took seconds, growing faster than the member count.
    member = {"type": "way", "role": "outer", "geometry": geometry(*(square(E, N, 10)[:-1] * 499 + [(E, N)]))}
    relation = {"type": "relation", "id": 1, "tags": {"building": "yes"}, "members": [member] * 60}
    stats = {}
    assert parse({"elements": [relation]}, stats) == ([], [])
    assert stats == {"osm building relations unusable": 1}


def test_unhashable_member_role_is_skipped():
    # TypeError (unhashable role) before; such a member is now not used.
    relation = {"type": "relation", "id": 9, "tags": {"building": "yes"},
                "members": [{"type": "way", "role": ["outer"], "geometry": geometry(*square(E, N, 10))}]}
    stats = {}
    assert parse({"elements": [relation]}, stats) == ([], [])
    assert stats == {"osm building relations unusable": 1}


def test_a_bridge_reaching_across_the_globe_is_bounded():
    # Densifying it to 1.5 m steps took minutes and gigabytes.
    far = way(1, "residential", [[E, N], [E + 10, N], [E + 9_000_000, N + 3_000_000]], nodes=[1, 2, 3], bridge="yes")
    stats = {}
    heights = transport.bridge_heights([far], [], DEM, stats)
    assert transport.densified_count(far.en) > transport.MAX_DECK_POINTS
    assert transport.bridge_deck(FRAME, far, heights, [0, 0, 0]) is None


def test_bridge_heights_refuse_a_huge_bridge_group():
    # The heights of a group come from a dense solve: 30,000 free nodes
    # needed 7 GB.
    n = 30_000
    en = np.stack([E + np.arange(n) * 2.0, np.full(n, N)], axis=1)
    long = way(1, "footway", en, nodes=list(range(n)), bridge="yes")
    stats = {}
    heights = transport.bridge_heights([long], [], DEM, stats)
    assert stats["bridges too large"] == 1 and heights == {}


def test_stac_asset_names_cannot_leave_the_cache(tmp_path):
    # The asset name became the download path: "../../x" wrote outside the
    # cache (and "C:\\x" anywhere on Windows).
    for name in ("../../../x" + SUFFIX, "..\\x" + SUFFIX, "/x" + SUFFIX, "C:\\x" + SUFFIX, "x/y" + SUFFIX):
        f = item("swissalti3d_2026_2683-1247")
        f["assets"][name] = f["assets"].pop(f"swissalti3d_2026_2683-1247{SUFFIX}")
        with pytest.raises(StacError, match="asset name"):
            stac.select([f], stac.SWISSALTI3D, BBOX, tmp_path)


@pytest.mark.parametrize(
    "features",
    [
        [5],  # AttributeError
        [{"id": 5}],  # TypeError from the id pattern
        [{"id": "swissalti3d_2026_2683-1247"}],  # KeyError: no geometry
        [{"id": "swissalti3d_2026_2683-1247", "geometry": {"type": "Polygon", "coordinates": [[[8.537, 47.368], [8.551, 47.378], [8.551, 47.368], [8.537, 47.378], [8.537, 47.368]]]}}],  # GEOSException
        [dict(item("swissalti3d_2026_2683-1247"), assets=[1])],  # AttributeError
        [dict(item("swissalti3d_2026_2683-1247"), assets={"swissalti3d_2026_2683-1247" + SUFFIX: {"file:checksum": "1220" + "ab" * 32}})],  # KeyError: no href
        [dict(item("swissalti3d_2026_2683-1247"), assets={"swissalti3d_2026_2683-1247" + SUFFIX: {"href": "https://data.geo.admin.ch/a b", "file:checksum": "1220" + "ab" * 32}})],  # http.client.InvalidURL later
        [dict(item("swissalti3d_2026_2683-1247"), assets={"swissalti3d_2026_2683-1247" + SUFFIX: {"href": "https://data.geo.admin.ch/a", "file:checksum": "1220ab"}})],
    ],
)
def test_malformed_stac_items(tmp_path, features):
    with pytest.raises(StacError):
        stac.select(features, stac.SWISSALTI3D, BBOX, tmp_path)


def test_complex_stac_footprints_are_refused(tmp_path):
    # Found by fuzzing (seed 0xC0FFEE): a footprint of 3,000 identical rings
    # made GEOS's overlay take gigabytes and minutes. swisstopo's have 5 points.
    f = item("swissalti3d_2026_2683-1247")
    f["geometry"]["coordinates"] = [INSIDE] * 3000
    with pytest.raises(StacError, match="more than 1000 points"):
        stac.select([f], stac.SWISSALTI3D, BBOX, tmp_path)


def test_stac_pages_are_bounded(tmp_path, monkeypatch):
    # A next link back to the same page looped forever.
    page = json.dumps({"features": [], "links": [{"rel": "next", "href": stac.STAC_ROOT + "/same"}]}).encode()
    monkeypatch.setattr(stac, "fetch_bytes", lambda url, max_bytes: page)
    with pytest.raises(StacError, match="pages"):
        stac._search(stac.SWISSALTI3D, BBOX, tmp_path, offline=False, refresh=True)


@pytest.mark.parametrize("cached", [b"{}", b"[" * 100_000, b'[{"id": 5}]'])
def test_malformed_cached_stac_search(tmp_path, monkeypatch, cached):
    monkeypatch.setattr(stac, "fetch_bytes", lambda url, max_bytes: b'{"features":[]}')
    stac._search(stac.SWISSALTI3D, BBOX, tmp_path, offline=False, refresh=True)
    (search,) = (tmp_path / "stac").rglob("search-*.json")
    search.write_bytes(cached)
    with pytest.raises(StacError):
        features = stac._search(stac.SWISSALTI3D, BBOX, tmp_path, offline=True, refresh=False)
        stac.select(features, stac.SWISSALTI3D, BBOX, tmp_path)


def _with_entry(data: bytes, tag: int, value: int, *, field: int = 8, size: int = 4) -> bytes:
    """`data` with the IFD entry of `tag` changed (count at 4, value at 8)."""
    out = bytearray(data)
    ifd = struct.unpack_from("<I", out, 4)[0]
    for i in range(struct.unpack_from("<H", out, ifd)[0]):
        at = ifd + 2 + 12 * i
        if struct.unpack_from("<H", out, at)[0] == tag:
            out[at + field : at + field + size] = value.to_bytes(size, "little")
            return bytes(out)
    raise AssertionError(f"no tag {tag}")


@pytest.mark.parametrize(
    "data",
    [
        _with_entry(TIFFS[0], 256, 0xFFFFFF),  # ImageWidth: a 2^24 x 24 raster, MemoryError
        _with_entry(_with_entry(TIFFS[0], 256, 0xFFFF), 257, 0xFFFF),  # 4 billion pixels
        _with_entry(TIFFS[1], 259, 50000),  # ZSTD on deflate data: imagecodecs error
        _with_entry(TIFFS[1], 279, 7),  # StripByteCounts too small: DeflateError
        TIFFS[0][:200],  # truncated: TiffFileError
        b"II*\x00" + b"\xff" * 12,
        # ImageLength 0 or 1: accepted, then Dem.height raised IndexError.
        _with_entry(TIFFS[0], 257, 0),
        _with_entry(TIFFS[0], 257, 1),
    ],
    ids=["wide", "huge", "wrong-codec", "short-strip", "truncated", "garbage", "no-rows", "one-row"],
)
def test_malformed_geotiffs(tmp_path, data):
    (tmp_path / "t.tif").write_bytes(data)
    with pytest.raises(DemError):
        Dem([tmp_path / "t.tif"])


@pytest.mark.parametrize("dtype", [np.complex64, np.uint8, np.int16, np.bool_])
def test_geotiffs_hold_floating_point_heights(tmp_path, dtype):
    # Found by the long fuzz campaign (as warnings): a complex raster lost
    # its imaginary part, and integers or booleans became heights.
    path = tmp_path / "t.tif"
    path.write_bytes(geotiff(Z, dtype=dtype))
    with pytest.raises(DemError, match="not floating-point heights"):
        Dem([path])


def test_float64_geotiffs_are_read(tmp_path):
    path = tmp_path / "t.tif"
    path.write_bytes(geotiff(Z, dtype=np.float64))
    assert Dem([path]).height(np.array([E0 + 0.25]), np.array([N1 - 0.25]))[0] == pytest.approx(Z[0, 0])


def test_dem_mosaic_is_bounded(tmp_path):
    # Two tiles 1000 km apart: a mosaic of 4e12 pixels (MemoryError, or
    # swapping); a NaN or infinite scale: ValueError from round().
    (tmp_path / "a.tif").write_bytes(TIFFS[0])
    (tmp_path / "b.tif").write_bytes(geotiff(Z, e0=E0 + 1_000_000, n1=N1 + 1_000_000))
    with pytest.raises(DemError, match="too large an area"):
        Dem([tmp_path / "a.tif", tmp_path / "b.tif"])


def _sheet(tmp_path: Path, xml: str, names=("sheet.gml",)) -> Path:
    path = tmp_path / "sheet.citygml.zip"
    with zipfile.ZipFile(path, "w") as z:
        for name in names:
            z.writestr(_entry(name), xml)
    return path


@pytest.mark.parametrize(
    "xml,names",
    [
        (SHEET_XML, ("a.gml", "b.gml")),  # TypeError: ZipInfo objects sorted
        (SHEET_XML[:-30], ("sheet.gml",)),  # ParseError: truncated
        (SHEET_XML.replace("?>", '?><!DOCTYPE m [<!ENTITY a "aaaaaaaaaa"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;"><!ENTITY c "&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;"><!ENTITY d "&c;&c;&c;&c;&c;&c;&c;&c;&c;&c;"><!ENTITY e "&d;&d;&d;&d;&d;&d;&d;&d;&d;&d;"><!ENTITY f "&e;&e;&e;&e;&e;&e;&e;&e;&e;&e;"><!ENTITY g "&f;&f;&f;&f;&f;&f;&f;&f;&f;&f;">]>', 1).replace("#x", "&g;"), ("sheet.gml",)),  # ParseError: entity expansion
        (HEADER + "<a>" * 300 + "</a>" * 300 + "</core:CityModel>", ("sheet.gml",)),  # nested deeper than the limit
        (SHEET_XML.replace("UTF-8", "bogus", 1), ("sheet.gml",)),  # LookupError: unknown encoding (found by fuzzing)
    ],
    ids=["two-members", "truncated", "entity-expansion", "deep", "unknown-encoding"],
)
def test_malformed_citygml_sheets(tmp_path, xml, names):
    with pytest.raises(CityGmlError):
        read_buildings(_sheet(tmp_path, xml, names), ENVELOPE, {})


def test_corrupt_zip_is_a_citygml_error(tmp_path):
    for i, data in enumerate((b"PK\x03\x04" + b"\0" * 40, SHEETS[0][:-30], SHEETS[0].replace(b"sheet.gml", b"sheet.GMX"))):
        path = tmp_path / f"{i}.zip"
        path.write_bytes(data)
        with pytest.raises(CityGmlError):
            read_buildings(path, ENVELOPE, {})


@pytest.mark.parametrize(
    "edit",
    [('srsDimension="3"', 'srsDimension="x"'), ("2683200", "2683200abc"), ("2683200", "nan"), ("2683200", "1e300")],
    ids=["dimension", "not-a-number", "nan", "huge"],
)
def test_unreadable_coordinates_are_counted(tmp_path, edit):
    # ValueError from int() or float(); NaN and huge values reached shapely.
    xml = SHEET_XML.replace(*edit)
    stats = {}
    found = read_buildings(_sheet(tmp_path, xml), ENVELOPE, stats)
    assert stats.get("polygons unreadable", 0) > 0 or len(found) < 2
    for b in found:
        buildings.swiss_footprint(b)


def _building_xml(gml_id: str, roofs: list[str]) -> str:
    polygons = "".join(
        "<gml:surfaceMember><gml:Polygon><gml:exterior><gml:LinearRing><gml:posList>"
        f"{r}</gml:posList></gml:LinearRing></gml:exterior></gml:Polygon></gml:surfaceMember>"
        for r in roofs
    )
    return (
        f'<core:cityObjectMember><bldg:Building gml:id="{gml_id}"><bldg:boundedBy><bldg:RoofSurface>'
        f"<bldg:lod2MultiSurface><gml:MultiSurface>{polygons}</gml:MultiSurface></bldg:lod2MultiSurface>"
        "</bldg:RoofSurface></bldg:boundedBy></bldg:Building></core:cityObjectMember>"
    )


SQUARE_ROOF = "2683200 1247200 410 2683210 1247200 410 2683210 1247210 410 2683200 1247210 410 "


def test_overlong_rings_are_unreadable(tmp_path):
    # A roof ring retracing its square 6,000 times hung the triangulation
    # (earcut) for minutes; LOD2 faces have tens of points.
    xml = HEADER + _building_xml("ID_R", [SQUARE_ROOF * 6000]) + "</core:CityModel>"
    stats = {}
    (b,) = read_buildings(_sheet(tmp_path, xml), ENVELOPE, stats)
    assert b.surfaces == [] and stats["polygons unreadable"] == 1
    assert buildings.swiss_shell(FRAME, b, [0, 0, 0], stats) is None


def test_oversized_buildings_are_skipped(tmp_path, monkeypatch):
    # Every ring is allowed, the building as a whole is not (the limit is
    # lowered here so the sheet stays small).
    monkeypatch.setattr(citygml, "MAX_BUILDING_POINTS", 1_000)
    xml = HEADER + _building_xml("ID_BIG", [SQUARE_ROOF * 25] * 11) + _building_xml("ID_OK", [SQUARE_ROOF]) + "</core:CityModel>"
    stats = {}
    assert [b.gml_id for b in read_buildings(_sheet(tmp_path, xml), ENVELOPE, stats)] == ["ID_OK"]
    assert stats["buildings too large"] == 1


def test_building_without_surfaces_has_no_footprint(tmp_path):
    # footprint_points() raised IndexError on a building inside the envelope
    # (its solid is) but without any typed surface.
    xml = (
        HEADER + '<core:cityObjectMember><bldg:Building gml:id="ID_E"><bldg:lod2Solid><gml:Solid><gml:exterior>'
        "<gml:CompositeSurface><gml:surfaceMember><gml:Polygon><gml:exterior><gml:LinearRing><gml:posList>"
        "2683200 1247200 410 2683210 1247200 410 2683210 1247210 410 2683200 1247200 410</gml:posList>"
        "</gml:LinearRing></gml:exterior></gml:Polygon></gml:surfaceMember></gml:CompositeSurface></gml:exterior>"
        "</gml:Solid></bldg:lod2Solid></bldg:Building></core:cityObjectMember></core:CityModel>"
    )
    (b,) = read_buildings(_sheet(tmp_path, xml), ENVELOPE, {})
    assert b.surfaces == [] and buildings.swiss_footprint(b) is None


def test_elements_outside_buildings_are_not_kept():
    # iterparse kept every element outside the buildings: a small zip of a
    # huge sheet could fill the memory.
    xml = (HEADER + "<core:cityObjectMember><x/></core:cityObjectMember>" * 200_000 + "</core:CityModel>").encode()
    tracemalloc.start()
    try:
        assert list(_buildings(io.BytesIO(xml), "test")) == []
        peak = tracemalloc.get_traced_memory()[1]
    finally:
        tracemalloc.stop()
    assert peak < 16 << 20, peak


@pytest.mark.parametrize(
    "cached",
    [
        b"[]", b'{"stats":{},"buildings":[{"gml_id":"ID_A"}]}',  # KeyError
        b'{"stats":{"a":"b"},"buildings":[]}',
        b'{"stats":{},"buildings":[{"gml_id":"ID_A","objektart":null,"egid":null,"surfaces":[["roof",[[[1,2]]]]]}]}',
        b"[" * 100_000,
    ],
)
def test_malformed_derived_cache(tmp_path, cached):
    sheet = tmp_path / "sheet.citygml.zip"
    sheet.write_bytes(SHEETS[0])
    load_buildings(sheet, ENVELOPE, tmp_path / "cache", {})
    (path,) = (tmp_path / "cache" / "derived").glob("*.json")
    path.write_bytes(cached)
    with pytest.raises(CityGmlError, match="delete it and retry"):
        load_buildings(sheet, ENVELOPE, tmp_path / "cache", {})


def test_overpass_cache_refuses_damaged_files(tmp_path):
    cache = tmp_path / "cache"
    bbox = BBox(8.54, 47.37, 8.55, 47.38)
    key = __import__("hashlib").sha256(overpass.query(bbox).encode()).hexdigest()[:16]
    path = cache / "osm" / f"overpass-{key}.json"
    path.parent.mkdir(parents=True)
    path.write_bytes(b'{"elements":[],"osm3s":[]}')
    with pytest.raises(OverpassError, match="delete it and retry"):
        overpass.fetch(bbox, cache, offline=True, refresh=False)
