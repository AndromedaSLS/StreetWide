"""The water layer from OpenStreetMap water areas (ADR-0014), on synthetic
data: selection, assembly of relations (nested rings, loud failures), the
bound on repairing invalid rings, levels per swissALTI3D tile (medians,
independent of the area), the cut per tile, the report, the manifests and
determinism."""

import time

import numpy as np
import pytest
import shapely
from shapely.geometry import Polygon

from tools.ingest import water
from tools.ingest.assemble import OSM_CREDIT, OSM_ON_TERRAIN_LICENSE, SWISSTOPO_CREDIT, SchemaError, write_world
from tools.ingest.dem import Dem
from tools.ingest.geo import Area, Frame
from tools.ingest.osm import RingError, join_rings
from tools.ingest.pipeline import _lv95_polygon, _rect, build_water
from tools.ingest.stac import SourceFile
from tools.ingest.tests.helpers import overpass_geometry, write_dem_tile
from tools.worldfmt import cjson, glb, schemas
from tools.worldfmt.store import NOTICE_FILES, Store

FRAME = Frame((2145, 1434))
# swissALTI3D tiles, in LV95 kilometres; T1 is T0's western neighbour.
T0, T1 = (2683, 1247), (2682, 1247)
# Two z17 tiles: WEST straddles T1 and T0, EAST lies inside T0.
WEST, EAST = (68644, 45902), (68645, 45902)
AREA = Area(68644.0, 45902.0, 68646.0, 45903.0)
VERSION = "2026-11-24T12:00:00Z"

# A 40 m wide river from T1 into T0, with an island. All shores are on half
# metres, so no 2 m grid point lies on a shore or exactly 3 m from one.
W, E, S, N = 2682950.5, 2683250.5, 1247290.5, 1247330.5
ISLAND = [(2683180.5, 1247305.5), (2683199.5, 1247305.5), (2683199.5, 1247314.5), (2683180.5, 1247314.5)]
T1_HEIGHT = 430.0


def t0_height(n):
    """T0's terrain rises 1 cm per metre northwards."""
    return 405.0 + 0.01 * (np.asarray(n, dtype=np.float64) - 1247300.0)


@pytest.fixture(scope="module")
def dems(tmp_path_factory):
    """The same two terrain tiles as an area of one tile or of two would load
    them (2 m pixels), and T0 with a false ramp: its western tenth, which
    holds about 40% of the river's samples, 1.3 m higher."""
    d = tmp_path_factory.mktemp("dem")
    centres_n = T0[1] * 1000.0 + 1000.0 - (np.arange(500) + 0.5) * 2.0
    z0 = np.repeat(t0_height(centres_n)[:, None], 500, axis=1)
    t0 = write_dem_tile(d / "t0.tif", T0, z0, 2.0)
    t1 = write_dem_tile(d / "t1.tif", T1, np.full((500, 500), T1_HEIGHT), 2.0)
    ramp = z0.copy()
    ramp[:, :50] += 1.3  # pixel centres west of E 2683100
    t0_ramp = write_dem_tile(d / "t0-ramp.tif", T0, ramp, 2.0)
    return {"T0": Dem([t0]), "T0+T1": Dem([t0, t1]), "ramp": Dem([t0_ramp, t1])}


def _way(id, en, **tags):
    nodes = [id * 100 + i for i in range(len(en))]
    if en[0] == en[-1]:
        nodes[-1] = nodes[0]
    return {"type": "way", "id": id, "tags": tags, "nodes": nodes, "geometry": overpass_geometry(en)}


def _relation(id, members, **tags):
    return {
        "type": "relation",
        "id": id,
        "tags": {"type": "multipolygon", **tags},
        "members": [{"type": "way", "ref": r, "role": role, "geometry": overpass_geometry(en)} for r, role, en in members],
    }


def _square(e, n, size):
    return [(e, n), (e + size, n), (e + size, n + size), (e, n + size), (e, n)]


