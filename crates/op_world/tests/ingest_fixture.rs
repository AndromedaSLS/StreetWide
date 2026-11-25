//! The ingest fixture (`fixtures/world/`): one real tile of the Zurich old
//! town written by the Python ingest (`tools/ingest`). The Rust reference
//! implementation must accept every byte of it, on every platform.

mod common;

use op_world::PointsDocument;
use op_world::Store;
use op_world::glb::{self, Material};
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
    assert!(has(Layer::Water, Collider::None, "tile:", osm));

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

/// The Limmat (RFC 0003, ADR-0014): the tile's one water asset, visual
/// only, from the OpenStreetMap relation and placed by swissALTI3D heights.
#[test]
fn ingest_fixture_has_the_limmat_as_one_level_water_asset() {
    let store = store();
    let (_, world) = store.read_world().expect("world.json");
    let tile = &world.tiles[0];
    let index = TileIndex::from_bytes(&store.read_object(&tile.tile_id, ObjectKind::Json).unwrap())
        .unwrap();
    let water: Vec<_> = index
        .assets
        .iter()
        .filter(|a| a.layer == Layer::Water)
        .collect();
    let [entry] = water[..] else {
        panic!("one water asset per tile: {water:#?}")
    };
    assert_eq!(entry.feature_id, format!("tile:{}", tile.tile));
    assert_eq!(entry.collider, Collider::None);
    assert_eq!(entry.license, "ODbL-1.0 AND LicenseRef-swisstopo-OGD");
    // Water sorts after every other layer, so it comes last.
    assert_eq!(index.assets.last(), Some(entry));

    let m = AssetManifest::from_bytes(
        &store
            .read_object(&entry.manifest_id, ObjectKind::Json)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        (m.layer, m.tile, m.physics.collider),
        (Layer::Water, tile.tile, Collider::None)
    );
    let datasets: Vec<&str> = m
        .provenance
        .sources
        .iter()
        .map(|s| s.dataset.as_str())
        .collect();
    assert_eq!(datasets, ["OpenStreetMap", "swissALTI3D"]);
    assert!(
        m.provenance.sources[0]
            .source_ids
            .contains(&"relation/1598599".to_string()),
        "the Limmat's relation: {:?}",
        m.provenance.sources[0]
    );

    // Flat water facing up. One level per water body and terrain tile; in
    // the region frame the Earth's curvature bends it by a few centimetres.
    let mesh = glb::read(
        &store
            .read_object(&entry.content_id, ObjectKind::Glb)
            .unwrap(),
    )
    .unwrap();
    let prims: Vec<_> = mesh.nodes.iter().flat_map(|n| &n.primitives).collect();
    assert!(!prims.is_empty());
    for p in prims {
        assert_eq!(p.material, Material::Water);
        assert!(!p.indices.is_empty());
        assert!(p.normals.iter().all(|n| *n == [0.0, 1.0, 0.0]));
    }
    let b = entry.bounds_mm;
    assert!(b.max[1] - b.min[1] < 100, "{b:?}");
}

/// The tile's trees and light points (RFC 0004, ADR-0015, ADR-0016): one
/// points document per layer and source, visual only, under CC0 and the
/// swisstopo terms of the ground they stand on, from the city's answers
/// for LV95 square 2683-1247.
#[test]
fn ingest_fixture_has_the_tiles_trees_and_light_points() {
    let store = store();
    let (_, world) = store.read_world().expect("world.json");
    let tile = &world.tiles[0];
    let index = TileIndex::from_bytes(&store.read_object(&tile.tile_id, ObjectKind::Json).unwrap())
        .unwrap();
    for (layer, source, count) in [
        (Layer::Vegetation, "stadt-zuerich:baumkataster", 33),
        (Layer::StreetObjects, "stadt-zuerich:beleuchtung", 135),
    ] {
        let listed: Vec<_> = index.assets.iter().filter(|a| a.layer == layer).collect();
        let [entry] = listed[..] else {
            panic!(
                "one {} asset per tile and source: {listed:#?}",
                layer.as_str()
            )
        };
        assert_eq!(entry.feature_id, format!("tile:{}/{source}", tile.tile));
        assert_eq!(entry.collider, Collider::None);
        assert_eq!(entry.license, "CC0-1.0 AND LicenseRef-swisstopo-OGD");
        let doc = PointsDocument::from_bytes(
            &store
                .read_object(&entry.content_id, ObjectKind::Json)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(doc.points.len(), count, "{source}");
        assert_eq!(doc.source(), Some(source));
        assert_eq!(doc.bounds(), Some(entry.bounds_mm));
    }
    assert_eq!(validate_store(&store).points, 33 + 135);
    assert!(
        world
            .attribution
            .contains(&"Quelle: Stadt Zürich".to_string()),
        "{:?}",
        world.attribution
    );
}
