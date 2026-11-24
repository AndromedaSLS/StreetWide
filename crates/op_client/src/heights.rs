//! Terrain heights for the detailed look: walls count their storeys from
//! the ground in front of them, not from the bottom of the building model
//! (swissBUILDINGS3D extends walls down to the lowest terrain point under a
//! building, which on a slope can be a whole storey or more below the
//! street).
//!
//! Each terrain mesh gets a grid of its triangles once, so a lookup tests a
//! handful of triangles.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::sync::Arc;

use bevy::prelude::Resource;
use op_stream::Streamer;
use op_world::glb::GlbMesh;
use op_world::{ContentId, Layer};

/// Grid cell size (m).
const CELL_M: f64 = 4.0;

/// A height lookup.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Height {
    /// Terrain height (region frame, m).
    At(f64),
    /// The terrain mesh there is still loading.
    Loading,
    /// No terrain there.
    Outside,
}

struct Grid {
    mesh: Arc<GlbMesh>,
    /// Tile origin (region frame, m).
    origin: [f64; 3],
    /// Local x, z of the grid's corner.
    min: [f64; 2],
    cols: usize,
    rows: usize,
    /// Per cell: (primitive, first index) of the triangles touching it.
    cells: Vec<Vec<(u16, u32)>>,
}

impl Grid {
    fn build(mesh: Arc<GlbMesh>, origin: [f64; 3]) -> Grid {
        let prims: Vec<_> = mesh.nodes.iter().flat_map(|n| &n.primitives).collect();
        let (mut min, mut max) = ([f64::MAX; 2], [f64::MIN; 2]);
        for p in &prims {
            for v in &p.positions {
                for (k, c) in [0, 2].into_iter().enumerate() {
                    min[k] = min[k].min(f64::from(v[c]));
                    max[k] = max[k].max(f64::from(v[c]));
                }
            }
        }
        let cols = (((max[0] - min[0]) / CELL_M).ceil() as usize).max(1);
        let rows = (((max[1] - min[1]) / CELL_M).ceil() as usize).max(1);
        let mut cells = vec![Vec::new(); cols * rows];
        let cell =
            |v: f64, lo: f64, n: usize| (((v - lo) / CELL_M).floor().max(0.0) as usize).min(n - 1);
        for (pi, p) in prims.iter().enumerate() {
            for (ti, t) in p.indices.as_chunks::<3>().0.iter().enumerate() {
                let xs = t.iter().map(|&i| f64::from(p.positions[i as usize][0]));
                let zs = t.iter().map(|&i| f64::from(p.positions[i as usize][2]));
                let (x0, x1) = xs.fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(v), b.max(v)));
                let (z0, z1) = zs.fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(v), b.max(v)));
                for r in cell(z0, min[1], rows)..=cell(z1, min[1], rows) {
                    for c in cell(x0, min[0], cols)..=cell(x1, min[0], cols) {
                        cells[r * cols + c].push((pi as u16, (ti * 3) as u32));
                    }
                }
            }
        }
        Grid {
            mesh,
            origin,
            min,
            cols,
            rows,
            cells,
        }
    }

    /// The highest terrain surface at a region-frame point.
    fn height(&self, x: f64, z: f64) -> Option<f64> {
        let (lx, lz) = (x - self.origin[0], z - self.origin[2]);
        let c = ((lx - self.min[0]) / CELL_M).floor();
        let r = ((lz - self.min[1]) / CELL_M).floor();
        if c < 0.0 || r < 0.0 || c as usize >= self.cols || r as usize >= self.rows {
            return None;
        }
        let prims: Vec<_> = self.mesh.nodes.iter().flat_map(|n| &n.primitives).collect();
        let mut best: Option<f64> = None;
        for &(pi, start) in &self.cells[r as usize * self.cols + c as usize] {
            let p = prims[pi as usize];
            let s = start as usize;
            let tri = [0, 1, 2].map(|k| p.positions[p.indices[s + k] as usize].map(f64::from));
            if let Some(y) = crate::travel::height_in_triangle(tri, lx, lz) {
                best = Some(best.map_or(y, |b| b.max(y)));
            }
        }
        best.map(|y| y + self.origin[1])
    }
}

/// Grids of the terrain meshes seen so far.
#[derive(Resource, Default)]
pub struct HeightIndex {
    grids: BTreeMap<ContentId, Grid>,
}

impl HeightIndex {
    /// The terrain height at a region-frame point; requests the terrain
    /// mesh if it is not loaded yet.
    pub fn ground(&mut self, streamer: &Streamer, x: f64, z: f64) -> Height {
        for tile in streamer.index().tiles() {
            let o = tile.origin_mm.map(|v| v as f64 / 1000.0);
            for a in tile.assets.iter().filter(|a| a.layer == Layer::Terrain) {
                let b = a.bounds_mm;
                let inside = |k: usize, v: f64| {
                    v >= b.min[k] as f64 / 1000.0 + o[k] && v <= b.max[k] as f64 / 1000.0 + o[k]
                };
                if !inside(0, x) || !inside(2, z) {
                    continue;
                }
                let grid = match self.grids.entry(a.content_id) {
                    Entry::Occupied(e) => e.into_mut(),
                    Entry::Vacant(e) => match streamer.get(&a.content_id) {
                        Some(mesh) => e.insert(Grid::build(mesh, o)),
                        None => {
                            streamer.request(&a.content_id);
                            return Height::Loading;
                        }
                    },
                };
                if let Some(h) = grid.height(x, z) {
                    return Height::At(h);
                }
            }
        }
        Height::Outside
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// The fixture's spawn is 1 m above the terrain.
    #[test]
    fn terrain_height_under_the_fixture_spawn() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/world");
        let streamer = Streamer::open(&dir, 2, 64 << 20).unwrap();
        let spawn = streamer
            .index()
            .world()
            .spawn
            .position_mm
            .map(|v| v as f64 / 1000.0);
        let mut index = HeightIndex::default();
        let mut h = Height::Loading;
        for _ in 0..4000 {
            h = index.ground(&streamer, spawn[0], spawn[2]);
            if h != Height::Loading {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let Height::At(y) = h else { panic!("{h:?}") };
        assert!(
            (spawn[1] - 1.0 - y).abs() < 0.05,
            "terrain {y}, spawn {}",
            spawn[1]
        );
        assert_eq!(
            index.ground(&streamer, spawn[0] + 5_000.0, spawn[2]),
            Height::Outside
        );
    }
}
