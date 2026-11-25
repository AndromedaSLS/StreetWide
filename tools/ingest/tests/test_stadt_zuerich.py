"""The City of Zurich's trees and light points (ADR-0015): fetching,
caching and validation, on small synthetic answers (no network)."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import pytest
from shapely.geometry import box

from tools.ingest import http, stadt_zuerich
from tools.ingest.stadt_zuerich import LIGHTS, TREES, StadtZuerichError

SQUARE = (2683, 1247)


def tree(number, e, n, **props):
    p = {"baumnummer": number, "poi_id": number, "baumgattunglat": "Tilia", "baumartlat": "cordata",
         "baumnamelat": "Tilia cordata 'Erecta'", "baumnamedeu": "Winterlinde", "baumtyp": 4,
         "baumtyptext": "Höhe:10-20m", "genauigkeit": "Eingemessen", "geometrie_gdo": None,
         "kategorie": "Strassenbaum", "kronendurchmesser": 6, "objectid": 1, "pflanzjahr": 1990,
         "quartier": "Rathaus", "status": "Strassenbaum", "strasse": "Limmatquai"}
    p.update(props)
    return {"type": "Feature", "id": f"baumkataster_baumstandorte.{number}",
            "geometry": {"type": "Point", "coordinates": [e, n]}, "properties": p}


def light(number, e, n, orientation=90.0, **props):
    p = {"art": 0, "art_txt": "Beleuchtung", "geometrie_gdo": None, "nisnr": number, "objectid": 7, "objid": "7",
         "orientierung": orientation}
    p.update(props)
    return {"type": "Feature", "id": f"ewz_brennstelle_p.{number}",
            "geometry": {"type": "Point", "coordinates": [e, n]}, "properties": p}


def answer_bytes(features, crs="urn:ogc:def:crs:EPSG::2056") -> bytes:
    doc = {"type": "FeatureCollection", "bbox": [8.5, 47.3, 8.6, 47.4],
           "crs": {"type": "name", "properties": {"name": crs}}, "features": features}
    return json.dumps(doc, ensure_ascii=False).encode()


def metadata_bytes(dataset=TREES, license_id="cc-zero", date="22.11.2026") -> bytes:
    return json.dumps({"success": True, "result": {"name": dataset.package, "license_id": license_id,
                                                   "dateLastUpdated": date}}).encode()


def hits_bytes(n: int) -> bytes:
    return (f'<wfs:FeatureCollection xmlns:wfs="http://www.opengis.net/wfs" timeStamp="2026-11-25T20:27:59"'
            f' numberOfFeatures="{n}">\n</wfs:FeatureCollection>').encode()


def answer(dataset, features, square=SQUARE):
    return stadt_zuerich.Answer(dataset, square, "ab" * 32, "2026-11-22", len(features), features, {})


class FakeCity:
    """Answers the three requests of a fetch, and counts them."""

    def __init__(self, dataset, features, hits=None, dates=("22.11.2026", "22.11.2026"), license_id="cc-zero"):
        self.dataset, self.body = dataset, answer_bytes(features)
        self.hits = len(features) if hits is None else hits
        self.dates, self.license_id = list(dates), license_id
        self.urls: list[str] = []

    def __call__(self, url, *, max_bytes, data=None, retries=2):
        self.urls.append(url)
        if "package_show" in url:
            return metadata_bytes(self.dataset, self.license_id, self.dates.pop(0))
        if "RESULTTYPE=hits" in url:
            return hits_bytes(self.hits)
        return self.body


@pytest.fixture
def city(monkeypatch):
    def install(fake):
        monkeypatch.setattr(stadt_zuerich, "fetch_bytes", fake)
        return fake
    return install


# ------------------------------------------------------------- fetching --


def test_squares_cover_the_area_inside_the_city():
    area = box(2683100, 1247100, 2684900, 1247900)
    assert stadt_zuerich.squares(area) == [(2683, 1247), (2684, 1247)]
    # Touching a square's edge is not covering it.
    assert stadt_zuerich.squares(box(2683000, 1247000, 2684000, 1248000)) == [(2683, 1247)]
    # Bern: outside the city, nothing is asked for.
    assert stadt_zuerich.squares(box(2600000, 1199000, 2601000, 1200000)) == []


def test_requests_are_plain_boxes_in_lv95():
    url = stadt_zuerich._wfs_url(TREES, SQUARE, hits=False)
    assert url.startswith("https://www.ogd.stadt-zuerich.ch/wfs/geoportal/Baumkataster?")
    for part in ["SERVICE=WFS", "VERSION=1.1.0", "REQUEST=GetFeature", "TYPENAME=baumkataster_baumstandorte",
                 "OUTPUTFORMAT=application%2Fvnd.geo%2Bjson", "SRSNAME=EPSG:2056",
                 "BBOX=2683000,1247000,2684000,1248000,EPSG:2056"]:
        assert part in url.split("?")[1].split("&"), part
    hits = stadt_zuerich._wfs_url(LIGHTS, SQUARE, hits=True)
    assert "RESULTTYPE=hits" in hits and "TYPENAME=ewz_brennstelle_p" in hits and "OUTPUTFORMAT" not in hits


def test_metadata_gives_the_date_and_checks_the_licence():
    assert stadt_zuerich.parse_metadata(metadata_bytes(), TREES) == ("2026-11-22", "cc-zero")
    with pytest.raises(StadtZuerichError, match="states the licence 'cc-by'"):
        stadt_zuerich.parse_metadata(metadata_bytes(license_id="cc-by"), TREES)
    with pytest.raises(StadtZuerichError, match="dateLastUpdated"):
        stadt_zuerich.parse_metadata(metadata_bytes(date="2026-11-22"), TREES)
    with pytest.raises(StadtZuerichError, match="invalid dateLastUpdated"):
        stadt_zuerich.parse_metadata(metadata_bytes(date="31.02.2026"), TREES)
    with pytest.raises(StadtZuerichError, match="another dataset"):
        stadt_zuerich.parse_metadata(metadata_bytes(LIGHTS), TREES)
    with pytest.raises(StadtZuerichError, match="not JSON"):
        stadt_zuerich.parse_metadata(b"<html>", TREES)


def test_hits_and_answers_are_checked():
    assert stadt_zuerich.parse_hits(hits_bytes(1027), "x") == 1027
    with pytest.raises(StadtZuerichError, match="no feature count"):
        stadt_zuerich.parse_hits(b"<ows:ExceptionReport/>", "x")
    assert stadt_zuerich.parse_answer(answer_bytes([]), "x") == []
    with pytest.raises(StadtZuerichError, match="not in urn:ogc:def:crs:EPSG::2056"):
        stadt_zuerich.parse_answer(answer_bytes([], crs="urn:ogc:def:crs:OGC:1.3:CRS84"), "x")
    no_crs = json.loads(answer_bytes([]))
    del no_crs["crs"]
    with pytest.raises(StadtZuerichError, match="not in urn"):
        stadt_zuerich.parse_answer(json.dumps(no_crs).encode(), "x")
    for bad in [b"[]", b'{"type": "Feature"}', b"\xff", b'{"type": "FeatureCollection", "crs": '
                b'{"type": "name", "properties": {"name": "urn:ogc:def:crs:EPSG::2056"}}, "features": {}}']:
        with pytest.raises(StadtZuerichError):
            stadt_zuerich.parse_answer(bad, "x")


def test_a_fetch_is_cached_with_its_sidecar_and_checked_again(tmp_path: Path, city):
    features = [tree("AA-1", 2683500.0, 1247500.0), tree("AA-2", 2683600.0, 1247600.0)]
    fake = city(FakeCity(TREES, features))
    a = stadt_zuerich.fetch_answer(TREES, SQUARE, tmp_path, offline=False, refresh=False)
    assert len(fake.urls) == 4 and all(u.startswith("https://") for u in fake.urls)
    body = answer_bytes(features)
    assert a.sha256 == hashlib.sha256(body).hexdigest() and a.date == "2026-11-22" and a.hits == 2
    assert a.version == f"2026-11-22 sha256:{a.sha256}"
    folder = tmp_path / "stadt-zuerich" / "baumkataster_baumstandorte"
    assert (folder / "2683-1247.json").read_bytes() == body
    side = json.loads((folder / "2683-1247.sidecar.json").read_text(encoding="utf-8"))
    assert side["sha256"] == a.sha256 and side["size"] == len(body) and side["license"] == "cc-zero"
    assert side["features"] == side["hits"] == 2 and side["publisher_date"] == "2026-11-22"
    assert side["url"] == stadt_zuerich._wfs_url(TREES, SQUARE, hits=False)
    assert not list(folder.glob("*.part"))

    # From the cache, without a request; offline too.
    again = stadt_zuerich.fetch_answer(TREES, SQUARE, tmp_path, offline=True, refresh=False)
    assert len(fake.urls) == 4 and again.version == a.version and again.features == a.features
    # A changed answer no longer matches its sidecar.
    (folder / "2683-1247.json").write_bytes(body.replace(b"AA-2", b"AA-3"))
    with pytest.raises(StadtZuerichError, match="does not match its sidecar"):
        stadt_zuerich.fetch_answer(TREES, SQUARE, tmp_path, offline=True, refresh=False)
    # --refresh fetches again.
    fake.dates = ["22.11.2026", "22.11.2026"]
    stadt_zuerich.fetch_answer(TREES, SQUARE, tmp_path, offline=False, refresh=True)
    assert len(fake.urls) == 8 and (folder / "2683-1247.json").read_bytes() == body


def test_fetches_that_stop_the_run(tmp_path: Path, city):
    features = [light("LEU1", 2683500.0, 1247500.0)]
    with pytest.raises(StadtZuerichError, match="not cached and --offline is set"):
        stadt_zuerich.fetch_answer(LIGHTS, SQUARE, tmp_path, offline=True, refresh=False)
    city(FakeCity(LIGHTS, features, hits=2))
    with pytest.raises(StadtZuerichError, match="1 features, but the service counts 2"):
        stadt_zuerich.fetch_answer(LIGHTS, SQUARE, tmp_path, offline=False, refresh=False)
    city(FakeCity(LIGHTS, features, dates=("22.11.2026", "29.11.2026")))
    with pytest.raises(StadtZuerichError, match="updated while it was fetched"):
        stadt_zuerich.fetch_answer(LIGHTS, SQUARE, tmp_path, offline=False, refresh=False)
    city(FakeCity(LIGHTS, features, license_id="odc-odbl"))
    with pytest.raises(StadtZuerichError, match="states the licence 'odc-odbl'"):
        stadt_zuerich.fetch_answer(LIGHTS, SQUARE, tmp_path, offline=False, refresh=False)
    # Nothing was cached by a fetch that stopped.
    assert not (tmp_path / "stadt-zuerich").exists() or not list((tmp_path / "stadt-zuerich").rglob("*.json"))
    with pytest.raises(StadtZuerichError, match="the city's hosts only"):
        stadt_zuerich._fetch("https://example.org/wfs", 10)
    with pytest.raises(StadtZuerichError, match="the city's hosts only"):
        stadt_zuerich._fetch("http://www.ogd.stadt-zuerich.ch/wfs", 10)


def test_the_user_agent_is_generic():
    assert "@" not in http.USER_AGENT and "http" not in http.USER_AGENT.lower()


# -------------------------------------------------------------- records --


def test_tree_records_keep_the_registers_own_attributes():
    stats: dict[str, int] = {}
    features = [
        tree("AA-2", 2683500.0, 1247500.0),
        tree("AA-10", 2683510.0, 1247510.0, kronendurchmesser=8, baumartlat="spec.", pflanzjahr=None,
             baumnamelat="Tilia cv.", genauigkeit="Bildschirmeingabe"),
        tree("AA-3", 2683520.0, 1247520.0, baumartlat="x hispanica", baumgattunglat="Platanus",
             baumnamelat="Platanus x hispanica", kronendurchmesser=None, status="Grünanlage"),
    ]
    recs = stadt_zuerich.records(TREES, [answer(TREES, features)], stats)
    assert [r.point_id for r in recs] == ["stadt-zuerich:baumkataster:AA-10", "stadt-zuerich:baumkataster:AA-2",
                                          "stadt-zuerich:baumkataster:AA-3"]
    by_id = {r.local_id: r for r in recs}
    assert by_id["AA-2"].attributes == {
        "crown_diameter_mm": 6000, "genus": "Tilia", "height_mm": None, "leaf_cycle": None, "leaf_type": None,
        "planted_year": 1990, "species": "Tilia cordata", "taxon": "Tilia cordata 'Erecta'",
        "trunk_circumference_mm": None}
    a10 = by_id["AA-10"]
    assert a10.attributes["crown_diameter_mm"] is None and a10.crown_default
    assert a10.attributes["species"] is None and a10.attributes["planted_year"] is None
    assert a10.survey == "Bildschirmeingabe"
    assert by_id["AA-3"].attributes["species"] == "Platanus x hispanica"
    assert by_id["AA-3"].attributes["crown_diameter_mm"] is None and not by_id["AA-3"].crown_default
    assert stats == {}
    # Never kept: street, quarter, status, size class, internal ids, German name.
    for r in recs:
        assert not {"strasse", "quartier", "status", "baumtyp", "objectid", "baumnamedeu"} & set(r.attributes)


def test_records_of_the_wrong_form_are_left_out_and_counted():
    stats: dict[str, int] = {}
    features = [
        tree("OK-1", 2683500.0, 1247500.0),
        tree("BAD GENUS", 2683500.0, 1247500.0),  # a space: not a local id
        tree("G-1", 2683500.0, 1247500.0, baumgattunglat="x Sycoparrotia"),
        tree("Y-1", 2683500.0, 1247500.0, pflanzjahr="1990"),
        tree("C-1", 2683500.0, 1247500.0, kronendurchmesser=0),
        tree("O-1", 2684002.0, 1247500.0),  # 2 m outside the square
        tree("E-1", 2684000.5, 1247500.0),  # within its 1 m margin
        {**tree("P-1", 0, 0), "geometry": {"type": "LineString", "coordinates": [[0, 0], [1, 1]]}},
        {**tree("P-2", 0, 0), "geometry": {"type": "Point", "coordinates": [2683500.0, float("nan")]}},
        "not a feature",
    ]
    recs = stadt_zuerich.records(TREES, [answer(TREES, features)], stats)
    assert [r.local_id for r in recs] == ["E-1", "OK-1"]
    name = "Baumkataster Stadt Zürich left out: "
    assert stats == {name + "id of the wrong form": 1, name + "baumgattunglat of the wrong form": 1,
                     name + "pflanzjahr of the wrong form": 1, name + "kronendurchmesser of the wrong form": 1,
                     name + "outside its square": 1, name + "not a point": 2, name + "not a feature": 1}


def test_a_status_outside_the_six_stops_the_run_naming_the_records():
    features = [tree("AA-1", 2683500.0, 1247500.0), tree("AA-2", 2683500.0, 1247500.0, status="Privat"),
                tree("AA-3", 2683500.0, 1247500.0, status=None), tree("AA-4", 2683500.0, 1247500.0, status="Bund")]
    with pytest.raises(StadtZuerichError, match=r"3 trees with a status outside .*: AA-2 \(Privat\), AA-3 \(None\), "
                                                r"AA-4 \(Bund\)"):
        stadt_zuerich.records(TREES, [answer(TREES, features)], {})
    for status in stadt_zuerich.STATUSES:
        stadt_zuerich.records(TREES, [answer(TREES, [tree("AA-1", 2683500.0, 1247500.0, status=status)])], {})


def test_records_in_two_squares_must_agree():
    on_edge = light("LEU1", 2684000.0, 1247500.0)
    stats: dict[str, int] = {}
    a, b = answer(LIGHTS, [on_edge]), answer(LIGHTS, [dict(on_edge)], square=(2684, 1247))
    recs = stadt_zuerich.records(LIGHTS, [a, b], stats)
    assert len(recs) == 1 and recs[0].answer is a
    assert stats == {"Öffentliche Beleuchtung der Stadt Zürich records in two squares or listed twice": 1}
    moved = light("LEU1", 2684000.0, 1247501.0)
    with pytest.raises(StadtZuerichError, match="record LEU1 differs between squares 2683-1247 and 2684-1247"):
        stadt_zuerich.records(LIGHTS, [a, answer(LIGHTS, [moved], square=(2684, 1247))], {})
    with pytest.raises(StadtZuerichError, match="record LEU1 differs in square 2683-1247"):
        stadt_zuerich.records(LIGHTS, [answer(LIGHTS, [on_edge, moved])], {})


def test_light_orientations():
    stats: dict[str, int] = {}
    features = [light("LEU1", 2683500.0, 1247500.0, 295.1), light("LEU2", 2683500.0, 1247500.0, 0.0),
                light("LEU3", 2683500.0, 1247500.0, None), light("LEU4", 2683500.0, 1247500.0, 360.0),
                light("LEU5", 2683500.0, 1247500.0, "90"), light("LEU6", 2683500.0, 1247500.0, 0)]
    recs = {r.local_id: r for r in stadt_zuerich.records(LIGHTS, [answer(LIGHTS, features)], stats)}
    assert recs["LEU1"].attributes == {"orientation_deg": 295.1}
    assert recs["LEU2"].attributes == {"orientation_deg": None} and recs["LEU2"].orientation_zero
    assert recs["LEU6"].orientation_zero
    assert recs["LEU3"].attributes == {"orientation_deg": None} and not recs["LEU3"].orientation_zero
    assert set(recs) == {"LEU1", "LEU2", "LEU3", "LEU6"}
    assert stats == {"Öffentliche Beleuchtung der Stadt Zürich left out: orientierung of the wrong form": 2}


def test_point_ids_are_the_local_ids_byte_for_byte():
    recs = stadt_zuerich.records(LIGHTS, [answer(LIGHTS, [light("LEU105227", 2683500.0, 1247500.0)])], {})
    assert recs[0].point_id == "stadt-zuerich:beleuchtung:LEU105227"
    assert TREES.source == "stadt-zuerich:baumkataster" and LIGHTS.source == "stadt-zuerich:beleuchtung"
    # At most 256 characters with the source (26 of them).
    longest = stadt_zuerich.records(LIGHTS, [answer(LIGHTS, [light("X" * 230, 2683500.0, 1247500.0)])], {})
    assert len(longest[0].point_id) == 256
    stats: dict[str, int] = {}
    assert stadt_zuerich.records(LIGHTS, [answer(LIGHTS, [light("X" * 231, 2683500.0, 1247500.0)])], stats) == []
    assert stats == {"Öffentliche Beleuchtung der Stadt Zürich left out: id of the wrong form": 1}
