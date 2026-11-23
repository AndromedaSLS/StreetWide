//! The ingest fixture (`fixtures/world/`): one real tile of the Zurich old
//! town written by the Python ingest (`tools/ingest`). The Rust reference
//! implementation must accept every byte of it, on every platform.

mod common;

use op_world::Store;
use op_world::manifest::{AssetManifest, Collider, Layer};
use op_world::store::ObjectKind;
use op_world::tile::TileIndex;
use op_world::validate::validate_store;

fn store() -> Store {
    Store::new(common::repo_root().join("fixtures/world"))
}

#[test]
fn ingest_fixture_is_valid() {
    let report = validate_store(&store());
    assert!(report.errors.is_empty(), "errors: {:#?}", report.errors);
    assert_eq!(report.tiles, 1);
    assert!(
        report.assets > 10 && report.triangles > 10_000,
        "{report:?}"
    );
}

#[test]
fn ingest_fixture_has_every_ingested_layer_and_licence() {
    let store = store();
    let (_, world) = store.read_world().expect("world.json");
    let tile = &world.tiles[0];
    let index = TileIndex::from_bytes(&store.read_object(&tile.tile_id, ObjectKind::Json).unwrap())
        .unwrap();
    let has = |layer: Layer, collider: Collider, prefix: &str, licence: &str| {
        index.assets.iter().any(|a| {
            a.layer == layer
                && a.collider == collider
                && a.feature_id.starts_with(prefix)
                && a.license == licence
        })
    };
    let osm = "ODbL-1.0 AND LicenseRef-swisstopo-OGD";
    let swisstopo = "LicenseRef-swisstopo-OGD";
    assert!(has(Layer::Terrain, Collider::Trimesh, "tile:", swisstopo));
    assert!(has(
        Layer::Shell,
        Collider::Trimesh,
        "swisstopo:swissbuildings3d:",
        swisstopo
    ));
    assert!(has(Layer::Transport, Collider::None, "tile:", osm));
    assert!(has(Layer::Transport, Collider::Trimesh, "osm:way:", osm));

    for a in &index.assets {
        let m = AssetManifest::from_bytes(
            &store.read_object(&a.manifest_id, ObjectKind::Json).unwrap(),
        )
        .unwrap();
        assert!(
            !m.fictional,
            "{}: ingested data is not fictional",
            a.feature_id
        );
        let osm_derived = m.provenance.sources.iter().any(|s| s.license == "ODbL-1.0");
        assert_eq!(
            osm_derived,
            m.provenance.license.contains("ODbL-1.0"),
            "{}: OSM-derived assets and only those are ODbL",
            a.feature_id
        );
    }
}
