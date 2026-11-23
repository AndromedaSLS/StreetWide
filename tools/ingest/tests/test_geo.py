import json
import math
from pathlib import Path

import numpy as np
import pytest

from tools.ingest.geo import Area, BBox, Frame, lonlat_to_lv95, lonlat_to_tilef, lv95_to_lonlat, tilef_to_lonlat, to_mm

REPO = Path(__file__).resolve().parents[3]
ZURICH = BBox(8.5377, 47.3687, 8.5507, 47.3774)


def test_lv95_matches_swisstopo_stac_footprints():
    # Corners of the swissALTI3D tile 2683-1247 as published in its STAC item.
    for (e, n), (lon, lat) in [
        ((2683000, 1247000), (8.5375049, 47.3686138)),
        ((2684000, 1248000), (8.5509303, 47.3774805)),
    ]:
        got = lv95_to_lonlat(e, n)
        assert got == pytest.approx((lon, lat), abs=1e-7)


def test_lv95_round_trip_is_within_two_millimetres():
    # The 2D transform evaluates height 0 on the Bessel ellipsoid one way
    # and on WGS84 the other way; that costs about 1 mm, far below the
    # accuracy of any of the data.
    e = np.array([2683000.0, 2683512.3, 2683999.9])
    n = np.array([1247000.0, 1247488.8, 1247999.9])
    e2, n2 = lonlat_to_lv95(*lv95_to_lonlat(e, n))
    assert np.max(np.abs(e2 - e)) < 2e-3 and np.max(np.abs(n2 - n)) < 2e-3


def test_tile_maths_round_trip():
    x, y = lonlat_to_tilef(8.54397, 47.37017)
    assert (int(x), int(y)) == (68646, 45903)
    assert tilef_to_lonlat(x, y) == pytest.approx((8.54397, 47.37017), abs=1e-12)


def _enu_reference(lat0, lon0, lat, lon, h):
    """Independent WGS84 geodetic -> ECEF -> ENU, written out by hand."""
    a, f = 6378137.0, 1 / 298.257223563
    e2 = f * (2 - f)

    def ecef(p, lam, hh):
        p, lam = math.radians(p), math.radians(lam)
        nn = a / math.sqrt(1 - e2 * math.sin(p) ** 2)
        return ((nn + hh) * math.cos(p) * math.cos(lam), (nn + hh) * math.cos(p) * math.sin(lam), (nn * (1 - e2) + hh) * math.sin(p))

    x0, x = ecef(lat0, lon0, 0.0), ecef(lat, lon, h)
    d = [x[i] - x0[i] for i in range(3)]
    p, lam = math.radians(lat0), math.radians(lon0)
    east = -math.sin(lam) * d[0] + math.cos(lam) * d[1]
    north = -math.sin(p) * math.cos(lam) * d[0] - math.sin(p) * math.sin(lam) * d[1] + math.cos(p) * d[2]
    up = math.cos(p) * math.cos(lam) * d[0] + math.cos(p) * math.sin(lam) * d[1] + math.sin(p) * d[2]
    return east, up, -north


def test_frame_matches_hand_written_enu():
    frame = Frame((2145, 1434))
    lat0, lon0 = frame.anchor_e9["lat"] / 1e9, frame.anchor_e9["lon"] / 1e9
    got = frame.from_lonlat(8.5409, 47.3729, 431.5)[0]
    assert got == pytest.approx(_enu_reference(lat0, lon0, 47.3729, 8.5409, 431.5), abs=1e-6)


def test_format_fixture_region_and_origin_follow_rfc_0001():
    """The stage-2 golden fixture uses exactly what the ingest computes."""
    world = json.loads((REPO / "fixtures/format/world/world.json").read_text(encoding="utf-8"))
    frame = Frame((2145, 1434))
    assert world["region"] == {"key": "12/2145/1434", "anchor_e9": frame.anchor_e9}
    tile = world["tiles"][0]
    assert tile["tile"] == "17/68646/45908"
    assert tile["origin_mm"] == frame.tile_origin_mm(68646, 45908)


def test_zurich_area():
    area = Area.from_bbox(ZURICH)
    assert len(area.tiles()) == 30
    assert area.region() == (2145, 1434)
    assert area.bbox() == ZURICH  # queries use the bbox exactly as given
    fixture = Area.from_tile(68646, 45903).intersect(area)
    assert fixture.tiles() == [(68646, 45903)]
    assert fixture.y1 == area.y1


def test_to_mm_rounds_half_up():
    assert to_mm([0.0005, -0.0005, 1.2344, -1.2346]).tolist() == [1, 0, 1234, -1235]
