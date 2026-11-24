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
use op_world::glb::{self, GlbMesh, Material, Node, Primitive, f32_to_mm, mm_to_f32};
use op_world::manifest::{Collider, Layer, Source};
use op_world::sample_change::{Draft, sample_draft, write_bundle};
use op_world::{AssetManifest, ContentId, ObjectKind, Store, TileIndex};

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

/// A SplitMix64 stream of numbers below `n`, for test data.
fn numbers(seed: u64) -> impl FnMut(u64) -> u64 {
    let mut state = seed;
    move |n| {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        (z ^ (z >> 31)) % n
    }
}

/// `p` with each triangle replaced by the triangles `parts` makes of its
/// corners (in millimetres), unindexed. Every new vertex takes the normal
/// of its triangle's first corner.
fn split(p: &Primitive, parts: impl Fn([[i64; 3]; 3]) -> Vec<[[i64; 3]; 3]>) -> Primitive {
    let mm = |i: u32| p.positions[i as usize].map(|v| f32_to_mm(v).expect("snapped"));
    let (mut positions, mut normals) = (Vec::new(), Vec::new());
    for &[a, b, c] in p.indices.as_chunks::<3>().0 {
        let normal = p.normals[a as usize];
        for part in parts([mm(a), mm(b), mm(c)]) {
            positions.extend(part.map(|v| v.map(mm_to_f32)));
            normals.extend([normal; 3]);
        }
    }
    Primitive {
        material: p.material,
        indices: (0..positions.len() as u32).collect(),
        positions,
        normals,
    }
}

/// Each triangle split in four at the midpoints of its edges (snapped to
/// the millimetre): the same surface, four times as many triangles.
fn subdivided(p: &Primitive) -> Primitive {
    split(p, |[a, b, c]| {
        let mid = |u: [i64; 3], v: [i64; 3]| [0, 1, 2].map(|k| (u[k] + v[k]).div_euclid(2));
        let (ab, bc, ca) = (mid(a, b), mid(b, c), mid(c, a));
        vec![[a, ab, ca], [ab, b, bc], [ca, bc, c], [ab, bc, ca]]
    })
}

/// Each triangle split in three at its centroid.
fn split_at_centroids(p: &Primitive) -> Primitive {
    split(p, |[a, b, c]| {
        let g = [0, 1, 2].map(|k| (a[k] + b[k] + c[k]).div_euclid(3));
        vec![[a, b, g], [b, c, g], [c, a, g]]
    })
}

/// `fixtures/world`, copied to a directory of its own, with the shell of
/// the sample's building changed by `edit` (its manifest, tile index and
/// world manifest follow). Returns the directory and the new shell.
fn world_with_shell(name: &str, edit: impl FnOnce(&mut GlbMesh)) -> (PathBuf, GlbMesh) {
    let dir = common::temp_dir(&format!("world-{name}"));
    for f in common::files_under(&world()) {
        let to = dir.join(&f);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(world().join(&f), to).unwrap();
    }
    let change = sample_draft(&dir).unwrap().change.changes[0].clone();
    let store = Store::new(&dir);
    let (_, mut w) = store.read_world().unwrap();
    let pos = w.tiles.iter().position(|t| t.tile == change.tile).unwrap();
    let mut index = TileIndex::from_bytes(
        &store
            .read_object(&w.tiles[pos].tile_id, ObjectKind::Json)
            .unwrap(),
    )
    .unwrap();
    let k = index
        .assets
        .iter()
        .position(|a| a.layer == Layer::Shell && a.feature_id == change.feature_id)
        .unwrap();
    let mut manifest = AssetManifest::from_bytes(
        &store
            .read_object(&index.assets[k].manifest_id, ObjectKind::Json)
            .unwrap(),
    )
    .unwrap();
    let mut shell = glb::read(
        &store
            .read_object(&index.assets[k].content_id, ObjectKind::Glb)
            .unwrap(),
    )
    .unwrap();
    edit(&mut shell);
    let bytes = glb::write(&shell).unwrap();
    manifest.content_id = store.write_object(&bytes, ObjectKind::Glb).unwrap();
    manifest.size_bytes = bytes.len() as u64;
    manifest.bounds_mm = shell.bounds_mm().unwrap();
    let asset = &mut index.assets[k];
    asset.manifest_id = store
        .write_object(&manifest.to_bytes().unwrap(), ObjectKind::Json)
        .unwrap();
    asset.content_id = manifest.content_id;
    asset.bounds_mm = manifest.bounds_mm;
    w.tiles[pos].bounds_mm = index.bounds().unwrap();
    w.tiles[pos].tile_id = store
        .write_object(&index.to_bytes().unwrap(), ObjectKind::Json)
        .unwrap();
    store.write_world(&w).unwrap();
    (dir, shell)
}

