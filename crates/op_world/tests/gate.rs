//! The contribution gate (RFC 0002) on the golden fixture
//! (`fixtures/changes/facade-sample`) and on deliberately broken variants:
//! each check must catch what it is for.

mod common;

use std::path::{Path, PathBuf};

use op_world::contrib::{
    Change, ChangeSet, CheckReport, Decision, INGEST_AUTHOR, MergeRecord, Op, Outcome, Review,
    Verdict, verify_log,
};
use op_world::gate;
use op_world::glb::{Material, f32_to_mm, mm_to_f32};
use op_world::manifest::{Collider, Layer, Source};
use op_world::sample_change::{Draft, sample_draft, write_bundle};
use op_world::{ContentId, ObjectKind, Store, TileIndex};

fn world() -> PathBuf {
    common::repo_root().join("fixtures/world")
}

fn sample_bundle() -> PathBuf {
    common::repo_root().join("fixtures/changes/facade-sample")
}

fn gate_run(bundle: &Path, name: &str) -> CheckReport {
    let candidate = common::temp_dir(&format!("candidate-{name}")).join("candidate");
    gate::run(&world(), bundle, &candidate).expect("the gate runs")
}

/// Builds a variant of the sample in a temp dir and runs the gate on it.
fn variant(name: &str, edit: impl FnOnce(&mut Draft)) -> CheckReport {
    let bundle = common::temp_dir(&format!("bundle-{name}"));
    let mut draft = sample_draft(&world()).unwrap();
    edit(&mut draft);
    write_bundle(&mut draft, &bundle).unwrap();
    gate_run(&bundle, name)
}

fn outcome(report: &CheckReport, id: &str) -> Outcome {
    report
        .checks
        .iter()
        .find(|c| c.id == id)
        .expect("check present")
        .outcome
}

fn assert_fails(report: &CheckReport, id: &str) {
    assert_eq!(
        outcome(report, id),
        Outcome::Fail,
        "{id} should fail: {report:#?}"
    );
    assert!(!report.passed);
}

#[test]
fn the_sample_passes_and_its_report_is_pinned() {
    let report = gate_run(&sample_bundle(), "sample");
    assert!(report.passed, "{report:#?}");
    let expect = |id, o| assert_eq!(outcome(&report, id), o, "{id}");
    for id in ["G1", "G2", "G3", "G4", "G5", "G7", "G8"] {
        expect(id, Outcome::Pass);
    }
    expect("G6", Outcome::Review);
    expect("G9", Outcome::NotApplicable);
    expect("G10", Outcome::NotApplicable);
    let committed = std::fs::read(sample_bundle().join("report.json")).unwrap();
    assert_eq!(
        report.to_bytes().unwrap(),
        committed,
        "the report differs from the committed one; regenerate with `cargo run -p op_world --example make_change_fixtures -- fixtures`"
    );
    assert_eq!(CheckReport::from_bytes(&committed).unwrap(), report);
}

#[test]
fn the_sample_regenerates_byte_for_byte() {
    let out = common::temp_dir("regenerate-change");
    let mut draft = sample_draft(&world()).unwrap();
    write_bundle(&mut draft, &out).unwrap();
    let committed: Vec<String> = common::files_under(&sample_bundle())
        .into_iter()
        .filter(|f| f != "report.json")
        .collect();
    assert_eq!(common::files_under(&out), committed);
    for f in committed {
        assert_eq!(
            std::fs::read(out.join(&f)).unwrap(),
            std::fs::read(sample_bundle().join(&f)).unwrap(),
            "{f}"
        );
    }
}

#[test]
fn g1_rejects_an_unreadable_change_set() {
    let bundle = common::temp_dir("bundle-g1");
    std::fs::write(bundle.join("change.json"), b"{}").unwrap();
    let r = gate_run(&bundle, "g1");
    assert_fails(&r, "G1");
    assert!(r.checks[1..].iter().all(|c| c.outcome == Outcome::NotRun));
    assert_eq!(r.candidate_world_id, None);
}

