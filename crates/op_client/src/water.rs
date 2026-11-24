//! Water (ADR-0014): where a tile's water surface is, and the terrain under
//! it, drawn lower for display only.
//!
//! The terrain model carries a river's surface as ground, in places above
//! the water's level (the Limmat's false ramp), where it would hide the
//! water. The client draws flat terrain under the water just below it;
//! banks stay. The simulation never reads these meshes: collision still
//! follows the terrain (RFC 0003).

use std::f64::consts::FRAC_1_SQRT_2;

use op_world::glb::{GlbMesh, Material, Primitive};

use crate::grid::ColumnGrid;

/// Terrain under water is drawn this far below the water surface (m).
pub const SINK_M: f64 = 0.10;

/// How far above the water the terrain model can carry the water's own
/// surface (m). In zurich-altstadt, flat terrain under the Limmat as mapped
/// (at least [`SHORE_M`] inside its surface) rises to 1.33 m above the
/// water along the false ramp, and the Rathausbrücke, which the terrain
/// model carries over the river like ground, lies 2.68 m or more above it.
/// The quay walkway along the Schipfe, on the shore, lies about 1.7 m above
/// it. Terrain over water higher than this is a bridge or a quay: it is not
/// drawn lower, and travel may land on it.
pub const MAX_ABOVE_WATER_M: f64 = 1.6;

/// The ingest buffers each water outline by this much, so that the shore
/// tucks under the banks (ADR-0014). Terrain nearer than this to the edge
/// of the water surface is on the bank, flat or not, and stays.
pub const SHORE_M: f64 = 1.0;

/// Points this close to a triangle count as on it (m). Water and terrain
/// both end at the tile edge with millimetre-snapped vertices, and must
/// agree there on which terrain is under water.
pub const EDGE_TOLERANCE_M: f64 = 0.01;

/// Terrain whose normal is within 10 degrees of vertical lies under the
/// water; steeper terrain is a bank and stays (cosine of 10 degrees).
const FLAT_MIN_NORMAL_Y: f64 = 0.984_807_753;

/// Where to look for the shore around a point: every 45 degrees.
const AROUND: [[f64; 2]; 8] = [
    [1.0, 0.0],
    [FRAC_1_SQRT_2, FRAC_1_SQRT_2],
    [0.0, 1.0],
    [-FRAC_1_SQRT_2, FRAC_1_SQRT_2],
    [-1.0, 0.0],
    [-FRAC_1_SQRT_2, -FRAC_1_SQRT_2],
    [0.0, -1.0],
    [FRAC_1_SQRT_2, -FRAC_1_SQRT_2],
];

/// Whether a surface at height `y` over water at `level` counts as the
/// water: the terrain model's stand-in for its surface, rather than a
/// bridge or a quay above it.
pub fn counts_as_water(y: f64, level: f64) -> bool {
    y <= level + MAX_ABOVE_WATER_M
}

fn triangles<'a>(
    prims: impl Iterator<Item = &'a Primitive> + 'a,
) -> impl Iterator<Item = [[f64; 3]; 3]> + 'a {
    prims.flat_map(|p| {
        p.indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| t.map(|i| p.positions[i as usize].map(f64::from)))
    })
}

fn primitives(mesh: &GlbMesh) -> impl Iterator<Item = &Primitive> {
    mesh.nodes.iter().flat_map(|n| &n.primitives)
}

