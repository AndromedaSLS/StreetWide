"""swissALTI3D terrain heights: a mosaic of 0.5 m GeoTIFF tiles in LV95.

Heights are sampled bilinearly between pixel centres. Points whose four
surrounding pixels are not all valid return NaN; callers drop geometry
there rather than guess.
"""

from __future__ import annotations

from pathlib import Path

import numpy as np
import tifffile

_PIXEL_IS_AREA = 1
_LV95 = 2056
_GDAL_NODATA = 42113


class DemError(ValueError):
    """A terrain file is not in the expected form."""


def _read_tile(path: Path) -> tuple[float, float, float, np.ndarray]:
    with tifffile.TiffFile(path) as tif:
        page = tif.pages[0]
        geo = page.geotiff_tags or {}
        if int(geo.get("ProjectedCSTypeGeoKey", 0)) != _LV95:
            raise DemError(f"{path.name}: not in LV95 (EPSG:2056)")
        if int(geo.get("GTRasterTypeGeoKey", 0)) != _PIXEL_IS_AREA:
            raise DemError(f"{path.name}: expected PixelIsArea")
        sx, sy, _ = (float(v) for v in geo["ModelPixelScale"])
        tie = [float(v) for v in geo["ModelTiepoint"]]
        if sx != sy or sx <= 0 or tie[0] != 0 or tie[1] != 0:
            raise DemError(f"{path.name}: unsupported georeferencing")
        z = page.asarray().astype(np.float64)
        if z.ndim != 2:
            raise DemError(f"{path.name}: expected a single-band raster")
        tag = page.tags.get(_GDAL_NODATA)
        if tag is not None:
            z[z == float(str(tag.value).strip())] = np.nan
    return tie[3], tie[4], sx, z


class Dem:
    def __init__(self, paths: list[Path]):
        if not paths:
            raise DemError("no terrain tiles")
        tiles = [_read_tile(p) for p in paths]
        s = tiles[0][2]
        if any(t[2] != s for t in tiles):
            raise DemError("terrain tiles have different resolutions")
        self.s = s
        self.e0 = min(t[0] for t in tiles)
        self.n1 = max(t[1] for t in tiles)
        e1 = max(t[0] + t[3].shape[1] * s for t in tiles)
        n0 = min(t[1] - t[3].shape[0] * s for t in tiles)
        h = round((self.n1 - n0) / s)
        w = round((e1 - self.e0) / s)
        self.z = np.full((h, w), np.nan)
        for e, n, _, z in tiles:
            col = (e - self.e0) / s
            row = (self.n1 - n) / s
            if col != round(col) or row != round(row):
                raise DemError("terrain tiles are not on a common grid")
            r, c = round(row), round(col)
            self.z[r : r + z.shape[0], c : c + z.shape[1]] = z

    def height(self, e, n) -> np.ndarray:
        """Bilinear height at LV95 points; NaN outside the data."""
        e = np.asarray(e, dtype=np.float64)
        n = np.asarray(n, dtype=np.float64)
        fx = (e - self.e0) / self.s - 0.5
        fy = (self.n1 - n) / self.s - 0.5
        j = np.floor(fx)
        i = np.floor(fy)
        tx = fx - j
        ty = fy - i
        rows, cols = self.z.shape
        ok = (i >= 0) & (j >= 0) & (i < rows - 1) & (j < cols - 1)
        ii = np.where(ok, i, 0).astype(np.intp)
        jj = np.where(ok, j, 0).astype(np.intp)
        z00 = self.z[ii, jj]
        z01 = self.z[ii, jj + 1]
        z10 = self.z[ii + 1, jj]
        z11 = self.z[ii + 1, jj + 1]
        top = z00 + (z01 - z00) * tx
        bottom = z10 + (z11 - z10) * tx
        out = top + (bottom - top) * ty
        return np.where(ok, out, np.nan)
