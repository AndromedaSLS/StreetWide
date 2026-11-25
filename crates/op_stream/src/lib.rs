//! StreetWide tile store and streaming.
//!
//! - [`WorldIndex`]: a world store opened for reading (RFC 0001): the world
//!   manifest and every tile index, and mesh decoding.
//! - [`StoreSource`]: synchronous collision geometry for the simulation;
//!   never `Pending`. Replay checks use it.
//! - [`Streamer`]: background loading for the client. A pool of loader
//!   threads decodes meshes into an LRU cache; the renderer and the
//!   simulation read from the same cache, so they share one loading path.
//! - [`StreamSource`]: the simulation's view of a [`Streamer`]. It answers
//!   `Pending` while a mesh is still loading, so the simulation stalls
//!   instead of guessing (ADR-0003).
//!
//! Positions are converted from the glb profile's millimetre-snapped f32
//! back to exact integer millimetres and moved into the sim frame with
//! integer arithmetic only, so every platform and both sources produce the
//! same geometry.

mod streamer;

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

use op_kernel::{
    AssetKey, BoundsMm, ColliderAsset, ColliderMesh, Fetch, GeometryError, GeometrySource,
};
use op_world::glb::{self, GlbMesh, f32_to_mm};
use op_world::limits::Document;
use op_world::{
    Collider, ContentId, ObjectKind, Store, TileAsset, TileIndex, TileKey, WorldManifest,
};

pub use streamer::{StreamSource, Streamer};

#[derive(Debug, Clone)]
pub struct StreamError(pub String);

impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StreamError {}

impl From<op_world::Error> for StreamError {
    fn from(e: op_world::Error) -> Self {
        StreamError(e.to_string())
    }
}

/// One tile of an opened world.
pub struct Tile {
    pub key: TileKey,
    /// Region-frame origin (mm).
    pub origin_mm: [i64; 3],
    pub assets: Vec<TileAsset>,
}

/// A collider asset placed in a sim frame.
#[derive(Clone, Debug)]
pub struct PlacedCollider {
    pub asset: ColliderAsset,
    pub content_id: ContentId,
    /// Tile origin minus sim origin (mm).
    pub offset_mm: [i64; 3],
}

/// A world store opened for reading.
pub struct WorldIndex {
    store: Store,
    world_id: ContentId,
    world: WorldManifest,
    tiles: Vec<Tile>,
}

impl WorldIndex {
    /// Reads `world.json` and every tile index (they are small; meshes are
    /// loaded on demand).
    pub fn open(dir: &Path) -> Result<Self, StreamError> {
        Self::from_store(Store::new(dir))
    }

    /// Like [`WorldIndex::open`], for any store, e.g. a layered one (the
    /// contribution gate's candidate world).
    pub fn from_store(store: Store) -> Result<Self, StreamError> {
        let (world_id, world) = store.read_world()?;
        let mut tiles = Vec::with_capacity(world.tiles.len());
        for wt in &world.tiles {
            let index =
                TileIndex::from_bytes(&store.read_document(&wt.tile_id, Document::TileIndex)?)?;
            tiles.push(Tile {
                key: wt.tile,
                origin_mm: wt.origin_mm,
                assets: index.assets,
            });
        }
        Ok(WorldIndex {
            store,
            world_id,
            world,
            tiles,
        })
    }

    pub fn world_id(&self) -> ContentId {
        self.world_id
    }

    pub fn world(&self) -> &WorldManifest {
        &self.world
    }

    pub fn tiles(&self) -> &[Tile] {
        &self.tiles
    }

    /// Every `collider: trimesh` asset with bounds in the sim frame whose
    /// origin is `sim_origin_mm` (region frame), sorted by key.
    pub fn colliders(&self, sim_origin_mm: [i64; 3]) -> Vec<PlacedCollider> {
        let mut out = Vec::new();
        for t in &self.tiles {
            let offset_mm = [0, 1, 2].map(|i| t.origin_mm[i] - sim_origin_mm[i]);
            for a in t.assets.iter().filter(|a| a.collider == Collider::Trimesh) {
                out.push(PlacedCollider {
                    asset: ColliderAsset {
                        key: AssetKey {
                            tile: [t.key.x, t.key.y],
                            layer: a.layer.as_str().to_string(),
                            feature_id: a.feature_id.clone(),
                        },
                        content_id: *a.content_id.as_bytes(),
                        bounds_mm: BoundsMm {
                            min: a.bounds_mm.min,
                            max: a.bounds_mm.max,
                        }
                        .translated(offset_mm),
                    },
                    content_id: a.content_id,
                    offset_mm,
                });
            }
        }
        out.sort_by(|a, b| a.asset.key.cmp(&b.asset.key));
        out
    }

