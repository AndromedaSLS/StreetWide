//! Background mesh loading with an LRU cache.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

use op_kernel::{BoundsMm, ColliderAsset, ColliderMesh, Fetch, GeometryError, GeometrySource};
use op_world::ContentId;
use op_world::glb::GlbMesh;

use crate::{PlacedCollider, StreamError, WorldIndex, collider_mesh, colliders_in, find};

struct Entry {
    mesh: Arc<GlbMesh>,
    bytes: usize,
    last_used: u64,
}

#[derive(Default)]
struct Cache {
    entries: BTreeMap<[u8; 32], Entry>,
    /// `last_used` -> id, for eviction in least-recently-used order.
    by_age: BTreeMap<u64, [u8; 32]>,
    in_flight: BTreeSet<[u8; 32]>,
    failed: BTreeMap<[u8; 32], String>,
    clock: u64,
    bytes: usize,
    budget: usize,
}

impl Cache {
    fn touch(&mut self, id: &[u8; 32]) -> Option<Arc<GlbMesh>> {
        self.clock += 1;
        let clock = self.clock;
        let e = self.entries.get_mut(id)?;
        self.by_age.remove(&e.last_used);
        e.last_used = clock;
        self.by_age.insert(clock, *id);
        Some(Arc::clone(&e.mesh))
    }

    fn insert(&mut self, id: [u8; 32], mesh: GlbMesh) {
        let bytes = mesh
            .nodes
            .iter()
            .flat_map(|n| &n.primitives)
            .map(|p| p.positions.len() * 24 + p.indices.len() * 4)
            .sum::<usize>()
            + 64;
        self.clock += 1;
        self.bytes += bytes;
        self.by_age.insert(self.clock, id);
        if let Some(old) = self.entries.insert(
            id,
            Entry {
                mesh: Arc::new(mesh),
                bytes,
                last_used: self.clock,
            },
        ) {
            self.bytes -= old.bytes;
            self.by_age.remove(&old.last_used);
        }
        // Evict the least recently used meshes, but never the one just added.
        while self.bytes > self.budget && self.entries.len() > 1 {
            let Some((&age, &victim)) = self.by_age.iter().next() else {
                break;
            };
            if victim == id {
                break;
            }
            self.by_age.remove(&age);
            if let Some(e) = self.entries.remove(&victim) {
                self.bytes -= e.bytes;
            }
        }
    }
}

fn lock(m: &Mutex<Cache>) -> MutexGuard<'_, Cache> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Loads meshes of one world on background threads into an LRU cache.
pub struct Streamer {
    index: Arc<WorldIndex>,
    cache: Arc<Mutex<Cache>>,
    jobs: Option<Sender<[u8; 32]>>,
    workers: Vec<JoinHandle<()>>,
}

impl Streamer {
    /// Opens the store at `dir` with `workers` loader threads and a cache of
    /// about `budget_bytes` of decoded meshes.
    pub fn open(dir: &Path, workers: usize, budget_bytes: usize) -> Result<Self, StreamError> {
        let index = Arc::new(WorldIndex::open(dir)?);
        let cache = Arc::new(Mutex::new(Cache {
            budget: budget_bytes,
            ..Cache::default()
        }));
        let (tx, rx) = mpsc::channel::<[u8; 32]>();
        let rx = Arc::new(Mutex::new(rx));
        let workers = (0..workers.max(1))
            .map(|i| {
                let (index, cache, rx) = (Arc::clone(&index), Arc::clone(&cache), Arc::clone(&rx));
                std::thread::Builder::new()
                    .name(format!("op_stream loader {i}"))
                    .spawn(move || worker(&index, &cache, &rx))
                    .map_err(|e| StreamError(format!("cannot start loader thread: {e}")))
            })
            .collect::<Result<_, _>>()?;
        Ok(Streamer {
            index,
            cache,
            jobs: Some(tx),
            workers,
        })
    }

    pub fn index(&self) -> &WorldIndex {
        &self.index
    }

    /// Asks for a mesh to be loaded unless it is cached or on its way.
    pub fn request(&self, id: &ContentId) {
        let key = *id.as_bytes();
        let mut c = lock(&self.cache);
        if c.entries.contains_key(&key) || c.in_flight.contains(&key) || c.failed.contains_key(&key)
        {
            return;
        }
        if let Some(jobs) = &self.jobs
            && jobs.send(key).is_ok()
        {
            c.in_flight.insert(key);
        }
    }

    /// The mesh if it is loaded (and marks it as recently used).
    pub fn get(&self, id: &ContentId) -> Option<Arc<GlbMesh>> {
        lock(&self.cache).touch(id.as_bytes())
    }

    /// Why a mesh could not be loaded, if it failed.
    pub fn error(&self, id: &ContentId) -> Option<String> {
        lock(&self.cache).failed.get(id.as_bytes()).cloned()
    }

    /// Meshes requested but not loaded yet.
    pub fn in_flight(&self) -> usize {
        lock(&self.cache).in_flight.len()
    }

    /// Decoded bytes held in the cache.
    pub fn cached_bytes(&self) -> usize {
        lock(&self.cache).bytes
    }
}

impl Drop for Streamer {
    fn drop(&mut self) {
        self.jobs = None; // closes the channel; workers finish their job and exit
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

fn worker(index: &WorldIndex, cache: &Mutex<Cache>, jobs: &Mutex<Receiver<[u8; 32]>>) {
    loop {
        let next = jobs.lock().unwrap_or_else(|e| e.into_inner()).recv();
        let Ok(key) = next else { return };
        let result = index.load_mesh(&ContentId::from_bytes(key));
        let mut c = lock(cache);
        c.in_flight.remove(&key);
        match result {
            Ok(mesh) => c.insert(key, mesh),
            Err(e) => {
                c.failed.insert(key, e.0);
            }
        }
    }
}

/// The simulation's view of a [`Streamer`]: collision geometry in a sim
/// frame, `Pending` while meshes load.
pub struct StreamSource {
    streamer: Arc<Streamer>,
    colliders: Vec<PlacedCollider>,
}

impl StreamSource {
    pub fn new(streamer: Arc<Streamer>, sim_origin_mm: [i64; 3]) -> Self {
        let colliders = streamer.index().colliders(sim_origin_mm);
        StreamSource {
            streamer,
            colliders,
        }
    }

    pub fn streamer(&self) -> &Streamer {
        &self.streamer
    }
}

impl GeometrySource for StreamSource {
    fn colliders_in(
        &mut self,
        area: &BoundsMm,
    ) -> Result<Fetch<Vec<ColliderAsset>>, GeometryError> {
        Ok(Fetch::Ready(colliders_in(&self.colliders, area)))
    }

    fn mesh(&mut self, asset: &ColliderAsset) -> Result<Fetch<Arc<ColliderMesh>>, GeometryError> {
        let placed = find(&self.colliders, &asset.key)?;
        if let Some(glb) = self.streamer.get(&placed.content_id) {
            let mesh = collider_mesh(&glb, placed.offset_mm)
                .map_err(|e| GeometryError(format!("{:?}: {e}", asset.key)))?;
            return Ok(Fetch::Ready(Arc::new(mesh)));
        }
        if let Some(e) = self.streamer.error(&placed.content_id) {
            return Err(GeometryError(format!("{:?}: {e}", asset.key)));
        }
        self.streamer.request(&placed.content_id);
        Ok(Fetch::Pending)
    }
}
