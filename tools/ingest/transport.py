"""Transport layer from OSM highways: draped ribbons and bridge decks.

- Ordinary ways become flat ribbons draped on the terrain, a few
  centimetres above it, cut at tile edges: one visual-only asset per tile
  (`collider: none`; you walk on the terrain underneath).
- Each bridge way becomes its own deck asset (`collider: trimesh`), a slab
  whose top runs between the terrain heights where the bridge meets the
  street network, so the Limmat can be crossed.
"""

from __future__ import annotations

from collections import defaultdict

import numpy as np
from shapely.geometry import LineString, Polygon

from tools.ingest.dem import Dem
from tools.ingest.geo import Frame
from tools.ingest.mesh import MeshBuilder, unit, vertex_normals
from tools.ingest.osm import Way, parse_length_m

# Default carriageway widths (m) for ways without a usable `width` tag.
WIDTH_M = {
    "motorway": 10.0, "trunk": 9.0, "primary": 8.0, "secondary": 7.0, "tertiary": 6.0,
    "motorway_link": 5.0, "trunk_link": 5.0, "primary_link": 5.0, "secondary_link": 5.0,
    "tertiary_link": 5.0, "unclassified": 5.0, "residential": 5.0, "living_street": 4.5,
    "service": 3.5, "busway": 4.0, "road": 4.0, "track": 3.0, "pedestrian": 4.0,
    "footway": 2.0, "path": 1.5, "cycleway": 2.0, "bridleway": 2.0, "steps": 2.0,
}
FOOT = {"pedestrian", "footway", "path", "cycleway", "bridleway", "steps", "track"}
# Height above the terrain. Footpaths sit 1 cm above roads where they overlap.
LIFT_M = {"road": 0.05, "path": 0.06}
MAX_SEGMENT_M = 1.5
DECK_THICKNESS_M = 0.5
MIN_WIDTH_M, MAX_WIDTH_M = 0.5, 30.0
# Bounds on what one (possibly crafted) Overpass way can make the ingest do.
# Real bridges are a few hundred metres long with tens of nodes.
MAX_DECK_POINTS = 20_000
"""Most points along one bridge deck (30 km at MAX_SEGMENT_M)."""
MAX_BRIDGE_NODES = 4_096
"""Most free nodes in one group of connected bridges, whose heights come
from a dense linear solve (memory grows with the square of the count)."""


def is_bridge(w: Way) -> bool:
    return w.tags.get("bridge", "no") != "no"


def exclusion(w: Way) -> str | None:
    """Why a highway way is left out, or None if it is used."""
    t = w.tags
    if t["highway"] not in WIDTH_M:
        return f"highway={t['highway']}"
    if t.get("area") == "yes":
        return "area=yes (squares are not drawn yet)"
    if t.get("tunnel", "no") not in ("no", "building_passage"):
        return "tunnel"
    if t.get("indoor", "no") != "no":
        return "indoor"
    try:
        if int(t.get("layer", "0")) < 0:
            return "layer below ground"
    except ValueError:
        pass
    return None


def material(w: Way) -> str:
    return "path" if w.tags["highway"] in FOOT else "road"


def width_m(w: Way) -> float:
    v = parse_length_m(w.tags.get("width")) or parse_length_m(w.tags.get("est_width"))
    return float(np.clip(v if v else WIDTH_M[w.tags["highway"]], MIN_WIDTH_M, MAX_WIDTH_M))


def densify(en: np.ndarray, max_seg: float = MAX_SEGMENT_M) -> tuple[np.ndarray, np.ndarray]:
    """Inserts points so no segment is longer than `max_seg`. Returns the
    points and, for each, its position as a fractional index into `en`."""
    keep = np.concatenate([[True], np.linalg.norm(np.diff(en, axis=0), axis=1) > 1e-6])
    en = en[keep]
    src = np.flatnonzero(keep).astype(np.float64)
    pts, pos = [en[0]], [src[0]]
    for i in range(len(en) - 1):
        k = max(1, int(np.ceil(np.linalg.norm(en[i + 1] - en[i]) / max_seg)))
        for s in range(1, k + 1):
            t = s / k
            pts.append(en[i] + (en[i + 1] - en[i]) * t)
            pos.append(src[i] + (src[i + 1] - src[i]) * t)
    return np.asarray(pts), np.asarray(pos)