def river(id=10, dx=0.0, broken=False):
    """The river: two outer ways that meet end to end (one given backwards)
    and an island. `broken` stops the second way 5 m short of the first."""
    a = [(W + dx, S), (E + dx, S), (E + dx, N)]
    b = [(W + dx, S), (W + dx, N), (E + dx - (5.0 if broken else 0.0), N)]
    island = [(e + dx, n) for e, n in ISLAND + ISLAND[:1]]
    members = [(id * 10 + 1, "outer", a), (id * 10 + 2, "outer", b), (id * 10 + 3, "inner", island)]
    return _relation(id, members, natural="water", water="river", name="Test river")


def pond(id=7):
    """A 7 m pond in EAST: too small for 10 samples 3 m inside."""
    return _way(id, _square(2683150.5, 1247350.5, 7.0), natural="water", water="pond")


def doc(*elements):
    return {"elements": list(elements), "osm3s": {"timestamp_osm_base": VERSION}}


def _area_poly(area):
    return _lv95_polygon(_rect(area.x0, area.y0, area.x1, area.y1))


def _clips(area):
    area_poly = _area_poly(area)
    return {(x, y): _lv95_polygon(_rect(x, y, x + 1, y + 1)).intersection(area_poly) for x, y in area.tiles()}


def _dem_files(tmp_path):
    return [
        SourceFile(f"swissalti3d_2026_{e}-{n}", f"{e}-{n}", "2026", "https://data.geo.admin.ch/t.tif",
                   "https://data.geo.admin.ch/api/stac/v1/items/t", "0" * 64, tmp_path / "t.tif")
        for e, n in (T1, T0)
    ]


def _build(dems, tmp_path, elements):
    bodies = water.parse(doc(*elements), AREA.bbox(), {})
    origins = {t: FRAME.tile_origin_mm(*t) for t in AREA.tiles()}
    stats = {}
    assets, report = build_water(FRAME, dems["T0+T1"], _dem_files(tmp_path), bodies, VERSION, _area_poly(AREA),
                                 _clips(AREA), origins, stats)
    return bodies, assets, report, origins, stats


@pytest.mark.parametrize("tags,reason", [
    ({"natural": "water"}, None),
    # Every `water` value ADR-0014 keeps, spelled out rather than taken
    # from the module, so dropping one fails here.
    *[({"natural": "water", "water": kind}, None)
      for kind in ("river", "canal", "lake", "oxbow", "lagoon", "reservoir", "pond")],
    ({"waterway": "riverbank"}, None),
    ({"landuse": "reservoir"}, None),
    ({"natural": "water", "water": "wastewater"}, "water=wastewater"),
    ({"natural": "water", "amenity": "fountain"}, "amenity=fountain"),
    ({"natural": "water", "amenity": "drinking_water"}, "amenity=drinking_water"),
    ({"natural": "water", "water": "reflecting_pool"}, "water=reflecting_pool"),
    ({"natural": "water", "water": "basin"}, "water=basin"),
    ({"natural": "water", "water": "river", "tunnel": "culvert"}, "tunnel=culvert"),
    ({"waterway": "riverbank", "covered": "yes"}, "covered=yes"),
    ({"landuse": "reservoir", "location": "underground"}, "location=underground"),
])
def test_exclusion_follows_adr_0014(tags, reason):
    assert water.exclusion(tags) == reason


def test_selection_keeps_the_river_with_its_island_and_leaves_out_fountains_and_hidden_water():
    left_out = {
        1: {"amenity": "fountain"},
        2: {"water": "reflecting_pool"},
        3: {"tunnel": "culvert"},
        4: {"covered": "yes"},
        5: {"location": "underground"},
        6: {"water": "basin"},
    }
    elements = [_way(i, _square(2683500.0 + 20 * i, 1247500.0, 10.0), natural="water", **t) for i, t in left_out.items()]
    elements += [pond(), _way(8, _square(2683700.0, 1247500.0, 10.0)[:-1], natural="water"), river()]
    stats = {}
    bodies = water.parse(doc(*reversed(elements)), AREA.bbox(), stats)
    assert [b.ref for b in bodies] == ["relation/10", "way/7"]  # sorted by type and id
    shape = bodies[0].shape
    assert isinstance(shape, Polygon) and len(shape.interiors) == 1
    assert shape.area == pytest.approx(300 * 40 - 19 * 9, rel=1e-4)
    for reason in ("amenity=fountain", "water=reflecting_pool", "tunnel=culvert", "covered=yes",
                   "location=underground", "water=basin"):
        assert stats[f"OSM water areas left out: {reason}"] == 1
    assert stats["OSM water ways not closed or empty"] == 1
    assert stats["OSM water areas"] == 2