    /// Reads, verifies and decodes one mesh.
    pub fn load_mesh(&self, id: &ContentId) -> Result<GlbMesh, StreamError> {
        Ok(glb::read(&self.store.read_object(id, ObjectKind::Glb)?)?)
    }
}

/// The collision mesh of a decoded asset, moved by `offset_mm` into the
/// sim frame.
pub fn collider_mesh(mesh: &GlbMesh, offset_mm: [i64; 3]) -> Result<ColliderMesh, StreamError> {
    let mut out = ColliderMesh::default();
    for prim in mesh.nodes.iter().flat_map(|n| &n.primitives) {
        let base = u32::try_from(out.vertices_mm.len())
            .map_err(|_| StreamError("mesh too large".into()))?;
        for p in &prim.positions {
            let mut v = [0i64; 3];
            for i in 0..3 {
                v[i] = f32_to_mm(p[i])
                    .ok_or_else(|| StreamError("position not mm-snapped".into()))?
                    + offset_mm[i];
            }
            out.vertices_mm.push(v);
        }
        for t in prim.indices.as_chunks::<3>().0 {
            out.triangles.push([t[0] + base, t[1] + base, t[2] + base]);
        }
    }
    Ok(out)
}

fn find<'a>(
    colliders: &'a [PlacedCollider],
    key: &AssetKey,
) -> Result<&'a PlacedCollider, GeometryError> {
    colliders
        .binary_search_by(|c| c.asset.key.cmp(key))
        .map(|i| &colliders[i])
        .map_err(|_| GeometryError(format!("unknown asset {key:?}")))
}

fn colliders_in(colliders: &[PlacedCollider], area: &BoundsMm) -> Vec<ColliderAsset> {
    colliders
        .iter()
        .filter(|c| c.asset.bounds_mm.intersects(area))
        .map(|c| c.asset.clone())
        .collect()
}

/// Collision geometry of a world store, read synchronously.
pub struct StoreSource {
    index: WorldIndex,
    colliders: Vec<PlacedCollider>,
    meshes: BTreeMap<AssetKey, Arc<ColliderMesh>>,
}

impl StoreSource {
    /// Opens the store at `dir` for a sim frame whose origin is
    /// `sim_origin_mm` (region frame).
    pub fn open(dir: &Path, sim_origin_mm: [i64; 3]) -> Result<Self, StreamError> {
        Self::from_store(Store::new(dir), sim_origin_mm)
    }

    /// Like [`StoreSource::open`], for any store.
    pub fn from_store(store: Store, sim_origin_mm: [i64; 3]) -> Result<Self, StreamError> {
        let index = WorldIndex::from_store(store)?;
        let colliders = index.colliders(sim_origin_mm);
        Ok(StoreSource {
            index,
            colliders,
            meshes: BTreeMap::new(),
        })
    }

    pub fn world_id(&self) -> ContentId {
        self.index.world_id()
    }

    pub fn world(&self) -> &WorldManifest {
        self.index.world()
    }

    /// Every collider asset of the world, sorted by key.
    pub fn colliders(&self) -> impl Iterator<Item = &ColliderAsset> {
        self.colliders.iter().map(|c| &c.asset)
    }
}

impl GeometrySource for StoreSource {
    fn colliders_in(
        &mut self,
        area: &BoundsMm,
    ) -> Result<Fetch<Vec<ColliderAsset>>, GeometryError> {
        Ok(Fetch::Ready(colliders_in(&self.colliders, area)))
    }

    fn mesh(&mut self, asset: &ColliderAsset) -> Result<Fetch<Arc<ColliderMesh>>, GeometryError> {
        if let Some(m) = self.meshes.get(&asset.key) {
            return Ok(Fetch::Ready(Arc::clone(m)));
        }
        let placed = find(&self.colliders, &asset.key)?;
        let glb = self
            .index
            .load_mesh(&placed.content_id)
            .map_err(|e| GeometryError(format!("{:?}: {e}", asset.key)))?;
        let mesh = Arc::new(
            collider_mesh(&glb, placed.offset_mm)
                .map_err(|e| GeometryError(format!("{:?}: {e}", asset.key)))?,
        );
        self.meshes.insert(asset.key.clone(), Arc::clone(&mesh));
        Ok(Fetch::Ready(mesh))
    }
}
