"""Tree heights from swissSURFACE3D Raster minus swissALTI3D (ADR-0016,
option A with the 1.5 m rule, policy 1, measured from the anchor).

For each tree, the top is the 0.5 m pixel with the largest value of
surface minus terrain among the pixels whose centres lie within 1.5 m of
the trunk, leaving out pixels within 0.5 m of a swissBUILDINGS3D footprint
(underground buildings are not footprints here: nothing of them shows in
the surface). The height is that pixel's surface minus the ground at the
trunk, where the anchor stands on the terrain mesh (RFC 0004 section 5),
rounded half up to the millimetre. It is None, with a reason, when
- every pixel of the disc is masked or has no data;
- the register's planting year is the surface model's year or later
  (the year of the item that holds the top pixel);
- the height is under 3 m or over 50 m.

Ties go to the first pixel in row-major order (north to south, then west
to east), so the result depends only on the tree and the source tiles
around it, never on the area being ingested. Surface and terrain share
swisstopo's 0.5 m grid; a mosaic on another grid is refused.
"""

from __future__ import annotations

import math
from dataclasses import dataclass, field

import numpy as np
import shapely

from tools.ingest.dem import Dem

RADIUS_M = 1.5
MASK_M = 0.5
MIN_MM, MAX_MM = 3_000, 50_000
NO_PIXEL = "no pixel (every pixel masked or without data)"
PLANTED = "planted in or after the surface model's year"
UNDER = "under 3 m"
OVER = "over 50 m"
REASONS = (NO_PIXEL, PLANTED, UNDER, OVER)


@dataclass
class Height:
    mm: int | None
    reason: str | None
    tiles: set[tuple[int, int]] = field(default_factory=set)
    """The LV95 kilometre tiles (east, north) whose pixels the disc read."""


def disc_tiles(e: float, n: float) -> set[tuple[int, int]]:
    """The LV95 kilometre tiles a tree's disc (and so its heights) can read."""
    r = RADIUS_M
    return {(ke, kn)
            for ke in range(math.floor((e - r) / 1000), math.floor((e + r) / 1000) + 1)
            for kn in range(math.floor((n - r) / 1000), math.floor((n + r) / 1000) + 1)}


def _grid_offset(surface: Dem, terrain: Dem) -> tuple[int, int]:
    if surface.s != terrain.s:
        raise ValueError("the surface and terrain models have different resolutions")
    dc = (surface.e0 - terrain.e0) / surface.s
    dr = (terrain.n1 - surface.n1) / surface.s
    if dc != round(dc) or dr != round(dr):
        raise ValueError("the surface and terrain models are not on one grid")
    return round(dr), round(dc)


def measure(trees: list[tuple[float, float, float, int | None]], surface: Dem | None, terrain: Dem,
            years: dict[tuple[int, int], int], footprints: list) -> list[Height]:
    """Heights of `trees`, each (east, north, ground at the anchor in the
    sources' heights, planting year or None). `years` gives the surface
    item's year per kilometre tile; `footprints` are the swissBUILDINGS3D
    footprints that mask pixels."""
    if surface is None:
        return [Height(None, NO_PIXEL) for _ in trees]
    dr, dc = _grid_offset(surface, terrain)
    s = surface.s
    rows, cols = surface.z.shape
    # Every disc's pixels, in row-major order per tree.
    owner, rr, cc = [], [], []
    for k, (e, n, _, _) in enumerate(trees):
        c0 = math.floor((e - RADIUS_M - surface.e0) / s)
        c1 = math.floor((e + RADIUS_M - surface.e0) / s)
        r0 = math.floor((surface.n1 - (n + RADIUS_M)) / s)
        r1 = math.floor((surface.n1 - (n - RADIUS_M)) / s)
        for r in range(max(r0, 0), min(r1, rows - 1) + 1):
            cy = surface.n1 - (r + 0.5) * s
            for c in range(max(c0, 0), min(c1, cols - 1) + 1):
                cx = surface.e0 + (c + 0.5) * s
                if (cx - e) ** 2 + (cy - n) ** 2 <= RADIUS_M ** 2:
                    owner.append(k)
                    rr.append(r)
                    cc.append(c)
    owner, rr, cc = np.asarray(owner, dtype=np.intp), np.asarray(rr, dtype=np.intp), np.asarray(cc, dtype=np.intp)
    cx = surface.e0 + (cc + 0.5) * s
    cy = surface.n1 - (rr + 0.5) * s
    masked = np.zeros(len(owner), dtype=bool)
    if footprints and len(owner):
        index = shapely.STRtree(footprints)
        hits = index.query(shapely.points(cx, cy), predicate="dwithin", distance=MASK_M)
        masked[hits[0]] = True
    top = surface.z[rr, cc] if len(owner) else np.zeros(0)
    tr, tc = rr + dr, cc + dc
    inside = (tr >= 0) & (tc >= 0) & (tr < terrain.z.shape[0]) & (tc < terrain.z.shape[1])
    ground = np.full(len(owner), np.nan)
    ground[inside] = terrain.z[tr[inside], tc[inside]]
    above = top - ground
    usable = ~masked & np.isfinite(above)
    out = []
    starts = np.searchsorted(owner, np.arange(len(trees) + 1))
    for k, (_, _, anchor_m, planted) in enumerate(trees):
        sl = slice(starts[k], starts[k + 1])
        tiles = {(math.floor(x / 1000), math.floor(y / 1000)) for x, y in zip(cx[sl], cy[sl])}
        ok = np.nonzero(usable[sl])[0]
        if len(ok) == 0:
            out.append(Height(None, NO_PIXEL, tiles))
            continue
        best = ok[int(np.argmax(above[sl][ok]))] + starts[k]
        year = years.get((math.floor(cx[best] / 1000), math.floor(cy[best] / 1000)))
        if planted is not None and year is not None and planted >= year:
            out.append(Height(None, PLANTED, tiles))
            continue
        mm = math.floor((float(top[best]) - anchor_m) * 1000.0 + 0.5)
        if mm < MIN_MM:
            out.append(Height(None, UNDER, tiles))
        elif mm > MAX_MM:
            out.append(Height(None, OVER, tiles))
        else:
            out.append(Height(mm, None, tiles))
    return out
