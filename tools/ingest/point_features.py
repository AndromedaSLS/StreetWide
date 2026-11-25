"""Point features (RFC 0004): records placed on the ground and gathered into
one points document per tile and source.

- **Tile:** a point belongs to the tile that contains it, with half-open
  edges as for buildings; points outside the area are left out.
- **Anchor** (RFC 0004 section 5): `x` and `z` are the source position in
  the region frame, `y` the surface of the tile's terrain mesh there,
  interpolated exactly on the mesh's millimetre triangles and rounded half
  up to the millimetre. G8 measures anchors against that mesh, so each
  anchor lies within 1 mm of it. A point with no terrain triangle below
  (no terrain data, or a terrain cell cut by the area's edge) is left out:
  it would fail G8.
- **Yaw** (ADR-0015, rule `adr-0015:yaw-from-orientierung:1`): ewz's
  `orientierung`, read as degrees counter-clockwise from LV95 grid east,
  the direction of the lamp's arm, turned into RFC 0004's yaw (1/65536 of
  a turn clockwise from north in the region frame) and marked as
  estimated. Exactly 0 gives no value.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

import numpy as np

from tools.ingest.geo import Area, Frame, lonlat_to_tilef, lv95_to_lonlat, to_mm
from tools.ingest.mesh import MeshBuilder
from tools.ingest.stadt_zuerich import Record
from tools.worldfmt import points as worldfmt_points

YAW_RULE = "adr-0015:yaw-from-orientierung:1"
NO_TERRAIN = "no terrain under the point"
OUTSIDE = "outside the area"


class Ground:
    """A tile's terrain mesh as anchors meet it: its triangles in
    millimetres relative to the tile origin, as written in its glb."""

    def __init__(self, mesh: MeshBuilder):
        tris = []
        for prim in mesh.node("ground").primitives:
            pos = np.asarray(prim.positions_mm, dtype=np.int64)
            tris.append(pos[np.asarray(prim.indices, dtype=np.int64).reshape(-1, 3)])
        self.tris = np.concatenate(tris) if tris else np.zeros((0, 3, 3), dtype=np.int64)
        self.lo = self.tris.min(axis=1) if len(self.tris) else np.zeros((0, 3), dtype=np.int64)
        self.hi = self.tris.max(axis=1) if len(self.tris) else np.zeros((0, 3), dtype=np.int64)

    def height(self, x: int, z: int) -> int | None:
        """The surface at (x, z) in millimetres, rounded half up, from the
        first triangle (in the mesh's order) whose horizontal projection
        holds the point, edges included, computed exactly as G8 does; None
        if no triangle holds it."""
        near = np.nonzero((self.lo[:, 0] <= x) & (x <= self.hi[:, 0]) & (self.lo[:, 2] <= z) & (z <= self.hi[:, 2]))[0]
        for k in near:
            a, b, c = ([int(v) for v in self.tris[k, i]] for i in range(3))
            d = (b[0] - a[0]) * (c[2] - a[2]) - (c[0] - a[0]) * (b[2] - a[2])
            u = (x - a[0]) * (c[2] - a[2]) - (c[0] - a[0]) * (z - a[2])
            v = (b[0] - a[0]) * (z - a[2]) - (x - a[0]) * (b[2] - a[2])
            if d < 0:
                u, v, d = -u, -v, -d
            if d == 0 or u < 0 or v < 0 or u + v > d:
                continue
            num = a[1] * d + u * (b[1] - a[1]) + v * (c[1] - a[1])
            return (2 * num + d) // (2 * d)
        return None


def yaw_from_orientation(frame: Frame, e: float, n: float, h: float, degrees: float) -> int:
    """Rule `adr-0015:yaw-from-orientierung:1`: `degrees` counter-clockwise
    from LV95 grid east, at (e, n, h), as RFC 0004's yaw: 1/65536 of a turn
    clockwise from north in the region frame (whose north is -z)."""
    t = math.radians(degrees)
    p = frame.from_lv95(np.array([e, e + math.cos(t)]), np.array([n, n + math.sin(t)]), h)
    vx, vz = float(p[1, 0] - p[0, 0]), float(p[1, 2] - p[0, 2])
    return math.floor(math.atan2(vx, -vz) / (2.0 * math.pi) * 65536.0 + 0.5) % 65536


@dataclass
class Placed:
    record: Record
    tile: tuple[int, int]
    anchor_mm: list[int]
    ground_m: float
    """The anchor's ground in the sources' heights (metres), for heights
    measured from the anchor (ADR-0016)."""
    yaw: int | None = None


def _bump(stats: dict[str, int], key: str) -> None:
    stats[key] = stats.get(key, 0) + 1


def place(records: list[Record], frame: Frame, dem, area: Area, origins: dict[tuple[int, int], list[int]],
          grounds: dict[tuple[int, int], Ground], stats: dict[str, int], what: str) -> list[Placed]:
    """The records inside the area, each on its tile's terrain. Records left
    out are counted in `stats` as "<what> left out: <reason>"."""
    if not records:
        return []
    e = np.array([r.e for r in records])
    n = np.array([r.n for r in records])
    lon, lat = lv95_to_lonlat(e, n)
    h = np.asarray(dem.height(e, n), dtype=np.float64)
    pts = frame.from_lonlat(lon, lat, np.nan_to_num(h, nan=0.0))
    mm = to_mm(pts)
    out = []
    for i, r in enumerate(records):
        tx, ty = lonlat_to_tilef(float(lon[i]), float(lat[i]))
        if not area.contains(tx, ty):
            _bump(stats, f"{what} left out: {OUTSIDE}")
            continue
        tile = (int(tx), int(ty))
        ground = grounds.get(tile)
        if not np.isfinite(h[i]) or ground is None:
            _bump(stats, f"{what} left out: {NO_TERRAIN}")
            continue
        x = int(mm[i, 0]) - origins[tile][0]
        z = int(mm[i, 2]) - origins[tile][2]
        y = ground.height(x, z)
        if y is None:
            _bump(stats, f"{what} left out: {NO_TERRAIN}")
            continue
        # The tile origin is at height 0, so y is the frame's up coordinate;
        # near the point, up is the sources' vertical.
        ground_m = float(h[i]) + (y / 1000.0 - float(pts[i, 1]))
        placed = Placed(r, tile, [x, y, z], ground_m)
        o = r.attributes.get("orientation_deg")
        if o is not None:
            placed.yaw = yaw_from_orientation(frame, r.e, r.n, float(h[i]), o)
        out.append(placed)
    return out


def document(kind: str, placed: list[Placed], heights: dict[str, int | None] | None = None) -> dict:
    """The points document of one tile and source (RFC 0004 section 3),
    checked by `tools.worldfmt.points`."""
    pts = []
    for p in sorted(placed, key=lambda q: q.record.point_id.encode()):
        if kind == "tree":
            attributes = dict(p.record.attributes)
            attributes["height_mm"] = (heights or {}).get(p.record.point_id)
            estimated = []
        else:
            attributes = {"count": None, "light_height_mm": None, "method": None, "mount": None, "purpose": None,
                          "yaw": p.yaw}
            estimated = ["yaw"] if p.yaw is not None else []
        pts.append({"id": p.record.point_id, "kind": kind, "anchor_mm": list(p.anchor_mm),
                    "attributes": attributes, "estimated": estimated})
    rules = {"light": {"yaw": YAW_RULE}} if any(q["estimated"] for q in pts) else {}
    doc = {"format": worldfmt_points.FORMAT, "contract_version": worldfmt_points.CONTRACT_VERSION,
           "points": pts, "rules": rules}
    found = worldfmt_points.problems(doc)
    if found:
        raise ValueError(f"points document: {found[:3]}")
    return doc
