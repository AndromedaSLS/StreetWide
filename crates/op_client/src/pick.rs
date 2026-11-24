//! Picking for inspect mode: what a ray from the camera meets first among
//! the meshes the scene draws, exactly as drawn.
//!
//! Bevy's mesh picking cannot serve here: the client keeps its meshes in
//! the render world only (`RenderAssetUsages::RENDER_WORLD`), and it would
//! test every triangle of every part. Instead the scene registers the
//! decoded mesh of each asset it shows (shared with the stream cache, or the
//! terrain as it draws it under water, see `water`), and a ray is tested on
//! the CPU:
//! - broad phase: the box of every shown asset, grown by
//!   [`COINCIDENT_M`], in one tight loop; the boxes the ray passes through
//!   are searched nearest first, until one starts beyond the best hit;
//! - narrow phase: every triangle of a small mesh; a large one (terrain,
//!   street ribbons, big buildings) through a grid of 4 m columns (`grid`),
//!   built on first use and walked along the ray.
//!
//! Where surfaces overlap, the nearest hit wins; then a short probe across
//! the hit surface lets a layer of higher [`rank`] within [`COINCIDENT_M`]
//! win, so a street ribbon wins over the terrain poking through it, and a
//! bridge deck over the terrain model's own copy of the bridge.
//!
//! Rendering only: the simulation never sees any of this.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use bevy::math::{BVec3A, DVec3, Vec3, Vec3A};
use bevy::prelude::Resource;
use op_kernel::AssetKey;
use op_world::Layer;
use op_world::glb::{GlbMesh, Material, Primitive};

use crate::grid::ColumnGrid;

/// Surfaces this close to the nearest hit, measured across it, count as the
/// same place, where the higher-ranked layer wins (m).
pub const COINCIDENT_M: f32 = 0.25;
/// Nothing nearer is picked, nor drawn: the camera's near plane (see
/// `scene::projection`) (m).
pub const NEAR_M: f32 = 0.1;
/// Meshes with at least this many triangles are searched through a grid.
const GRID_MIN_TRIANGLES: u32 = 512;
/// Padding of boxes and grid cells, against rounding (m).
const PAD_M: f32 = 0.01;
/// Barycentric tolerance, so that a ray through an edge shared by two
/// triangles hits at least one of them.
const EDGE_EPS: f64 = 1e-7;
/// The highest [`rank`].
const TOP_RANK: u8 = 4;

/// Which layer wins where surfaces coincide: what lies on what.
pub fn rank(layer: Layer) -> u8 {
    match layer {
        // On a building's walls, and (provisionally) what stands in front
        // of them.
        Layer::Facade | Layer::Furniture | Layer::Interactivity => TOP_RANK,
        Layer::Shell | Layer::Interior => 3,
        // Street ribbons and bridge decks lie on the terrain.
        Layer::Transport => 2,
        Layer::Terrain | Layer::Water => 1,
    }
}

/// Where a ray from `o` along `d` meets the triangle `a b c`, as the
/// multiple of `d` (Möller-Trumbore). Both sides count, as the client draws
/// both; points on an edge count. Rays parallel to the triangle miss.
pub fn ray_triangle(o: DVec3, d: DVec3, a: DVec3, b: DVec3, c: DVec3) -> Option<f64> {
    let (e1, e2) = (b - a, c - a);
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - a;
    let u = s.dot(p) * inv;
    if !(-EDGE_EPS..=1.0 + EDGE_EPS).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < -EDGE_EPS || u + v > 1.0 + EDGE_EPS {
        return None;
    }
    Some(e2.dot(q) * inv)
}

/// A ray, ready to be tested against many boxes.
#[derive(Clone, Copy)]
struct BoxRay {
    o: Vec3A,
    /// The direction's component-wise inverse.
    inv: Vec3A,
    /// The axes the ray runs parallel to.
    parallel: BVec3A,
}

impl BoxRay {
    fn new(o: Vec3, d: Vec3) -> BoxRay {
        let inv = Vec3A::from(d).recip();
        BoxRay {
            o: o.into(),
            inv,
            parallel: inv.abs().cmpeq(Vec3A::INFINITY),
        }
    }

    /// Where the ray enters and leaves the box from `min` to `max`, as
    /// multiples of its direction (the slab test, in SIMD lanes). The
    /// entry is negative when the origin is inside.
    fn through(&self, min: Vec3A, max: Vec3A) -> Option<(f32, f32)> {
        let a = (min - self.o) * self.inv;
        let b = (max - self.o) * self.inv;
        let (mut near, mut far) = (a.min(b), a.max(b));
        if self.parallel.any() {
            // Parallel to a slab: inside it or never. (On its boundary,
            // 0 × ∞ above is NaN.)
            let inside = self.o.cmpge(min) & self.o.cmple(max);
            if (self.parallel & !inside).any() {
                return None;
            }
            near = Vec3A::select(self.parallel, Vec3A::NEG_INFINITY, near);
            far = Vec3A::select(self.parallel, Vec3A::INFINITY, far);
        }
        let (t0, t1) = (near.max_element(), far.min_element());
        (t0 <= t1).then_some((t0, t1))
    }
}

/// The work one cast did, for the details panel (F3) and the tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PickStats {
    /// Assets whose triangles were searched.
    pub assets: u32,
    /// Triangles tested.
    pub triangles: u32,
    /// Grids built.
    pub grids_built: u32,
}

/// One shown asset: its mesh as drawn, where it is drawn.
pub struct PickEntry {
    /// Index into the world's tiles.
    pub tile: usize,
    pub layer: Layer,
    /// Render-frame position of the mesh's frame (its tile's origin).
    offset: Vec3,
    /// Render-frame bounds of the mesh, padded.
    min: Vec3,
    max: Vec3,
    mesh: Arc<GlbMesh>,
    /// Per primitive: its node, its place in the node, and the number of
    /// its first triangle.
    prims: Vec<(u32, u32, u32)>,
    triangles: u32,
    grid: OnceLock<ColumnGrid>,
}

/// The nearest triangle hit so far along a ray, within a range.
struct Search {
    o: DVec3,
    d: DVec3,
    t0: f64,
    t1: f64,
    best: Option<(f64, u32)>,
}

impl Search {
    fn test(&mut self, id: u32, [a, b, c]: [DVec3; 3]) {
        if let Some(t) = ray_triangle(self.o, self.d, a, b, c)
            && t >= self.t0
            && t <= self.t1
            && self.best.is_none_or(|(best, _)| t < best)
        {
            self.best = Some((t, id));
        }
    }
}

