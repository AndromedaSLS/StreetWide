"""Synthetic stand-ins for the downloaded sources."""

from __future__ import annotations

import numpy as np


class PlaneDem:
    """A terrain whose height is a plane over LV95 (and NaN outside a box)."""

    def __init__(self, h0=400.0, de=0.01, dn=-0.02, box=None):
        self.h0, self.de, self.dn, self.box = h0, de, dn, box

    def height(self, e, n):
        e = np.asarray(e, dtype=np.float64)
        n = np.asarray(n, dtype=np.float64)
        h = self.h0 + self.de * (e - 2683000.0) + self.dn * (n - 1247000.0)
        if self.box is not None:
            e0, n0, e1, n1 = self.box
            h = np.where((e >= e0) & (e <= e1) & (n >= n0) & (n <= n1), h, np.nan)
        return h


def write_dem_tile(path, tile, z, s):
    """A swissALTI3D-like GeoTIFF (LV95, PixelIsArea) for the 1 km tile
    `tile` (LV95 kilometres east, north); `z` has its first row at the north
    edge and pixels of `s` metres."""
    import tifffile

    keys = [1, 1, 0, 3, 1024, 0, 1, 1, 1025, 0, 1, 1, 3072, 0, 1, 2056]
    tags = [
        (33550, "d", 3, (s, s, 0.0), True),
        (33922, "d", 6, (0.0, 0.0, 0.0, tile[0] * 1000.0, tile[1] * 1000.0 + z.shape[0] * s, 0.0), True),
        (34735, "H", len(keys), keys, True),
        (42113, "s", 0, "-9999", True),
    ]
    tifffile.imwrite(path, np.asarray(z, dtype=np.float32), extratags=tags)
    return path


def overpass_geometry(en):
    """LV95 points as an Overpass `geometry` list (7 decimals, like the API)."""
    from tools.ingest.geo import lv95_to_lonlat

    lon, lat = lv95_to_lonlat(np.array([p[0] for p in en], float), np.array([p[1] for p in en], float))
    return [{"lon": round(float(a), 7), "lat": round(float(b), 7)} for a, b in zip(lon, lat)]


def way(id, highway, en, nodes=None, **tags):
    from tools.ingest.osm import Way

    en = np.asarray(en, dtype=np.float64)
    return Way(id, {"highway": highway, **tags}, nodes or list(range(id * 100, id * 100 + len(en))), en)
