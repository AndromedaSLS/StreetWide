"""Tree heights by ADR-0016's rule, on synthetic surface and terrain tiles."""

from __future__ import annotations

from pathlib import Path

import numpy as np
from shapely.geometry import box

from tools.ingest import heights
from tools.ingest.dem import Dem
from tools.ingest.tests.helpers import write_dem_tile

TILE = (2683, 1247)
S = 0.5
GROUND = 400.0


def tiles(tmp_path: Path, bumps: dict[tuple[float, float], float], tile=TILE, name="t"):
    """A flat terrain tile at 400 m and a surface tile with `bumps`: the
    pixel whose centre is at (east, north) stands that many metres higher."""
    n = int(1000 / S)
    terrain = np.full((n, n), GROUND, dtype=np.float32)
    surface = terrain.copy()
    for (e, nn), dh in bumps.items():
        col = int((e - tile[0] * 1000) / S)
        row = int((tile[1] * 1000 + 1000 - nn) / S)
        surface[row, col] = GROUND + dh
    t = write_dem_tile(tmp_path / f"{name}-terrain.tif", tile, terrain, S)
    s = write_dem_tile(tmp_path / f"{name}-surface.tif", tile, surface, S)
    return t, s


def at(e, n):
    """The centre of the pixel holding (e, n)."""
    return (np.floor(e / S) + 0.5) * S, (np.floor(n / S) + 0.5) * S


def measure(tmp_path, bumps, trees, footprints=(), year=2024, anchor=GROUND):
    t, s = tiles(tmp_path, bumps)
    return heights.measure([(e, n, anchor, planted) for e, n, planted in trees], Dem([s]), Dem([t]),
                           {TILE: year}, list(footprints))


def test_the_highest_pixel_within_1_5_m_from_the_anchor(tmp_path: Path):
    e, n = 2683500.1, 1247500.2
    bumps = {at(e + 1.0, n): 12.3456, at(e - 0.5, n + 0.5): 9.0, at(e + 2.0, n): 30.0}  # the last is too far
    (h,) = measure(tmp_path, bumps, [(e, n, 1990)])
    assert h.mm == 12346 and h.reason is None and h.tiles == {TILE}
    # Measured from the anchor: a lower anchor gives a taller tree.
    (h,) = measure(tmp_path, bumps, [(e, n, 1990)], anchor=GROUND - 0.25)
    assert h.mm == 12596


def test_buildings_mask_pixels_within_0_5_m(tmp_path: Path):
    e, n = 2683500.1, 1247500.2
    top = at(e + 1.0, n)
    bumps = {top: 20.0, at(e - 0.5, n + 0.5): 9.0}
    # A footprint 0.5 m from the top pixel's centre masks it; one 0.6 m away does not.
    near = box(top[0] + 0.5, top[1] - 5, top[0] + 10, top[1] + 5)
    far = box(top[0] + 0.6, top[1] - 5, top[0] + 10, top[1] + 5)
    assert measure(tmp_path, bumps, [(e, n, 1990)], [near])[0].mm == 9000
    assert measure(tmp_path, bumps, [(e, n, 1990)], [far])[0].mm == 20000
    everything = box(e - 5, n - 5, e + 5, n + 5)
    (h,) = measure(tmp_path, bumps, [(e, n, 1990)], [everything])
    assert (h.mm, h.reason) == (None, heights.NO_PIXEL)


def test_null_heights_and_their_reasons(tmp_path: Path):
    e, n = 2683500.1, 1247500.2
    low = measure(tmp_path, {at(e, n): 2.9994}, [(e, n, 1990)])[0]
    assert (low.mm, low.reason) == (None, heights.UNDER)
    floor = measure(tmp_path, {at(e, n): 2.9996}, [(e, n, 1990)])[0]
    assert floor.mm == 3000  # rounded half up to the millimetre first
    high = measure(tmp_path, {at(e, n): 50.001}, [(e, n, 1990)])[0]
    assert (high.mm, high.reason) == (None, heights.OVER)
    assert measure(tmp_path, {at(e, n): 50.0}, [(e, n, 1990)])[0].mm == 50000
    planted = measure(tmp_path, {at(e, n): 10.0}, [(e, n, 2024), (e, n, 2023), (e, n, None)], year=2024)
    assert [(h.mm, h.reason) for h in planted] == [(None, heights.PLANTED), (10000, None), (10000, None)]
    # No surface data at all.
    t, _ = tiles(tmp_path, {})
    (none,) = heights.measure([(e, n, GROUND, 1990)], None, Dem([t]), {}, [])
    assert (none.mm, none.reason) == (None, heights.NO_PIXEL)


def test_ties_go_to_the_first_pixel_north_to_south_then_west_to_east(tmp_path: Path):
    e, n = 2683500.25, 1247500.25  # a pixel centre
    bumps = {(e + 0.5, n): 10.0, (e - 0.5, n): 10.0, (e, n - 1.0): 10.0, (e, n + 1.0): 10.0}
    (h,) = measure(tmp_path, bumps, [(e, n, 1990)])
    assert h.mm == 10000
    # Same heights, different surfaces: the result depends only on the
    # tree, never on the other trees measured with it.
    alone = measure(tmp_path, bumps, [(e, n, 1990)])
    together = measure(tmp_path, bumps, [(e + 100, n, 1990), (e, n, 1990), (e - 300, n - 300, 1990)])
    assert together[1].mm == alone[0].mm


def test_a_disc_reads_both_tiles_across_an_edge(tmp_path: Path):
    e, n = 2684000.4, 1247500.25  # 0.4 m east of the edge between 2683 and 2684
    west_t, west_s = tiles(tmp_path, {(2683999.75, 1247500.25): 6.0}, name="w")
    east_t, east_s = tiles(tmp_path, {(2684001.25, 1247500.25): 7.0}, tile=(2684, 1247), name="e")
    (h,) = heights.measure([(e, n, GROUND, 1990)], Dem([west_s, east_s]), Dem([west_t, east_t]),
                           {(2683, 1247): 2024, (2684, 1247): 2024}, [])
    assert h.mm == 7000 and h.tiles == {(2683, 1247), (2684, 1247)}
    assert heights.disc_tiles(e, n) == {(2683, 1247), (2684, 1247)}
    assert heights.disc_tiles(2683500.0, 1247500.0) == {TILE}