impl PickEntry {
    /// `mesh` as drawn at `offset` (render frame); `None` if it has no
    /// triangles.
    pub fn new(tile: usize, layer: Layer, offset: Vec3, mesh: Arc<GlbMesh>) -> Option<PickEntry> {
        let mut prims = Vec::new();
        let mut triangles = 0u32;
        let (mut lo, mut hi) = (Vec3::MAX, Vec3::MIN);
        for (n, node) in mesh.nodes.iter().enumerate() {
            for (k, prim) in node.primitives.iter().enumerate() {
                prims.push((n as u32, k as u32, triangles));
                triangles += (prim.indices.len() / 3) as u32;
                for p in &prim.positions {
                    lo = lo.min(Vec3::from_array(*p));
                    hi = hi.max(Vec3::from_array(*p));
                }
            }
        }
        (triangles > 0).then(|| PickEntry {
            tile,
            layer,
            offset,
            min: offset + lo - Vec3::splat(PAD_M),
            max: offset + hi + Vec3::splat(PAD_M),
            mesh,
            prims,
            triangles,
            grid: OnceLock::new(),
        })
    }

    fn primitive(&self, i: usize) -> &Primitive {
        let (n, k, _) = self.prims[i];
        &self.mesh.nodes[n as usize].primitives[k as usize]
    }

    fn corners(prim: &Primitive, t: &[u32; 3]) -> [DVec3; 3] {
        t.map(|i| Vec3::from_array(prim.positions[i as usize]).as_dvec3())
    }

    /// Triangle number `id` (mesh frame) and its material.
    fn triangle(&self, id: u32) -> ([DVec3; 3], Material) {
        let i = self.prims.partition_point(|p| p.2 <= id) - 1;
        let prim = self.primitive(i);
        let j = (id - self.prims[i].2) as usize * 3;
        let t = [prim.indices[j], prim.indices[j + 1], prim.indices[j + 2]];
        (Self::corners(prim, &t), prim.material)
    }

    /// The nearest hit at `t0..=t1` along a render-frame ray: its distance
    /// and triangle.
    fn hit(&self, o: Vec3, d: Vec3, t0: f64, t1: f64, stats: &mut PickStats) -> Option<(f64, u32)> {
        self.hit_with(o, d, t0, t1, self.triangles >= GRID_MIN_TRIANGLES, stats)
    }

    fn hit_with(
        &self,
        o: Vec3,
        d: Vec3,
        t0: f64,
        t1: f64,
        use_grid: bool,
        stats: &mut PickStats,
    ) -> Option<(f64, u32)> {
        stats.assets += 1;
        let mut s = Search {
            o: (o - self.offset).as_dvec3(),
            d: d.as_dvec3(),
            t0,
            t1,
            best: None,
        };
        if use_grid {
            let grid = self.grid.get_or_init(|| {
                stats.grids_built += 1;
                self.columns()
            });
            grid.walk([s.o.x, s.o.z], [s.d.x, s.d.z], s.t0, s.t1, |ids, exit| {
                for &id in ids {
                    stats.triangles += 1;
                    s.test(id, self.triangle(id).0);
                }
                // Nothing in a later cell can be nearer.
                s.best.is_some_and(|(t, _)| t <= exit)
            });
        } else {
            for (i, &(_, _, first)) in self.prims.iter().enumerate() {
                let prim = self.primitive(i);
                for (j, t) in prim.indices.as_chunks::<3>().0.iter().enumerate() {
                    stats.triangles += 1;
                    s.test(first + j as u32, Self::corners(prim, t));
                }
            }
        }
        s.best
    }

    /// Its triangles in columns, seen from above in the mesh's frame:
    /// each in every cell its extent, padded against rounding, touches.
    fn columns(&self) -> ColumnGrid {
        let pad = f64::from(PAD_M);
        let mut extents = Vec::with_capacity(self.triangles as usize);
        for i in 0..self.prims.len() {
            let prim = self.primitive(i);
            for t in prim.indices.as_chunks::<3>().0 {
                let c = Self::corners(prim, t);
                extents.push([0, 2].map(|k| {
                    let v = c.map(|p| p[k]);
                    [
                        v[0].min(v[1]).min(v[2]) - pad,
                        v[0].max(v[1]).max(v[2]) + pad,
                    ]
                }));
            }
        }
        ColumnGrid::new(&extents)
    }
}

/// What a ray met.
pub struct Hit<'a> {
    pub key: &'a AssetKey,
    pub entry: &'a PickEntry,
    /// Distance from the ray's origin (m).
    pub t: f32,
    /// Render frame.
    pub point: Vec3,
    /// The surface's normal, facing the ray's origin.
    pub normal: Vec3,
    pub material: Material,
}

/// An asset's box and rank, for the broad phase: kept apart from the
/// entries, so that testing every box of the district is a tight loop.
#[derive(Clone, Copy)]
struct Bounds {
    min: Vec3A,
    max: Vec3A,
    rank: u8,
}

/// The assets in the scene, for picking.
#[derive(Resource, Default)]
pub struct PickIndex {
    /// In no particular order; `bounds` runs alongside.
    entries: Vec<(AssetKey, PickEntry)>,
    bounds: Vec<Bounds>,
    /// Where each asset is in `entries`.
    slots: BTreeMap<AssetKey, usize>,
    /// Bumped whenever an asset comes or goes.
    generation: u64,
}

impl PickIndex {
    pub fn insert(&mut self, key: AssetKey, entry: PickEntry) {
        let bounds = Bounds {
            min: entry.min.into(),
            max: entry.max.into(),
            rank: rank(entry.layer),
        };
        match self.slots.get(&key) {
            Some(&i) => {
                self.entries[i] = (key, entry);
                self.bounds[i] = bounds;
            }
            None => {
                self.slots.insert(key.clone(), self.entries.len());
                self.entries.push((key, entry));
                self.bounds.push(bounds);
            }
        }
        self.generation += 1;
    }

    pub fn remove(&mut self, key: &AssetKey) {
        let Some(i) = self.slots.remove(key) else {
            return;
        };
        self.entries.swap_remove(i);
        self.bounds.swap_remove(i);
        if let Some((moved, _)) = self.entries.get(i) {
            self.slots.insert(moved.clone(), i);
        }
        self.generation += 1;
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// What a ray from `origin` along `dir` meets first, up to `max_t`,
    /// with the overlap rule applied: a surface of a higher-ranked layer
    /// within [`COINCIDENT_M`] of the hit, across it, wins over it.
    pub fn cast(
        &self,
        origin: Vec3,
        dir: Vec3,
        max_t: f32,
        stats: &mut PickStats,
    ) -> Option<Hit<'_>> {
        let dir = dir.try_normalize()?;
        let near = self.near_ray(origin, dir, NEAR_M, max_t);
        let first = self.nearest(&near, origin, dir, stats)?;
        match self.probe(&first, &near, origin, dir, stats) {
            Some(better) => Some(better),
            None => Some(first),
        }
    }

    /// The assets whose boxes, grown by [`COINCIDENT_M`], a ray (`dir` of
    /// unit length) passes through between `t_min` and `t_max`, with where
    /// it enters and leaves them, nearest first. They hold every surface
    /// the ray can meet, and every surface within [`COINCIDENT_M`] of one it
    /// meets: the overlap rule's probe needs no other.
    fn near_ray(&self, o: Vec3, d: Vec3, t_min: f32, t_max: f32) -> Vec<(f32, f32, usize)> {
        let (ray, grow) = (BoxRay::new(o, d), Vec3A::splat(COINCIDENT_M));
        let mut boxes: Vec<(f32, f32, usize)> = self
            .bounds
            .iter()
            .enumerate()
            .filter_map(|(i, b)| {
                let (t0, t1) = ray.through(b.min - grow, b.max + grow)?;
                (t1 >= t_min && t0 <= t_max).then_some((t0.max(t_min), t1.min(t_max), i))
            })
            .collect();
        boxes.sort_by(|a, b| a.0.total_cmp(&b.0));
        boxes
    }