def test_rings_join_in_any_order_and_direction_or_fail_loudly():
    a, b, c = [(0.0, 0.0), (1.0, 0.0)], [(1.0, 1.0), (1.0, 0.0)], [(1.0, 1.0), (0.0, 1.0), (0.0, 0.0)]
    (ring,) = join_rings([c, a, b])
    assert ring[0] == ring[-1] and len(ring) == 5
    assert set(ring) == {(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)}
    with pytest.raises(RingError):
        join_rings([a, b])


def test_a_broken_relation_that_reaches_the_area_stops_the_ingest():
    with pytest.raises(water.WaterError, match=r"relation 20 \(Test river\).*closed rings"):
        water.parse(doc(river(20, broken=True)), AREA.bbox(), {})
    stats = {}
    far = river(30, dx=7000.0, broken=True)  # 7 km east of the area
    assert water.parse(doc(far), AREA.bbox(), stats) == []
    assert stats["OSM water relations broken outside the area"] == 1


def _first_way_outline(rel):
    rel["members"][0]["role"] = "outline"


def _every_outer_way_outline(rel):
    for m in rel["members"]:
        if m["role"] == "outer":
            m["role"] = "outline"


def _island_without_geometry(rel):
    rel["members"][2]["geometry"] = []


def _every_way_inner(rel):
    for m in rel["members"]:
        m["role"] = "inner"


@pytest.mark.parametrize("damage,reason", [
    # A role other than outer or inner, on one way or on every outer way:
    # skipping them would drop the river.
    (_first_way_outline, "its member way {}1 has the role 'outline', not outer or inner"),
    (_every_outer_way_outline, "its member way {}1 has the role 'outline', not outer or inner"),
    # A member way without geometry: the island would be filled with water.
    (_island_without_geometry, "its member way {}3 has no geometry"),
    # Only inner rings: no water at all.
    (_every_way_inner, "its member ways enclose no area"),
], ids=["one-role", "every-outer-role", "no-geometry", "no-area"])
def test_a_relation_that_gives_no_true_area_stops_the_ingest(damage, reason):
    rel = river(40)
    damage(rel)
    message = rf"relation 40 \(Test river\) reaches the area, but {reason.format(40)}\. "
    with pytest.raises(water.WaterError, match=message):
        water.parse(doc(rel), AREA.bbox(), {})
    # Far from the area, the same damage is only counted.
    far = river(50, dx=7000.0)
    damage(far)
    stats = {}
    assert water.parse(doc(far), AREA.bbox(), stats) == []
    assert stats["OSM water relations broken outside the area"] == 1


def test_a_pond_on_an_island_in_a_lake_is_water():
    """OSM rings nest: inside an odd number of rings is land, inside an
    even number water again, whichever order the members come in."""
    e, n = 2683500.5, 1247500.5
    lake, island, pond = _square(e, n, 200.0), _square(e + 50, n + 50, 100.0), _square(e + 90, n + 90, 20.0)
    for members in ([(1, "outer", lake), (2, "inner", island), (3, "outer", pond)],
                    [(3, "outer", pond), (2, "inner", island), (1, "outer", lake)]):
        stats = {}
        (body,) = water.parse(doc(_relation(60, members, natural="water", water="lake")), AREA.bbox(), stats)
        assert body.shape.area == pytest.approx(200**2 - 100**2 + 20**2, rel=1e-4)
        assert body.shape.contains(shapely.Point(e + 100, n + 100))  # the pond
        assert not body.shape.contains(shapely.Point(e + 60, n + 60))  # the island
        assert body.shape.contains(shapely.Point(e + 20, n + 20))  # the lake
        assert "OSM water rings whose role does not match their nesting" not in stats
    # An island mapped as an outer ring inside the lake follows its role
    # (water), as before, and is counted.
    stats = {}
    members = [(1, "outer", lake), (2, "outer", island)]
    (body,) = water.parse(doc(_relation(61, members, natural="water")), AREA.bbox(), stats)
    assert body.shape.area == pytest.approx(200**2, rel=1e-4)
    assert stats["OSM water rings whose role does not match their nesting"] == 1


