//! Helpers for the merge, rerecord and compare-worlds tests: private copies
//! of the fixture world, the change sets of `full_gate.rs`, and a world log
//! checked file by file.

#![allow(dead_code)] // each test binary uses a different subset

use std::path::{Path, PathBuf};
use std::process::Command;

use op_kernel::geometry::{m_to_mm, sim_origin_mm};
use op_replay::{Replay, play};
use op_stream::{StoreSource, WorldIndex};
use op_world::contrib::{
    Change, ChangeSet, CheckReport, MergeRecord, Op, Review, Verdict, verify_log,
};
use op_world::gate::{COLLECTIVE_CREDIT, CONTRIBUTIONS_DATASET};
use op_world::glb::{GlbMesh, Material, Node, Primitive, f32_to_mm, mm_to_f32};
use op_world::manifest::Source;
use op_world::sample_change::{SAMPLE_AUTHOR, sample_draft, write_bundle};
use op_world::{BoundsMm, Collider, ContentId, Layer, Store};

pub fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn fixture_world() -> PathBuf {
    repo().join("fixtures/world")
}

pub fn facade_sample() -> PathBuf {
    repo().join("fixtures/changes/facade-sample")
}

pub fn sample_path_file() -> PathBuf {
    repo().join("fixtures/replays/sample.oprep")
}

pub fn sample_replay() -> Replay {
    Replay::from_bytes(&read(sample_path_file())).unwrap()
}

