"""Coordinates: Swiss LV95, WGS84, Web Mercator tiles and the region frame.

RFC 0001 section 1:
- tiles are Web Mercator z17 cells inside z12 regions;
- each region has an East-North-Up frame at the centre of its z12 cell,
  with glTF axes X = east, Y = up, Z = south, in metres;
- heights are the source's heights (LN02/LHN95 for swisstopo), treated as
  heights above the WGS84 ellipsoid.

The LV95 <-> WGS84 transformation is an explicit PROJ pipeline (the
CH1903+ -> WGS84 geocentric translation that PROJ itself uses for
EPSG:2056 -> EPSG:4326), so the result never depends on which datum grids
happen to be installed. Tile maths uses Python's `math` on scalars.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

import numpy as np
from pyproj import Transformer
from pyproj.enums import TransformDirection

TILE_ZOOM = 17
REGION_ZOOM = 12
REGION_SHIFT = TILE_ZOOM - REGION_ZOOM

SOMERC_LV95 = (
    "+proj=somerc +lat_0=46.9524055555556 +lon_0=7.43958333333333 +k_0=1"
    " +x_0=2600000 +y_0=1200000 +ellps=bessel"
)
LV95_TO_WGS84 = (
    f"+proj=pipeline +step +inv {SOMERC_LV95} +step +proj=cart +ellps=bessel"
    " +step +proj=helmert +x=674.374 +y=15.056 +z=405.346"
    " +step +inv +proj=cart +ellps=WGS84"
)

_lv95 = Transformer.from_pipeline(LV95_TO_WGS84)


def lv95_to_lonlat(e, n):
    """LV95 easting/northing (m) -> WGS84 lon/lat (degrees). Arrays or scalars."""
    lon, lat = _lv95.transform(e, n)
    return lon, lat


def lonlat_to_lv95(lon, lat):
    """WGS84 lon/lat (degrees) -> LV95 easting/northing (m)."""
    e, n = _lv95.transform(lon, lat, direction=TransformDirection.INVERSE)
    return e, n


def lonlat_to_tilef(lon: float, lat: float, zoom: int = TILE_ZOOM) -> tuple[float, float]:
    """WGS84 -> fractional Web Mercator tile coordinates (x east, y south)."""
    n = float(1 << zoom)
    x = (lon + 180.0) / 360.0 * n
    y = (1.0 - math.asinh(math.tan(math.radians(lat))) / math.pi) / 2.0 * n
    return x, y


def tilef_to_lonlat(x: float, y: float, zoom: int = TILE_ZOOM) -> tuple[float, float]:
    """Fractional Web Mercator tile coordinates -> WGS84 lon/lat."""
    n = float(1 << zoom)
    lon = x / n * 360.0 - 180.0
    lat = math.degrees(math.atan(math.sinh(math.pi * (1.0 - 2.0 * y / n))))
    return lon, lat


def to_mm(metres) -> np.ndarray:
    """Rounds metres to integer millimetres (half up, so it commutes with
    integer shifts: tiles that share a point agree on it)."""
    return np.floor(np.asarray(metres, dtype=np.float64) * 1000.0 + 0.5).astype(np.int64)


@dataclass(frozen=True)
class BBox:
    minlon: float
    minlat: float
    maxlon: float
    maxlat: float

    @staticmethod
    def parse(text: str) -> BBox:
        parts = [float(p) for p in text.split(",")]
        if len(parts) != 4:
            raise ValueError("bbox must be minlon,minlat,maxlon,maxlat")
        b = BBox(*parts)
        if not (-180 <= b.minlon < b.maxlon <= 180 and -85 <= b.minlat < b.maxlat <= 85):
            raise ValueError(f"invalid bbox {text}")
        return b

    def __str__(self) -> str:
        return f"{self.minlon!r},{self.minlat!r},{self.maxlon!r},{self.maxlat!r}"

    def ring(self) -> list[tuple[float, float]]:
        return [
            (self.minlon, self.minlat),
            (self.maxlon, self.minlat),
            (self.maxlon, self.maxlat),
            (self.minlon, self.maxlat),
        ]


@dataclass(frozen=True)
class Area:
    """The area of interest as a rectangle in fractional z17 tile units.

    Web Mercator is separable, so both a lon/lat bbox and a tile are exact
    rectangles here. `y0` is the north edge. `given` keeps a bbox exactly as
    the user wrote it, so download queries (and their cache keys) use it
    verbatim.
    """

    x0: float
    y0: float
    x1: float
    y1: float
    given: BBox | None = None

    @staticmethod
    def from_bbox(b: BBox) -> Area:
        x0, y1 = lonlat_to_tilef(b.minlon, b.minlat)
        x1, y0 = lonlat_to_tilef(b.maxlon, b.maxlat)
        return Area(x0, y0, x1, y1, b)

    @staticmethod
    def from_tile(x: int, y: int) -> Area:
        return Area(float(x), float(y), float(x + 1), float(y + 1))

    def intersect(self, other: Area) -> Area:
        a = Area(max(self.x0, other.x0), max(self.y0, other.y0), min(self.x1, other.x1), min(self.y1, other.y1))
        if a.x0 >= a.x1 or a.y0 >= a.y1:
            raise ValueError("the areas do not overlap")
        return a

    def contains(self, x: float, y: float) -> bool:
        return self.x0 <= x < self.x1 and self.y0 <= y < self.y1

    def tiles(self) -> list[tuple[int, int]]:
        """Tiles overlapping the area with positive area, sorted by x then y."""
        xs = range(math.floor(self.x0), math.ceil(self.x1))
        ys = range(math.floor(self.y0), math.ceil(self.y1))
        return [(x, y) for x in xs for y in ys]

    def bbox(self) -> BBox:
        if self.given is not None:
            return self.given
        minlon, maxlat = tilef_to_lonlat(self.x0, self.y0)
        maxlon, minlat = tilef_to_lonlat(self.x1, self.y1)
        return BBox(minlon, minlat, maxlon, maxlat)

    def region(self) -> tuple[int, int]:
        regions = {(x >> REGION_SHIFT, y >> REGION_SHIFT) for x, y in self.tiles()}
        if len(regions) != 1:
            raise ValueError(
                "the area crosses a z12 region boundary; world format 0.1 supports one region"
            )
        return regions.pop()


def tile_key(x: int, y: int) -> str:
    return f"{TILE_ZOOM}/{x}/{y}"


def region_key(x: int, y: int) -> str:
    return f"{REGION_ZOOM}/{x}/{y}"


def parse_tile_key(key: str) -> tuple[int, int]:
    z, x, y = (int(p) for p in key.split("/"))
    if z != TILE_ZOOM or not (0 <= x < 1 << z and 0 <= y < 1 << z):
        raise ValueError(f"{key!r} is not a z{TILE_ZOOM} tile key")
    return x, y


class Frame:
    """The East-North-Up frame of one z12 region (RFC 0001 section 1)."""

    def __init__(self, region: tuple[int, int]):
        self.region = region
        lon, lat = tilef_to_lonlat(region[0] + 0.5, region[1] + 0.5, REGION_ZOOM)
        self.anchor_e9 = {"lat": round(lat * 1e9), "lon": round(lon * 1e9)}
        lat0 = self.anchor_e9["lat"] / 1e9
        lon0 = self.anchor_e9["lon"] / 1e9
        self._enu = Transformer.from_pipeline(
            "+proj=pipeline +step +proj=cart +ellps=WGS84"
            f" +step +proj=topocentric +ellps=WGS84 +lat_0={lat0!r} +lon_0={lon0!r} +h_0=0"
        )

    @property
    def key(self) -> str:
        return region_key(*self.region)

    def from_lonlat(self, lon, lat, h) -> np.ndarray:
        """WGS84 lon/lat/height -> frame metres, shape (n, 3)."""
        lon = np.atleast_1d(np.asarray(lon, dtype=np.float64))
        lat = np.atleast_1d(np.asarray(lat, dtype=np.float64))
        h = np.broadcast_to(np.asarray(h, dtype=np.float64), lon.shape)
        e, n, u = self._enu.transform(lon, lat, h)
        return np.stack([np.asarray(e), np.asarray(u), -np.asarray(n)], axis=-1)

    def from_lv95(self, e, n, h) -> np.ndarray:
        lon, lat = lv95_to_lonlat(np.asarray(e, dtype=np.float64), np.asarray(n, dtype=np.float64))
        return self.from_lonlat(lon, lat, h)

    def tile_origin_mm(self, x: int, y: int) -> list[int]:
        """Tile centre at height 0, in region-frame millimetres, with y = 0."""
        lon, lat = tilef_to_lonlat(x + 0.5, y + 0.5)
        p = to_mm(self.from_lonlat(lon, lat, 0.0))[0]
        return [int(p[0]), 0, int(p[2])]