def test_a_self_intersecting_way_is_repaired_drawn_and_counted(dems, tmp_path):
    """ADR-0014: polygons are repaired if invalid. A bow tie becomes its two
    triangles of water."""
    e, n = 2683200.5, 1247350.5
    bow_tie = _way(9, [(e, n), (e + 40, n + 40), (e + 40, n), (e, n + 40), (e, n)], natural="water")
    stats = {}
    (body,) = water.parse(doc(bow_tie), AREA.bbox(), stats)
    assert stats["OSM water areas"] == 1
    assert body.shape.is_valid and body.shape.area == pytest.approx(800.0, abs=0.5)
    _, assets, report, _, _ = _build(dems, tmp_path, [bow_tie])
    (asset,) = assets
    assert asset.tile == EAST and asset.mesh.node(asset.feature_id).primitives
    assert report["area_m2"] == pytest.approx(800.0, abs=0.5)
    assert report["tiles"]["17/68645/45902"]["drawn_m2"] > 800.0
    assert [lv["body"] for lv in report["levels"]] == ["way/9"]


def _star(n, k, e=2683200.5, n0=1247350.5, radius=100.0):
    """A closed star polygon {n/k} of radius 100 m: with k near n/2, each
    edge crosses most others."""
    angle = 2 * np.pi * k * np.arange(n) / n
    points = [(e + radius * float(np.cos(a)), n0 + radius * float(np.sin(a))) for a in angle]
    return points + points[:1]


def _bodensee_like():
    """A ring of 14,540 points (Bodensee's longest joined ring has 14,540)
    round a 43 km circle, with a ragged shore: each point moves up to 41 m
    in or out, which gives about 500 pairs of segments that are not
    neighbours and whose boxes overlap (Bodensee has 503). The shore stays
    simple (every point keeps its angle), so the ring is valid. Returns it
    and the same ring with one defect, a 50 m spike out and back to the
    same point, where the ring touches itself."""
    count = 14_539
    rng = np.random.default_rng(14_540)
    angle = 2 * np.pi * np.arange(count) / count
    r = 43_000.0 + rng.uniform(-41.0, 41.0, count)
    ring = [(2_700_000.5 + float(a * np.cos(t)), 1_250_000.5 + float(a * np.sin(t))) for a, t in zip(r, angle)]
    k = 5_000
    out = (2_700_000.5 + float((r[k] + 50.0) * np.cos(angle[k])), 1_250_000.5 + float((r[k] + 50.0) * np.sin(angle[k])))
    spiked = ring[:k + 1] + [out, ring[k]] + ring[k + 1:]
    return ring + ring[:1], spiked + spiked[:1]


def test_the_repair_bound_counts_pairs_of_segments_that_are_not_neighbours():
    """ADR-0014, open question 1, option E: pairs of segments that are not
    neighbours (the first and the last segment are neighbours too) and whose
    bounding boxes overlap, counted up to the cap plus one."""
    e, n = 2683200.5, 1247350.5
    bow_tie = [(e, n), (e + 40, n + 40), (e + 40, n), (e, n + 40), (e, n)]
    assert water.overlapping_pairs(np.array(bow_tie), 10) == 1  # the two diagonals
    assert water.overlapping_pairs(np.array(_square(e, n, 40.0)), 10) == 0
    # A repeated point makes no segment of its own.
    doubled = bow_tie[:2] + bow_tie[1:]
    assert water.overlapping_pairs(np.array(doubled), 10) == 1
    # The count stops one past the cap.
    star = np.array(_star(500, 249))
    assert water.overlapping_pairs(star, 1000) == 1001
    assert water.overlapping_pairs(star, 5) == 6
    assert water.overlapping_pairs(star, 10**9) > 100_000


