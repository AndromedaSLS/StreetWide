//! `StoreSource` on the committed ingest fixture (`fixtures/world`).

use std::path::PathBuf;

use op_kernel::geometry::sim_origin_mm;
use op_kernel::{BoundsMm, Fetch, GeometrySource};
use op_stream::StoreSource;

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
