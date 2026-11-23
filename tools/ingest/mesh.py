"""Mesh assembly for the StreetWide glb profile.

Geometry arrives in region-frame metres (float64). It leaves as integer
millimetres relative to the tile origin, with unit normals and triangles
that are front-facing (counter-clockwise) when seen from the side their
normal points to. Triangles that collapse when snapped to the millimetre
are dropped, so the physics never sees a degenerate triangle.
"""

from __future__ import annotations

import mapbox_earcut
import numpy as np

from tools.ingest.geo import to_mm
from tools.worldfmt.glb import PALETTE, Node, Primitive


def unit(v: np.ndarray) -> np.ndarray:
    """Normalises rows; zero rows stay zero."""
    n = np.linalg.norm(v, axis=-1, keepdims=True)
    return np.divide(v, n, out=np.zeros_like(v), where=n > 0)


def newell_normal(ring: np.ndarray) -> np.ndarray:
    """Area-weighted normal of a (possibly non-planar) ring, not normalised."""
    nxt = np.roll(ring, -1, axis=0)
    return 0.5 * np.cross(ring, nxt).sum(axis=0)


def triangulate_polygon(rings: list[np.ndarray]) -> tuple[np.ndarray, np.ndarray] | None:
    """Triangulates a planar polygon with holes (rings in metres, shape (n, 3)).

    Returns (points, triangles) with triangles wound counter-clockwise
    around the polygon's Newell normal, or None if it is degenerate.
    """
    ext = rings[0]
    centre = ext.mean(axis=0)
    rings = [r - centre for r in rings]
    n = newell_normal(rings[0])
    area = float(np.linalg.norm(n))
    if not np.isfinite(area) or area < 1e-6:
        return None
    n = n / area
    axis = np.zeros(3)
    axis[int(np.argmin(np.abs(n)))] = 1.0
    u = np.cross(axis, n)
    u /= np.linalg.norm(u)
    v = np.cross(n, u)
    pts = np.concatenate(rings)
    flat = np.stack([pts @ u, pts @ v], axis=1)
    ends = np.cumsum([len(r) for r in rings]).astype(np.uint32)
    idx = np.asarray(mapbox_earcut.triangulate_float64(flat, ends), dtype=np.int64).reshape(-1, 3)
    if len(idx) == 0:
        return None
    a, b, c = flat[idx[:, 0]], flat[idx[:, 1]], flat[idx[:, 2]]
    signed = (b[:, 0] - a[:, 0]) * (c[:, 1] - a[:, 1]) - (b[:, 1] - a[:, 1]) * (c[:, 0] - a[:, 0])
    flip = signed < 0
    idx[flip] = idx[flip][:, [0, 2, 1]]
    return pts + centre, idx


class MeshBuilder:
    """Collects triangles per palette material for one glb node."""

    def __init__(self, origin_mm: list[int]):
        self.origin = np.asarray(origin_mm, dtype=np.int64)
        self._parts: dict[str, list[tuple[np.ndarray, np.ndarray, np.ndarray]]] = {}
        self.dropped_triangles = 0

    def add(self, material: str, points_m: np.ndarray, normals: np.ndarray, tris: np.ndarray) -> None:
        """Adds indexed triangles. `points_m` are region-frame metres."""
        if material not in PALETTE:
            raise ValueError(f"unknown material {material}")
        local = to_mm(points_m) - self.origin
        tris = np.asarray(tris, dtype=np.int64).reshape(-1, 3)
        a, b, c = local[tris[:, 0]], local[tris[:, 1]], local[tris[:, 2]]
        keep = np.any(np.cross(b - a, c - a) != 0, axis=1)
        self.dropped_triangles += int((~keep).sum())
        tris = tris[keep]
        if len(tris) == 0:
            return
        used, remap = np.unique(tris, return_inverse=True)
        nrm = unit(np.asarray(normals, dtype=np.float64))[used]
        if np.any(np.linalg.norm(nrm, axis=1) == 0):
            raise ValueError("zero normal")
        self._parts.setdefault(material, []).append((local[used], nrm, remap.reshape(-1, 3)))

    def add_flat(self, material: str, tri_points_m: np.ndarray, towards: np.ndarray) -> None:
        """Adds separate flat-shaded triangles, shape (t, 3, 3), each turned
        to face `towards` (one vector per triangle, or one for all)."""
        tri = np.array(tri_points_m, dtype=np.float64)
        if len(tri) == 0:
            return
        n = np.cross(tri[:, 1] - tri[:, 0], tri[:, 2] - tri[:, 0])
        towards = np.broadcast_to(np.asarray(towards, dtype=np.float64), n.shape)
        flip = np.einsum("ij,ij->i", n, towards) < 0
        tri[flip] = tri[flip][:, [0, 2, 1]]
        n[flip] = -n[flip]
        ok = np.linalg.norm(n, axis=1) > 1e-12
        tri, n = tri[ok], n[ok]
        pts = tri.reshape(-1, 3)
        normals = np.repeat(unit(n), 3, axis=0)
        self.add(material, pts, normals, np.arange(len(pts)).reshape(-1, 3))

    def add_polygon(self, material: str, rings_m: list[np.ndarray], towards: np.ndarray | None = None) -> bool:
        """Adds a planar polygon with holes, flat-shaded. Returns False if
        it is degenerate. If `towards` is given, the face is turned to it."""
        t = triangulate_polygon(rings_m)
        if t is None:
            return False
        pts, tris = t
        n = unit(newell_normal(rings_m[0] - rings_m[0].mean(axis=0)))
        if towards is not None and float(n @ towards) < 0:
            n = -n
            tris = tris[:, [0, 2, 1]]
        self.add(material, pts, np.broadcast_to(n, pts.shape), tris)
        return True

    def is_empty(self) -> bool:
        return not self._parts

    def node(self, name: str) -> Node:
        prims = []
        for material in PALETTE:
            parts = self._parts.get(material)
            if not parts:
                continue
            positions, normals, indices, base = [], [], [], 0
            for p, n, t in parts:
                positions.append(p)
                normals.append(n)
                indices.append(t + base)
                base += len(p)
            prims.append(
                Primitive(
                    material=material,
                    positions_mm=[tuple(v) for v in np.concatenate(positions).tolist()],
                    normals=[tuple(v) for v in np.concatenate(normals).astype(np.float32).tolist()],
                    indices=np.concatenate(indices).reshape(-1).tolist(),
                )
            )
        return Node(name=name, primitives=prims)


def vertex_normals(points: np.ndarray, tris: np.ndarray, fallback=(0.0, 1.0, 0.0)) -> np.ndarray:
    """Area-weighted vertex normals of an indexed mesh."""
    tris = np.asarray(tris, dtype=np.int64).reshape(-1, 3)
    a, b, c = points[tris[:, 0]], points[tris[:, 1]], points[tris[:, 2]]
    fn = np.cross(b - a, c - a)
    acc = np.zeros_like(points)
    for k in range(3):
        np.add.at(acc, tris[:, k], fn)
    out = unit(acc)
    zero = np.linalg.norm(out, axis=1) == 0
    out[zero] = fallback
    return out
