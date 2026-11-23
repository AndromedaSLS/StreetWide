//! StreetWide tile store and streaming.
//!
//! [`StoreSource`] serves the collision geometry of a world store
//! (RFC 0001) to the simulation through [`op_kernel::GeometrySource`]. It
//! reads synchronously and is never `Pending`, which is what replay checks
//! need. Stage 5 adds background loading for the client on top of it.
//!
//! Positions are converted from the glb profile's millimetre-snapped f32
//! back to exact integer millimetres and moved into the sim frame with
//! integer arithmetic only, so every platform sees the same geometry.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

use op_kernel::{
    AssetKey, BoundsMm, ColliderAsset, ColliderMesh, Fetch, GeometryError, GeometrySource,
};
use op_world::glb::{self, f32_to_mm};
use op_world::{Collider, ContentId, ObjectKind, Store, TileIndex, WorldManifest};

#[derive(Debug)]
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

struct Entry {
    asset: ColliderAsset,
    content_id: ContentId,
    /// Tile origin minus sim origin (mm).
    offset: [i64; 3],
}

/// Collision geometry of a world store, in a sim frame.
pub struct StoreSource {
    store: Store,
    world_id: ContentId,
    world: WorldManifest,
    entries: Vec<Entry>,
    meshes: BTreeMap<AssetKey, Arc<ColliderMesh>>,
}

impl StoreSource {
    /// Opens the store at `dir` and indexes every `collider: trimesh`
    /// asset, with bounds in the sim frame whose origin is `sim_origin_mm`
    /// (region frame).
    pub fn open(dir: &Path, sim_origin_mm: [i64; 3]) -> Result<Self, StreamError> {
        let store = Store::new(dir);
        let (world_id, world) = store.read_world()?;
        let mut entries = Vec::new();
        for wt in &world.tiles {
            let index = TileIndex::from_bytes(&store.read_object(&wt.tile_id, ObjectKind::Json)?)?;
            let offset = [0, 1, 2].map(|i| wt.origin_mm[i] - sim_origin_mm[i]);
            for a in &index.assets {
                if a.collider != Collider::Trimesh {
                    continue;
                }
                let b = a.bounds_mm;
                entries.push(Entry {
                    asset: ColliderAsset {
                        key: AssetKey {
                            tile: [wt.tile.x, wt.tile.y],
                            layer: a.layer.as_str().to_string(),
                            feature_id: a.feature_id.clone(),
                        },
                        content_id: *a.content_id.as_bytes(),
                        bounds_mm: BoundsMm {
                            min: b.min,
                            max: b.max,
                        }
                        .translated(offset),
                    },
                    content_id: a.content_id,
                    offset,
                });
            }
        }
        entries.sort_by(|a, b| a.asset.key.cmp(&b.asset.key));
        Ok(StoreSource {
            store,
            world_id,
            world,
            entries,
            meshes: BTreeMap::new(),
        })
    }

    pub fn world_id(&self) -> ContentId {
        self.world_id
    }

    pub fn world(&self) -> &WorldManifest {
        &self.world
    }

    /// Every collider asset of the world, sorted by key.
    pub fn colliders(&self) -> impl Iterator<Item = &ColliderAsset> {
        self.entries.iter().map(|e| &e.asset)
    }

    fn load(&self, e: &Entry) -> Result<ColliderMesh, StreamError> {
        let mesh = glb::read(&self.store.read_object(&e.content_id, ObjectKind::Glb)?)?;
        let mut out = ColliderMesh::default();
        for prim in mesh.nodes.iter().flat_map(|n| &n.primitives) {
            let base = u32::try_from(out.vertices_mm.len())
                .map_err(|_| StreamError("mesh too large".into()))?;
            for p in &prim.positions {
                let mut v = [0i64; 3];
                for i in 0..3 {
                    v[i] = f32_to_mm(p[i]).ok_or_else(|| {
                        StreamError(format!("{:?}: position not mm-snapped", e.asset.key))
                    })? + e.offset[i];
                }
                out.vertices_mm.push(v);
            }
            for t in prim.indices.as_chunks::<3>().0 {
                out.triangles.push([t[0] + base, t[1] + base, t[2] + base]);
            }
        }
        Ok(out)
    }
}

impl GeometrySource for StoreSource {
    fn colliders_in(
        &mut self,
        area: &BoundsMm,
    ) -> Result<Fetch<Vec<ColliderAsset>>, GeometryError> {
        Ok(Fetch::Ready(
            self.entries
                .iter()
                .filter(|e| e.asset.bounds_mm.intersects(area))
                .map(|e| e.asset.clone())
                .collect(),
        ))
    }

    fn mesh(&mut self, asset: &ColliderAsset) -> Result<Fetch<Arc<ColliderMesh>>, GeometryError> {
        if let Some(m) = self.meshes.get(&asset.key) {
            return Ok(Fetch::Ready(Arc::clone(m)));
        }
        let i = self
            .entries
            .binary_search_by(|e| e.asset.key.cmp(&asset.key))
            .map_err(|_| GeometryError(format!("unknown asset {:?}", asset.key)))?;
        let mesh = Arc::new(
            self.load(&self.entries[i])
                .map_err(|e| GeometryError(e.0))?,
        );
        self.meshes.insert(asset.key.clone(), Arc::clone(&mesh));
        Ok(Fetch::Ready(mesh))
    }
}