pub fn read(path: impl AsRef<Path>) -> Vec<u8> {
    let path = path.as_ref();
    std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

pub fn world_id(world: &Path) -> ContentId {
    Store::new(world).read_world().unwrap().0
}

/// A fresh temporary directory, deleted again at the end of the test.
pub struct Temp(PathBuf);

impl Temp {
    pub fn new(name: &str) -> Temp {
        let dir =
            std::env::temp_dir().join(format!("op_replay-tools-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }

    pub fn join(&self, path: &str) -> PathBuf {
        self.0.join(path)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// All files under `dir`, as sorted relative paths with `/` separators.
pub fn files_under(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                let rel = path.strip_prefix(base).unwrap();
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

pub fn copy_dir(from: &Path, to: &Path) {
    for f in files_under(from) {
        let dst = to.join(&f);
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        std::fs::copy(from.join(&f), dst).unwrap();
    }
}

/// A private copy of the fixture world in `t/<name>`.
pub fn world_copy(t: &Temp, name: &str) -> PathBuf {
    let dir = t.join(name);
    copy_dir(&fixture_world(), &dir);
    dir
}

pub fn better() -> Review {
    Review {
        reviewer: "github:reviewer".into(),
        verdict: Verdict::Better,
        notes: "Checked (test).".into(),
    }
}

/// The sample walk's feet every second in the fixture world, region frame
/// (mm).
pub fn sample_path() -> Vec<[i64; 3]> {
    let r = sample_replay();
    let origin = sim_origin_mm(r.header.spawn.position_mm);
    let mut source = StoreSource::open(&fixture_world(), origin).unwrap();
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

/// The house the sample walk sprints into (`full_gate.rs`).
pub const WALL_HOUSE: &str = "swisstopo:swissbuildings3d:f1e4f3ab-6046-439a-b6ba-bd74c711bee6";

/// The building the sample facade sits on.
pub fn facade_shell() -> String {
    sample_draft(&fixture_world()).unwrap().change.changes[0]
        .feature_id
        .clone()
}

/// The swisstopo buildings 40 m or more from the sample walk, in tile
/// order: the first is `full_gate.rs`'s far building.
pub fn far_shells() -> Vec<String> {
    let path = sample_path();
    let index = WorldIndex::open(&fixture_world()).unwrap();
    let tile = &index.tiles()[0];
    tile.assets
        .iter()
        .filter(|a| a.layer == Layer::Shell && a.feature_id.starts_with("swisstopo:"))
        .filter(|a| {
            let b = a.bounds_mm.translated(tile.origin_mm);
            path.iter().all(|p| {
                [0, 2]
                    .iter()
                    .any(|&k| p[k] < b.min[k] - 40_000 || p[k] > b.max[k] + 40_000)
            })
        })
        .map(|a| a.feature_id.clone())
        .collect()
}

/// A change set planned on `world` that removes these shells, written to
/// `bundle`.
pub fn removal(world: &Path, shells: &[&str], author: &str, bundle: &Path) -> ChangeSet {
    let index = WorldIndex::open(world).unwrap();
    let tile = &index.tiles()[0];
    let mut changes: Vec<Change> = shells
        .iter()
        .map(|fid| {
            let shell = tile
                .assets
                .iter()
                .find(|a| a.layer == Layer::Shell && a.feature_id == *fid)
                .unwrap_or_else(|| panic!("no shell {fid}"));
            Change {
                op: Op::Remove,
                tile: tile.key,
                layer: Layer::Shell,
                feature_id: fid.to_string(),
                parent_version_id: Some(shell.manifest_id),
                manifest_id: None,
            }
        })
        .collect();
    changes.sort_by(|a, b| a.feature_id.cmp(&b.feature_id));
    let change = ChangeSet {
        format: ChangeSet::FORMAT.into(),
        contract_version: "0.1.0".into(),
        base_world_id: index.world_id(),
        title: format!("Remove {} building(s) (test)", shells.len()),
        author: author.into(),
        rationale: "Demolished (test).".into(),
        changes,
    };
    std::fs::create_dir_all(bundle).unwrap();
    std::fs::write(bundle.join("change.json"), change.to_bytes().unwrap()).unwrap();
    change
}

/// `full_gate.rs`'s far-building removal: the first swisstopo building
/// 40 m from the sample walk, removed by `streetwide:fixtures`.
pub fn far_removal(world: &Path, bundle: &Path) -> ChangeSet {
    removal(world, &[far_shells()[0].as_str()], SAMPLE_AUTHOR, bundle)
}

/// An axis-aligned box from `min` to `max` (mm), as in `full_gate.rs`.
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

/// `full_gate.rs`'s 3 m block on the street nearest the tile centre, a
/// contributed shell with a collider, written to `bundle`.
pub fn block_on_a_street(bundle: &Path) {
    let index = WorldIndex::open(&fixture_world()).unwrap();
    let tile = &index.tiles()[0];
    let ribbon = tile
        .assets
        .iter()
        .find(|a| a.layer == Layer::Transport && a.collider == Collider::None)
        .unwrap();
    let glb = index.load_mesh(&ribbon.content_id).unwrap();
    let p = &glb.nodes[0].primitives[0];
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
    let mut d = sample_draft(&fixture_world()).unwrap();
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
    write_bundle(&mut d, bundle).unwrap();
}

/// Applies the change set in `bundle` to `world` without asking whether it
/// may be merged: copies in the bundle's objects and the candidate world
/// that `op_world`'s gate builds. For tests that need a world the gate
/// would refuse.
pub fn apply_unchecked(world: &Path, bundle: &Path, scratch: &Path) -> ContentId {
    let candidate = scratch.join("candidate");
    let report = op_world::gate::run(world, bundle, &candidate).unwrap();
    let id = report.candidate_world_id.expect("a candidate world");
    if bundle.join("objects").is_dir() {
        copy_dir(&bundle.join("objects"), &world.join("objects"));
    }
    copy_dir(&candidate, world);
    assert_eq!(world_id(world), id);
    let v = op_world::validate::validate_store(&Store::new(world));
    assert!(v.errors.is_empty(), "{:?}", v.errors);
    id
}

/// Checks a log directory file by file: `log/00000000.json` onwards, and
/// for every record its change set, report and world under their hex ids,
/// and nothing else. Returns the records.
pub fn check_log(dir: &Path) -> Vec<MergeRecord> {
    let files = files_under(dir);
    let (mut records, mut expected) = (Vec::new(), Vec::new());
    for (i, name) in files.iter().filter(|f| f.starts_with("log/")).enumerate() {
        assert_eq!(*name, format!("log/{i:08}.json"));
        let bytes = read(dir.join(name));
        let r = MergeRecord::from_bytes(&bytes).unwrap();
        let mut named = vec![(
            format!("worlds/{}.json", r.world_after.hex()),
            r.world_after,
        )];
        if let Some(id) = r.change_id {
            named.push((format!("changes/{}/change.json", id.hex()), id));
        }
        if let Some(id) = r.check_report_id {
            let path = format!("reports/{}.json", id.hex());
            CheckReport::from_bytes(&read(dir.join(&path))).unwrap();
            named.push((path, id));
        }
        for (path, id) in named {
            assert_eq!(ContentId::of(&read(dir.join(&path))), id, "{path}");
            expected.push(path);
        }
        expected.push(name.clone());
        records.push((ContentId::of(&bytes), r));
    }
    expected.sort();
    expected.dedup();
    assert_eq!(files, expected, "the log holds its records and their files");
    verify_log(&records).unwrap();
    records.into_iter().map(|(_, r)| r).collect()
}

/// Runs the `op_replay` binary: exit code, stdout, stderr.
pub fn op_replay(args: &[&dyn AsRef<std::ffi::OsStr>]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_op_replay"))
        .args(args.iter().map(|a| a.as_ref()))
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}