/// Squared distance from (x, z) to the segment from `a` to `b`, seen from
/// above.
fn distance2_to_segment(x: f64, z: f64, a: [f64; 3], b: [f64; 3]) -> f64 {
    let (dx, dz) = (b[0] - a[0], b[2] - a[2]);
    let len2 = dx * dx + dz * dz;
    let t = if len2 > 0.0 {
        (((x - a[0]) * dx + (z - a[2]) * dz) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (ex, ez) = (x - a[0] - t * dx, z - a[2] - t * dz);
    ex * ex + ez * ez
}

/// The height of a triangle's plane at (x, z), if the point lies inside the
/// triangle seen from above or within [`EDGE_TOLERANCE_M`] of it.
fn level_in_triangle(t: &[[f64; 3]; 3], x: f64, z: f64) -> Option<f64> {
    let [a, b, c] = *t;
    let det = (b[2] - c[2]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[2] - c[2]);
    if det.abs() < 1e-12 {
        return None;
    }
    let w0 = ((b[2] - c[2]) * (x - c[0]) + (c[0] - b[0]) * (z - c[2])) / det;
    let w1 = ((c[2] - a[2]) * (x - c[0]) + (a[0] - c[0]) * (z - c[2])) / det;
    let w2 = 1.0 - w0 - w1;
    let inside = w0 >= -1e-9 && w1 >= -1e-9 && w2 >= -1e-9;
    let tolerance2 = EDGE_TOLERANCE_M * EDGE_TOLERANCE_M;
    let near = || (0..3).any(|i| distance2_to_segment(x, z, t[i], t[(i + 1) % 3]) <= tolerance2);
    (inside || near()).then(|| w0 * a[1] + w1 * b[1] + w2 * c[1])
}

/// The water level at (x, z) in the tile frame of a water mesh, testing
/// every triangle: for single lookups, where a [`WaterSurface`] would cost
/// more to build than it saves.
pub fn level_in_mesh(mesh: &GlbMesh, x: f64, z: f64) -> Option<f64> {
    triangles(primitives(mesh))
        .filter_map(|t| level_in_triangle(&t, x, z))
        .reduce(f64::max)
}

/// Triangles sorted into a grid of cells seen from above (`grid`), so a
/// lookup tests a handful of them. Coordinates are in a tile's frame.
struct TriangleGrid {
    tris: Vec<[[f64; 3]; 3]>,
    /// Per cell: the triangles within [`EDGE_TOLERANCE_M`] of it.
    columns: ColumnGrid,
}

impl TriangleGrid {
    fn new(tris: Vec<[[f64; 3]; 3]>) -> TriangleGrid {
        // Per triangle: x and z extent, widened by the tolerance.
        let extents: Vec<[[f64; 2]; 2]> = tris
            .iter()
            .map(|t| {
                [0, 2].map(|k| {
                    let lo = t.iter().map(|p| p[k]).fold(f64::MAX, f64::min);
                    let hi = t.iter().map(|p| p[k]).fold(f64::MIN, f64::max);
                    [lo - EDGE_TOLERANCE_M, hi + EDGE_TOLERANCE_M]
                })
            })
            .collect();
        TriangleGrid {
            columns: ColumnGrid::new(&extents),
            tris,
        }
    }

    /// The height at (x, z) of the highest triangle the point lies on, seen
    /// from above (within [`EDGE_TOLERANCE_M`]).
    fn height(&self, x: f64, z: f64) -> Option<f64> {
        self.columns
            .at(x, z)?
            .iter()
            .filter_map(|&i| level_in_triangle(&self.tris[i as usize], x, z))
            .reduce(f64::max)
    }
}

/// The water triangles of one tile, for fast lookups. Coordinates are in
/// the tile's frame.
pub struct WaterSurface(TriangleGrid);

impl WaterSurface {
    pub fn new(mesh: &GlbMesh) -> WaterSurface {
        WaterSurface(TriangleGrid::new(triangles(primitives(mesh)).collect()))
    }

    /// The water level at (x, z) in the tile frame, if the point is on the
    /// water.
    pub fn level(&self, x: f64, z: f64) -> Option<f64> {
        self.0.height(x, z)
    }

    /// Whether (x, z) lies under the water as mapped rather than on the
    /// strip of [`SHORE_M`] that the ingest adds round it: the surface goes
    /// on for [`SHORE_M`] all round. `ground` is the tile's terrain: beyond
    /// it lies the next tile, whose water this surface does not hold.
    fn covers(&self, x: f64, z: f64, ground: &TriangleGrid) -> bool {
        AROUND.iter().all(|[dx, dz]| {
            let (px, pz) = (x + SHORE_M * dx, z + SHORE_M * dz);
            self.level(px, pz).is_some() || ground.height(px, pz).is_none()
        })
    }
}

/// A tile's terrain mesh as the client draws it beside the water of the tile
/// (ADR-0014). A terrain vertex moves down to [`SINK_M`] below the water
/// surface there if it lies under the water as mapped, at least [`SHORE_M`]
/// inside the surface; if its normal is within 10 degrees of vertical; and
/// if it lies higher than [`SINK_M`] below the water but still
/// [`counts_as_water`]. Banks, quays and bridges stay. For display only.
pub fn sink_terrain(mesh: &GlbMesh, water: &WaterSurface) -> GlbMesh {
    let is_terrain = |p: &&Primitive| p.material == Material::Terrain;
    let ground = TriangleGrid::new(triangles(primitives(mesh).filter(is_terrain)).collect());
    let mut out = mesh.clone();
    for prim in out.nodes.iter_mut().flat_map(|n| &mut n.primitives) {
        if prim.material != Material::Terrain {
            continue;
        }
        for (p, n) in prim.positions.iter_mut().zip(&prim.normals) {
            let n = n.map(f64::from);
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if len < 1e-12 || n[1] / len < FLAT_MIN_NORMAL_Y {
                continue;
            }
            let (x, y, z) = (f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
            let Some(level) = water.level(x, z) else {
                continue;
            };
            if y > level - SINK_M && counts_as_water(y, level) && water.covers(x, z, &ground) {
                p[1] = (level - SINK_M) as f32;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use op_world::glb::Node;

    /// A water mesh: a 10 m square at 5 m, as two triangles, and a 4 m
    /// square at 7 m further east in a second node.
    fn pond() -> GlbMesh {
        let square = |x0: f32, x1: f32, z0: f32, z1: f32, y: f32| Primitive {
            material: Material::Water,
            positions: vec![[x0, y, z0], [x1, y, z0], [x1, y, z1], [x0, y, z1]],
            normals: vec![[0.0, 1.0, 0.0]; 4],
            // Counter-clockwise seen from above (x east, z south).
            indices: vec![0, 2, 1, 0, 3, 2],
        };
        GlbMesh {
            nodes: vec![
                Node {
                    name: "tile:17/1/1".into(),
                    primitives: vec![square(0.0, 10.0, 0.0, 10.0, 5.0)],
                },
                Node {
                    name: "test:upper-pond".into(),
                    primitives: vec![square(20.0, 24.0, 0.0, 4.0, 7.0)],
                },
            ],
        }
    }

    #[test]
    fn levels_on_the_water_and_just_beside_it() {
        let mesh = pond();
        let surface = WaterSurface::new(&mesh);
        for (x, z, want) in [
            (5.0, 5.0, Some(5.0)),
            // On the diagonal shared by both triangles, and at a corner.
            (3.0, 3.0, Some(5.0)),
            (10.0, 10.0, Some(5.0)),
            (22.0, 1.0, Some(7.0)),
            // 5 mm past the edge counts; 5 cm does not.
            (10.005, 5.0, Some(5.0)),
            (10.05, 5.0, None),
            (5.0, -0.05, None),
            (15.0, 5.0, None),
            (-100.0, 5.0, None),
            (5.0, 1e6, None),
        ] {
            let close = |got: Option<f64>| match (got, want) {
                (Some(a), Some(b)) => (a - b).abs() < 1e-9,
                (a, b) => a == b,
            };
            let (grid, brute) = (surface.level(x, z), level_in_mesh(&mesh, x, z));
            assert!(close(grid), "grid at {x}, {z}: {grid:?}");
            assert!(close(brute), "mesh at {x}, {z}: {brute:?}");
        }
    }

    #[test]
    fn a_surface_without_triangles_has_no_water() {
        let surface = WaterSurface::new(&GlbMesh::default());
        assert_eq!(surface.level(0.0, 0.0), None);
    }

    /// A tile's terrain around [`pond`]: a grid of 0.5 m cells from x = -4
    /// to 22 m and z = -4 to 14 m, so the tile's edge at x = 22 m cuts the
    /// upper pond. It is flat and 1.2 m above the lower pond (6.2 m) west of
    /// x = 18 m, and 0.6 m above the upper pond (7.6 m) east of it: a false
    /// ramp under the water, a quay beside it. `special` sets the height and
    /// normal of single vertices.
    fn terrain(special: &[([f32; 2], f32, [f32; 3])]) -> GlbMesh {
        let (cols, rows) = (53, 37);
        let mut prim = Primitive {
            material: Material::Terrain,
            positions: Vec::new(),
            normals: Vec::new(),
            indices: Vec::new(),
        };
        for r in 0..rows {
            for c in 0..cols {
                let (x, z) = (-4.0 + 0.5 * c as f32, -4.0 + 0.5 * r as f32);
                let (y, n) = special
                    .iter()
                    .find(|s| s.0 == [x, z])
                    .map_or((if x < 18.0 { 6.2 } else { 7.6 }, [0.0, 1.0, 0.0]), |s| {
                        (s.1, s.2)
                    });
                prim.positions.push([x, y, z]);
                prim.normals.push(n);
            }
        }
        for r in 0..rows - 1 {
            for c in 0..cols - 1 {
                let (i, w) = ((r * cols + c) as u32, cols as u32);
                // Counter-clockwise seen from above (x east, z south).
                prim.indices
                    .extend([i, i + w, i + 1, i + 1, i + w, i + w + 1]);
            }
        }
        GlbMesh {
            nodes: vec![Node {
                name: "tile:17/1/1".into(),
                primitives: vec![prim],
            }],
        }
    }

    #[test]
    fn flat_terrain_under_the_water_as_mapped_is_drawn_below_it() {
        let up = [0.0, 1.0, 0.0];
        // Tilted 20 degrees (a bank) and 5 degrees (still flat).
        let bank = [0.0, 0.939_692_6, 0.342_020_1];
        let gentle = [0.0, 0.996_194_7, 0.087_155_7];
        let mesh = terrain(&[
            ([3.0, 2.0], 6.2, gentle),
            ([4.0, 2.0], 5.03, up),
            ([5.0, 2.0], 4.8, up),
            ([6.0, 2.0], 5.5, bank),
            ([7.0, 2.0], 8.0, up),
            ([8.0, 8.0], 6.2, [0.0, -1.0, 0.0]),
        ]);
        let out = sink_terrain(&mesh, &WaterSurface::new(&pond()));
        let (before, after) = (&mesh.nodes[0].primitives[0], &out.nodes[0].primitives[0]);
        assert_eq!(
            (&after.indices, &after.normals),
            (&before.indices, &before.normals)
        );
        let y_at = |x: f32, z: f32| {
            let i = before.positions.iter().position(|p| [p[0], p[2]] == [x, z]);
            after.positions[i.unwrap()][1]
        };
        for (x, z, want) in [
            // The false ramp, 1.2 m above the water, at least 1 m inside it:
            // down to 0.10 m below the water.
            (5.0, 5.0, 4.9),
            (1.0, 1.0, 4.9),
            (9.0, 5.0, 4.9),
            (3.0, 2.0, 4.9),
            // Just above the water: down too. Already below: stays.
            (4.0, 2.0, 4.9),
            (5.0, 2.0, 4.8),
            // A bank, and a bridge the terrain model carries 3 m over the
            // water, stay; downward normals are not flat ground.
            (6.0, 2.0, 5.5),
            (7.0, 2.0, 8.0),
            (8.0, 8.0, 6.2),
            // The quay beside the water, and on the 1 m strip round it that
            // the ingest adds, stays.
            (12.0, 5.0, 6.2),
            (10.0, 5.0, 6.2),
            (9.5, 5.0, 6.2),
            (5.0, 0.5, 6.2),
            (0.5, 9.5, 6.2),
            // The upper pond goes on past the tile's edge at x = 22 m, so
            // there is no shore there to keep clear of.
            (21.0, 2.0, 6.9),
            (22.0, 2.0, 6.9),
            (22.0, 1.0, 6.9),
            (20.5, 2.0, 7.6),
            (22.0, 0.5, 7.6),
        ] {
            let y = y_at(x, z);
            assert!((y - want).abs() < 1e-5, "{x}, {z}: {y}, want {want}");
        }
        // Nothing else moves: only vertices at least 1 m inside a pond.
        for (b, a) in before.positions.iter().zip(&after.positions) {
            let inside = (1.0..=9.0).contains(&b[0]) && (1.0..=9.0).contains(&b[2])
                || (21.0..=22.0).contains(&b[0]) && (1.0..=3.0).contains(&b[2]);
            assert_eq!([a[0], a[2]], [b[0], b[2]]);
            assert!(a[1] == b[1] || inside, "{b:?} moved to {a:?}");
        }
    }

    /// The river's surface in the terrain model, below the water or up to
    /// the top of the Limmat's false ramp, counts as water; a quay walkway
    /// and the Rathausbrücke deck do not.
    #[test]
    fn only_surfaces_near_the_water_count_as_water() {
        assert!(counts_as_water(405.37, 405.41));
        assert!(counts_as_water(405.41 + 1.33, 405.41));
        assert!(!counts_as_water(405.41 + 1.7, 405.41));
        assert!(!counts_as_water(405.41 + 2.68, 405.41));
    }
}