#[test]
fn g2_rejects_a_stale_parent() {
    let stale = ContentId::of(b"an older version");
    let r = variant("g2", |d| {
        d.change.changes[0].op = Op::Replace;
        d.change.changes[0].parent_version_id = Some(stale);
        d.manifest.parent_version_id = Some(stale);
    });
    assert_fails(&r, "G2");
}

#[test]
fn g3_rejects_closed_layers_and_wrong_colliders() {
    let r = variant("g3-collider", |d| {
        d.manifest.physics.collider = Collider::Trimesh
    });
    assert_fails(&r, "G3");
    assert_eq!(
        outcome(&r, "G9"),
        Outcome::NotRun,
        "colliders reach the simulation"
    );

    let r = variant("g3-interior", |d| {
        d.change.changes[0].layer = Layer::Interior;
        d.manifest.layer = Layer::Interior;
        d.manifest.fictional = true;
    });
    assert_fails(&r, "G3");
}

/// The manifest id of the fixture's water in `tile` (the Limmat).
fn fixture_water(tile: op_world::TileKey) -> ContentId {
    let store = Store::new(world());
    let (_, w) = store.read_world().unwrap();
    let wt = w.tiles.iter().find(|t| t.tile == tile).unwrap();
    let index =
        TileIndex::from_bytes(&store.read_object(&wt.tile_id, ObjectKind::Json).unwrap()).unwrap();
    index
        .assets
        .iter()
        .find(|a| a.layer == Layer::Water)
        .expect("the fixture tile has water")
        .manifest_id
}

/// The sample turned into a new version of its tile's water surface (RFC
/// 0003): feature id `tile:<key>`, palette material `water`, no collider,
/// replacing the fixture's water.
fn as_water(d: &mut Draft) {
    let feature_id = format!("tile:{}", d.change.changes[0].tile);
    let parent = Some(fixture_water(d.change.changes[0].tile));
    d.change.changes[0].op = Op::Replace;
    d.change.changes[0].parent_version_id = parent;
    d.change.changes[0].layer = Layer::Water;
    d.change.changes[0].feature_id = feature_id.clone();
    d.manifest.parent_version_id = parent;
    d.manifest.layer = Layer::Water;
    d.manifest.feature_id = feature_id.clone();
    d.manifest.physics.collider = Collider::None;
    let node = &mut d.mesh.nodes[0];
    node.name = feature_id;
    for p in &mut node.primitives {
        p.material = Material::Water;
    }
}

#[test]
fn g3_sends_water_fixes_to_the_source() {
    let r = variant("g3-water", as_water);
    assert_fails(&r, "G3");
    let g3 = r.checks.iter().find(|c| c.id == "G3").unwrap();
    assert!(
        g3.details
            .iter()
            .any(|d| d.contains("fix it in the source (OpenStreetMap)")),
        "{g3:#?}"
    );
    assert_eq!(outcome(&r, "G1"), Outcome::Pass, "{r:#?}");
    assert_eq!(outcome(&r, "G2"), Outcome::Pass, "{r:#?}");
    assert_eq!(
        outcome(&r, "G9"),
        Outcome::NotApplicable,
        "water never reaches the simulation"
    );
}

#[test]
fn a_baseline_refresh_with_water_is_reviewed() {
    let r = variant("g3-water-baseline", |d| {
        as_water(d);
        d.change.author = INGEST_AUTHOR.into();
    });
    assert_eq!(outcome(&r, "G3"), Outcome::Review, "{r:#?}");
    assert_eq!(outcome(&r, "G1"), Outcome::Pass, "{r:#?}");
    assert_eq!(outcome(&r, "G2"), Outcome::Pass, "{r:#?}");
    assert!(r.passed, "{r:#?}");
}