@pytest.mark.parametrize("cap,repaired", [(1, True), (0, False)])
def test_a_ring_is_broken_only_above_the_cap(monkeypatch, cap, repaired):
    """A bow tie has one such pair: repaired with a cap of one, broken (a
    way counted as unusable) with a cap of zero."""
    monkeypatch.setattr(water, "MAX_REPAIR_PAIRS", cap)
    e, n = 2683200.5, 1247350.5
    bow_tie = _way(9, [(e, n), (e + 40, n + 40), (e + 40, n), (e, n + 40), (e, n)], natural="water")
    stats = {}
    bodies = water.parse(doc(bow_tie), AREA.bbox(), stats)
    assert len(bodies) == int(repaired)
    assert stats.get("OSM water ways unusable", 0) == int(not repaired)


def test_a_bodensee_like_ring_is_still_repaired():
    """A long real ring with one defect repairs, as ADR-0014 measured for
    Bodensee: here 14,540 points in eight member ways joined into one ring,
    about 500 pairs, and a spike."""
    ring, spiked = _bodensee_like()
    assert Polygon(ring).is_valid and not Polygon(spiked).is_valid
    pairs = water.overlapping_pairs(np.array(spiked), water.MAX_REPAIR_PAIRS)
    assert 400 <= pairs <= 600, pairs
    # Member ways of at most 2,000 points, end to end, as OSM maps a lake.
    ways = [spiked[i:i + 2_000] for i in range(0, len(spiked) - 1, 1_999)]
    assert all(len(w) <= 2_000 for w in ways) and ways[-1][-1] == spiked[0]
    rel = _relation(80, [(800 + i, "outer", w) for i, w in enumerate(ways)], natural="water", water="lake",
                    name="Bodensee-like")
    stats = {}
    start = time.perf_counter()
    (body,) = water.parse(doc(rel), AREA.bbox(), stats)
    seconds = time.perf_counter() - start
    assert body.shape.is_valid
    assert body.shape.area == pytest.approx(Polygon(ring).area, rel=1e-6)
    assert stats["OSM water areas"] == 1
    assert seconds < 30, f"took {seconds:.1f} s"


def _no_repair(geom):
    raise AssertionError("a ring over the cap was sent to make_valid")


def test_a_ring_over_the_cap_is_refused_before_any_repair(monkeypatch):
    """A 500-point star, which GEOS took over 300 s to repair (ADR-0014),
    is refused within the time it takes to count: as a way it is counted as
    unusable; as a relation, joined from two member ways, it stops the
    ingest if it reaches the area and is counted otherwise."""
    monkeypatch.setattr(water, "make_valid", _no_repair)
    star = _star(500, 249)
    assert not Polygon(star).is_valid
    start = time.perf_counter()

    stats = {}
    assert water.parse(doc(_way(70, star, natural="water")), AREA.bbox(), stats) == []
    assert stats["OSM water ways unusable"] == 1

    def relation(id, dx=0.0):
        moved = [(e + dx, n) for e, n in star]
        members = [(id * 10 + 1, "outer", moved[:251]), (id * 10 + 2, "outer", moved[250:])]
        return _relation(id, members, natural="water", water="lake", name="Star lake")

    message = (r"relation 71 \(Star lake\) reaches the area, but a ring of 501 points crosses or touches itself "
               r"too often to be repaired: more than 1000 pairs of its segments that are not neighbours have "
               r"overlapping bounding boxes\. Water is never dropped silently")
    with pytest.raises(water.WaterError, match=message):
        water.parse(doc(relation(71)), AREA.bbox(), {})
    stats = {}
    assert water.parse(doc(relation(72, dx=7000.0)), AREA.bbox(), stats) == []  # 7 km east of the area
    assert stats["OSM water relations broken outside the area"] == 1
    seconds = time.perf_counter() - start
    assert seconds < 10, f"took {seconds:.1f} s"


def test_samples_stay_strictly_inside_their_terrain_tile():
    e, n = water.grid(T0, (2682000.0, 1246000.0, 2685000.0, 1249000.0))
    assert len(e) == 499 * 499
    assert (e.min(), e.max(), n.min(), n.max()) == (2683002.0, 2683998.0, 1247002.0, 1247998.0)
    assert np.all(e % 2 == 0) and np.all(n % 2 == 0)


