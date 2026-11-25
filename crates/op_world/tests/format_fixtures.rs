//! The world-format golden fixture (`fixtures/format/`) must validate,
//! regenerate byte for byte, and keep its pinned world id. These tests run
//! on Linux, Windows and macOS in CI; together with the Python tests in
//! `tools/worldfmt` they show that ids are the same everywhere.

mod common;

use std::collections::BTreeMap;

use op_world::sample::{MeshSpec, build_sample_world, parse_mesh_specs, parse_points_specs};
use op_world::validate::validate_store;
use op_world::{ContentId, Layer, ObjectKind, PointsDocument, Store};

fn specs() -> (BTreeMap<String, MeshSpec>, BTreeMap<Layer, PointsDocument>) {
    let dir = common::format_dir();
    (
        parse_mesh_specs(&std::fs::read(dir.join("meshes.json")).unwrap()).expect("meshes.json"),
        parse_points_specs(&std::fs::read(dir.join("points.json")).unwrap()).expect("points.json"),
    )
}

#[test]
fn fixture_world_is_valid() {
    let report = validate_store(&common::fixture_store());
    assert!(report.errors.is_empty(), "errors: {:#?}", report.errors);
    assert_eq!(
        (report.tiles, report.assets, report.triangles, report.points),
        (1, 4, 12, 4)
    );
    assert_eq!(report.world_id, Some(common::expected_world_id()));
}

/// The golden points documents (RFC 0004): `points.json` written
/// canonically gives the pinned ids, and the committed objects are those
/// bytes. `tools/worldfmt/tests/test_points.py` checks the same in Python.
#[test]
fn golden_points_documents_have_their_pinned_ids() {
    let (_, points) = specs();
    let pinned = common::expected_points_ids();
    assert_eq!(
        points.keys().collect::<Vec<_>>(),
        [&Layer::StreetObjects, &Layer::Vegetation]
    );
    assert_eq!(
        pinned.keys().collect::<Vec<_>>(),
        points.keys().collect::<Vec<_>>()
    );
    let store = common::fixture_store();
    for (layer, doc) in &points {
        let bytes = doc.to_bytes().unwrap();
        let id = ContentId::of(&bytes);
        assert_eq!(id, pinned[layer], "{layer:?}");
        assert_eq!(doc.id().unwrap(), id);
        let committed = store.read_object(&id, ObjectKind::Json).unwrap();
        assert!(
            committed == bytes,
            "{layer:?}: the committed document differs"
        );
        assert_eq!(&PointsDocument::from_bytes(&committed).unwrap(), doc);
    }
}

#[test]
fn fixture_world_regenerates_byte_for_byte() {
    let (specs, points) = specs();
    let out = common::temp_dir("regenerate");
    let id = build_sample_world(&Store::new(&out), &specs, &points).expect("sample world builds");
    assert_eq!(id, common::expected_world_id());

    let committed = common::format_dir().join("world");
    let files = common::files_under(&committed);
    assert_eq!(common::files_under(&out), files, "different set of files");
    for f in &files {
        let a = std::fs::read(committed.join(f)).unwrap();
        let b = std::fs::read(out.join(f)).unwrap();
        assert!(a == b, "{f} differs from a fresh build");
    }
    std::fs::remove_dir_all(&out).ok();
}

#[test]
fn building_twice_gives_identical_ids() {
    let (specs, points) = specs();
    let a = common::temp_dir("twice-a");
    let b = common::temp_dir("twice-b");
    let id_a = build_sample_world(&Store::new(&a), &specs, &points).unwrap();
    let id_b = build_sample_world(&Store::new(&b), &specs, &points).unwrap();
    assert_eq!(id_a, id_b);
    std::fs::remove_dir_all(&a).ok();
    std::fs::remove_dir_all(&b).ok();
}

#[test]
fn tampered_object_is_caught() {
    let src = common::format_dir().join("world");
    let dst = common::temp_dir("tamper");
    for f in common::files_under(&src) {
        let to = dst.join(&f);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(src.join(&f), &to).unwrap();
    }
    // Change one byte of a mesh: its hash no longer matches its name.
    let glb = common::files_under(&dst)
        .into_iter()
        .find(|f| f.ends_with(".glb"))
        .unwrap();
    let path = dst.join(&glb);
    let mut bytes = std::fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    std::fs::write(&path, bytes).unwrap();

    let report = validate_store(&Store::new(&dst));
    assert!(!report.is_ok());
    assert!(
        report.errors.iter().any(|e| e.contains("hash mismatch")),
        "{:#?}",
        report.errors
    );
    std::fs::remove_dir_all(&dst).ok();
}
