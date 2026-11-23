"""The OSM fallback: footprints are extruded only where swissBUILDINGS3D
has no building (synthetic data, as required by the stage 3 checkpoint)."""

import numpy as np
import pytest
from shapely.geometry import Polygon, box

from tools.ingest import buildings
from tools.ingest.geo import Frame
from tools.ingest.osm import Footprint
from tools.ingest.tests.helpers import PlaneDem

E, N = 2683500.0, 1247500.0
SWISS = box(E, N, E + 20, N + 20)  # one swissBUILDINGS3D footprint


def fp(id, shape, **tags):
    return Footprint("way", id, {"building": "yes", **tags}, shape)


def test_only_uncovered_footprints_are_extruded():
    cases = [
        fp(1, box(E + 5, N + 5, E + 15, N + 15)),  # fully covered
        fp(2, box(E + 15, N, E + 35, N + 20)),  # 25 % covered: above the 20 % limit
        fp(3, box(E + 18, N, E + 38, N + 20)),  # 10 % covered: extruded
        fp(4, box(E + 100, N, E + 110, N + 10)),  # not covered at all
        fp(5, box(E + 200, N, E + 210, N + 10), building="roof"),
        fp(6, box(E + 300, N, E + 310, N + 10), location="underground"),
    ]
    stats = {}
    got = buildings.fallback_candidates(cases, SWISS, stats)
    assert [f.id for f in got] == [3, 4]
    assert stats["OSM buildings not extruded: covered by swissBUILDINGS3D"] == 2
    assert stats["OSM buildings not extruded: building=roof"] == 1


@pytest.mark.parametrize("tags,height,rule", [
    ({"height": "12.5"}, 12.5, "height"),
    ({"building:levels": "4"}, 12.0, "building:levels"),
    ({}, 10.0, "default"),
])
def test_fallback_height(tags, height, rule):
    assert buildings.fallback_height(tags) == (height, rule)


def test_extruded_shell_stands_on_the_lowest_ground_and_faces_out():
    dem = PlaneDem(h0=400.0, de=0.1, dn=0.0)
    court = Polygon([(E, N), (E + 30, N), (E + 30, N + 30), (E, N + 30)], [[(E + 10, N + 10), (E + 20, N + 10), (E + 20, N + 20), (E + 10, N + 20)]])
    frame = Frame((2145, 1434))
    stats = {}
    mesh = buildings.fallback_shell(frame, dem, fp(7, court, **{"building:levels": "2"}), [0, 0, 0], stats)
    prims = {p.material: p for p in mesh.node("osm:way:7").primitives}
    assert list(prims) == ["roof", "wall", "ground"]
    ground_y = np.array(prims["ground"].positions_mm)[:, 1] / 1000
    roof_y = np.array(prims["roof"].positions_mm)[:, 1] / 1000
    base = 400 + 0.1 * (E - 2683000)  # lowest corner (west edge)
    # In the region frame, height 0 drops with the Earth's curvature away
    # from the anchor (about 0.3 m here), so compare in the frame.
    assert ground_y.max() == pytest.approx(frame.from_lv95(E, N, base)[0][1], abs=0.01)
    # Roof and ground plates share their vertices, so their mean heights
    # differ by exactly the storey height (2 levels of 3 m).
    assert roof_y.mean() - ground_y.mean() == pytest.approx(6.0, abs=0.002)
    assert max(n[1] for n in prims["ground"].normals) < -0.99
    assert min(n[1] for n in prims["roof"].normals) > 0.99
    # Walls: 4 outer + 4 courtyard walls, 2 triangles each, all facing away
    # from the solid (outer walls outwards, courtyard walls into the court).
    w = prims["wall"]
    assert len(w.indices) == 16 * 3
    pts = np.array(w.positions_mm, float) / 1000
    centre_xz = pts[:, [0, 2]].mean(axis=0)
    for i in range(0, len(w.indices), 3):
        mid = pts[w.indices[i : i + 3]].mean(axis=0)[[0, 2]]
        n = np.array(w.normals[w.indices[i]])[[0, 2]]
        r = np.abs(mid - centre_xz).max()
        inner = r < 10  # courtyard walls are within 5 m of the centre
        assert (np.dot(n, mid - centre_xz) < 0) == inner