def densified_count(en: np.ndarray, max_seg: float = MAX_SEGMENT_M) -> int:
    """How many points `densify` makes of a polyline, without making them."""
    keep = np.concatenate([[True], np.linalg.norm(np.diff(en, axis=0), axis=1) > 1e-6])
    lengths = np.linalg.norm(np.diff(en[keep], axis=0), axis=1)
    return 1 + int(np.maximum(1, np.ceil(lengths / max_seg)).sum())


def offsets(en: np.ndarray, half_width: float) -> tuple[np.ndarray, np.ndarray]:
    """Left and right edges of a ribbon around a polyline (mitred joins,
    limited to twice the half width)."""
    d = unit(np.diff(en, axis=0))
    left = np.stack([-d[:, 1], d[:, 0]], axis=1)
    n_in = np.vstack([left[:1], left])
    n_out = np.vstack([left, left[-1:]])
    miter = unit(n_in + n_out)
    bad = np.linalg.norm(miter, axis=1) == 0
    miter[bad] = n_out[bad]
    cos = np.einsum("ij,ij->i", miter, n_out)
    scale = half_width / np.maximum(cos, 0.5)
    off = miter * scale[:, None]
    return en + off, en - off


def _strip_triangles(n: int) -> np.ndarray:
    """Triangles of a strip with vertices L0, R0, L1, R1, ... (upward)."""
    i = np.arange(n - 1) * 2
    return np.stack([np.stack([i, i + 1, i + 2], 1), np.stack([i + 2, i + 1, i + 3], 1)], 1).reshape(-1, 3)


def add_ribbon(mesh: MeshBuilder, frame: Frame, dem: Dem, en: np.ndarray, w: Way, stats: dict[str, int]) -> None:
    pts, _ = densify(en)
    if len(pts) < 2:
        return
    left, right = offsets(pts, width_m(w) / 2)
    strip = np.empty((2 * len(pts), 2))
    strip[0::2], strip[1::2] = left, right
    h = dem.height(strip[:, 0], strip[:, 1]) + LIFT_M[material(w)]
    tris = _strip_triangles(len(pts))
    ok = np.all(np.isfinite(h[tris]), axis=1)
    stats["ribbon triangles without terrain"] = stats.get("ribbon triangles without terrain", 0) + int((~ok).sum())
    tris = tris[ok]
    if len(tris) == 0:
        return
    p = frame.from_lv95(strip[:, 0], strip[:, 1], np.nan_to_num(h))
    mesh.add(material(w), p, vertex_normals(p, tris), tris)


def tile_ribbons(frame: Frame, dem: Dem, ways: list[Way], clip: Polygon, origin_mm, stats) -> tuple[MeshBuilder, list[str]]:
    """Ribbons of the (non-bridge) ways inside `clip`, the tile cut to the area."""
    mesh = MeshBuilder(origin_mm)
    used: list[str] = []
    x0, y0, x1, y1 = clip.bounds
    for w in ways:
        if w.en[:, 0].max() < x0 or w.en[:, 0].min() > x1 or w.en[:, 1].max() < y0 or w.en[:, 1].min() > y1:
            continue
        piece = LineString(w.en).intersection(clip)
        lines = [g for g in getattr(piece, "geoms", [piece]) if isinstance(g, LineString) and g.length > 0.2]
        for line in lines:
            add_ribbon(mesh, frame, dem, np.asarray(line.coords)[:, :2], w, stats)
        if lines:
            used.append(w.ref)
    stats["ribbon triangles dropped (mm snap)"] = stats.get("ribbon triangles dropped (mm snap)", 0) + mesh.dropped_triangles
    return mesh, used