/// A facade "skin" of `count` small triangles, each with its corners at
/// random points of one wall triangle of `shell`, 30 mm off the wall and
/// no higher than the shell's highest point. Unindexed, as `tools/contrib`
/// writes facades.
fn skin(shell: &GlbMesh, feature_id: &str, count: usize) -> GlbMesh {
    let top = shell.bounds_mm().unwrap().max[1];
    let walls: Vec<[[f64; 3]; 3]> = shell
        .nodes
        .iter()
        .flat_map(|n| &n.primitives)
        .filter(|p| p.material == Material::Wall)
        .flat_map(|p| {
            p.indices
                .as_chunks::<3>()
                .0
                .iter()
                .map(|t| t.map(|i| p.positions[i as usize].map(f64::from)))
        })
        .collect();
    let mut next = numbers(0x5EED);
    let (mut positions, mut normals) = (Vec::new(), Vec::new());
    while positions.len() < 3 * count {
        let [a, b, c] = walls[next(walls.len() as u64) as usize];
        let (ab, ac) = (
            [0, 1, 2].map(|k| b[k] - a[k]),
            [0, 1, 2].map(|k| c[k] - a[k]),
        );
        let n = [
            ab[1] * ac[2] - ab[2] * ac[1],
            ab[2] * ac[0] - ab[0] * ac[2],
            ab[0] * ac[1] - ab[1] * ac[0],
        ];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len < 1e-6 {
            continue; // a degenerate wall triangle
        }
        let n = n.map(|v| v / len);
        for _ in 0..3 {
            let (mut s, mut u) = (next(1001), next(1001));
            if s + u > 1000 {
                (s, u) = (1000 - s, 1000 - u);
            }
            let (s, u) = (s as f64 / 1000.0, u as f64 / 1000.0);
            let mm = [0, 1, 2]
                .map(|k| ((a[k] + ab[k] * s + ac[k] * u + n[k] * 0.03) * 1000.0).round() as i64);
            positions.push([mm[0], mm[1].min(top), mm[2]].map(mm_to_f32));
            normals.push(n.map(|v| v as f32));
        }
    }
    GlbMesh {
        nodes: vec![Node {
            name: feature_id.into(),
            primitives: vec![Primitive {
                material: Material::Wall,
                indices: (0..positions.len() as u32).collect(),
                positions,
                normals,
            }],
        }],
    }
}

/// A facade at the G7 maximum (49,800 triangles, just under 4 MiB) on a
/// building whose roof is divided into 19,200 triangles: everything is
/// within its budget, so G8 must measure the facade, and it passes. Found
/// by review: a shell lists its roof triangles before its walls, so G8
/// went through every roof triangle near a wall vertex before it met the
/// wall, and gave up as too complex.
#[test]
fn g8_measures_a_dense_facade_on_a_finely_divided_roof() {
    let (dense, shell) = world_with_shell("dense-roof", |shell| {
        for p in shell.nodes.iter_mut().flat_map(|n| &mut n.primitives) {
            if p.material == Material::Roof {
                for _ in 0..4 {
                    *p = subdivided(p);
                }
            }
        }
    });
    assert_eq!(shell.triangle_count(), 19_320);
    let mut draft = sample_draft(&dense).unwrap();
    draft.mesh = skin(&shell, &draft.change.changes[0].feature_id, 49_800);
    let bundle = common::temp_dir("bundle-dense-roof");
    write_bundle(&mut draft, &bundle).unwrap();
    let size = draft.manifest.size_bytes;
    assert!(
        size <= gate::FACADE_MAX_BYTES && size > gate::FACADE_MAX_BYTES / 100 * 99,
        "{size} bytes"
    );
    let candidate = common::temp_dir("candidate-dense-roof").join("candidate");
    let r = gate::run(&dense, &bundle, &candidate).expect("the gate runs");
    assert!(r.passed, "{r:#?}");
    for id in ["G1", "G7", "G8"] {
        assert_eq!(outcome(&r, id), Outcome::Pass, "{id}: {r:#?}");
    }
}

/// The committed sample facade on a world whose canonical shell of the
/// building has 149,760 triangles, more than the G7 shell budget. G7 checks
/// only what a change set proposes, so G8 measures against such a shell, and
/// the sample passes. Found by review: G8 was not run for any facade on such
/// a building, which then could never pass.
#[test]
fn g8_measures_against_a_canonical_shell_over_the_shell_budget() {
    let (big, shell) = world_with_shell("big-shell", |shell| {
        for p in shell.nodes.iter_mut().flat_map(|n| &mut n.primitives) {
            for _ in 0..4 {
                *p = subdivided(p);
            }
            *p = split_at_centroids(p);
        }
    });
    assert_eq!(shell.triangle_count(), 149_760);
    let candidate = common::temp_dir("candidate-big-shell").join("candidate");
    let r = gate::run(&big, &sample_bundle(), &candidate).expect("the gate runs");
    assert!(r.passed, "{r:#?}");
    for id in ["G1", "G7", "G8"] {
        assert_eq!(outcome(&r, id), Outcome::Pass, "{id}: {r:#?}");
    }
}

