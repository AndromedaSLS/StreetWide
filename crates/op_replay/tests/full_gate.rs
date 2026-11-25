//! The complete contribution gate (RFC 0002, ADR-0009) with the
//! simulation-based checks: G9 walkability and G10 replays.

use std::path::{Path, PathBuf};

use op_kernel::geometry::{m_to_mm, sim_origin_mm};
use op_replay::gate::check_change;
use op_replay::{Replay, play};
use op_stream::{StoreSource, WorldIndex};
use op_world::contrib::{Change, CheckReport, Op, Outcome};
use op_world::gate::{COLLECTIVE_CREDIT, CONTRIBUTIONS_DATASET};
use op_world::glb::{GlbMesh, Material, Node, Primitive, f32_to_mm, mm_to_f32};
use op_world::manifest::Source;
use op_world::sample_change::{Draft, SAMPLE_AUTHOR, sample_draft, write_bundle};
use op_world::{BoundsMm, Collider, Layer};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn world() -> PathBuf {
    repo().join("fixtures/world")
}

/// A fresh temporary directory, deleted again when this is dropped.
struct Temp(PathBuf);

impl std::ops::Deref for Temp {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp(name: &str) -> Temp {
    let dir = std::env::temp_dir().join(format!("op_replay-gate-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Temp(dir)
}

fn sample_replay() -> Replay {
    Replay::from_bytes(&std::fs::read(repo().join("fixtures/replays/sample.oprep")).unwrap())
        .unwrap()
}

fn run(name: &str, edit: impl FnOnce(&mut Draft)) -> CheckReport {
    let bundle = temp(&format!("bundle-{name}"));
    let mut draft = sample_draft(&world()).unwrap();
    edit(&mut draft);
    write_bundle(&mut draft, &bundle).unwrap();
    gate(&bundle, name)
}

fn gate(bundle: &Path, name: &str) -> CheckReport {
    let dir = temp(&format!("candidate-{name}"));
    check_change(&world(), bundle, &dir.join("candidate"), &[sample_replay()]).unwrap()
}

fn outcome(r: &CheckReport, id: &str) -> Outcome {
    r.checks.iter().find(|c| c.id == id).unwrap().outcome
}

/// Removes the swisstopo shell with this feature id.
fn remove_shell(d: &mut Draft, feature_id: &str) {
    let index = WorldIndex::open(&world()).unwrap();
    let tile = &index.tiles()[0];
    let shell = tile
        .assets
        .iter()
        .find(|a| a.layer == Layer::Shell && a.feature_id == feature_id)
        .unwrap();
    d.change.changes = vec![Change {
        op: Op::Remove,
        tile: tile.key,
        layer: Layer::Shell,
        feature_id: feature_id.into(),
        parent_version_id: Some(shell.manifest_id),
        manifest_id: None,
    }];
}

/// The sample walk's feet every second, in the region frame (mm).
fn sample_path() -> Vec<[i64; 3]> {
    let r = sample_replay();
    let origin = sim_origin_mm(r.header.spawn.position_mm);
    let mut source = StoreSource::open(&world(), origin).unwrap();
    let mut out = Vec::new();
    play(&r.header, &r.inputs, &mut source, |sim| {
        if sim.ticks() % 60 == 0 {
            let f = sim.player().feet().map(m_to_mm);
            out.push([f[0] + origin[0], f[1] + origin[1], f[2] + origin[2]]);
        }
    })
    .unwrap();
    out
}

#[test]
fn facades_never_reach_the_simulation_checks() {
    let r = gate(&repo().join("fixtures/changes/facade-sample"), "facade");
    assert!(r.passed);
    assert_eq!(outcome(&r, "G9"), Outcome::NotApplicable);
    assert_eq!(outcome(&r, "G10"), Outcome::NotApplicable);
    let committed =
        std::fs::read(repo().join("fixtures/changes/facade-sample/report.json")).unwrap();
    assert_eq!(
        r.to_bytes().unwrap(),
        committed,
        "the full gate agrees with op_world's report"
    );
}

#[test]
fn removing_the_house_the_sample_walk_runs_into_breaks_the_replay() {
    // The sample walk sprints west into this house's wall (stage 4).
    let r = run("remove-wall", |d| {
        remove_shell(
            d,
            "swisstopo:swissbuildings3d:f1e4f3ab-6046-439a-b6ba-bd74c711bee6",
        )
    });
    assert_eq!(outcome(&r, "G9"), Outcome::Pass, "{r:#?}");
    assert_eq!(outcome(&r, "G10"), Outcome::Fail, "{r:#?}");
    assert!(!r.passed);
}

#[test]
fn removing_a_building_away_from_the_path_keeps_the_replay() {
    let path = sample_path();
    let index = WorldIndex::open(&world()).unwrap();
    let tile = &index.tiles()[0];
    let far = tile
        .assets
        .iter()
        .filter(|a| a.layer == Layer::Shell && a.feature_id.starts_with("swisstopo:"))
        .find(|a| {
            let b = a.bounds_mm.translated(tile.origin_mm);
            path.iter().all(|p| {
                [0, 2]
                    .iter()
                    .any(|&k| p[k] < b.min[k] - 40_000 || p[k] > b.max[k] + 40_000)
            })
        })
        .expect("a building 40 m from the path");
    let fid = far.feature_id.clone();
    let r = run("remove-far", |d| remove_shell(d, &fid));
    assert_eq!(outcome(&r, "G10"), Outcome::Pass, "{r:#?}");
    assert_eq!(outcome(&r, "G9"), Outcome::Pass, "{r:#?}");
}

/// An axis-aligned box from `min` to `max` (mm), 24 vertices with face
/// normals.
fn block(min: [i64; 3], max: [i64; 3]) -> Primitive {
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::new();
    for axis in 0..3 {
        for side in [0, 1] {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            let base = positions.len() as u32;
            for (a, b) in [(0, 0), (1, 0), (1, 1), (0, 1)] {
                let mut p = [0i64; 3];
                p[axis] = if side == 0 { min[axis] } else { max[axis] };
                p[u] = if a == 0 { min[u] } else { max[u] };
                p[v] = if b == 0 { min[v] } else { max[v] };
                positions.push(p.map(mm_to_f32));
                let mut n = [0.0f32; 3];
                n[axis] = if side == 0 { -1.0 } else { 1.0 };
                normals.push(n);
            }
            indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
    Primitive {
        material: Material::Wall,
        positions,
        normals,
        indices,
    }
}

#[test]
fn a_block_on_a_street_fails_walkability() {
    let index = WorldIndex::open(&world()).unwrap();
    let tile = &index.tiles()[0];
    let ribbon = tile
        .assets
        .iter()
        .find(|a| a.layer == Layer::Transport && a.collider == Collider::None)
        .unwrap();
    let glb = index.load_mesh(&ribbon.content_id).unwrap();
    let p = &glb.nodes[0].primitives[0];
    // The ribbon triangle closest to the tile centre.
    let centre = p
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| {
            let mut c = [0i64; 3];
            for &i in t {
                for k in 0..3 {
                    c[k] += f32_to_mm(p.positions[i as usize][k]).unwrap();
                }
            }
            c.map(|v| v / 3)
        })
        .min_by_key(|c| c[0].abs() + c[2].abs())
        .unwrap();
    let fid = "streetwide:test-block";
    let r = run("block", |d| {
        d.mesh = GlbMesh {
            nodes: vec![Node {
                name: fid.into(),
                primitives: vec![block(
                    [centre[0] - 1_500, centre[1] - 300, centre[2] - 1_500],
                    [centre[0] + 1_500, centre[1] + 3_000, centre[2] + 1_500],
                )],
            }],
        };
        d.manifest.layer = Layer::Shell;
        d.manifest.feature_id = fid.into();
        d.manifest.physics.collider = Collider::Trimesh;
        d.manifest.fictional = true;
        d.manifest.bounds_mm = BoundsMm {
            min: [0; 3],
            max: [0; 3],
        };
        d.manifest.provenance.license = "CC-BY-SA-4.0".into();
        d.manifest.provenance.attribution = vec![COLLECTIVE_CREDIT.into()];
        d.manifest.provenance.sources = vec![Source {
            dataset: CONTRIBUTIONS_DATASET.into(),
            version: "0.1.0".into(),
            license: "CC-BY-SA-4.0".into(),
            attribution: COLLECTIVE_CREDIT.into(),
            url: "https://github.com/AndromedaSLS/StreetWide".into(),
            source_ids: vec![SAMPLE_AUTHOR.into()],
        }];
        d.change.changes[0].layer = Layer::Shell;
        d.change.changes[0].feature_id = fid.into();
    });
    for id in ["G1", "G2", "G3", "G4", "G5", "G7"] {
        assert_eq!(outcome(&r, id), Outcome::Pass, "{id}: {r:#?}");
    }
    assert_eq!(outcome(&r, "G9"), Outcome::Fail, "{r:#?}");
    assert!(!r.passed);
}