def bridge_heights(bridges: list[Way], others: list[Way], dem: Dem, stats: dict[str, int]) -> dict[int, float]:
    """Deck height at every bridge node.

    Nodes where a bridge meets a non-bridge highway, and loose bridge ends,
    are anchored to the terrain. The heights in between are harmonic
    (inverse-length weighted): along a simple chain of bridge ways this is
    linear interpolation by distance.
    """
    pos: dict[int, np.ndarray] = {}
    adj: dict[int, dict[int, float]] = defaultdict(dict)
    for w in bridges:
        for node, p in zip(w.nodes, w.en):
            pos[node] = p
        for a, b, pa, pb in zip(w.nodes, w.nodes[1:], w.en, w.en[1:]):
            if a != b:
                length = max(float(np.linalg.norm(pb - pa)), 0.01)
                adj[a][b] = adj[b][a] = 1.0 / length
    street_nodes = {n for w in others for n in w.nodes}
    ids = sorted(pos)
    ground = dict(zip(ids, dem.height([pos[n][0] for n in ids], [pos[n][1] for n in ids]).tolist()))
    heights: dict[int, float] = {}
    seen: set[int] = set()
    for start in ids:
        if start in seen:
            continue
        comp, stack = [], [start]
        seen.add(start)
        while stack:
            n = stack.pop()
            comp.append(n)
            for m in sorted(adj[n]):
                if m not in seen:
                    seen.add(m)
                    stack.append(m)
        comp.sort()
        anchors = [n for n in comp if (n in street_nodes or len(adj[n]) <= 1) and np.isfinite(ground[n])]
        if not anchors:
            anchors = [n for n in comp if np.isfinite(ground[n])]
            stats["bridges without street connection"] = stats.get("bridges without street connection", 0) + 1
        if not anchors:
            stats["bridges without terrain"] = stats.get("bridges without terrain", 0) + 1
            continue
        fixed = {n: ground[n] for n in anchors}
        free = [n for n in comp if n not in fixed]
        if len(free) > MAX_BRIDGE_NODES:
            stats["bridges too large"] = stats.get("bridges too large", 0) + 1
            continue
        if free:
            index = {n: i for i, n in enumerate(free)}
            a = np.zeros((len(free), len(free)))
            b = np.zeros(len(free))
            for n in free:
                i = index[n]
                for m, wgt in sorted(adj[n].items()):
                    a[i, i] += wgt
                    if m in fixed:
                        b[i] += wgt * fixed[m]
                    else:
                        a[i, index[m]] -= wgt
            for n, h in zip(free, np.linalg.solve(a, b).tolist()):
                fixed[n] = h
        heights.update(fixed)
    return heights


def bridge_deck(frame: Frame, w: Way, heights: dict[int, float], origin_mm) -> MeshBuilder | None:
    if any(n not in heights for n in w.nodes) or densified_count(w.en) > MAX_DECK_POINTS:
        return None
    node_h = np.array([heights[n] for n in w.nodes]) + LIFT_M["road"]
    pts, pos = densify(w.en)
    if len(pts) < 2:
        return None
    h = np.interp(pos, np.arange(len(node_h)), node_h)
    left, right = offsets(pts, width_m(w) / 2)
    lt = frame.from_lv95(left[:, 0], left[:, 1], h)
    rt = frame.from_lv95(right[:, 0], right[:, 1], h)
    lb = frame.from_lv95(left[:, 0], left[:, 1], h - DECK_THICKNESS_M)
    rb = frame.from_lv95(right[:, 0], right[:, 1], h - DECK_THICKNESS_M)

    def quads(a0, a1, b0, b1):
        return np.concatenate([np.stack([a0, b0, a1], 1), np.stack([a1, b0, b1], 1)])

    up = np.array([0.0, 1.0, 0.0])
    outward_left = np.concatenate([lt[:-1] - rt[:-1]] * 2)
    mesh = MeshBuilder(origin_mm)
    mesh.add_flat("bridge", quads(lt[:-1], lt[1:], rt[:-1], rt[1:]), up)
    mesh.add_flat("bridge", quads(lb[:-1], lb[1:], rb[:-1], rb[1:]), -up)
    mesh.add_flat("bridge", quads(lt[:-1], lt[1:], lb[:-1], lb[1:]), outward_left)
    mesh.add_flat("bridge", quads(rt[:-1], rt[1:], rb[:-1], rb[1:]), -outward_left)
    start = quads(lt[:1], rt[:1], lb[:1], rb[:1])
    end = quads(lt[-1:], rt[-1:], lb[-1:], rb[-1:])
    mesh.add_flat("bridge", start, lt[0] - lt[1])
    mesh.add_flat("bridge", end, lt[-1] - lt[-2])
    return None if mesh.is_empty() else mesh
