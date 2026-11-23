"""Terrain: one regular grid mesh per tile from swissALTI3D.

The grid is defined in Web Mercator: each z17 tile is split into
CELLS x CELLS cells (z24 pixels, about 1.6 m in Zurich). A vertex is the
point at an integer z24 pixel corner, so tiles that share an edge compute
exactly the same vertices there and the terrain has no cracks. Normals are
taken from the neighbouring grid points, including those in the next tile,
so shading is continuous across tile edges too.
"""

from __future__ import annotations

import numpy as np

from tools.ingest.dem import Dem
from tools.ingest.geo import Area, Frame, TILE_ZOOM, lonlat_to_lv95, tilef_to_lonlat
from tools.ingest.mesh import MeshBuilder, unit

CELLS = 128
_SUB_ZOOM = TILE_ZOOM + 7  # 2^7 = CELLS


def _grid(frame: Frame, dem: Dem, tx: int, ty: int) -> np.ndarray:
    """Region-frame points of the tile's grid plus a one-cell border,
    shape (CELLS + 3, CELLS + 3, 3); NaN where there is no terrain data."""
    gx = np.arange(tx * CELLS - 1, (tx + 1) * CELLS + 2)
    gy = np.arange(ty * CELLS - 1, (ty + 1) * CELLS + 2)
    lons = np.array([tilef_to_lonlat(float(x), 0.0, _SUB_ZOOM)[0] for x in gx])
    lats = np.array([tilef_to_lonlat(0.0, float(y), _SUB_ZOOM)[1] for y in gy])
    lon, lat = np.meshgrid(lons, lats)
    e, n = lonlat_to_lv95(lon.ravel(), lat.ravel())
    h = dem.height(e, n)
    pts = frame.from_lonlat(lon.ravel(), lat.ravel(), np.nan_to_num(h, nan=0.0))
    pts[np.isnan(h)] = np.nan
    return pts.reshape(len(gy), len(gx), 3)


def _normals(p: np.ndarray) -> np.ndarray:
    """Normals of the inner grid points from central differences, falling
    back to one-sided differences next to missing data."""
    c = p[1:-1, 1:-1]

    def diff(plus: np.ndarray, minus: np.ndarray) -> np.ndarray:
        plus = np.where(np.isnan(plus), c, plus)
        minus = np.where(np.isnan(minus), c, minus)
        return plus - minus

    along_x = diff(p[1:-1, 2:], p[1:-1, :-2])  # towards east
    along_z = diff(p[2:, 1:-1], p[:-2, 1:-1])  # towards south
    n = unit(np.cross(along_z, along_x))
    n[np.linalg.norm(n, axis=-1) == 0] = (0.0, 1.0, 0.0)
    return n


def build(frame: Frame, dem: Dem, area: Area, tile: tuple[int, int], origin_mm: list[int], stats: dict[str, int]):
    """The terrain mesh of one tile, clipped to the area. None if empty."""
    tx, ty = tile
    p = _grid(frame, dem, tx, ty)
    normals = _normals(p)
    inner = p[1:-1, 1:-1]
    size = CELLS + 1
    gx = np.arange(tx * CELLS, (tx + 1) * CELLS)
    gy = np.arange(ty * CELLS, (ty + 1) * CELLS)
    in_x = (gx >= area.x0 * CELLS) & (gx + 1 <= area.x1 * CELLS)
    in_y = (gy >= area.y0 * CELLS) & (gy + 1 <= area.y1 * CELLS)
    valid = ~np.isnan(inner[..., 1])
    cell_ok = (
        in_y[:, None]
        & in_x[None, :]
        & valid[:-1, :-1]
        & valid[:-1, 1:]
        & valid[1:, :-1]
        & valid[1:, 1:]
    )
    stats["terrain cells without data"] = stats.get("terrain cells without data", 0) + int(
        (in_y[:, None] & in_x[None, :] & ~cell_ok).sum()
    )
    rows, cols = np.nonzero(cell_ok)
    if len(rows) == 0:
        return None
    nw = rows * size + cols
    ne = nw + 1
    sw = nw + size
    se = sw + 1
    # Counter-clockwise seen from above (+Y): NW, SW, NE and NE, SW, SE.
    tris = np.stack([np.stack([nw, sw, ne], 1), np.stack([ne, sw, se], 1)], 1).reshape(-1, 3)
    mesh = MeshBuilder(origin_mm)
    flat_pts = np.nan_to_num(inner.reshape(-1, 3))
    mesh.add("terrain", flat_pts, normals.reshape(-1, 3), tris)
    stats["terrain triangles dropped"] = stats.get("terrain triangles dropped", 0) + mesh.dropped_triangles
    return mesh
