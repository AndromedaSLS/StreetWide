//! ADR-0017's rules for licences and credits in whole-world validation,
//! which ADR-0015 and ADR-0016 ask for before their data enters a world:
//! an asset's licence joins licence ids with `AND` only, each source names
//! one licence id, every source's licence is among the asset's, and every
//! source's credit is in the asset's attribution. G1 validates every
//! candidate world, so the rules reach baseline refreshes too, which skip
//! G4 and G5. `tools/worldfmt/tests/test_licences.py` checks the same cases
//! in Python, message for message.

mod common;

use op_world::manifest::{Provenance, Source};
use op_world::validate::validate_store;
use op_world::{AssetManifest, ObjectKind, Store, TileIndex};

/// The format fixture in a temporary store, with the house's manifest
/// changed by `edit`; its tile index and the world are written again.
fn fixture_with(name: &str, edit: impl Fn(&mut AssetManifest)) -> common::TempDir {
    let src = common::format_dir().join("world");
    let dir = common::temp_dir(name);
    for f in common::files_under(&src) {
        let to = dir.join(&f);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(src.join(&f), &to).unwrap();
    }
    let store = Store::new(dir.path());
    let (_, mut world) = store.read_world().unwrap();
    let tile = &mut world.tiles[0];
    let mut index =
        TileIndex::from_bytes(&store.read_object(&tile.tile_id, ObjectKind::Json).unwrap())
            .unwrap();
    let entry = index
        .assets
        .iter_mut()
        .find(|a| a.feature_id == "streetwide:sample-house")
        .unwrap();
    let mut manifest = AssetManifest::from_bytes(
        &store
            .read_object(&entry.manifest_id, ObjectKind::Json)
            .unwrap(),
    )
    .unwrap();
    edit(&mut manifest);
    entry.manifest_id = store
        .write_object(&manifest.to_bytes().unwrap(), ObjectKind::Json)
        .unwrap();
    entry.license = manifest.provenance.license.clone();
    tile.tile_id = store
        .write_object(&index.to_bytes().unwrap(), ObjectKind::Json)
        .unwrap();
    world.attribution = {
        let mut credits = world.attribution.clone();
        credits.extend(manifest.provenance.attribution.iter().cloned());
        credits.sort();
        credits.dedup();
        credits
    };
    store.write_world(&world).unwrap();
    dir
}

fn osm() -> Source {
    Source {
        dataset: "OpenStreetMap".into(),
        version: "2026-11-23T08:22:35Z".into(),
        license: "ODbL-1.0".into(),
        attribution: "© OpenStreetMap contributors".into(),
        url: "https://www.openstreetmap.org/".into(),
        source_ids: vec!["way/1".into()],
    }
}

/// The cases shared with `tools/worldfmt/tests/test_licences.py`: a
/// provenance edited from the house's, and the problems it has.
/// A case: its name, how it edits the house's provenance, and the problems
/// the result has.
type Case = (&'static str, fn(&mut Provenance), Vec<&'static str>);

