"""Point features (RFC 0004) from records: anchors on the terrain mesh,
ADR-0015's yaw rule and the points documents."""

from __future__ import annotations

from fractions import Fraction

import numpy as np
import pytest

from tools.ingest import point_features, terrain
from tools.ingest.geo import Area, Frame, lonlat_to_lv95, tilef_to_lonlat
from tools.ingest.mesh import MeshBuilder
from tools.ingest.point_features import Ground, Placed
from tools.ingest.stadt_zuerich import LIGHTS, TREES, Answer, Record
from tools.ingest.tests.helpers import PlaneDem
from tools.worldfmt import points

FRAME = Frame((2145, 1434))
TILE = (68646, 45903)
ANSWER = Answer(TREES, (2683, 1247), "ab" * 32, "2026-11-22", 0, [], {})


def triangle_mesh() -> MeshBuilder:
    m = MeshBuilder([0, 0, 0])
    pts = np.array([[0.0, 1.0, 0.0], [4.0, 2.0, 0.0], [0.0, 3.0, -4.0], [4.0, 4.0, -4.0]])
    # Counter-clockwise seen from above: NW (0,-4), SW (0,0), NE (4,-4); NE, SW, SE (4,0).
    m.add("terrain", pts, np.tile([0.0, 1.0, 0.0], (4, 1)), [[2, 0, 3], [3, 0, 1]])
    return m


def exact(x, z):
    """The surface of `triangle_mesh` in millimetres: y = 1 + x/4 - z/2 (metres)."""
    return Fraction(1000) + Fraction(x, 4) - Fraction(z, 2)


def test_the_ground_is_the_mesh_surface_to_the_millimetre():
    g = Ground(triangle_mesh())
    for x, z in [(0, 0), (4000, 0), (0, -4000), (4000, -4000), (2000, -2000), (1, -1), (1234, -3001), (3999, -1),
                 (2001, -1999)]:
        y = g.height(x, z)
        assert y is not None and abs(y - exact(x, z)) <= Fraction(1, 2), (x, z, y)
        assert y == int(np.floor(float(exact(x, z)) + 0.5)), (x, z)
    for x, z in [(-1, 0), (4001, -10), (10, 1), (10, -4001)]:
        assert g.height(x, z) is None


def test_no_mesh_no_ground():
    assert Ground(MeshBuilder([0, 0, 0])).height(0, 0) is None


def test_yaw_from_orientation_turns_grid_directions_into_the_region_frame():
    e, n = 2683500.0, 1247500.0
    yaws = [point_features.yaw_from_orientation(FRAME, e, n, 410.0, d) for d in (0.0, 90.0, 180.0, 270.0)]
    # East is a quarter turn clockwise from north; LV95's grid north is
    # within about 1 degree of the region frame's north in Zurich.
    for got, want in zip(yaws, (16384, 0, 49152, 32768)):
        assert min((got - want) % 65536, (want - got) % 65536) < 200, yaws
    # A quarter turn counter-clockwise in the grid is a quarter turn
    # counter-clockwise in the frame, to within a unit.
    assert all(abs(((yaws[k] - yaws[k + 1]) % 65536) - 16384) <= 1 for k in range(3))
    assert point_features.yaw_from_orientation(FRAME, e, n, 410.0, 359.999) == pytest.approx(yaws[0], abs=2)


def records(kind, positions, **attributes):
    out = []
    for k, (e, n) in enumerate(positions):
        dataset = TREES if kind == "tree" else LIGHTS
        local = f"X-{k:03d}"
        attrs = {"crown_diameter_mm": None, "genus": "Tilia", "height_mm": None, "leaf_cycle": None, "leaf_type": None,
                 "planted_year": 1990, "species": None, "taxon": None, "trunk_circumference_mm": None} \
            if kind == "tree" else {"orientation_deg": attributes.get("orientation", 45.0)}
        out.append(Record(f"{dataset.source}:{local}", local, e, n, ANSWER, attrs))
    return out