    /// The nearest hit along a ray (`dir` of unit length) in the assets
    /// `boxes` lists.
    fn nearest(
        &self,
        boxes: &[(f32, f32, usize)],
        o: Vec3,
        d: Vec3,
        stats: &mut PickStats,
    ) -> Option<Hit<'_>> {
        let mut best: Option<(f64, u32, &AssetKey, &PickEntry)> = None;
        for &(t0, t1, i) in boxes {
            // In f64, as the hits are: an exact tie with the best hit stays
            // in range, whichever way its distance would round to f32.
            let limit = match best {
                // Boxes come nearest first: none further on can hold a
                // nearer hit.
                Some((t, ..)) if f64::from(t0) > t => break,
                Some((t, ..)) => f64::from(t1).min(t),
                None => f64::from(t1),
            };
            let (k, e) = &self.entries[i];
            // Exact ties go to the smaller key, so that what is picked
            // does not depend on the order assets came in.
            if let Some((t, id)) = e.hit(o, d, f64::from(t0), limit, stats)
                && best.is_none_or(|(b, _, bk, _)| t < b || (t == b && k < bk))
            {
                best = Some((t, id, k, e));
            }
        }
        let (t, id, key, entry) = best?;
        let point = o + d * t as f32;
        Some(finish(key, entry, id, point, (t as f32).max(0.0), d))
    }

    /// The overlap rule: from the hit point, a segment of
    /// ±[`COINCIDENT_M`] along the hit surface's normal. Measured across
    /// the surface, it works the same at grazing angles, where a window
    /// along the ray would reach far beyond the hit. The highest-ranked
    /// layer it meets above the hit's own wins; at equal rank, the one
    /// nearest the camera's side.
    fn probe<'a>(
        &'a self,
        first: &Hit<'a>,
        boxes: &[(f32, f32, usize)],
        cam: Vec3,
        cam_dir: Vec3,
        stats: &mut PickStats,
    ) -> Option<Hit<'a>> {
        let r = rank(first.entry.layer);
        if r >= TOP_RANK {
            return None;
        }
        let a = first.point + first.normal * COINCIDENT_M;
        let d = -first.normal;
        let (len, ray) = (2.0 * COINCIDENT_M, BoxRay::new(a, d));
        let mut best: Option<(u8, f64, u32, &AssetKey, &PickEntry)> = None;
        for &(_, _, i) in boxes {
            let b = &self.bounds[i];
            if b.rank <= r || best.is_some_and(|best| b.rank < best.0) {
                continue;
            }
            let Some((t0, t1)) = ray.through(b.min, b.max) else {
                continue;
            };
            if t1 < 0.0 || t0 > len {
                continue;
            }
            let (k, e) = &self.entries[i];
            let (t0, t1) = (f64::from(t0.max(0.0)), f64::from(t1.min(len)));
            if let Some((t, id)) = e.hit(a, d, t0, t1, stats)
                && best.is_none_or(|(br, bt, _, bk, _)| {
                    b.rank > br || (b.rank == br && (t < bt || (t == bt && k < bk)))
                })
            {
                best = Some((b.rank, t, id, k, e));
            }
        }
        let (_, t, id, key, entry) = best?;
        let point = a + d * t as f32;
        Some(finish(key, entry, id, point, point.distance(cam), cam_dir))
    }
}

/// A hit on triangle `id` of `entry` at `point`, seen along `dir`.
fn finish<'a>(
    key: &'a AssetKey,
    entry: &'a PickEntry,
    id: u32,
    point: Vec3,
    t: f32,
    dir: Vec3,
) -> Hit<'a> {
    let ([a, b, c], material) = entry.triangle(id);
    let n = (b - a).cross(c - a).normalize_or_zero().as_vec3();
    Hit {
        key,
        entry,
        t,
        point,
        normal: if n.dot(dir) > 0.0 { -n } else { n },
        material,
    }
}

/// Helpers for the tests of `pick` and `inspect`.
#[cfg(test)]
pub(crate) mod test_util {
    use bevy::math::Vec3;
    use op_world::glb::{GlbMesh, Material};

    /// The ray from 20 m in front of the first wall of `mesh` facing out
    /// horizontally to its middle, and that triangle's number in the
    /// walls' primitive.
    pub(crate) fn at_a_wall(mesh: &GlbMesh) -> (Vec3, Vec3, usize) {
        let walls = mesh.nodes[0]
            .primitives
            .iter()
            .find(|p| p.material == Material::Wall)
            .expect("walls");
        for (i, t) in walls.indices.as_chunks::<3>().0.iter().enumerate() {
            let n = Vec3::from_array(walls.normals[t[0] as usize]);
            if n.y.abs() < 0.1 {
                let centre = t
                    .iter()
                    .map(|&i| Vec3::from_array(walls.positions[i as usize]))
                    .sum::<Vec3>()
                    / 3.0;
                let n = n.normalize();
                return (centre + n * 20.0, -n, i);
            }
        }
        panic!("no wall facing out")
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::at_a_wall;
    use super::*;
    use op_world::TileKey;
    use op_world::glb::Node;

    /// Where a ray from `o` along `d` enters and leaves the box from `min`
    /// to `max`, as multiples of `d`.
    fn ray_box(o: Vec3, d: Vec3, min: Vec3, max: Vec3) -> Option<(f32, f32)> {
        BoxRay::new(o, d).through(min.into(), max.into())
    }

    /// A deterministic sequence of numbers in [0, 1).
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }

        fn range(&mut self, lo: f32, hi: f32) -> f32 {
            lo + (hi - lo) * self.next()
        }
    }

    /// A mesh of one node and one primitive per material.
    fn mesh(name: &str, prims: Vec<(Material, Vec<[[f32; 3]; 3]>)>) -> Arc<GlbMesh> {
        let primitives = prims
            .into_iter()
            .map(|(material, tris)| {
                let positions: Vec<[f32; 3]> = tris.iter().flatten().copied().collect();
                Primitive {
                    material,
                    normals: vec![[0.0, 1.0, 0.0]; positions.len()],
                    indices: (0..positions.len() as u32).collect(),
                    positions,
                }
            })
            .collect();
        Arc::new(GlbMesh {
            nodes: vec![Node {
                name: name.into(),
                primitives,
            }],
        })
    }

    /// A horizontal rectangle at height `y`, as two triangles.
    fn flat(x: [f32; 2], z: [f32; 2], y: f32) -> Vec<[[f32; 3]; 3]> {
        let [a, b, c, d] = [
            [x[0], y, z[0]],
            [x[1], y, z[0]],
            [x[1], y, z[1]],
            [x[0], y, z[1]],
        ];
        vec![[a, c, b], [a, d, c]]
    }

    /// A horizontal grid of `cell`-sized squares over a rectangle, with
    /// heights from `height`.
    fn sheet(
        x: [f32; 2],
        z: [f32; 2],
        cell: f32,
        height: impl Fn(f32, f32) -> f32,
    ) -> Vec<[[f32; 3]; 3]> {
        let (nx, nz) = (
            ((x[1] - x[0]) / cell).round() as usize,
            ((z[1] - z[0]) / cell).round() as usize,
        );
        let at = |i: usize, j: usize| {
            let (px, pz) = (x[0] + i as f32 * cell, z[0] + j as f32 * cell);
            [px, height(px, pz), pz]
        };
        let mut tris = Vec::new();
        for j in 0..nz {
            for i in 0..nx {
                let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
                tris.push([a, c, b]);
                tris.push([a, d, c]);
            }
        }
        tris
    }

    /// The key of an asset of tile 17/1/1.
    fn key(layer: Layer, id: &str) -> AssetKey {
        crate::scene::asset_key(TileKey { x: 1, y: 1 }, layer, id)
    }

    fn add(index: &mut PickIndex, layer: Layer, id: &str, mesh: Arc<GlbMesh>) {
        index.insert(
            key(layer, id),
            PickEntry::new(0, layer, Vec3::ZERO, mesh).unwrap(),
        );
    }

    /// The layer and feature id a ray picks.
    fn picked(index: &PickIndex, o: Vec3, d: Vec3) -> Option<(String, String)> {
        let hit = index.cast(o, d, 2_000.0, &mut PickStats::default())?;
        Some((hit.key.layer.clone(), hit.key.feature_id.clone()))
    }

    fn v(x: f64, y: f64, z: f64) -> DVec3 {
        DVec3::new(x, y, z)
    }

    #[test]
    fn ray_triangle_hits_both_sides_and_shared_edges() {
        // A unit square at y = 0 as two triangles sharing the diagonal.
        let (a, b, c, d) = (
            v(0.0, 0.0, 0.0),
            v(1.0, 0.0, 0.0),
            v(1.0, 0.0, 1.0),
            v(0.0, 0.0, 1.0),
        );
        let down = v(0.0, -1.0, 0.0);
        let square =
            |o: DVec3, dir: DVec3| [ray_triangle(o, dir, a, c, b), ray_triangle(o, dir, a, d, c)];
        // From above and from below.
        assert_eq!(square(v(0.7, 2.0, 0.2), down), [Some(2.0), None]);
        assert_eq!(
            square(v(0.2, -3.0, 0.7), v(0.0, 1.0, 0.0)),
            [None, Some(3.0)]
        );
        // Along the shared diagonal and at the corners: no gap.
        for k in 0..=10 {
            let s = f64::from(k) / 10.0;
            let hits = square(v(s, 1.0, s), down);
            assert!(hits.contains(&Some(1.0)), "{s}: {hits:?}");
        }
        for corner in [a, b, c, d] {
            assert!(square(corner + v(0.0, 1.0, 0.0), down).contains(&Some(1.0)));
        }
        // Outside, and parallel to the square (in its plane or above it).
        assert_eq!(square(v(1.01, 1.0, 0.5), down), [None, None]);
        assert_eq!(square(v(-1.0, 0.0, 0.5), v(1.0, 0.0, 0.0)), [None, None]);
        assert_eq!(square(v(-1.0, 0.5, 0.5), v(1.0, 0.0, 0.0)), [None, None]);
        // Behind the ray's origin: a negative distance, which callers drop.
        assert_eq!(square(v(0.7, -1.0, 0.2), down)[0], Some(-1.0));
    }

    #[test]
    fn ray_box_entry_and_exit() {
        let (min, max) = (Vec3::new(0.0, 0.0, 0.0), Vec3::new(2.0, 1.0, 4.0));
        let x = Vec3::X;
        assert_eq!(
            ray_box(Vec3::new(-1.0, 0.5, 1.0), x, min, max),
            Some((1.0, 3.0))
        );
        // From inside: the entry is behind.
        assert_eq!(
            ray_box(Vec3::new(1.0, 0.5, 1.0), x, min, max),
            Some((-1.0, 1.0))
        );
        // Parallel to two slabs: inside them, on their boundary, outside.
        assert_eq!(
            ray_box(Vec3::new(1.0, 5.0, 3.0), -Vec3::Y, min, max),
            Some((4.0, 5.0))
        );
        assert_eq!(
            ray_box(Vec3::new(2.0, 5.0, 4.0), -Vec3::Y, min, max),
            Some((4.0, 5.0))
        );
        assert_eq!(ray_box(Vec3::new(2.5, 5.0, 3.0), -Vec3::Y, min, max), None);
        // Pointing away, and passing beside.
        assert!(ray_box(Vec3::new(-1.0, 0.5, 1.0), -x, min, max).is_none_or(|(_, t1)| t1 < 0.0));
        assert_eq!(ray_box(Vec3::new(-1.0, 1.5, 1.0), x, min, max), None);
        // A flat box (a horizontal surface) is still hit from above.
        let (fmin, fmax) = (Vec3::new(0.0, 3.0, 0.0), Vec3::new(1.0, 3.0, 1.0));
        assert_eq!(
            ray_box(Vec3::new(0.5, 4.0, 0.5), -Vec3::Y, fmin, fmax),
            Some((1.0, 1.0))
        );
        let slanted = Vec3::new(0.3, -1.0, 0.2).normalize();
        assert!(ray_box(Vec3::new(0.2, 4.0, 0.2), slanted, fmin, fmax).is_some());
    }

    /// The grid finds exactly what testing every triangle finds: on rolling
    /// ground with walls, for rays from everywhere, including straight down,
    /// along the axes and from outside the grid.
    #[test]
    fn column_grid_agrees_with_brute_force() {
        let mut rng = Lcg(7);
        let mut bumps = Lcg(11);
        let heights: Vec<f32> = (0..41 * 41).map(|_| bumps.range(-0.4, 0.4)).collect();
        let ground = sheet([-60.0, 60.0], [-60.0, 60.0], 3.0, |x, z| {
            let (i, j) = (((x + 60.0) / 3.0) as usize, ((z + 60.0) / 3.0) as usize);
            400.0 + 0.05 * x + heights[j.min(40) * 41 + i.min(40)]
        });
        let mut walls = Vec::new();
        for _ in 0..40 {
            let (x, z, len, h) = (
                rng.range(-50.0, 50.0),
                rng.range(-50.0, 50.0),
                rng.range(1.0, 9.0),
                rng.range(3.0, 20.0),
            );
            let (a, b) = ([x, 398.0, z], [x + len, 398.0, z + len * 0.3]);
            let (c, d) = ([b[0], 400.0 + h, b[2]], [a[0], 400.0 + h, a[2]]);
            walls.push([a, b, c]);
            walls.push([a, c, d]);
        }
        let mesh = mesh(
            "test:ground",
            vec![(Material::Terrain, ground), (Material::Wall, walls)],
        );
        let offset = Vec3::new(30.0, -5.0, -12.0);
        let entry = PickEntry::new(0, Layer::Terrain, offset, mesh).unwrap();
        assert!(entry.triangles >= GRID_MIN_TRIANGLES);
        let mut stats = (PickStats::default(), PickStats::default());
        let fixed = [
            (Vec3::new(31.5, 420.0, -11.0), -Vec3::Y),
            (Vec3::new(30.0, 401.0, -12.0), Vec3::X),
            (Vec3::new(30.0, 400.5, -100.0), Vec3::Z),
            (Vec3::new(-200.0, 430.0, 0.0), Vec3::new(1.0, -0.2, 0.05)),
            (Vec3::new(35.0, 402.0, -10.0), Vec3::new(0.0, -0.01, -1.0)),
        ];
        // Most from above the ground, some from beyond its edges; most
        // downwards, some level or rising.
        let random = (0..2_000).map(|_| {
            let o = offset
                + Vec3::new(
                    rng.range(-70.0, 70.0),
                    rng.range(399.0, 440.0),
                    rng.range(-70.0, 70.0),
                );
            let d = Vec3::new(
                rng.range(-1.0, 1.0),
                rng.range(-1.0, 0.1),
                rng.range(-1.0, 1.0),
            );
            (o, d.normalize())
        });
        let mut hits = 0;
        for (o, d) in fixed
            .into_iter()
            .map(|(o, d)| (o, d.normalize()))
            .chain(random)
        {
            let Some((t0, t1)) = ray_box(o, d, entry.min, entry.max) else {
                continue;
            };
            let (t0, t1) = (f64::from(t0.max(0.0)), f64::from(t1));
            let grid = entry.hit_with(o, d, t0, t1, true, &mut stats.0);
            let brute = entry.hit_with(o, d, t0, t1, false, &mut stats.1);
            match (grid, brute) {
                (Some((a, _)), Some((b, _))) => {
                    assert!((a - b).abs() < 1e-9, "{o} {d}: grid {a}, all {b}");
                    hits += 1;
                }
                (a, b) => assert_eq!(a.map(|h| h.0), b.map(|h| h.0), "{o} {d}"),
            }
        }
        // Many rays leave the ground's square before coming down to it.
        assert!(hits > 600, "{hits}");
        assert!(stats.0.triangles * 10 < stats.1.triangles, "{stats:?}");
        assert_eq!((stats.0.grids_built, stats.1.grids_built), (1, 0));
    }

    /// Boxes nearer than the best hit are searched; boxes beyond it never.
    #[test]
    fn nearest_asset_wins_and_far_assets_are_skipped() {
        let mut index = PickIndex::default();
        for (i, z) in [-30.0f32, -10.0, -20.0].into_iter().enumerate() {
            let wall = vec![
                [[-1.0, 0.0, z], [1.0, 0.0, z], [1.0, 3.0, z]],
                [[-1.0, 0.0, z], [1.0, 3.0, z], [-1.0, 3.0, z]],
            ];
            add(
                &mut index,
                Layer::Shell,
                &format!("test:{i}"),
                mesh("test:x", vec![(Material::Wall, wall)]),
            );
        }
        let mut stats = PickStats::default();
        let hit = index
            .cast(Vec3::new(0.0, 1.5, 0.0), Vec3::NEG_Z, 100.0, &mut stats)
            .unwrap();
        assert_eq!(hit.key.feature_id, "test:1");
        assert!((hit.t - 10.0).abs() < 1e-5 && (hit.point.z + 10.0).abs() < 1e-5);
        assert_eq!(hit.normal, Vec3::Z, "facing the camera");
        assert_eq!(stats.assets, 1, "{stats:?}");
        assert_eq!(stats.triangles, 2);
        // Nothing nearer than the near plane, nothing beyond the range.
        let mut near = PickStats::default();
        assert!(
            index
                .cast(Vec3::new(0.0, 1.5, -9.95), Vec3::NEG_Z, 100.0, &mut near)
                .is_some_and(|h| h.key.feature_id == "test:2")
        );
        assert!(
            index
                .cast(Vec3::new(0.0, 1.5, 0.0), Vec3::NEG_Z, 5.0, &mut near)
                .is_none()
        );
        assert!(
            index
                .cast(Vec3::new(0.0, 1.5, 0.0), Vec3::Z, 100.0, &mut near)
                .is_none()
        );
    }

    /// Exact ties go to the smaller key, whichever asset came in first:
    /// also where the distance to them does not round to f32 exactly.
    #[test]
    fn exact_ties_go_to_the_smaller_key_in_any_order() {
        let square = || {
            mesh(
                "test:x",
                vec![(Material::Ground, flat([-5.0, 5.0], [-5.0, 5.0], 0.0))],
            )
        };
        let one = PickEntry::new(0, Layer::Shell, Vec3::ZERO, square()).unwrap();
        let (mut rng, mut rounded_down) = (Lcg(5), 0);
        for _ in 0..64 {
            // Slanted rays from eye height.
            let o = Vec3::new(rng.range(-2.0, 2.0), 1.65, rng.range(-2.0, 2.0));
            let d = Vec3::new(rng.range(-0.4, 0.4), -1.0, rng.range(-0.4, 0.4)).normalize();
            for order in [["test:a", "test:b"], ["test:b", "test:a"]] {
                let mut index = PickIndex::default();
                for id in order {
                    add(&mut index, Layer::Shell, id, square());
                }
                assert_eq!(
                    picked(&index, o, d),
                    Some(("shell".into(), "test:a".into())),
                    "{o} {d}, {order:?}"
                );
            }
            let (t, _) = one
                .hit(o, d, 0.0, 100.0, &mut PickStats::default())
                .unwrap();
            rounded_down += usize::from(f64::from(t as f32) < t);
        }
        // The case f32 bounds got wrong.
        assert!(rounded_down > 0);
    }

    /// Each asset is drawn at its tile's offset in the render frame, where
    /// rays come from and hits are given: small meshes and gridded ones.
    #[test]
    fn assets_are_picked_where_their_tile_puts_them() {
        let wall = vec![
            [[-1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 3.0, 0.0]],
            [[-1.0, 0.0, 0.0], [1.0, 3.0, 0.0], [-1.0, 3.0, 0.0]],
        ];
        let ground = sheet([-50.0, 50.0], [-50.0, 50.0], 2.0, |_, _| 400.0);
        assert!(ground.len() >= GRID_MIN_TRIANGLES as usize);
        let offsets = [
            Vec3::new(210.0, -3.5, -180.0),
            Vec3::new(-415.25, 12.0, 37.5),
            Vec3::new(0.0, 0.0, 206.0),
        ];
        let mut index = PickIndex::default();
        for (t, offset) in offsets.into_iter().enumerate() {
            let tile = TileKey {
                x: 68_646 + t as u32,
                y: 45_903,
            };
            for (layer, m) in [
                (
                    Layer::Shell,
                    mesh("test:x", vec![(Material::Wall, wall.clone())]),
                ),
                (
                    Layer::Terrain,
                    mesh("test:x", vec![(Material::Terrain, ground.clone())]),
                ),
            ] {
                let entry = PickEntry::new(t, layer, offset, m).unwrap();
                index.insert(crate::scene::asset_key(tile, layer, "test:x"), entry);
            }
        }
        for (t, offset) in offsets.into_iter().enumerate() {
            let cast = |o: Vec3, d: Vec3| {
                let h = index.cast(o, d, 2_000.0, &mut PickStats::default())?;
                Some((
                    h.key.tile[0] - 68_646,
                    h.entry.tile,
                    h.entry.layer,
                    h.t,
                    h.point,
                ))
            };
            let close = |a: Vec3, b: Vec3| a.distance(b) < 1e-3;
            // The wall, 7 m ahead.
            let wall_at = offset + Vec3::new(0.3, 1.5, 0.0);
            let (key, entry, layer, dist, point) =
                cast(wall_at + Vec3::Z * 7.0, Vec3::NEG_Z).unwrap();
            assert_eq!((key, entry, layer), (t as u32, t, Layer::Shell));
            assert!(
                (dist - 7.0).abs() < 1e-3 && close(point, wall_at),
                "{dist} {point}"
            );
            // The ground, 20 m straight down, and along a slanted ray.
            let ground_at = offset + Vec3::new(10.3, 400.0, -7.7);
            let (key, entry, layer, dist, point) =
                cast(ground_at + Vec3::Y * 20.0, Vec3::NEG_Y).unwrap();
            assert_eq!((key, entry, layer), (t as u32, t, Layer::Terrain));
            assert!(
                (dist - 20.0).abs() < 1e-3 && close(point, ground_at),
                "{dist} {point}"
            );
            let d = Vec3::new(0.6, -0.5, 0.2).normalize();
            let (_, entry, _, dist, point) = cast(ground_at - d * 30.0, d).unwrap();
            assert_eq!(entry, t);
            assert!(
                (dist - 30.0).abs() < 1e-3 && close(point, ground_at),
                "{dist} {point}"
            );
        }
        // Where the meshes would be without their offsets: nothing.
        let mut stats = PickStats::default();
        let origin = Vec3::new(0.3, 1.5, 7.0);
        assert!(index.cast(origin, Vec3::NEG_Z, 100.0, &mut stats).is_none());
    }

    /// The facade skin of the sample contribution lies 3 cm outside its
    /// shell's walls: it is what the ray meets, and through a gap in it the
    /// shell is. (`inspect` turns both into the same building.)
    #[test]
    fn a_facade_skin_lies_over_its_shell() {
        use op_world::{ObjectKind, Store, glb};
        let world =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/world");
        let draft = op_world::sample_change::sample_draft(&world).unwrap();
        let store = Store::new(&world);
        let (_, w) = store.read_world().unwrap();
        let index_bytes = store
            .read_object(&w.tiles[0].tile_id, ObjectKind::Json)
            .unwrap();
        let tile = op_world::TileIndex::from_bytes(&index_bytes).unwrap();
        let shell_asset = tile
            .assets
            .iter()
            .find(|a| a.layer == Layer::Shell && a.feature_id == draft.manifest.feature_id)
            .unwrap();
        let shell = glb::read(
            &store
                .read_object(&shell_asset.content_id, ObjectKind::Glb)
                .unwrap(),
        )
        .unwrap();
        // Aim at the middle of a wall, from 20 m out.
        let (o, d, t) = at_a_wall(&shell);

        let mut index = PickIndex::default();
        let id = draft.manifest.feature_id.clone();
        add(&mut index, Layer::Shell, &id, Arc::new(shell.clone()));
        add(&mut index, Layer::Facade, &id, Arc::new(draft.mesh.clone()));
        let hit = index.cast(o, d, 100.0, &mut PickStats::default()).unwrap();
        assert_eq!(
            (hit.key.layer.as_str(), hit.key.feature_id.as_str()),
            ("facade", id.as_str())
        );
        assert!((hit.t - (20.0 - 0.03)).abs() < 0.01, "{}", hit.t);
        // A gap in the facade, where that triangle was: the shell shows.
        let mut holed = draft.mesh.clone();
        let skin = &mut holed.nodes[0].primitives[0];
        skin.indices.drain(t * 3..t * 3 + 3);
        add(&mut index, Layer::Facade, &id, Arc::new(holed));
        let hit = index.cast(o, d, 100.0, &mut PickStats::default()).unwrap();
        assert_eq!(
            (hit.key.layer.as_str(), hit.key.feature_id.as_str()),
            ("shell", id.as_str())
        );
        assert!((hit.t - 20.0).abs() < 0.01, "{}", hit.t);
    }

    /// The river of travel's tests: ground at 400 m, water at 400.03 m from
    /// x = -10 to 10 m, a bridge deck at 403 m from z = -3 to 3 m; and here
    /// the terrain model's copy of the bridge, as at the Rathausbrücke,
    /// 2 cm above the deck where it carries it.
    fn river(index: &mut PickIndex) {
        let terrain = [
            sheet([-50.0, -10.0], [-50.0, 50.0], 2.0, |_, _| 400.0),
            sheet([10.0, 50.0], [-50.0, 50.0], 2.0, |_, _| 400.0),
            sheet([-10.0, 10.0], [-50.0, -3.0], 2.0, |_, _| 399.5),
            sheet([-10.0, 10.0], [3.0, 50.0], 2.0, |_, _| 399.5),
            // Carried bridge: the terrain model's surface over the river.
            sheet([-10.0, 10.0], [-3.0, 3.0], 1.0, |_, _| 403.02),
        ]
        .concat();
        add(
            index,
            Layer::Terrain,
            "tile:17/1/1",
            mesh("tile:17/1/1", vec![(Material::Terrain, terrain)]),
        );
        add(
            index,
            Layer::Water,
            "tile:17/1/1",
            mesh(
                "tile:17/1/1",
                vec![(Material::Water, flat([-10.0, 10.0], [-50.0, 50.0], 400.03))],
            ),
        );
        add(
            index,
            Layer::Transport,
            "osm:way:1",
            mesh(
                "osm:way:1",
                vec![(Material::Bridge, flat([-12.0, 12.0], [-3.0, 3.0], 403.0))],
            ),
        );
    }

    #[test]
    fn bridge_decks_win_over_water_and_the_terrain_carrying_them() {
        let mut index = PickIndex::default();
        river(&mut index);
        let down = Vec3::new(0.0, -1.0, 0.0);
        // On the bridge, where the terrain model's copy lies 2 cm above the
        // deck and is hit first: the deck wins.
        assert_eq!(
            picked(&index, Vec3::new(0.0, 420.0, 0.0), down),
            Some(("transport".into(), "osm:way:1".into()))
        );
        // From the side, at a glancing angle.
        let glancing = Vec3::new(1.0, -0.12, 0.3).normalize();
        let o = Vec3::new(2.0, 403.02, 1.0) - glancing * 40.0;
        assert_eq!(
            picked(&index, o, glancing).map(|p| p.0),
            Some("transport".into())
        );
        // Beside the bridge, the water; past the river, the ground.
        assert_eq!(
            picked(&index, Vec3::new(0.0, 420.0, 10.0), down),
            Some(("water".into(), "tile:17/1/1".into()))
        );
        assert_eq!(
            picked(&index, Vec3::new(20.0, 420.0, 10.0), down).map(|p| p.0),
            Some("terrain".into())
        );
        // Under the bridge, looking up: the deck's underside.
        assert_eq!(
            picked(&index, Vec3::new(11.0, 401.0, 0.0), Vec3::Y).map(|p| p.0),
            Some("transport".into())
        );
    }

    #[test]
    fn ribbons_win_over_the_terrain_they_lie_on() {
        let ground = sheet([-100.0, 100.0], [-100.0, 100.0], 4.0, |_, _| 400.0);
        for (dy, wins) in [(0.05, "transport"), (-0.02, "transport"), (-1.0, "terrain")] {
            let mut index = PickIndex::default();
            add(
                &mut index,
                Layer::Terrain,
                "tile:17/1/1",
                mesh("tile:17/1/1", vec![(Material::Terrain, ground.clone())]),
            );
            let ribbon = flat([-3.0, 3.0], [-100.0, 100.0], 400.0 + dy);
            add(
                &mut index,
                Layer::Transport,
                "tile:17/1/1",
                mesh("tile:17/1/1", vec![(Material::Road, ribbon)]),
            );
            let eye = Vec3::new(0.0, 401.65, 60.0);
            // Looking down the street, steeply and at a grazing 1 degree.
            for pitch in [-30.0f32, -1.0] {
                let d = Vec3::new(0.0, pitch.to_radians().sin(), -pitch.to_radians().cos());
                let mut stats = PickStats::default();
                let hit = index.cast(eye, d, 2_000.0, &mut stats).unwrap();
                assert_eq!(hit.key.layer, wins, "{dy} m, {pitch} degrees");
                assert_eq!(hit.material == Material::Road, wins == "transport");
            }
            // Beside the street, the terrain.
            assert_eq!(
                picked(&index, Vec3::new(10.0, 401.65, 0.0), -Vec3::Y).map(|p| p.0),
                Some("terrain".into())
            );
        }
    }

    /// Terrain and water rank alike: whichever is drawn nearer wins, so a
    /// bank above the edge of the water stays terrain.
    #[test]
    fn banks_beat_water_just_below_them() {
        let mut index = PickIndex::default();
        let bank = sheet([9.0, 20.0], [-10.0, 10.0], 1.0, |x, _| {
            400.13 + (x - 9.0) * 0.2
        });
        add(
            &mut index,
            Layer::Terrain,
            "tile:17/1/1",
            mesh("tile:17/1/1", vec![(Material::Terrain, bank)]),
        );
        add(
            &mut index,
            Layer::Water,
            "tile:17/1/1",
            mesh(
                "tile:17/1/1",
                vec![(Material::Water, flat([-10.0, 10.0], [-10.0, 10.0], 400.03))],
            ),
        );
        assert_eq!(
            picked(&index, Vec3::new(9.5, 402.0, 0.0), -Vec3::Y).map(|p| p.0),
            Some("terrain".into())
        );
        assert_eq!(
            picked(&index, Vec3::new(8.5, 402.0, 0.0), -Vec3::Y).map(|p| p.0),
            Some("water".into())
        );
    }

    /// The terrain model carries the river's surface above the water in
    /// places (the Limmat's false ramp); the scene draws it lower and
    /// registers it as drawn, so it picks as the water you see.
    #[test]
    fn terrain_as_drawn_under_water_picks_as_water() {
        use crate::water::{WaterSurface, sink_terrain};
        // Ground at 400 m, and the ramp at 401 m in the river north of
        // z = -20 m; the water is at 400.03 m.
        let terrain = sheet([-50.0, 50.0], [-50.0, 50.0], 1.0, |x, z| {
            if x.abs() < 10.0 && z < -20.0 {
                401.0
            } else {
                400.0
            }
        });
        let terrain = mesh("tile:17/1/1", vec![(Material::Terrain, terrain)]);
        let water = mesh(
            "tile:17/1/1",
            vec![(Material::Water, flat([-10.0, 10.0], [-50.0, 50.0], 400.03))],
        );
        let sunk = Arc::new(sink_terrain(&terrain, &WaterSurface::new(&water)));
        for (drawn, wins) in [(sunk, "water"), (terrain, "terrain")] {
            let mut index = PickIndex::default();
            add(&mut index, Layer::Terrain, "tile:17/1/1", drawn);
            add(&mut index, Layer::Water, "tile:17/1/1", water.clone());
            assert_eq!(
                picked(&index, Vec3::new(0.3, 405.0, -35.2), -Vec3::Y).map(|p| p.0),
                Some(wins.into())
            );
        }
    }

    /// With four terrain tiles of 32,000 triangles and 100 buildings, a
    /// ray from eye height tests a tiny share of the triangles, and finds
    /// what testing all of them finds.
    #[test]
    fn pick_cost_stays_bounded() {
        let mut index = PickIndex::default();
        let mut rng = Lcg(3);
        for (i, (x, z)) in [(0.0f32, 0.0f32), (200.0, 0.0), (0.0, 200.0), (200.0, 200.0)]
            .into_iter()
            .enumerate()
        {
            let tile = sheet([x, x + 200.0], [z, z + 200.0], 1.6, |px, pz| {
                400.0 + 0.01 * px - 0.02 * pz
            });
            assert!(tile.len() >= 31_000);
            add(
                &mut index,
                Layer::Terrain,
                &format!("tile:17/{i}/0"),
                mesh("tile:17/0/0", vec![(Material::Terrain, tile)]),
            );
        }
        for i in 0..100 {
            let (x, z) = (rng.range(10.0, 390.0), rng.range(10.0, 390.0));
            let ground = 400.0 + 0.01 * x - 0.02 * z;
            let (lo, hi) = (
                [x - 4.0, ground - 1.0, z - 4.0],
                [x + 4.0, ground + 15.0, z + 4.0],
            );
            let mut walls = Vec::new();
            for (a, b) in [
                ((lo[0], lo[2]), (hi[0], lo[2])),
                ((hi[0], lo[2]), (hi[0], hi[2])),
                ((hi[0], hi[2]), (lo[0], hi[2])),
                ((lo[0], hi[2]), (lo[0], lo[2])),
            ] {
                walls.push([[a.0, lo[1], a.1], [b.0, lo[1], b.1], [b.0, hi[1], b.1]]);
                walls.push([[a.0, lo[1], a.1], [b.0, hi[1], b.1], [a.0, hi[1], a.1]]);
            }
            walls.extend(flat([lo[0], hi[0]], [lo[2], hi[2]], hi[1]));
            add(
                &mut index,
                Layer::Shell,
                &format!("test:{i}"),
                mesh("test:b", vec![(Material::Wall, walls)]),
            );
        }
        let total: u32 = index.entries.iter().map(|(_, e)| e.triangles).sum();
        let mut stats = PickStats::default();
        let rays = 200;
        for _ in 0..rays {
            let (x, z) = (rng.range(20.0, 380.0), rng.range(20.0, 380.0));
            let o = Vec3::new(x, 400.0 + 0.01 * x - 0.02 * z + 1.65, z);
            let yaw = rng.range(0.0, std::f32::consts::TAU);
            let pitch = rng.range(-0.6, 0.05);
            let d = Vec3::new(
                yaw.sin() * pitch.cos(),
                pitch.sin(),
                -yaw.cos() * pitch.cos(),
            );
            let near = index.near_ray(o, d, NEAR_M, 2_000.0);
            let found = index.nearest(&near, o, d, &mut stats).map(|h| h.t);
            let mut all = PickStats::default();
            let brute = index
                .entries
                .iter()
                .filter_map(|(_, e)| e.hit_with(o, d, f64::from(NEAR_M), 2_000.0, false, &mut all))
                .map(|(t, _)| t as f32)
                .reduce(f32::min);
            match (found, brute) {
                (Some(a), Some(b)) => assert!((a - b).abs() < 1e-3, "{o} {d}: {a} {b}"),
                (a, b) => assert_eq!(a, b, "{o} {d}"),
            }
        }
        let per_ray = stats.triangles / rays;
        assert!(
            per_ray * 200 < total,
            "{per_ray} triangles per ray of {total}"
        );
        assert!(stats.grids_built <= 4, "{stats:?}");
    }

    #[test]
    fn unloaded_assets_are_not_picked() {
        let mut index = PickIndex::default();
        let g0 = index.generation();
        add(
            &mut index,
            Layer::Terrain,
            "tile:17/1/1",
            mesh(
                "tile:17/1/1",
                vec![(Material::Terrain, flat([-5.0, 5.0], [-5.0, 5.0], 0.0))],
            ),
        );
        assert!(index.generation() > g0 && index.len() == 1);
        assert!(picked(&index, Vec3::Y, -Vec3::Y).is_some());
        let g1 = index.generation();
        index.remove(&key(Layer::Terrain, "tile:17/1/1"));
        assert!(index.generation() > g1 && index.len() == 0);
        assert_eq!(picked(&index, Vec3::Y, -Vec3::Y), None);
        // Removing what is not there changes nothing.
        let g2 = index.generation();
        index.remove(&key(Layer::Terrain, "tile:17/1/1"));
        assert_eq!(index.generation(), g2);
        // Assets come and go in any order; each stays where it is, and one
        // shown again replaces its old self.
        let pad = |x: f32| {
            mesh(
                "test:x",
                vec![(Material::Ground, flat([x, x + 1.0], [0.0, 1.0], 0.0))],
            )
        };
        for i in 0..4 {
            add(
                &mut index,
                Layer::Shell,
                &format!("test:{i}"),
                pad(i as f32 * 10.0),
            );
        }
        index.remove(&key(Layer::Shell, "test:0"));
        index.remove(&key(Layer::Shell, "test:2"));
        add(&mut index, Layer::Shell, "test:1", pad(10.0));
        add(&mut index, Layer::Shell, "test:5", pad(50.0));
        assert_eq!(index.len(), 3);
        for (x, want) in [
            (0.5, None),
            (10.5, Some("test:1")),
            (20.5, None),
            (30.5, Some("test:3")),
            (50.5, Some("test:5")),
        ] {
            let got = picked(&index, Vec3::new(x, 5.0, 0.5), -Vec3::Y);
            assert_eq!(got.map(|p| p.1), want.map(String::from), "{x}");
        }
        // A mesh without triangles is not registered at all.
        assert!(
            PickEntry::new(0, Layer::Shell, Vec3::ZERO, Arc::new(GlbMesh::default())).is_none()
        );
    }

    /// What picking costs with the whole Zurich district shown, from eye
    /// height all over it. Needs the district (README, Getting started,
    /// step 3); run it with
    /// `cargo test --release -p op_client district_pick_cost -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs data/world/zurich-altstadt; a measurement, not a check"]
    fn district_pick_cost() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../data/world/zurich-altstadt");
        let Ok(world) = op_stream::WorldIndex::open(&dir) else {
            eprintln!("no district at {}", dir.display());
            return;
        };
        let origin = world.world().spawn.position_mm;
        let mut index = PickIndex::default();
        let mut triangles = 0u64;
        for (t, tile) in world.tiles().iter().enumerate() {
            let offset = Vec3::from_array(
                [0, 1, 2].map(|k| (tile.origin_mm[k] - origin[k]) as f32 / 1000.0),
            );
            for a in &tile.assets {
                let mesh = Arc::new(world.load_mesh(&a.content_id).unwrap());
                let entry = PickEntry::new(t, a.layer, offset, mesh).unwrap();
                triangles += u64::from(entry.triangles);
                index.insert(
                    crate::scene::asset_key(tile.key, a.layer, &a.feature_id),
                    entry,
                );
            }
        }
        let (lo, hi) = index
            .bounds
            .iter()
            .fold((Vec3::MAX, Vec3::MIN), |(lo, hi), b| {
                (lo.min(b.min.into()), hi.max(b.max.into()))
            });
        let mut rng = Lcg(42);
        let mut eyes = Vec::new();
        // Grids are built as rays first need them.
        let mut grids = PickStats::default();
        let started = std::time::Instant::now();
        while eyes.len() < 2_000 {
            let (x, z) = (rng.range(lo.x, hi.x), rng.range(lo.z, hi.z));
            // At eye height on what is below, unless it is a roof.
            if let Some(h) = index.cast(Vec3::new(x, hi.y + 10.0, z), -Vec3::Y, 1e4, &mut grids)
                && matches!(h.entry.layer, Layer::Terrain | Layer::Transport)
            {
                let (yaw, pitch) = (rng.range(0.0, std::f32::consts::TAU), rng.range(-0.6, 0.1));
                let d = Vec3::new(
                    yaw.sin() * pitch.cos(),
                    pitch.sin(),
                    -yaw.cos() * pitch.cos(),
                );
                eyes.push((h.point + Vec3::Y * 1.65, d));
            }
        }
        for (o, d) in &eyes {
            index.cast(*o, *d, 2_000.0, &mut grids);
        }
        let warm_up = started.elapsed();
        let (mut times, mut stats, mut hits) = (Vec::new(), PickStats::default(), 0);
        for (o, d) in &eyes {
            let started = std::time::Instant::now();
            hits += usize::from(index.cast(*o, *d, 2_000.0, &mut stats).is_some());
            times.push(started.elapsed().as_secs_f64() * 1e6);
        }
        times.sort_by(f64::total_cmp);
        let n = eyes.len() as u32;
        eprintln!(
            "{} assets, {triangles} triangles; {n} rays from eye height, {hits} hits\n\
             per pick: median {:.1} us, p90 {:.1} us, max {:.1} us; on average {} assets and {} triangles searched\n\
             warm-up: {} grids built, {:.0} ms",
            index.len(),
            times[times.len() / 2],
            times[times.len() * 9 / 10],
            times[times.len() - 1],
            stats.assets / n,
            stats.triangles / n,
            grids.grids_built,
            warm_up.as_secs_f64() * 1e3,
        );
    }
}
