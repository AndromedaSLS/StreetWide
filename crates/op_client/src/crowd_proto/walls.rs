//! Building shells as walls, and the ground under the lanes: what the
//! crowd prototype's lanes are cleared against (`lanes`), and what its
//! check that nobody stands inside a shell measures (`bench`).
//!
//! A wall is a shell triangle that is roughly vertical. At a height, the
//! walls cut by a horizontal plane give line segments; a point's signed
//! distance to the nearest one says whether it is inside the shell (the
//! triangle's front face looks out, as glTF winds it). Client code: floats
//! are fine here, as in the ingest this stands in for.

use op_stream::WorldIndex;
use op_world::{Collider, Layer};

use crate::grid::ColumnGrid;
use crate::travel::height_in_triangle;

/// Grid cell (m).
const CELL: f64 = 2.0;
/// Triangles whose normal is at most this steep count as walls (|n.y|).
const WALL_MAX_NY: f64 = 0.7;

/// A uniform grid over the walls' extent; each cell lists the triangles
/// that touch it.
struct Grid {
    min: [f64; 2],
    cols: usize,
    rows: usize,
    starts: Vec<u32>,
    items: Vec<u32>,
}

impl Grid {
    fn new(extents: &[[f64; 4]]) -> Grid {
        let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
        for e in extents {
            lo = [lo[0].min(e[0]), lo[1].min(e[1])];
            hi = [hi[0].max(e[2]), hi[1].max(e[3])];
        }
        if extents.is_empty() {
            return Grid {
                min: [0.0; 2],
                cols: 0,
                rows: 0,
                starts: vec![0],
                items: Vec::new(),
            };
        }
        let cols = ((hi[0] - lo[0]) / CELL).ceil() as usize + 1;
        let rows = ((hi[1] - lo[1]) / CELL).ceil() as usize + 1;
        let cell = |v: f64, k: usize, n: usize| {
            (((v - lo[k]) / CELL).floor().max(0.0) as usize).min(n - 1)
        };
        let mut counts = vec![0u32; cols * rows + 1];
        let range = |e: &[f64; 4]| {
            let (c0, c1) = (cell(e[0], 0, cols), cell(e[2], 0, cols));
            let (r0, r1) = (cell(e[1], 1, rows), cell(e[3], 1, rows));
            (r0..=r1).flat_map(move |r| (c0..=c1).map(move |c| r * cols + c))
        };
        for e in extents {
            for c in range(e) {
                counts[c + 1] += 1;
            }
        }
        for i in 1..counts.len() {
            counts[i] += counts[i - 1];
        }
        let mut next = counts.clone();
        let mut items = vec![0u32; counts[cols * rows] as usize];
        for (id, e) in extents.iter().enumerate() {
            for c in range(e) {
                items[next[c] as usize] = id as u32;
                next[c] += 1;
            }
        }
        Grid {
            min: lo,
            cols,
            rows,
            starts: counts,
            items,
        }
    }

    /// Items in the cells that touch the box, possibly repeated.
    fn query(&self, x0: f64, z0: f64, x1: f64, z1: f64, out: &mut Vec<u32>) {
        out.clear();
        if self.cols == 0 {
            return;
        }
        let c0 = ((x0 - self.min[0]) / CELL).floor();
        let c1 = ((x1 - self.min[0]) / CELL).floor();
        let r0 = ((z0 - self.min[1]) / CELL).floor();
        let r1 = ((z1 - self.min[1]) / CELL).floor();
        if c1 < 0.0 || r1 < 0.0 || c0 >= self.cols as f64 || r0 >= self.rows as f64 {
            return;
        }
        let (c0, c1) = (c0.max(0.0) as usize, (c1 as usize).min(self.cols - 1));
        let (r0, r1) = (r0.max(0.0) as usize, (r1 as usize).min(self.rows - 1));
        for r in r0..=r1 {
            for c in c0..=c1 {
                let i = r * self.cols + c;
                out.extend_from_slice(
                    &self.items[self.starts[i] as usize..self.starts[i + 1] as usize],
                );
            }
        }
        out.sort_unstable();
        out.dedup();
    }
}

