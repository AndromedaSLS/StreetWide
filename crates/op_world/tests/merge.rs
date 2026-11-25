//! Merging contributions and the world log (RFC 0002 section 6): baseline,
//! merge the sample facade, take it down again.

mod common;

use std::path::{Path, PathBuf};

use op_world::contrib::{
    Change, ChangeSet, CheckReport, Decision, MergeRecord, Op, Review, Verdict, verify_log,
};
use op_world::gate;
use op_world::manifest::Layer;
use op_world::merge::{baseline, merge};
use op_world::store::NOTICE_FILES;
use op_world::validate::validate_store;
use op_world::{ContentId, ObjectKind, Store, TileIndex};

fn copy_dir(from: &Path, to: &Path) {
    for f in common::files_under(from) {
        let dst = to.join(&f);
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        std::fs::copy(from.join(&f), dst).unwrap();
    }
}

/// A private copy of the ingest fixture to merge into.
fn world_copy(name: &str) -> common::TempPath {
    let dir = common::temp_path(name, "world");
    copy_dir(&common::repo_root().join("fixtures/world"), &dir);
    dir
}

fn sample() -> PathBuf {
    common::repo_root().join("fixtures/changes/facade-sample")
}

fn review(verdict: Verdict) -> Review {
    Review {
        reviewer: "github:reviewer".into(),
        verdict,
        notes: "Checked against the RFC 0002 facade checklist.".into(),
    }
}

fn scratch(name: &str) -> common::TempPath {
    common::temp_path(name, "scratch")
}

#[test]
fn baseline_merge_and_takedown_form_a_verifiable_log() {
    let world = world_copy("merge-log");
    let (base_world, _) = Store::new(&world).read_world().unwrap();
    let base = baseline(&world, "The ingest fixture.").unwrap();
    let base_bytes = base.to_bytes().unwrap();

    let merged = merge(
        &world,
        &sample(),
        &base_bytes,
        Decision::Merged,
        vec![review(Verdict::Better)],
        "Test facade.",
        &scratch("merge-1"),
    )
    .unwrap();
    let report = CheckReport::from_bytes(&merged.report_bytes).unwrap();
    assert_eq!(Some(merged.record.world_after), report.candidate_world_id);
    let store = Store::new(&world);
    assert_eq!(store.read_world().unwrap().0, merged.record.world_after);
    let v = validate_store(&store);
    assert!(v.errors.is_empty(), "{:?}", v.errors);
    assert_eq!(v.assets, 46, "the facade joined the 45 fixture assets");

    // Take the facade down again: the world returns to the baseline exactly.
    let (_, w) = store.read_world().unwrap();
    let index = TileIndex::from_bytes(
        &store
            .read_object(&w.tiles[0].tile_id, ObjectKind::Json)
            .unwrap(),
    )
    .unwrap();
    let facade = index
        .assets
        .iter()
        .find(|a| a.layer == Layer::Facade)
        .unwrap();
    let takedown_bundle = common::temp_dir("takedown-bundle");
    let change = ChangeSet {
        format: ChangeSet::FORMAT.into(),
        contract_version: "0.1.0".into(),
        base_world_id: merged.record.world_after,
        title: "Remove the test facade".into(),
        author: "github:maintainer".into(),
        rationale: "Owner request (test).".into(),
        changes: vec![Change {
            op: Op::Remove,
            tile: w.tiles[0].tile,
            layer: Layer::Facade,
            feature_id: facade.feature_id.clone(),
            parent_version_id: Some(facade.manifest_id),
            manifest_id: None,
        }],
    };
    std::fs::write(
        takedown_bundle.join("change.json"),
        change.to_bytes().unwrap(),
    )
    .unwrap();
    let takedown = merge(
        &world,
        &takedown_bundle,
        &merged.record_bytes,
        Decision::Takedown,
        vec![],
        "Privacy request from the owner (test).",
        &scratch("merge-2"),
    )
    .unwrap();
    assert_eq!(takedown.record.world_after, base_world);

    let log: Vec<(ContentId, MergeRecord)> =
        [&base_bytes, &merged.record_bytes, &takedown.record_bytes]
            .into_iter()
            .map(|b| (ContentId::of(b), MergeRecord::from_bytes(b).unwrap()))
            .collect();
    assert_eq!(verify_log(&log).unwrap(), base_world);
}

/// The licence notices beside `world.json` (ADR-0019) are not part of the
/// world: with them, the store validates with the same world id, the gate
/// gives the same report, and a merge makes the same world and leaves them
/// as they were.
#[test]
fn the_notices_beside_a_world_change_no_id_and_a_merge_keeps_them() {
    let world = world_copy("merge-notices");
    let notice = |name: &str| format!("{name} of the test world\n").into_bytes();
    for name in NOTICE_FILES {
        std::fs::write(world.join(name), notice(name)).unwrap();
    }
    let fixture = Store::new(common::repo_root().join("fixtures/world"));
    let (fixture_id, _) = fixture.read_world().unwrap();
    let v = validate_store(&Store::new(&world));
    assert!(v.errors.is_empty(), "{:?}", v.errors);
    assert_eq!(v.world_id, Some(fixture_id));

    let committed = std::fs::read(sample().join("report.json")).unwrap();
    let candidate = common::temp_path("notices-candidate", "candidate");
    let report = gate::run(&world, &sample(), &candidate).unwrap();
    assert_eq!(report.to_bytes().unwrap(), committed, "the gate's report");

    let base = baseline(&world, "The ingest fixture.").unwrap();
    let merged = merge(
        &world,
        &sample(),
        &base.to_bytes().unwrap(),
        Decision::Merged,
        vec![review(Verdict::Better)],
        "Test facade.",
        &scratch("notices-merge-scratch"),
    )
    .unwrap();
    let expected = CheckReport::from_bytes(&committed).unwrap();
    assert_eq!(Some(merged.record.world_after), expected.candidate_world_id);
    for name in NOTICE_FILES {
        assert_eq!(
            std::fs::read(world.join(name)).unwrap(),
            notice(name),
            "{name}"
        );
    }
    let v = validate_store(&Store::new(&world));
    assert!(v.errors.is_empty(), "{:?}", v.errors);
    assert_eq!(v.world_id, Some(merged.record.world_after));
}

#[test]
fn a_refused_merge_changes_nothing() {
    let world = world_copy("merge-refused");
    let (before, _) = Store::new(&world).read_world().unwrap();
    let base = baseline(&world, "The ingest fixture.")
        .unwrap()
        .to_bytes()
        .unwrap();
    let not_better = merge(
        &world,
        &sample(),
        &base,
        Decision::Merged,
        vec![review(Verdict::NotBetter)],
        "Test.",
        &scratch("refused-1"),
    );
    assert!(
        not_better.is_err(),
        "a merge needs a reviewer to judge it better"
    );
    let wrong_log = baseline(
        &common::repo_root().join("fixtures/format/world"),
        "Another world.",
    )
    .unwrap()
    .to_bytes()
    .unwrap();
    let elsewhere = merge(
        &world,
        &sample(),
        &wrong_log,
        Decision::Merged,
        vec![review(Verdict::Better)],
        "Test.",
        &scratch("refused-2"),
    );
    assert!(elsewhere.is_err(), "the log must end at the store's world");
    assert_eq!(Store::new(&world).read_world().unwrap().0, before);
}