def _expected_samples(shape, t):
    """2 m grid points strictly inside terrain tile `t`, at least 3 m inside
    `shape`, by exact distances (the ingest uses a negative buffer)."""
    e0 = t[0] * 1000.0
    e, n = np.meshgrid(np.arange(e0 + 2, e0 + 1000, 2.0), np.arange(1247280.0, 1247342.0, 2.0))
    e, n = e.ravel(), n.ravel()
    keep = shapely.contains_xy(shape, e, n) & (shapely.distance(shape.boundary, shapely.points(e, n)) >= 3.0)
    return e[keep], n[keep]


def test_the_level_is_the_median_terrain_3_m_inside_plus_3_cm(dems):
    (body,) = water.parse(doc(river()), AREA.bbox(), {})
    cut = water.pieces(dems["T0+T1"], [body], _area_poly(AREA), {T0, T1}, {})
    levels = {p.terrain_tile: p.level for p in cut}
    assert sorted(levels) == [T1, T0]
    for t, height in ((T0, t0_height), (T1, lambda n: np.full(len(n), T1_HEIGHT))):
        _, n = _expected_samples(body.shape, t)
        assert levels[t].samples == len(n)
        assert levels[t].height_m == pytest.approx(float(np.median(height(n))) + 0.03, abs=1e-4)
    # The island sits in the middle of the river, so the median is the
    # terrain on the river's centre line.
    assert levels[T0].height_m == pytest.approx(405.13, abs=1e-4)


def test_the_level_is_the_median_not_the_mean_over_a_false_ramp(dems):
    """ADR-0014: the median keeps a false ramp, which lifts 40% of the
    samples by 1.3 m, from lifting the water. A mean would rise by half a
    metre."""
    (body,) = water.parse(doc(river()), AREA.bbox(), {})
    cut = water.pieces(dems["ramp"], [body], _area_poly(AREA), {T0, T1}, {})
    (level,) = [p.level for p in cut if p.terrain_tile == T0]
    h = dems["ramp"].height(*_expected_samples(body.shape, T0))
    assert level.samples == len(h)
    assert 0.35 < np.count_nonzero(h > t0_height(1247340.0)) / len(h) < 0.5
    assert level.height_m == pytest.approx(float(np.median(h)) + 0.03, abs=1e-6)
    assert level.height_m - (float(np.mean(h)) + 0.03) < -0.3
    assert level.p90_m > t0_height(1247300.0) + 1.2  # the report shows the ramp in the spread


def test_the_level_does_not_depend_on_the_area_and_the_mesh_only_at_its_edge(dems):
    """EAST alone loads only T0; WEST and EAST load T0 and T1, whose pixels
    meet T0's at the terrain tile edge. EAST's level is the same, and so is
    its mesh where the area covers the whole tile. Where the area's edge
    cuts the tile and the river, as the zurich-altstadt bbox cuts the
    fixture tile, the tile cut to the area depends on how far the area's
    edge runs (its points are spread along it), so the meshes differ, but
    by well under a millimetre."""
    (body,) = water.parse(doc(river()), AREA.bbox(), {})
    # Half tiles: the edge y = 45902.5 crosses the river.
    half, half_area = Area(68645.0, 45902.0, 68646.0, 45902.5), Area(68644.0, 45902.0, 68646.0, 45902.5)
    for areas, same_bytes in (((Area.from_tile(*EAST), AREA), True), ((half, half_area), False)):
        results = []
        for area in areas:
            dem, loaded = (dems["T0"], {T0}) if area.x0 == EAST[0] else (dems["T0+T1"], {T0, T1})
            cut = water.pieces(dem, [body], _area_poly(area), loaded, {})
            (level,) = [p.level for p in cut if p.terrain_tile == T0]
            w = water.tile_water(FRAME, dem, cut, _clips(area)[EAST], FRAME.tile_origin_mm(*EAST), {})
            node = w.mesh.node("tile:17/68645/45902")
            results.append((level, glb.write([node]), _covered(node)))
        assert results[0][0] == results[1][0]
        if same_bytes:
            assert results[0][1] == results[1][1]
        # Both cover the same ground: where the area's edge crosses about
        # 50 m of water, the two edges lie under a millimetre apart.
        a, b = results[0][2], results[1][2]
        assert a.hausdorff_distance(b) < 0.001 and a.symmetric_difference(b).area < 0.05


