import numpy as np
import pytest
from shapely.geometry import box

from tools.ingest import transport
from tools.ingest.geo import Frame, lv95_to_lonlat
from tools.ingest.osm import parse, parse_length_m
from tools.ingest.tests.helpers import PlaneDem, way

FRAME = Frame((2145, 1434))
E, N = 2683500.0, 1247500.0


def _geom(en):
    lon, lat = lv95_to_lonlat(np.array([p[0] for p in en]), np.array([p[1] for p in en]))
    return [{"lon": float(a), "lat": float(b)} for a, b in zip(lon, lat)]


def test_parse_sorts_and_builds_footprints_with_holes():
    sq = [(E, N), (E + 20, N), (E + 20, N + 20), (E, N + 20), (E, N)]
    hole = [(E + 5, N + 5), (E + 5, N + 15), (E + 15, N + 15), (E + 15, N + 5), (E + 5, N + 5)]
    doc = {"elements": [
        {"type": "way", "id": 9, "tags": {"highway": "footway"}, "nodes": [1, 2], "geometry": _geom(sq[:2])},
        {"type": "way", "id": 3, "tags": {"building": "yes"}, "nodes": [1, 2, 3, 4, 1], "geometry": _geom(sq)},
        {"type": "relation", "id": 7, "tags": {"building": "yes", "type": "multipolygon"}, "members": [
            {"type": "way", "ref": 1, "role": "outer", "geometry": _geom(sq)},
            {"type": "way", "ref": 2, "role": "inner", "geometry": _geom(hole)},
        ]},
    ]}
    ways, fps = parse(doc, {})
    assert [w.id for w in ways] == [9]
    assert [f.ref for f in fps] == ["relation/7", "way/3"]
    assert fps[0].shape.area == pytest.approx(400 - 100, abs=1e-3)
    assert fps[1].shape.area == pytest.approx(400, abs=1e-3)
    assert fps[0].feature_id == "osm:relation:7"


@pytest.mark.parametrize("text,value", [("3", 3.0), ("2.5 m", 2.5), ("3,5", 3.5), ("wide", None), ("-1", None), (None, None)])
def test_parse_length(text, value):
    assert parse_length_m(text) == value


def test_exclusions():
    assert transport.exclusion(way(1, "residential", [[0, 0], [1, 0]])) is None
    assert transport.exclusion(way(1, "footway", [[0, 0], [1, 0]], tunnel="building_passage")) is None
    assert transport.exclusion(way(1, "footway", [[0, 0], [1, 0]], tunnel="yes")) == "tunnel"
    assert transport.exclusion(way(1, "construction", [[0, 0], [1, 0]])) == "highway=construction"
    assert transport.exclusion(way(1, "pedestrian", [[0, 0], [1, 0]], area="yes")).startswith("area=yes")
    assert transport.exclusion(way(1, "service", [[0, 0], [1, 0]], layer="-1")) == "layer below ground"


def test_ribbon_offsets_and_width():
    left, right = transport.offsets(np.array([[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]), 1.0)
    assert left[0] == pytest.approx([0, 1]) and right[0] == pytest.approx([0, -1])
    assert left[1] == pytest.approx([9, 1]) and right[1] == pytest.approx([11, -1])  # mitred corner
    assert transport.width_m(way(1, "residential", [[0, 0], [1, 0]], width="7")) == 7.0
    assert transport.width_m(way(1, "footway", [[0, 0], [1, 0]])) == 2.0


def test_ribbons_are_cut_to_the_tile_and_face_up():
    w = way(1, "residential", [[E - 50, N], [E + 50, N]])
    stats = {}
    mesh, refs = transport.tile_ribbons(FRAME, PlaneDem(), [w], box(E - 20, N - 20, E + 20, N + 20), [0, 0, 0], stats)
    assert refs == ["way/1"]
    (prim,) = mesh.node("tile:17/1/1").primitives
    p = np.array(prim.positions_mm, float) / 1000
    assert np.ptp(p[:, 0]) == pytest.approx(40, abs=0.1)  # cut at the clip box
    assert min(n[1] for n in prim.normals) > 0.99


def test_bridge_heights_interpolate_between_street_anchors():
    dem = PlaneDem(h0=400.0, de=0.1, dn=0.0)  # 1 m per 10 m eastwards
    street_w = way(1, "residential", [[E - 10, N], [E, N]], nodes=[1, 2])
    street_e = way(2, "residential", [[E + 30, N], [E + 40, N]], nodes=[5, 6])
    b1 = way(3, "residential", [[E, N], [E + 10, N], [E + 20, N]], nodes=[2, 3, 4], bridge="yes")
    b2 = way(4, "footway", [[E + 20, N], [E + 30, N]], nodes=[4, 5], bridge="yes")
    h = transport.bridge_heights([b1, b2], [street_w, street_e], dem, {})
    ground = lambda e: 400 + 0.1 * (e - 2683000)  # noqa: E731
    assert h[2] == pytest.approx(ground(E)) and h[5] == pytest.approx(ground(E + 30))
    assert h[3] == pytest.approx(ground(E) + (ground(E + 30) - ground(E)) / 3)
    mesh = transport.bridge_deck(FRAME, b1, h, [0, 0, 0])
    (prim,) = mesh.node("osm:way:3").primitives
    assert prim.material == "bridge"
    ys = np.array(prim.positions_mm)[:, 1] / 1000
    assert ys.max() - ys.min() == pytest.approx(transport.DECK_THICKNESS_M + 2 / 3 * 3, abs=0.6)