fn cases() -> Vec<Case> {
    vec![
        ("as it is", |_| {}, vec![]),
        (
            "a second source, covered",
            |p| {
                p.sources.push(osm());
                p.license = "CC-BY-SA-4.0 AND ODbL-1.0".into();
                p.attribution
                    .insert(0, "© OpenStreetMap contributors".into());
                p.attribution.sort();
            },
            vec![],
        ),
        (
            "a source licence missing from the asset's",
            |p| {
                p.sources.push(osm());
                p.attribution.push("© OpenStreetMap contributors".into());
                p.attribution.sort();
            },
            vec![
                "provenance.sources[1].license \"ODbL-1.0\" is not in the asset's licence \"CC-BY-SA-4.0\" (ADR-0017)",
            ],
        ),
        (
            "a source credit missing from the asset's attribution",
            |p| {
                p.sources.push(osm());
                p.license = "CC-BY-SA-4.0 AND ODbL-1.0".into();
            },
            vec![
                "provenance.sources[1].attribution \"© OpenStreetMap contributors\" is not in the asset's attribution (ADR-0017)",
            ],
        ),
        (
            "an asset licence with OR",
            |p| p.license = "CC-BY-SA-4.0 OR MIT".into(),
            vec![
                "provenance.license \"CC-BY-SA-4.0 OR MIT\" must join licence ids with AND only (ADR-0017)",
            ],
        ),
        (
            "a source with two licences",
            |p| {
                p.sources[0].license = "CC-BY-SA-4.0 AND CC0-1.0".into();
                p.license = "CC-BY-SA-4.0 AND CC0-1.0".into();
            },
            vec![
                "provenance.sources[0].license \"CC-BY-SA-4.0 AND CC0-1.0\" must be one licence id (ADR-0017)",
            ],
        ),
        (
            "everything at once",
            |p| {
                p.license = "(CC-BY-SA-4.0)".into();
                p.sources[0].license = "MIT OR Apache-2.0".into();
                p.sources[0].attribution = "Someone else".into();
                p.sources.push(osm());
            },
            vec![
                "provenance.license \"(CC-BY-SA-4.0)\" must join licence ids with AND only (ADR-0017)",
                "provenance.sources[0].license \"MIT OR Apache-2.0\" must be one licence id (ADR-0017)",
                "provenance.sources[0].attribution \"Someone else\" is not in the asset's attribution (ADR-0017)",
                "provenance.sources[1].attribution \"© OpenStreetMap contributors\" is not in the asset's attribution (ADR-0017)",
            ],
        ),
    ]
}

#[test]
fn licence_problems_name_each_rule() {
    let store = common::fixture_store();
    let (_, world) = store.read_world().unwrap();
    let index = TileIndex::from_bytes(
        &store
            .read_object(&world.tiles[0].tile_id, ObjectKind::Json)
            .unwrap(),
    )
    .unwrap();
    let house = index
        .assets
        .iter()
        .find(|a| a.feature_id == "streetwide:sample-house")
        .unwrap();
    let manifest = AssetManifest::from_bytes(
        &store
            .read_object(&house.manifest_id, ObjectKind::Json)
            .unwrap(),
    )
    .unwrap();
    for (name, edit, wanted) in cases() {
        let mut p = manifest.provenance.clone();
        edit(&mut p);
        assert_eq!(p.licence_problems(), wanted, "{name}");
    }
}

#[test]
fn validation_applies_the_licence_rules_to_every_manifest() {
    let dir = fixture_with("licences-valid", |_| {});
    let report = validate_store(&Store::new(dir.path()));
    assert!(report.is_ok(), "{:#?}", report.errors);

    let dir = fixture_with("licences-uncovered", |m| {
        m.provenance.sources.push(osm());
        m.provenance
            .attribution
            .push("© OpenStreetMap contributors".into());
        m.provenance.attribution.sort();
    });
    let report = validate_store(&Store::new(dir.path()));
    assert_eq!(
        report.errors,
        [
            "tile 17/68646/45908, shell streetwide:sample-house: provenance.sources[1].license \"ODbL-1.0\" is not in the asset's licence \"CC-BY-SA-4.0\" (ADR-0017)"
        ],
        "{:#?}",
        report.errors
    );

    let dir = fixture_with("licences-uncredited", |m| {
        m.provenance.sources.push(osm());
        m.provenance.license = "CC-BY-SA-4.0 AND ODbL-1.0".into();
    });
    let report = validate_store(&Store::new(dir.path()));
    assert_eq!(
        report.errors,
        [
            "tile 17/68646/45908, shell streetwide:sample-house: provenance.sources[1].attribution \"© OpenStreetMap contributors\" is not in the asset's attribution (ADR-0017)"
        ],
        "{:#?}",
        report.errors
    );
}

#[test]
fn the_committed_worlds_pass_the_licence_rules() {
    for dir in ["fixtures/format/world", "fixtures/world"] {
        let report = validate_store(&Store::new(common::repo_root().join(dir)));
        assert!(report.is_ok(), "{dir}: {:#?}", report.errors);
    }
}
