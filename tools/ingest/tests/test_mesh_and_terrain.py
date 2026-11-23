import numpy as np
import pytest

from tools.ingest import terrain
from tools.ingest.geo import Area, Frame
from tools.ingest.mesh import MeshBuilder, triangulate_polygon
from tools.ingest.tests.helpers import PlaneDem
from tools.worldfmt import glb

FRAME = Frame((2145, 1434))


def _area(pts, tris):
    a, b, c = pts[tris[:, 0]], pts[tris[:, 1]], pts[tris[:, 2]]
    return np.cross(b - a, c - a)


def test_polygon_with_hole_keeps_area_and_winding():
    outer = np.array([[0, 0, 0], [10, 0, 0], [10, 0, -10], [0, 0, -10]], float)  # CCW seen from +Y
    hole = np.array([[3, 0, -3], [3, 0, -7], [7, 0, -7], [7, 0, -3]], float)
    pts, tris = triangulate_polygon([outer, hole])
    cross = _area(pts, tris)
    assert np.all(cross[:, 1] > 0)
    assert 0.5 * cross[:, 1].sum() == pytest.approx(100 - 16)


def test_degenerate_polygon_is_rejected():
    assert triangulate_polygon([np.array([[0, 0, 0], [1, 0, 0], [2, 0, 0]], float)]) is None


def test_builder_drops_triangles_that_collapse_at_the_millimetre():
    m = MeshBuilder([1000, 0, 0])
    pts = np.array([[1.0, 0, 0], [2.0, 0, 0], [1.0002, 0.0001, 0], [1, 0, 1], [2, 0, 1]])
    m.add("road", pts, np.tile([0.0, 1.0, 0.0], (5, 1)), [[0, 1, 2], [0, 3, 1], [1, 3, 4]])
    assert m.dropped_triangles == 1
    node = m.node("tile:17/1/1")
    (prim,) = node.primitives
    assert len(prim.indices) == 6 and len(prim.positions_mm) == 4
    assert prim.positions_mm[0] == (0, 0, 0)  # relative to the origin
    glb.write([node])  # inside the glb profile


def test_add_flat_turns_faces_towards_the_given_side():
    m = MeshBuilder([0, 0, 0])
    tri = np.array([[[0, 0, 0], [1, 0, 0], [0, 0, 1]]], float)  # faces down as given
    m.add_flat("roof", tri, [0, 1, 0])
    (prim,) = m.node("x:1").primitives
    assert prim.normals[0] == pytest.approx((0, 1, 0))
    p = np.array(prim.positions_mm, float)
    i = prim.indices
    assert np.cross(p[i[1]] - p[i[0]], p[i[2]] - p[i[0]])[1] > 0


def _region_points(tile):
    origin = FRAME.tile_origin_mm(*tile)
    two_tiles = Area(68645.0, 45902.0, 68647.0, 45903.0)
    mesh = terrain.build(FRAME, PlaneDem(), two_tiles, tile, origin, {})
    (prim,) = mesh.node("t:1").primitives
    return {tuple(np.add(v, origin)) for v in prim.positions_mm}, prim


def test_neighbouring_terrain_tiles_share_their_edge_exactly():
    left, _ = _region_points((68645, 45902))
    right, _ = _region_points((68646, 45902))
    shared = left & right
    assert len(shared) == terrain.CELLS + 1  # the whole common edge, no cracks


def test_terrain_is_a_full_upward_grid():
    _, prim = _region_points((68645, 45902))
    assert len(prim.positions_mm) == (terrain.CELLS + 1) ** 2
    assert len(prim.indices) == 6 * terrain.CELLS**2
    assert min(n[1] for n in prim.normals) > 0.99
    p = np.array(prim.positions_mm, float)
    t = np.array(prim.indices).reshape(-1, 3)
    assert np.all(np.cross(p[t[:, 1]] - p[t[:, 0]], p[t[:, 2]] - p[t[:, 0]])[:, 1] > 0)


def test_terrain_skips_cells_without_data():
    tile = (68645, 45902)
    stats = {}
    dem = PlaneDem(box=(2683000, 1247000, 2683150, 1248000))  # covers part of the tile
    mesh = terrain.build(FRAME, dem, Area(68645.0, 45902.0, 68646.0, 45903.0), tile, FRAME.tile_origin_mm(*tile), stats)
    (prim,) = mesh.node("t:1").primitives
    assert 0 < len(prim.indices) < 6 * terrain.CELLS**2
    assert stats["terrain cells without data"] > 0