#[test]
fn g4_rejects_odbl_missing_licences_and_proprietary_sources() {
    let r = variant("g4-odbl", |d| {
        d.manifest.provenance.sources.push(Source {
            dataset: "OpenStreetMap".into(),
            version: "2026-11-23T08:22:35Z".into(),
            license: "ODbL-1.0".into(),
            attribution: "© OpenStreetMap contributors".into(),
            url: "https://www.openstreetmap.org/".into(),
            source_ids: vec!["way/1".into()],
        });
        d.manifest.provenance.license =
            "CC-BY-SA-4.0 AND LicenseRef-swisstopo-OGD AND ODbL-1.0".into();
        d.manifest.provenance.attribution = vec![
            "The StreetWide contributors".into(),
            "© OpenStreetMap contributors".into(),
            "© swisstopo".into(),
        ];
    });
    assert_fails(&r, "G4");

    let r = variant("g4-missing", |d| {
        d.manifest.provenance.license = "CC-BY-SA-4.0".into()
    });
    assert_fails(&r, "G4");

    let r = variant("g4-proprietary", |d| {
        d.manifest.provenance.sources.push(Source {
            dataset: "Google Street View".into(),
            version: "2026".into(),
            license: "CC-BY-4.0".into(),
            attribution: "Google".into(),
            url: "https://maps.google.com/".into(),
            source_ids: vec![],
        });
        d.manifest.provenance.license =
            "CC-BY-4.0 AND CC-BY-SA-4.0 AND LicenseRef-swisstopo-OGD".into();
        d.manifest.provenance.attribution = vec![
            "Google".into(),
            "The StreetWide contributors".into(),
            "© swisstopo".into(),
        ];
    });
    assert_fails(&r, "G4");
}

#[test]
fn g5_requires_the_author_and_every_credit() {
    let r = variant("g5-author", |d| {
        d.change.author = "github:someone-else".into()
    });
    assert_fails(&r, "G5");
    let r = variant("g5-credit", |d| {
        d.manifest.provenance.attribution = vec!["© swisstopo".into()]
    });
    assert_fails(&r, "G5");
}

#[test]
fn g7_enforces_the_facade_budget() {
    let r = variant("g7", |d| {
        let p = &mut d.mesh.nodes[0].primitives[0];
        let once = p.indices.clone();
        while p.indices.len() / 3 <= gate::FACADE_MAX_TRIANGLES {
            p.indices.extend_from_slice(&once);
        }
    });
    assert_fails(&r, "G7");
}

#[test]
fn g8_keeps_facades_on_their_building() {
    let r = variant("g8-far", |d| {
        // Two metres east, staying on the millimetre grid.
        for v in &mut d.mesh.nodes[0].primitives[0].positions {
            v[0] = mm_to_f32(f32_to_mm(v[0]).unwrap() + 2_000);
        }
    });
    assert_fails(&r, "G8");

    let r = variant("g8-no-shell", |d| {
        d.change.changes[0].feature_id = "streetwide:no-such-building".into();
        d.manifest.feature_id = "streetwide:no-such-building".into();
        d.mesh.nodes[0].name = "streetwide:no-such-building".into();
    });
    assert_fails(&r, "G8");
}

/// G7 refuses a facade over its budget, so G8 does not spend time measuring
/// it (found by review while fuzzing the readers: G8's work grows with the
/// facade and the shell).
#[test]
fn g8_does_not_measure_a_facade_over_its_budget() {
    let r = variant("g8-over-budget", |d| {
        let p = &mut d.mesh.nodes[0].primitives[0];
        let once = p.indices.clone();
        while p.indices.len() / 3 <= gate::FACADE_MAX_TRIANGLES {
            p.indices.extend_from_slice(&once);
        }
    });
    assert_fails(&r, "G7");
    assert_eq!(outcome(&r, "G8"), Outcome::NotRun);
    let g8 = r.checks.iter().find(|c| c.id == "G8").unwrap();
    assert!(
        g8.details[0].ends_with("not checked, the facade is over its G7 budget"),
        "{g8:?}"
    );
}