def tile_point(fx, fy, tile=TILE):
    lon, lat = tilef_to_lonlat(tile[0] + fx, tile[1] + fy)
    e, n = lonlat_to_lv95(lon, lat)
    return float(e), float(n)


def test_points_are_placed_on_their_tiles_terrain():
    # About the northern half of the tile: its edge cuts terrain cell 64 (of 128).
    area = Area(TILE[0], TILE[1], TILE[0] + 1.0, TILE[1] + 0.503)
    dem = PlaneDem()
    origins = {TILE: FRAME.tile_origin_mm(*TILE)}
    mesh = terrain.build(FRAME, dem, area, TILE, origins[TILE], {})
    grounds = {TILE: Ground(mesh)}
    positions = [tile_point(0.25, 0.25), tile_point(0.75, 0.1), tile_point(0.5, 0.75), tile_point(0.5, 0.502)]
    stats: dict[str, int] = {}
    placed = point_features.place(records("light", positions), FRAME, dem, area, origins, grounds, stats, "Lights")
    # The third is in the southern half, outside the area; the fourth is in
    # a terrain cell that the area's edge cuts, so no terrain is under it.
    assert [p.record.local_id for p in placed] == ["X-000", "X-001"]
    assert stats == {"Lights left out: outside the area": 1, "Lights left out: no terrain under the point": 1}
    for p in placed:
        assert p.tile == TILE
        x, y, z = p.anchor_mm
        assert grounds[TILE].height(x, z) == y
        # On the plane of PlaneDem, to within the mesh's interpolation.
        want = FRAME.from_lv95(p.record.e, p.record.n, dem.height(p.record.e, p.record.n))[0]
        assert abs(x - (want[0] * 1000 - origins[TILE][0])) <= 1 and abs(z - (want[2] * 1000 - origins[TILE][2])) <= 1
        assert abs(y - want[1] * 1000) < 20
        assert p.ground_m == pytest.approx(float(dem.height(p.record.e, p.record.n)), abs=0.02)
        assert p.yaw is not None
    # No terrain data under a point.
    holes = PlaneDem(box=(0, 0, 1, 1))
    stats = {}
    assert point_features.place(records("light", positions[:1]), FRAME, holes, area, origins, grounds, stats, "L") == []
    assert stats == {"L left out: no terrain under the point": 1}


def test_documents_are_sorted_and_name_their_rules():
    def placed(kind, k, yaw=None, local=None):
        dataset = TREES if kind == "tree" else LIGHTS
        local = local or f"X-{k}"
        attrs = records(kind, [(0, 0)])[0].attributes
        return Placed(Record(f"{dataset.source}:{local}", local, 0, 0, ANSWER, attrs), TILE, [k, 400_000, -k], 400.0, yaw)

    lights = [placed("light", 2, 100), placed("light", 1), placed("light", 3, 0)]
    doc = point_features.document("light", lights)
    assert [p["id"] for p in doc["points"]] == [f"stadt-zuerich:beleuchtung:X-{k}" for k in (1, 2, 3)]
    assert [p["estimated"] for p in doc["points"]] == [[], ["yaw"], ["yaw"]]
    assert [p["attributes"]["yaw"] for p in doc["points"]] == [None, 100, 0]
    assert doc["rules"] == {"light": {"yaw": "adr-0015:yaw-from-orientierung:1"}}
    assert points.problems(doc) == []
    assert point_features.document("light", [placed("light", 1)])["rules"] == {}

    trees = [placed("tree", 1, local="B-1"), placed("tree", 2, local="A-10"), placed("tree", 3, local="A-9")]
    doc = point_features.document("tree", trees, {"stadt-zuerich:baumkataster:A-9": 12_345})
    assert [p["id"].split(":")[-1] for p in doc["points"]] == ["A-10", "A-9", "B-1"]  # byte order
    assert [p["attributes"]["height_mm"] for p in doc["points"]] == [None, 12_345, None]
    assert doc["rules"] == {} and all(p["estimated"] == [] for p in doc["points"])
    assert points.bounds(doc) == {"max": [3, 400_000, -1], "min": [1, 400_000, -3]}