def _covered(node):
    """The ground a mesh node covers, seen from above (x, z in metres)."""
    (prim,) = node.primitives
    p = np.array(prim.positions_mm, dtype=np.float64)[:, [0, 2]] / 1000.0
    return shapely.union_all([Polygon(p[t]) for t in np.array(prim.indices).reshape(-1, 3)])


def test_water_is_cut_per_tile_and_steps_at_the_terrain_tile_edge(dems, tmp_path):
    (body,), assets, report, origins, stats = _build(dems, tmp_path, [river()])
    clips = _clips(AREA)
    assert [(a.tile, a.layer, a.feature_id, a.collider) for a in assets] == [
        (WEST, "water", "tile:17/68644/45902", "none"),
        (EAST, "water", "tile:17/68645/45902", "none"),
    ]
    for a in assets:
        drawn = body.shape.buffer(1.0).intersection(clips[a.tile]).area
        assert report["tiles"][f"17/{a.tile[0]}/{a.tile[1]}"]["drawn_m2"] == pytest.approx(drawn, abs=0.06)
        (prim,) = a.mesh.node(a.feature_id).primitives
        assert prim.material == "water"
        assert all(n == (0.0, 1.0, 0.0) for n in prim.normals)
        p = (np.array(prim.positions_mm, dtype=np.float64) + origins[a.tile]) / 1000.0
        t = np.array(prim.indices).reshape(-1, 3)
        up = np.cross(p[t[:, 1]] - p[t[:, 0]], p[t[:, 2]] - p[t[:, 0]])[:, 1]
        assert np.all(up > 0)
        assert 0.5 * up.sum() == pytest.approx(drawn, rel=1e-3)
        # Every vertex lies in its own tile (compared in the frame; the
        # outline is taken at the water's height, as the frame leans).
        en = np.asarray(clips[a.tile].exterior.coords)
        outline = Polygon(FRAME.from_lv95(en[:, 0], en[:, 1], 405.0)[:, [0, 2]]).buffer(0.05)
        assert shapely.contains_xy(outline, p[:, 0], p[:, 2]).all()
    # WEST holds both levels: the surface steps at the terrain tile edge.
    (west,) = assets[0].mesh.node("w").primitives
    ys = np.array(west.positions_mm)[:, 1] / 1000.0
    assert ys.max() - ys.min() == pytest.approx(430.03 - 405.13, abs=0.2)
    assert not np.any((ys > ys.min() + 1) & (ys < ys.max() - 1))
    # Water is hidden where T0 rises more than 10 cm above its level: north
    # of N 1247323, a strip 7.5 m wide along the 250.5 m of river in T0. The
    # report scales the share of grid points by the area, and no tile hides
    # more water than it has.
    assert report["hidden_m2"] == pytest.approx(7.5 * 250.5, rel=0.1)
    assert all(v["hidden_m2"] <= v["area_m2"] for v in report["tiles"].values())
    assert report["area_m2"] == pytest.approx(body.shape.area, abs=0.2)
    assert [(lv["swissALTI3D"], lv["level_m"]) for lv in report["levels"]] == [
        ("swissalti3d_2026_2682-1247", 430.03),
        ("swissalti3d_2026_2683-1247", 405.13),
    ]
    assert stats["water triangles dropped (mm snap)"] == 0


def test_hidden_water_is_never_more_than_the_water(dems):
    """A sliver narrower than the 2 m grid can hold a whole column of grid
    points: counting 4 m² for each would hide more water than there is."""
    sliver = shapely.box(2683101.5, 1247300.0, 2683102.5, 1247400.0)  # 1 m by 100 m, on E 2683102
    under = water.Level("way/1", T0, 400.0, 1, 400.0, 400.0)  # below all of T0's terrain
    assert water.hidden_m2(dems["T0"], sliver, under) == pytest.approx(sliver.area)
    over = water.Level("way/1", T0, 410.0, 1, 410.0, 410.0)
    assert water.hidden_m2(dems["T0"], sliver, over) == 0.0