/// A shell and a facade crafted to keep G8 busy: 10,000 shell triangles,
/// each 4 km long, all cross the 2,000 columns of a facade 1 km away. G8
/// stops at its limit of grid steps, after a fraction of a second, and
/// fails the facade as too complex to check.
#[test]
fn g8_refuses_a_facade_too_complex_to_measure() {
    let (crafted, shell) = world_with_shell("crafted", |shell| {
        let line = [
            [-1000, 0, -1_000_000],
            [4_001_000, 0, -1_000_000],
            [-1000, 0, -999_990],
        ];
        shell.nodes.truncate(1);
        shell.nodes[0].primitives = vec![Primitive {
            material: Material::Wall,
            positions: line.iter().map(|p| p.map(mm_to_f32)).collect(),
            normals: vec![[0.0, 1.0, 0.0]; 3],
            indices: [0, 1, 2].repeat(10_000),
        }];
    });
    assert_eq!(shell.triangle_count(), 10_000);
    let mut draft = sample_draft(&crafted).unwrap();
    let positions: Vec<[f32; 3]> = (0..2001).map(|k| [mm_to_f32(2000 * k), 0.0, 0.0]).collect();
    draft.mesh.nodes[0].primitives = vec![Primitive {
        material: Material::Wall,
        normals: vec![[0.0, 0.0, 1.0]; positions.len()],
        indices: (0..positions.len() as u32).collect(),
        positions,
    }];
    let bundle = common::temp_dir("bundle-g8-crafted");
    write_bundle(&mut draft, &bundle).unwrap();
    let candidate = common::temp_dir("candidate-g8-crafted").join("candidate");
    let r = gate::run(&crafted, &bundle, &candidate).expect("the gate runs");
    assert_eq!(outcome(&r, "G7"), Outcome::Pass, "{r:#?}");
    assert_fails(&r, "G8");
    let g8 = r.checks.iter().find(|c| c.id == "G8").unwrap();
    assert_eq!(
        g8.details,
        [format!(
            "{} facade {}: too complex to check: measuring the facade against its shell takes more than {} grid steps",
            draft.change.changes[0].tile,
            draft.change.changes[0].feature_id,
            gate::FACADE_MAX_GRID_STEPS
        )]
    );
}

/// A change set that proposes a shell over its G7 budget, and a facade on
/// it: G7 refuses the shell, so G8 does not measure the facade against it.
#[test]
fn g8_does_not_measure_against_a_proposed_shell_over_its_budget() {
    let bundle = common::temp_dir("bundle-g8-proposed-shell");
    let mut draft = sample_draft(&world()).unwrap();
    write_bundle(&mut draft, &bundle).unwrap();
    // The building's shell, its roof listed again and again.
    let store = Store::new(world());
    let (_, w) = store.read_world().unwrap();
    let tile = draft.change.changes[0].tile;
    let wt = w.tiles.iter().find(|t| t.tile == tile).unwrap();
    let index =
        TileIndex::from_bytes(&store.read_object(&wt.tile_id, ObjectKind::Json).unwrap()).unwrap();
    let current = index
        .assets
        .iter()
        .find(|a| a.layer == Layer::Shell && a.feature_id == draft.change.changes[0].feature_id)
        .unwrap();
    let mut manifest = AssetManifest::from_bytes(
        &store
            .read_object(&current.manifest_id, ObjectKind::Json)
            .unwrap(),
    )
    .unwrap();
    let mut shell = glb::read(
        &store
            .read_object(&current.content_id, ObjectKind::Glb)
            .unwrap(),
    )
    .unwrap();
    let roof = &mut shell.nodes[0].primitives[0];
    assert_eq!(roof.material, Material::Roof);
    let once = roof.indices.clone();
    while shell.triangle_count() <= gate::SHELL_MAX_TRIANGLES {
        shell.nodes[0].primitives[0]
            .indices
            .extend_from_slice(&once);
    }
    let triangles = shell.triangle_count();
    let bytes = glb::write(&shell).unwrap();
    let objects = Store::new(&bundle);
    manifest.content_id = objects.write_object(&bytes, ObjectKind::Glb).unwrap();
    manifest.size_bytes = bytes.len() as u64;
    manifest.parent_version_id = Some(current.manifest_id);
    let mut replace = draft.change.changes[0].clone();
    replace.op = Op::Replace;
    replace.layer = Layer::Shell;
    replace.parent_version_id = Some(current.manifest_id);
    replace.manifest_id = Some(
        objects
            .write_object(&manifest.to_bytes().unwrap(), ObjectKind::Json)
            .unwrap(),
    );
    draft.change.changes.push(replace);
    std::fs::write(bundle.join("change.json"), draft.change.to_bytes().unwrap()).unwrap();

    let r = gate_run(&bundle, "g8-proposed-shell");
    assert_fails(&r, "G7");
    assert_eq!(outcome(&r, "G8"), Outcome::NotRun, "{r:#?}");
    let g8 = r.checks.iter().find(|c| c.id == "G8").unwrap();
    assert_eq!(
        g8.details,
        [format!(
            "{tile} facade {}: not checked, the shell this change set proposes has {triangles} triangles, more than the G7 budget of {}",
            draft.change.changes[0].feature_id,
            gate::SHELL_MAX_TRIANGLES
        )]
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