/// The walls of every building shell in a world (region frame, m).
pub struct Walls {
    tris: Vec<[[f64; 3]; 3]>,
    /// Each triangle's outward normal, flattened and of unit length.
    normals: Vec<[f64; 2]>,
    grid: Grid,
    pub shells: usize,
}

/// Where a horizontal plane at `h` cuts a triangle.
fn slice(t: &[[f64; 3]; 3], h: f64) -> Option<([f64; 2], [f64; 2])> {
    let mut pts = [[0.0; 2]; 2];
    let mut n = 0;
    for i in 0..3 {
        let (a, b) = (t[i], t[(i + 1) % 3]);
        let (da, db) = (a[1] - h, b[1] - h);
        if (da < 0.0) != (db < 0.0) {
            let s = da / (da - db);
            if n < 2 {
                pts[n] = [a[0] + (b[0] - a[0]) * s, a[2] + (b[2] - a[2]) * s];
            }
            n += 1;
        }
    }
    (n == 2).then_some((pts[0], pts[1]))
}

/// The nearest point of segment `ab` to `p`.
fn nearest_on(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> [f64; 2] {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let l2 = ab[0] * ab[0] + ab[1] * ab[1];
    if l2 < 1e-12 {
        return a;
    }
    let t = (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / l2).clamp(0.0, 1.0);
    [a[0] + ab[0] * t, a[1] + ab[1] * t]
}

impl Walls {
    /// Loads every shell of the world (synchronously).
    pub fn load(index: &WorldIndex) -> Result<Walls, String> {
        let (mut tris, mut normals, mut extents) = (Vec::new(), Vec::new(), Vec::new());
        let mut shells = 0;
        for tile in index.tiles() {
            let o = tile.origin_mm.map(|v| v as f64 / 1000.0);
            for a in tile.assets.iter().filter(|a| a.layer == Layer::Shell) {
                let mesh = index.load_mesh(&a.content_id).map_err(|e| e.to_string())?;
                shells += 1;
                for prim in mesh.nodes.iter().flat_map(|n| &n.primitives) {
                    for t in prim.indices.as_chunks::<3>().0 {
                        let p = t.map(|i| {
                            let v = prim.positions[i as usize];
                            [
                                o[0] + f64::from(v[0]),
                                o[1] + f64::from(v[1]),
                                o[2] + f64::from(v[2]),
                            ]
                        });
                        let u = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
                        let v = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
                        let n = [
                            u[1] * v[2] - u[2] * v[1],
                            u[2] * v[0] - u[0] * v[2],
                            u[0] * v[1] - u[1] * v[0],
                        ];
                        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                        if len < 1e-9 || n[1].abs() / len > WALL_MAX_NY {
                            continue;
                        }
                        let h = (n[0] * n[0] + n[2] * n[2]).sqrt();
                        normals.push([n[0] / h, n[2] / h]);
                        extents.push([
                            p[0][0].min(p[1][0]).min(p[2][0]),
                            p[0][2].min(p[1][2]).min(p[2][2]),
                            p[0][0].max(p[1][0]).max(p[2][0]),
                            p[0][2].max(p[1][2]).max(p[2][2]),
                        ]);
                        tris.push(p);
                    }
                }
            }
        }
        Ok(Walls {
            grid: Grid::new(&extents),
            tris,
            normals,
            shells,
        })
    }

    /// The signed distance (m) from `(x, z)` at height `h` to the nearest
    /// wall within `search`, negative inside a shell, with the wall's
    /// outward normal there; `None` if no wall is that close.
    pub fn signed_distance(
        &self,
        x: f64,
        z: f64,
        h: f64,
        search: f64,
        scratch: &mut Vec<u32>,
    ) -> Option<(f64, [f64; 2])> {
        self.grid
            .query(x - search, z - search, x + search, z + search, scratch);
        let p = [x, z];
        let mut best = f64::MAX;
        let mut hits: Vec<([f64; 2], [f64; 2], f64)> = Vec::new();
        for &i in scratch.iter() {
            let Some((a, b)) = slice(&self.tris[i as usize], h) else {
                continue;
            };
            let q = nearest_on(a, b, p);
            let d = ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt();
            if d <= search {
                best = best.min(d);
                hits.push((q, self.normals[i as usize], d));
            }
        }
        if best == f64::MAX {
            return None;
        }
        // The nearest wall's normal; at a corner, the sum of the normals of
        // the walls that meet at that same nearest point (never of two
        // walls that are merely as far away, on either side of a street).
        let q = hits
            .iter()
            .filter(|(_, _, d)| *d <= best)
            .map(|(hq, _, _)| *hq)
            .next()
            .unwrap_or(p);
        let mut n = [0.0, 0.0];
        for (hq, hn, d) in &hits {
            let same = (hq[0] - q[0]).abs() < 1e-3 && (hq[1] - q[1]).abs() < 1e-3;
            if *d <= best + 1e-3 && same {
                n = [n[0] + hn[0], n[1] + hn[1]];
            }
        }
        let l = (n[0] * n[0] + n[1] * n[1]).sqrt().max(1e-9);
        let n = [n[0] / l, n[1] / l];
        let outside = (p[0] - q[0]) * n[0] + (p[1] - q[1]) * n[1] >= 0.0;
        Some((if outside { best } else { -best }, n))
    }

    /// How far a ray from `(x, z)` along `dir` (unit) at height `h` goes
    /// before it meets a wall, up to `max`.
    pub fn ray(
        &self,
        x: f64,
        z: f64,
        h: f64,
        dir: [f64; 2],
        max: f64,
        scratch: &mut Vec<u32>,
    ) -> f64 {
        let (ex, ez) = (x + dir[0] * max, z + dir[1] * max);
        self.grid
            .query(x.min(ex), z.min(ez), x.max(ex), z.max(ez), scratch);
        let mut best = max;
        for &i in scratch.iter() {
            let Some((a, b)) = slice(&self.tris[i as usize], h) else {
                continue;
            };
            // Ray p + t d against segment a + u (b - a).
            let e = [b[0] - a[0], b[1] - a[1]];
            let den = dir[0] * e[1] - dir[1] * e[0];
            if den.abs() < 1e-12 {
                continue;
            }
            let w = [a[0] - x, a[1] - z];
            let t = (w[0] * e[1] - w[1] * e[0]) / den;
            let u = (w[0] * dir[1] - w[1] * dir[0]) / den;
            if (0.0..=1.0).contains(&u) && t >= 0.0 && t < best {
                best = t;
            }
        }
        best
    }
}

/// Terrain and transport surfaces (streets, paths, bridge decks): the
/// ground people walk on.
pub struct Ground {
    tris: Vec<[[f64; 3]; 3]>,
    grid: ColumnGrid,
}

impl Ground {
    pub fn load(index: &WorldIndex) -> Result<Ground, String> {
        let (mut tris, mut extents) = (Vec::new(), Vec::new());
        for tile in index.tiles() {
            let o = tile.origin_mm.map(|v| v as f64 / 1000.0);
            for a in tile.assets.iter().filter(|a| {
                a.collider == Collider::Trimesh
                    && matches!(a.layer, Layer::Terrain | Layer::Transport)
            }) {
                let mesh = index.load_mesh(&a.content_id).map_err(|e| e.to_string())?;
                for prim in mesh.nodes.iter().flat_map(|n| &n.primitives) {
                    for t in prim.indices.as_chunks::<3>().0 {
                        let p = t.map(|i| {
                            let v = prim.positions[i as usize];
                            [
                                o[0] + f64::from(v[0]),
                                o[1] + f64::from(v[1]),
                                o[2] + f64::from(v[2]),
                            ]
                        });
                        extents.push([0, 2].map(|k| {
                            [
                                p[0][k].min(p[1][k]).min(p[2][k]),
                                p[0][k].max(p[1][k]).max(p[2][k]),
                            ]
                        }));
                        tris.push(p);
                    }
                }
            }
        }
        Ok(Ground {
            grid: ColumnGrid::new(&extents),
            tris,
        })
    }

    /// The highest ground surface at a point (region frame, m).
    pub fn height(&self, x: f64, z: f64) -> Option<f64> {
        let ids = self.grid.at(x, z)?;
        let mut best: Option<f64> = None;
        for &i in ids {
            if let Some(y) = height_in_triangle(self.tris[i as usize], x, z) {
                best = Some(best.map_or(y, |b: f64| b.max(y)));
            }
        }
        best
    }
}
