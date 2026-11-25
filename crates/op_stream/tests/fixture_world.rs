//! `StoreSource` on the committed ingest fixture (`fixtures/world`).

use std::path::PathBuf;

use op_kernel::geometry::sim_origin_mm;
use op_kernel::{BoundsMm, Fetch, GeometrySource};
use op_stream::{StoreSource, WorldIndex};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/world")
}

#[test]
fn serves_every_collider_in_the_sim_frame() {
    let probe = StoreSource::open(&fixture(), [0, 0, 0]).expect("fixture opens");
    let spawn = probe.world().spawn.position_mm;
    let origin = sim_origin_mm(spawn);
    let mut source = StoreSource::open(&fixture(), origin).unwrap();

    let keys: Vec<_> = source.colliders().map(|a| a.key.clone()).collect();
    assert!(keys.windows(2).all(|w| w[0] < w[1]), "sorted and unique");
    assert!(keys.iter().any(|k| k.layer == "terrain"));
    assert!(
        !keys
            .iter()
            .any(|k| k.layer == "transport" && k.feature_id.starts_with("tile:")),
        "road ribbons have no collider"
    );
    assert!(
        keys.iter()
            .any(|k| k.layer == "transport" && k.feature_id.starts_with("osm:way:")),
        "bridge decks do"
    );
    // The fixture has the Limmat, but water is visual only (RFC 0003): it
    // never reaches the simulation.
    let index = WorldIndex::open(&fixture()).unwrap();
    assert!(
        index.tiles()[0]
            .assets
            .iter()
            .any(|a| a.layer.as_str() == "water")
    );
    assert!(
        !keys.iter().any(|k| k.layer == "water"),
        "water has no collider"
    );

    // The spawn point is near the sim origin; its surroundings are served.
    let near = BoundsMm::around([0, spawn[1], 0], [50_000; 3]);
    let Ok(Fetch::Ready(list)) = source.colliders_in(&near) else {
        panic!("ready")
    };
    assert!(list.len() > 5);
    for asset in &list {
        let Ok(Fetch::Ready(mesh)) = source.mesh(asset) else {
            panic!("mesh")
        };
        assert!(!mesh.triangles.is_empty());
        let n = mesh.vertices_mm.len() as u32;
        assert!(mesh.triangles.iter().flatten().all(|&i| i < n));
        // Every vertex lies within the asset's sim-frame bounds.
        let b = asset.bounds_mm;
        assert!(
            mesh.vertices_mm
                .iter()
                .all(|v| (0..3).all(|i| b.min[i] <= v[i] && v[i] <= b.max[i]))
        );
    }
    let terrain = list
        .iter()
        .find(|a| a.key.layer == "terrain")
        .expect("terrain near spawn");
    // Terrain heights around the spawn are those of the Limmat bank (about
    // 400-415 m above sea level).
    assert!(terrain.bounds_mm.min[1] > 395_000 && terrain.bounds_mm.max[1] < 440_000);
}

/// The licence notices beside `world.json` (ADR-0019) are not part of the
/// world: a store with them opens as the same world, with the same tiles
/// and colliders.
#[test]
fn a_store_with_notices_opens_as_the_same_world() {
    let dir = std::env::temp_dir().join(format!("op_stream-notices-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut folders = vec![PathBuf::new()];
    while let Some(rel) = folders.pop() {
        std::fs::create_dir_all(dir.join(&rel)).unwrap();
        for entry in std::fs::read_dir(fixture().join(&rel)).unwrap() {
            let entry = entry.unwrap();
            let rel = rel.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                folders.push(rel);
            } else {
                std::fs::copy(entry.path(), dir.join(&rel)).unwrap();
            }
        }
    }
    for name in op_world::store::NOTICE_FILES {
        std::fs::write(dir.join(name), format!("{name}\n")).unwrap();
    }
    let (plain, noticed) = (
        WorldIndex::open(&fixture()).unwrap(),
        WorldIndex::open(&dir).unwrap(),
    );
    assert_eq!(noticed.world_id(), plain.world_id());
    assert_eq!(noticed.tiles().len(), plain.tiles().len());
    let keys = |source: StoreSource| {
        source
            .colliders()
            .map(|a| a.key.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        keys(StoreSource::open(&dir, [0, 0, 0]).unwrap()),
        keys(StoreSource::open(&fixture(), [0, 0, 0]).unwrap())
    );
    std::fs::remove_dir_all(&dir).ok();
}