#[test]
fn collider_changes_cannot_pass_until_the_simulation_checks_exist() {
    // Remove the sample building's swisstopo shell.
    let r = variant("g9", |d| {
        let world = Store::new(world());
        let (_, w) = world.read_world().unwrap();
        let index = TileIndex::from_bytes(
            &world
                .read_object(&w.tiles[0].tile_id, ObjectKind::Json)
                .unwrap(),
        )
        .unwrap();
        let fid = d.change.changes[0].feature_id.clone();
        let shell = index
            .assets
            .iter()
            .find(|a| a.layer == Layer::Shell && a.feature_id == fid)
            .unwrap();
        d.change.changes = vec![Change {
            op: Op::Remove,
            tile: d.change.changes[0].tile,
            layer: Layer::Shell,
            feature_id: fid,
            parent_version_id: Some(shell.manifest_id),
            manifest_id: None,
        }];
    });
    assert_eq!(outcome(&r, "G3"), Outcome::Pass);
    assert_eq!(outcome(&r, "G9"), Outcome::NotRun);
    assert_eq!(outcome(&r, "G10"), Outcome::NotRun);
    assert!(!r.passed);
}

#[test]
fn change_sets_are_strict() {
    let good = ChangeSet::from_bytes(&std::fs::read(sample_bundle().join("change.json")).unwrap())
        .unwrap();
    let mut twice = good.clone();
    twice.changes.push(good.changes[0].clone());
    assert!(twice.to_bytes().is_err(), "duplicate changes");
    let mut parented = good.clone();
    parented.changes[0].parent_version_id = Some(ContentId::of(b"x"));
    assert!(parented.to_bytes().is_err(), "add with a parent");
    let mut email = good;
    email.author = "someone@example.org".into();
    assert!(
        email.to_bytes().is_err(),
        "authors are handles, not e-mail addresses"
    );
}

fn record(
    decision: Decision,
    previous: Option<ContentId>,
    before: Option<&str>,
    after: &str,
) -> MergeRecord {
    let id = |s: &str| ContentId::of(s.as_bytes());
    let linked = decision != Decision::Baseline;
    MergeRecord {
        format: MergeRecord::FORMAT.into(),
        contract_version: "0.1.0".into(),
        decision,
        change_id: linked.then(|| id("change")),
        world_before: before.map(id),
        world_after: id(after),
        check_report_id: linked.then(|| id("report")),
        reviews: if decision == Decision::Merged {
            vec![Review {
                reviewer: "github:reviewer".into(),
                verdict: Verdict::Better,
                notes: "Matches the building.".into(),
            }]
        } else {
            vec![]
        },
        reasons: "test".into(),
        previous_record_id: previous,
    }
}

#[test]
fn the_world_log_is_a_hash_chain() {
    let base = record(Decision::Baseline, None, None, "w0");
    let base_id = ContentId::of(&base.to_bytes().unwrap());
    let merged = record(Decision::Merged, Some(base_id), Some("w0"), "w1");
    let merged_id = ContentId::of(&merged.to_bytes().unwrap());
    let takedown = record(Decision::Takedown, Some(merged_id), Some("w1"), "w2");
    let log = vec![
        (base_id, base.clone()),
        (merged_id, merged.clone()),
        (ContentId::of(b"t"), takedown),
    ];
    assert_eq!(verify_log(&log).unwrap(), ContentId::of(b"w2"));

    let gap = vec![
        (base_id, base.clone()),
        (
            merged_id,
            record(Decision::Merged, Some(base_id), Some("w9"), "w1"),
        ),
    ];
    assert!(
        verify_log(&gap).is_err(),
        "world_before must continue the chain"
    );
    let wrong_link = vec![
        (base_id, base),
        (
            merged_id,
            record(Decision::Merged, Some(merged_id), Some("w0"), "w1"),
        ),
    ];
    assert!(
        verify_log(&wrong_link).is_err(),
        "previous_record_id must name the previous record"
    );
    assert!(
        verify_log(&[(merged_id, merged.clone())]).is_err(),
        "a log starts with a baseline"
    );

    let mut unreviewed = merged;
    unreviewed.reviews.clear();
    assert!(
        unreviewed.to_bytes().is_err(),
        "a merge needs a review judging it better"
    );
}