def test_water_with_no_grid_point_3_m_inside_is_not_drawn(dems, tmp_path):
    """ADR-0014 takes a level only from grid points at least 3 m inside the
    water: one is enough, and water with none is left out and reported,
    however many grid points lie nearer its shore."""
    # In EAST: a strip between grid lines, and a 5 m square with four grid
    # points inside but none 3 m inside.
    strip = [(2683281.2, 1247381.2), (2683281.8, 1247381.2), (2683281.8, 1247389.8), (2683281.2, 1247389.8)]
    small = _square(2683260.5, 1247380.5, 5.0)
    elements = [pond(), _way(8, strip + strip[:1], natural="water"), _way(9, small, natural="water")]
    _, assets, report, _, stats = _build(dems, tmp_path, elements)
    # The pond has one grid point 3 m inside: its level is the terrain there.
    (lv,) = report["levels"]
    assert (lv["body"], lv["samples"]) == ("way/7", 1)
    assert lv["level_m"] == pytest.approx(float(t0_height(1247354.0)) + 0.03, abs=1e-3)
    assert report["not_drawn"] == [
        {"body": "way/8", "swissALTI3D": "swissalti3d_2026_2683-1247"},
        {"body": "way/9", "swissALTI3D": "swissalti3d_2026_2683-1247"},
    ]
    assert stats["water pieces with no grid point 3 m inside (not drawn)"] == 2
    (asset,) = assets
    assert asset.sources[0]["source_ids"] == ["way/7"]
    assert report["tiles"]["17/68645/45902"]["area_m2"] == pytest.approx(49.0 + 0.6 * 8.6 + 25.0, abs=0.1)


def test_water_manifests_validate_and_carry_their_sources(dems, tmp_path):
    _, assets, _, origins, _ = _build(dems, tmp_path, [river(), pond()])
    store = Store(tmp_path / "world")
    write_world(store, FRAME, "water-test", assets, origins, [0, 0, 0], 0)
    _, world = store.read_world()
    items = {}
    for t in world["tiles"]:
        index = cjson.loads(store.read_object(t["tile_id"], "json"))
        assert schemas.errors(index) == []
        (entry,) = index["assets"]
        m = cjson.loads(store.read_object(entry["manifest_id"], "json"))
        assert schemas.errors(m) == []
        assert (m["layer"], m["feature_id"], m["physics"], entry["collider"]) == (
            "water", f"tile:{t['tile']}", {"collider": "none"}, "none")
        prov = m["provenance"]
        assert prov["license"] == OSM_ON_TERRAIN_LICENSE == "ODbL-1.0 AND LicenseRef-swisstopo-OGD"
        assert prov["attribution"] == sorted([OSM_CREDIT, SWISSTOPO_CREDIT])
        osm, alti = prov["sources"]
        assert (osm["dataset"], osm["version"], osm["license"]) == ("OpenStreetMap", VERSION, "ODbL-1.0")
        assert (alti["dataset"], alti["version"]) == ("swissALTI3D", "2026")
        items[t["tile"]] = (osm["source_ids"], alti["source_ids"])
    # Only the bodies drawn in a tile, and the terrain tiles that set their levels.
    assert items == {
        "17/68644/45902": (["relation/10"], ["swissalti3d_2026_2682-1247", "swissalti3d_2026_2683-1247"]),
        "17/68645/45902": (["relation/10", "way/7"], ["swissalti3d_2026_2683-1247"]),
    }
    # The schemas refuse a water asset that collides.
    assets[0].collider = "trimesh"
    with pytest.raises(SchemaError):
        write_world(Store(tmp_path / "bad"), FRAME, "water-test", assets, origins, [0, 0, 0], 0)


def test_two_runs_give_identical_bytes(dems, tmp_path):
    runs = []
    for k, elements in enumerate(([river(), pond()], [pond(), river()])):
        _, assets, report, origins, _ = _build(dems, tmp_path, elements)
        store = Store(tmp_path / f"run{k}")
        world_id, _ = write_world(store, FRAME, "water-test", assets, origins, [0, 0, 0], 0)
        files = {p.relative_to(store.root).as_posix(): p.read_bytes() for p in store.root.rglob("*") if p.is_file()}
        runs.append((world_id, files, report))
    assert runs[0] == runs[1]
    # The world, its licence notices, and per tile an index, a manifest and a mesh.
    assert len(runs[0][1]) == 1 + len(NOTICE_FILES) + 2 * 3
