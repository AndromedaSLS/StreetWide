//! Fuzzing of the world-format readers (roadmap M1, "Before announcing"):
//! canonical JSON and every document type with its rules, the glb reader,
//! the contribution documents and the world log, content ids, cell keys,
//! store paths and reads, and whole-store validation.
//!
//! Every reader of untrusted bytes must return an error, never panic, hang,
//! overflow or allocate without bound; what a reader accepts must re-encode
//! to exactly the bytes it read. The engine, its environment variables
//! (`STREETWIDE_FUZZ_ITERS`, `STREETWIDE_FUZZ_SEED`) and how a failure is
//! reported are described in `fuzzing/mod.rs`. Inputs that once broke a
//! reader are pinned at the end of this file.

mod common;
mod fuzzing;

use std::collections::BTreeMap;
use std::path::{Component, Path};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use fuzzing::{Rng, Target};
use op_world::cjson;
use op_world::contrib::{
    CONTRIBUTION_CONTRACT_VERSION, ChangeSet, CheckReport, Decision, MergeRecord, Review, Verdict,
    verify_log,
};
use op_world::gate;
use op_world::glb::{self, GlbMesh, Material, Node, Primitive, mm_to_f32};
use op_world::keys::{RegionKey, TileKey};
use op_world::rules;
use op_world::sample::parse_mesh_specs;
use op_world::validate::validate_store;
use op_world::{AssetManifest, ContentId, ObjectKind, Store, TileIndex, WorldManifest};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

// ----------------------------------------------------------------- corpus --

struct Doc {
    bytes: Vec<u8>,
    value: Option<Value>,
}

impl Doc {
    fn new(bytes: Vec<u8>) -> Self {
        let value = serde_json::from_slice(&bytes).ok();
        Doc { bytes, value }
    }

    fn format(&self) -> Option<&str> {
        self.value.as_ref()?.get("format")?.as_str()
    }
}

struct Corpus {
    /// Canonical JSON documents of every kind, plus hand-made edge cases.
    json: Vec<Doc>,
    /// Indexes into `json`, grouped by document kind (`format`), so that
    /// every kind gets fuzzed as often (most fixtures are asset manifests).
    kinds: Vec<Vec<usize>>,
    glb: Vec<Vec<u8>>,
}

fn corpus() -> &'static Corpus {
    static CORPUS: OnceLock<Corpus> = OnceLock::new();
    CORPUS.get_or_init(|| {
        let files = fuzzing::fixture_files(&[
            "fixtures/format",
            "fixtures/world",
            "fixtures/changes",
            "fixtures/replays",
        ]);
        let mut json = Vec::new();
        let mut glbs = Vec::new();
        for (name, bytes) in files {
            if name.ends_with(".json") {
                json.push(Doc::new(bytes));
            } else if name.ends_with(".glb") {
                glbs.push(bytes);
            } else if name.ends_with(".oprep") {
                // A replay's header is canonical JSON too.
                let len = u32::from_le_bytes(bytes[12..16].try_into().expect("4 bytes")) as usize;
                json.push(Doc::new(bytes[16..16 + len].to_vec()));
            }
        }
        json.extend(hand_made_json().into_iter().map(Doc::new));
        glbs.extend(hand_made_glbs());
        assert!(json.len() > 50 && glbs.len() > 40, "fixtures are missing");
        let mut by_kind: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for (i, d) in json.iter().enumerate() {
            by_kind
                .entry(d.format().unwrap_or("other"))
                .or_default()
                .push(i);
        }
        let kinds = by_kind.into_values().collect();
        Corpus {
            json,
            kinds,
            glb: glbs,
        }
    })
}

fn hand_made_json() -> Vec<Vec<u8>> {
    let mut docs: Vec<Vec<u8>> = [
        "{}",
        "[]",
        "null",
        "0",
        "\"\"",
        "{\"a\":null}",
        "{\"a\":9007199254740991,\"b\":-9007199254740991}",
        "{\"a\":[1,-2,null,true,false],\"b\":{\"c\":\"\\\\ \\\" \u{e9}\"}}",
    ]
    .iter()
    .map(|s| s.as_bytes().to_vec())
    .collect();
    // Nesting at the JSON parser's recursion limit (128).
    for depth in [127, 128, 129] {
        docs.push(format!("{}{}", "[".repeat(depth), "]".repeat(depth)).into_bytes());
    }
    for (_, record) in sample_log() {
        docs.push(record.to_bytes().expect("valid merge record"));
    }
    docs.push(largest_change_set());
    docs
}

/// The sample change set with the most changes a change set may have.
fn largest_change_set() -> Vec<u8> {
    let path = fuzzing::repo_root().join("fixtures/changes/facade-sample/change.json");
    let mut change = ChangeSet::from_bytes(&std::fs::read(path).expect("sample change set"))
        .expect("valid sample change set");
    let first = change.changes[0].clone();
    change.changes = (0..op_world::contrib::MAX_CHANGES)
        .map(|i| {
            let mut c = first.clone();
            c.feature_id = format!("{}-{i:02}", first.feature_id);
            c
        })
        .collect();
    change.to_bytes().expect("valid change set")
}

fn triangle(material: Material, x_mm: i64) -> Primitive {
    Primitive {
        material,
        positions: vec![
            [mm_to_f32(x_mm), 0.0, 0.0],
            [mm_to_f32(x_mm + 1000), 0.0, 0.0],
            [mm_to_f32(x_mm), 0.0, mm_to_f32(1000)],
        ],
        normals: vec![[0.0, 1.0, 0.0]; 3],
        indices: vec![0, 2, 1],
    }
}

fn hand_made_glbs() -> Vec<Vec<u8>> {
    let meshes = [
        GlbMesh {
            nodes: vec![Node {
                name: "osm:way:1".into(),
                primitives: vec![triangle(Material::Road, 0)],
            }],
        },
        GlbMesh {
            nodes: vec![
                Node {
                    name: "a:1".into(),
                    primitives: vec![triangle(Material::Roof, -5), triangle(Material::Wall, 7)],
                },
                Node {
                    name: "a:2".into(),
                    // 1000 km away: long millimetre decimals in min and max.
                    primitives: vec![triangle(Material::Water, 1_000_000_007)],
                },
            ],
        },
    ];
    meshes
        .iter()
        .map(|m| glb::write(m).expect("valid hand-made mesh"))
        .collect()
}

/// A world log: baseline, merged, takedown, merged.
fn sample_log() -> Vec<(ContentId, MergeRecord)> {
    let world = |n: u8| ContentId::of(&[b'w', n]);
    let mut log: Vec<(ContentId, MergeRecord)> = Vec::new();
    for (n, decision) in [
        Decision::Baseline,
        Decision::Merged,
        Decision::Takedown,
        Decision::Merged,
    ]
    .into_iter()
    .enumerate()
    {
        let n = n as u8;
        let previous = log.last().map(|(id, r)| (*id, r.world_after));
        let linked = |id: ContentId| (decision != Decision::Baseline).then_some(id);
        let record = MergeRecord {
            format: MergeRecord::FORMAT.into(),
            contract_version: CONTRIBUTION_CONTRACT_VERSION.into(),
            decision,
            change_id: linked(ContentId::of(&[b'c', n])),
            world_before: previous.map(|(_, w)| w),
            world_after: world(n),
            check_report_id: linked(ContentId::of(&[b'r', n])),
            reviews: if decision == Decision::Baseline {
                Vec::new()
            } else {
                vec![Review {
                    reviewer: "github:reviewer".into(),
                    verdict: Verdict::Better,
                    notes: "Checked.".into(),
                }]
            },
            reasons: format!("Record {n}."),
            previous_record_id: previous.map(|(id, _)| id),
        };
        let id = ContentId::of(&record.to_bytes().expect("valid record"));
        log.push((id, record));
    }
    log
}

// ------------------------------------------------- canonical JSON documents --

fn mutate_doc(rng: &mut Rng, c: &Corpus) -> Vec<u8> {
    let kind = rng.pick(&c.kinds);
    let doc = &c.json[*rng.pick(kind)];
    let donor = if rng.chance(70) {
        &c.json[*rng.pick(kind)]
    } else {
        rng.pick(&c.json)
    };
    fuzzing::mutate_json(
        rng,
        &doc.bytes,
        doc.value.as_ref(),
        &donor.bytes,
        donor.value.as_ref(),
    )
}

/// `from_bytes` must accept exactly the canonical documents without
/// problems, and `problems()` must cope with any canonical document.
fn consistent<T: Serialize + DeserializeOwned>(
    bytes: &[u8],
    problems: fn(&T) -> Vec<String>,
    accepts: fn(&[u8]) -> bool,
) {
    if let Ok(doc) = cjson::from_canonical::<T>(bytes) {
        assert_eq!(
            accepts(bytes),
            problems(&doc).is_empty(),
            "from_bytes and problems() disagree"
        );
    }
}

fn check_documents(bytes: &[u8]) {
    if let Ok(v) = cjson::from_canonical::<Value>(bytes) {
        let again = cjson::to_vec(&v).expect("accepted JSON re-encodes");
        assert!(again == bytes, "accepted JSON is not canonical");
    }
    let id = ContentId::of(bytes);
    if let Ok(d) = AssetManifest::from_bytes(bytes) {
        assert!(d.problems().is_empty());
        assert_eq!(d.version_id().expect("valid manifest"), id);
    }
    if let Ok(d) = TileIndex::from_bytes(bytes) {
        assert!(d.problems().is_empty());
        assert_eq!(d.id().expect("valid tile index"), id);
        assert!(d.bounds().is_some_and(|b| b.is_valid()));
    }
    if let Ok(d) = WorldManifest::from_bytes(bytes) {
        assert!(d.problems().is_empty());
        assert_eq!(d.id().expect("valid world manifest"), id);
    }
    if let Ok(d) = ChangeSet::from_bytes(bytes) {
        assert!(d.problems().is_empty());
        assert_eq!(d.id().expect("valid change set"), id);
    }
    if let Ok(d) = CheckReport::from_bytes(bytes) {
        assert!(d.problems().is_empty());
        assert!(d.to_bytes().expect("valid check report") == bytes);
    }
    if let Ok(d) = MergeRecord::from_bytes(bytes) {
        assert!(d.problems().is_empty());
        assert!(d.to_bytes().expect("valid merge record") == bytes);
        let alone = verify_log(&[(id, d.clone())]);
        assert_eq!(alone.is_ok(), d.decision == Decision::Baseline);
    }
    consistent(bytes, AssetManifest::problems, |b| {
        AssetManifest::from_bytes(b).is_ok()
    });
    consistent(bytes, TileIndex::problems, |b| {
        TileIndex::from_bytes(b).is_ok()
    });
    consistent(bytes, WorldManifest::problems, |b| {
        WorldManifest::from_bytes(b).is_ok()
    });
    consistent(bytes, ChangeSet::problems, |b| {
        ChangeSet::from_bytes(b).is_ok()
    });
    consistent(bytes, CheckReport::problems, |b| {
        CheckReport::from_bytes(b).is_ok()
    });
    consistent(bytes, MergeRecord::problems, |b| {
        MergeRecord::from_bytes(b).is_ok()
    });
    // Mesh specs (`fixtures/format/meshes.json`, plain JSON): any mesh the
    // writer accepts reads back unchanged.
    if let Ok(specs) = parse_mesh_specs(bytes) {
        for spec in specs.values() {
            if let Ok(mesh) = spec.to_mesh()
                && let Ok(written) = glb::write(&mesh)
            {
                assert_eq!(glb::read(&written).expect("a written mesh reads"), mesh);
            }
        }
    }
}

#[test]
fn canonical_json_and_documents() {
    let c = corpus();
    fuzzing::run(
        Target::new("documents", 6000),
        |rng| mutate_doc(rng, c),
        check_documents,
    );
}

// -------------------------------------------------------------------- glb --

fn mutate_glb(rng: &mut Rng, base: &[u8], donor: &[u8]) -> Vec<u8> {
    match rng.below(10) {
        0..=4 => {
            let Some((json, bin)) = fuzzing::glb_chunks(base) else {
                return base.to_vec();
            };
            let (mut json, mut bin) = (json.to_vec(), bin.to_vec());
            for _ in 0..rng.count(3) {
                match rng.below(5) {
                    0 | 1 => {
                        if let Ok(mut doc) = serde_json::from_slice::<Value>(&json) {
                            fuzzing::edit_glb_json(rng, &mut doc, bin.len());
                            json = serde_json::to_vec(&doc).expect("a JSON value serialises");
                        }
                    }
                    2 => {
                        if let Ok(mut doc) = serde_json::from_slice::<Value>(&json) {
                            fuzzing::edit_value(rng, &mut doc, None);
                            json = serde_json::to_vec(&doc).expect("a JSON value serialises");
                        }
                    }
                    3 => {
                        if let Some(edited) = fuzzing::mutate_json_text(rng, &json) {
                            json = edited;
                        }
                    }
                    _ => fuzzing::edit_glb_bin(rng, &mut bin),
                }
            }
            fuzzing::glb_pack(&json, &bin)
        }
        5 | 6 => {
            // The header's and the chunks' length and type fields.
            let mut out = base.to_vec();
            let json_len = fuzzing::glb_chunks(base).map_or(0, |(j, _)| j.len());
            let at = *rng.pick(&[0, 4, 8, 12, 16, 20 + json_len, 24 + json_len]);
            if out.len() >= at + 4 {
                let current = u32::from_le_bytes(out[at..at + 4].try_into().expect("4 bytes"));
                let v = fuzzing::interesting_u32(rng, out.len(), current);
                out[at..at + 4].copy_from_slice(&v.to_le_bytes());
            }
            out
        }
        _ => {
            let mut out = base.to_vec();
            fuzzing::mutate_bytes(rng, &mut out, donor);
            out
        }
    }
}

fn check_glb(bytes: &[u8]) {
    if let Ok(mesh) = glb::read(bytes) {
        mesh.validate().expect("an accepted mesh is valid");
        assert!(glb::write(&mesh).expect("an accepted mesh re-writes") == bytes);
        assert!(mesh.bounds_mm().is_some_and(|b| b.is_valid()));
        assert!(mesh.triangle_count() > 0);
    }
}

#[test]
fn glb_reader() {
    let c = corpus();
    fuzzing::run(
        Target::new("glb", 8000).parts(fuzzing::glb_split, fuzzing::glb_join),
        |rng| {
            let base = rng.pick(&c.glb);
            let donor = rng.pick(&c.glb);
            mutate_glb(rng, base, donor)
        },
        check_glb,
    );
}

/// A random mesh inside the profile: up to 5 nodes, each with up to 8
/// primitives of distinct palette materials in palette order.
fn random_mesh(rng: &mut Rng) -> GlbMesh {
    let mut nodes = Vec::new();
    for n in 0..rng.range(1, 5) {
        let mut primitives = Vec::new();
        for material in Material::PALETTE {
            if !rng.chance(35) {
                continue;
            }
            let count = rng.range(1, 40);
            let mut positions = Vec::with_capacity(count);
            let mut normals = Vec::with_capacity(count);
            for _ in 0..count {
                let mut p = [0.0; 3];
                let mut q = [0.0; 3];
                for k in 0..3 {
                    p[k] = mm_to_f32(rng.range_i(-5_000_000_000, 5_000_000_000));
                    q[k] = rng.range_i(-1000, 1000) as f32 / 1000.0;
                }
                positions.push(p);
                normals.push(q);
            }
            let indices = (0..3 * rng.range(1, 30))
                .map(|_| rng.below(count) as u32)
                .collect();
            primitives.push(Primitive {
                material,
                positions,
                normals,
                indices,
            });
        }
        if primitives.is_empty() {
            primitives.push(triangle(*rng.pick(&Material::PALETTE), 0));
        }
        nodes.push(Node {
            name: format!("osm:way:{n}"),
            primitives,
        });
    }
    GlbMesh { nodes }
}

/// Whatever the reference writer produces, the reader accepts unchanged:
/// the reader's limits never refuse a valid file.
#[test]
fn written_meshes_read_back() {
    fuzzing::run(
        Target::new("glb-written", 2000),
        |rng| glb::write(&random_mesh(rng)).expect("a mesh inside the profile writes"),
        |bytes| {
            let mesh = glb::read(bytes).expect("what the writer produces reads");
            assert!(glb::write(&mesh).expect("re-writes") == bytes);
        },
    );
}

// ------------------------------------------------ ids, keys and store paths --

fn check_ids_and_keys(bytes: &[u8]) {
    let s = String::from_utf8_lossy(bytes);
    if let Ok(id) = ContentId::parse(&s) {
        assert_eq!(id.to_string(), &*s);
        let via_serde: ContentId =
            serde_json::from_value(Value::String(s.to_string())).expect("serde agrees");
        assert_eq!(via_serde, id);
        // Object paths stay inside the store: objects/<aa>/<hex>.<ext>.
        let root = Path::new("store");
        for kind in [ObjectKind::Json, ObjectKind::Glb] {
            let path = Store::new(root).object_path(&id, kind);
            let parts: Vec<String> = path
                .strip_prefix(root)
                .expect("inside the store")
                .components()
                .map(|c| match c {
                    Component::Normal(p) => p.to_string_lossy().into_owned(),
                    other => panic!("{other:?} in an object path"),
                })
                .collect();
            let hex = id.hex();
            assert_eq!(
                parts,
                [
                    "objects".to_string(),
                    hex[..2].to_string(),
                    format!("{hex}.{}", kind.extension())
                ]
            );
        }
    }
    if let Ok(k) = TileKey::parse(&s) {
        assert_eq!(k.to_string(), &*s);
        assert!(k.x < TileKey::SIZE && k.y < TileKey::SIZE);
        let r = k.region();
        assert!(r.x < RegionKey::SIZE && r.y < RegionKey::SIZE);
    }
    if let Ok(k) = RegionKey::parse(&s) {
        assert_eq!(k.to_string(), &*s);
    }
    let _ = (
        rules::is_text(&s),
        rules::is_feature_id(&s),
        rules::is_license(&s),
        rules::is_https_url(&s),
        rules::is_slug(&s),
    );
}

#[test]
fn ids_keys_and_object_paths() {
    fuzzing::run(
        Target::new("ids-keys", 50_000),
        |rng| {
            let base = match rng.below(4) {
                0 => ContentId::of(&rng.next_u64().to_le_bytes()).to_string(),
                1 => format!("17/{}/{}", rng.below(140_000), rng.below(140_000)),
                2 => format!("12/{}/{}", rng.below(5000), rng.below(5000)),
                _ => fuzzing::interesting_string(rng),
            };
            let mut bytes = base.into_bytes();
            if rng.chance(70) {
                fuzzing::mutate_bytes(rng, &mut bytes, b"17/68646/45903 blake3:0123456789abcdef");
            }
            bytes
        },
        check_ids_and_keys,
    );
}

// ------------------------------------------------------------- store reads --

#[test]
fn store_reads() {
    let c = corpus();
    let worlds: Vec<&Doc> = c
        .json
        .iter()
        .filter(|d| d.format() == Some(WorldManifest::FORMAT))
        .collect();
    assert!(!worlds.is_empty());
    let dir = common::temp_dir("fuzz-store");
    let store = Store::new(&dir);
    let empty_root = dir.join("empty");
    fuzzing::run(
        Target::new("store", 300),
        |rng| {
            let doc = rng.pick(&worlds);
            let donor = rng.pick(&c.json);
            fuzzing::mutate_json(
                rng,
                &doc.bytes,
                doc.value.as_ref(),
                &donor.bytes,
                donor.value.as_ref(),
            )
        },
        |bytes| {
            std::fs::write(store.world_path(), bytes).expect("write world.json");
            if let Ok((id, world)) = store.read_world() {
                assert_eq!(id, ContentId::of(bytes));
                assert!(world.to_bytes().expect("a world read is valid") == bytes);
            }
            for kind in [ObjectKind::Json, ObjectKind::Glb] {
                // Readable under its own id, directly and through a fallback.
                let id = store.write_object(bytes, kind).expect("write object");
                assert_eq!(id, ContentId::of(bytes));
                assert!(store.has_object(&id, kind));
                assert!(store.read_object(&id, kind).expect("read back") == bytes);
                let layered = Store::layered(&empty_root, vec![dir.clone()]);
                assert!(layered.read_object(&id, kind).expect("found in a fallback") == bytes);
                // Under any other id, a hash mismatch.
                let other = ContentId::of(&[bytes, b"~".as_slice()].concat());
                let path = store.object_path(&other, kind);
                std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
                std::fs::write(&path, bytes).expect("write misnamed object");
                assert!(matches!(
                    store.read_object(&other, kind),
                    Err(op_world::Error::HashMismatch { .. })
                ));
                std::fs::remove_file(&path).expect("remove misnamed object");
            }
        },
    );
    std::fs::remove_dir_all(&dir).ok();
}

// ------------------------------------------------------ whole-store checks --

/// A world store in memory: `world.json` and its objects.
#[derive(Clone)]
struct Files {
    world: Vec<u8>,
    objects: Vec<(ObjectKind, Vec<u8>)>,
}

fn format_world() -> Files {
    let dir = fuzzing::repo_root().join("fixtures/format/world");
    let mut objects = Vec::new();
    for (name, bytes) in fuzzing::fixture_files(&["fixtures/format/world/objects"]) {
        let kind = if name.ends_with(".glb") {
            ObjectKind::Glb
        } else {
            ObjectKind::Json
        };
        objects.push((kind, bytes));
    }
    Files {
        world: std::fs::read(dir.join("world.json")).expect("world.json"),
        objects,
    }
}

fn replace(haystack: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(haystack.len());
    let mut i = 0;
    while i < haystack.len() {
        if haystack[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(haystack[i]);
            i += 1;
        }
    }
    out
}

/// After an object changed from `old` to `new` bytes, makes everything
/// that named it name the new version (and so on up to the root document),
/// so the store stays content-addressed and the checks reach the change. A
/// manifest naming a changed mesh also gets the mesh's new `size_bytes`.
fn readdress(files: &mut Files, old: &[u8], new: &[u8], mesh: bool) {
    let size = (
        format!("\"size_bytes\":{}", old.len()),
        format!("\"size_bytes\":{}", new.len()),
    );
    let mut pending = vec![(
        ContentId::of(old).to_string(),
        ContentId::of(new).to_string(),
        mesh,
    )];
    let mut steps = 0;
    while let Some((from, to, names_mesh)) = pending.pop() {
        steps += 1;
        if steps > 64 {
            break;
        }
        files.world = replace(&files.world, from.as_bytes(), to.as_bytes());
        for (kind, bytes) in &mut files.objects {
            if *kind == ObjectKind::Json && bytes.windows(from.len()).any(|w| w == from.as_bytes())
            {
                let mut updated = replace(bytes, from.as_bytes(), to.as_bytes());
                if names_mesh {
                    updated = replace(&updated, size.0.as_bytes(), size.1.as_bytes());
                }
                pending.push((
                    ContentId::of(bytes).to_string(),
                    ContentId::of(&updated).to_string(),
                    false,
                ));
                *bytes = updated;
            }
        }
    }
}

/// Mutates the root document or one object (then re-addresses), a few
/// times. Root-document donors are documents of the same `root_kind`.
fn mutate_files(rng: &mut Rng, base: &Files, c: &Corpus, root_kind: &str) -> Files {
    let mut files = base.clone();
    let roots: Vec<&Doc> = c
        .json
        .iter()
        .filter(|d| d.format() == Some(root_kind))
        .collect();
    for _ in 0..rng.count(3) {
        let k = rng.below(files.objects.len() + 1);
        if k == files.objects.len() {
            let doc = Doc::new(files.world.clone());
            let donor = if roots.is_empty() {
                rng.pick(&c.json)
            } else {
                *rng.pick(&roots)
            };
            files.world = fuzzing::mutate_json(
                rng,
                &doc.bytes,
                doc.value.as_ref(),
                &donor.bytes,
                donor.value.as_ref(),
            );
            continue;
        }
        let (kind, old) = files.objects[k].clone();
        let new = match kind {
            ObjectKind::Glb => {
                let donor = rng.pick(&c.glb);
                mutate_glb(rng, &old, donor)
            }
            ObjectKind::Json => {
                let doc = Doc::new(old.clone());
                let kind = doc.format().unwrap_or("other").to_string();
                let same: Vec<&Doc> = c
                    .json
                    .iter()
                    .filter(|d| d.format().unwrap_or("other") == kind)
                    .collect();
                // An earlier edit may have given it a kind no fixture has.
                let donor = if same.is_empty() {
                    rng.pick(&c.json)
                } else {
                    *rng.pick(&same)
                };
                fuzzing::mutate_json(
                    rng,
                    &doc.bytes,
                    doc.value.as_ref(),
                    &donor.bytes,
                    donor.value.as_ref(),
                )
            }
        };
        files.objects.push((kind, new.clone()));
        readdress(&mut files, &old, &new, kind == ObjectKind::Glb);
    }
    files
}

/// `world.json` and the objects as one input: length-prefixed parts.
fn encode(files: &Files) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(files.world.len() as u32).to_le_bytes());
    out.extend_from_slice(&files.world);
    for (kind, bytes) in &files.objects {
        out.push(u8::from(*kind == ObjectKind::Glb));
        out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(bytes);
    }
    out
}

fn decode(mut input: &[u8]) -> Option<Files> {
    fn part<'a>(input: &mut &'a [u8]) -> Option<&'a [u8]> {
        let len = u32::from_le_bytes(input.get(..4)?.try_into().ok()?) as usize;
        let bytes = input.get(4..4 + len)?;
        *input = &input[4 + len..];
        Some(bytes)
    }
    let world = part(&mut input)?.to_vec();
    let mut objects = Vec::new();
    while let Some((&kind, rest)) = input.split_first() {
        input = rest;
        let kind = if kind == 1 {
            ObjectKind::Glb
        } else {
            ObjectKind::Json
        };
        objects.push((kind, part(&mut input)?.to_vec()));
    }
    Some(Files { world, objects })
}

/// An encoded [`Files`] as parts, for shrinking: the root document, then
/// each object as its kind byte (1 for a mesh) followed by its bytes.
fn files_split(input: &[u8]) -> Option<Vec<Vec<u8>>> {
    let files = decode(input)?;
    let mut parts = vec![files.world];
    for (kind, bytes) in files.objects {
        let mut part = vec![u8::from(kind == ObjectKind::Glb)];
        part.extend(bytes);
        parts.push(part);
    }
    Some(parts)
}

fn files_join(parts: &[Vec<u8>]) -> Vec<u8> {
    let objects = parts
        .iter()
        .skip(1)
        .map(|part| match part.split_first() {
            Some((1, bytes)) => (ObjectKind::Glb, bytes.to_vec()),
            Some((_, bytes)) => (ObjectKind::Json, bytes.to_vec()),
            None => (ObjectKind::Json, Vec::new()),
        })
        .collect();
    encode(&Files {
        world: parts.first().cloned().unwrap_or_default(),
        objects,
    })
}

#[test]
fn files_split_and_join_round_trip() {
    let files = sample_bundle();
    let encoded = encode(&files);
    let parts = files_split(&encoded).expect("splits");
    assert_eq!(parts.len(), 1 + files.objects.len());
    assert_eq!(files_join(&parts), encoded);
}

/// The memory check reads the kernel's report (Linux).
#[test]
fn process_sizes_are_read_from_proc_status() {
    let status = "Name:\tfuzz\nVmPeak:\t  200 kB\nVmSize:\t  150 kB\nVmRSS:\t 10 kB\n";
    assert_eq!(
        fuzzing::status_sizes(status),
        Some((150 * 1024, 200 * 1024))
    );
    assert_eq!(fuzzing::status_sizes("VmSize:\t1 kB\n"), None);
    if cfg!(target_os = "linux") {
        let (size, peak) = fuzzing::address_space().expect("/proc/self/status");
        assert!(size > 0 && peak >= size);
    }
}

/// The engine notices a reader reserving what a length field claims, where
/// the kernel reports the process's size (Linux; found by review: it could
/// not). The reservation is 1 GiB, never touched, so it stands out from
/// what the tests running alongside hold.
#[test]
fn the_memory_check_catches_what_a_length_field_reserves() {
    if fuzzing::address_space().is_none() {
        return;
    }
    let caught = std::panic::catch_unwind(|| {
        fuzzing::run(
            Target::new("memory-check", 1).exactly(1).in_process(),
            |_| (1u32 << 30).to_le_bytes().to_vec(),
            |input| {
                let claimed = u32::from_le_bytes(input.try_into().expect("4 bytes")) as usize;
                std::hint::black_box(Vec::<u8>::with_capacity(claimed));
            },
        );
    });
    assert!(caught.is_err(), "a 1 GiB reservation went unnoticed");
}

#[test]
fn world_store_validation() {
    let c = corpus();
    let base = format_world();
    let dir = common::temp_dir("fuzz-validate");
    let store = Store::new(&dir);
    fuzzing::run(
        Target::new("validate", 400)
            .memory(64, 64 << 20)
            .parts(files_split, files_join),
        |rng| encode(&mutate_files(rng, &base, c, WorldManifest::FORMAT)),
        |input| {
            let Some(files) = decode(input) else {
                return;
            };
            for (kind, bytes) in &files.objects {
                store.write_object(bytes, *kind).expect("write object");
            }
            std::fs::write(store.world_path(), &files.world).expect("write world.json");
            let report = validate_store(&store);
            if report.is_ok() {
                assert_eq!(report.world_id, Some(ContentId::of(&files.world)));
            }
        },
    );
    std::fs::remove_dir_all(&dir).ok();
}

// ----------------------------------------------------- contribution bundles --

/// The sample contribution: `change.json` (as the root document), its
/// manifest and its mesh.
fn sample_bundle() -> Files {
    let objects = fuzzing::fixture_files(&["fixtures/changes/facade-sample/objects"])
        .into_iter()
        .map(|(name, bytes)| {
            let kind = if name.ends_with(".glb") {
                ObjectKind::Glb
            } else {
                ObjectKind::Json
            };
            (kind, bytes)
        })
        .collect();
    let change = fuzzing::repo_root().join("fixtures/changes/facade-sample/change.json");
    Files {
        world: std::fs::read(change).expect("change.json"),
        objects,
    }
}

/// A contribution bundle is untrusted input to the gate (RFC 0002), which
/// CI runs on pull requests: whatever the bundle holds, the gate must
/// produce a valid report about it (or an I/O error), never panic.
#[test]
fn contribution_bundles_through_the_gate() {
    let c = corpus();
    let world = fuzzing::repo_root().join("fixtures/world");
    let base = sample_bundle();
    let dir = common::temp_dir("fuzz-gate");
    let runs = AtomicU64::new(0);
    fuzzing::run(
        // The gate reads the whole fixture world whatever the bundle holds.
        Target::new("gate", 150)
            .memory(64, 256 << 20)
            .parts(files_split, files_join),
        |rng| encode(&mutate_files(rng, &base, c, ChangeSet::FORMAT)),
        |input| {
            let Some(files) = decode(input) else {
                return;
            };
            let n = runs.fetch_add(1, Ordering::Relaxed);
            let (bundle, candidate) = (dir.join(format!("b{n}")), dir.join(format!("c{n}")));
            std::fs::create_dir_all(&bundle).expect("bundle directory");
            std::fs::write(bundle.join("change.json"), &files.world).expect("write change.json");
            for (kind, bytes) in &files.objects {
                Store::new(&bundle)
                    .write_object(bytes, *kind)
                    .expect("write object");
            }
            let result = gate::run(&world, &bundle, &candidate);
            std::fs::remove_dir_all(&bundle).ok();
            std::fs::remove_dir_all(&candidate).ok();
            if let Ok(report) = result {
                assert_eq!(report.change_id, ContentId::of(&files.world));
                let bytes = report.to_bytes().expect("the gate's report is valid");
                assert_eq!(CheckReport::from_bytes(&bytes).expect("reads back"), report);
            }
        },
    );
    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------- world log --

/// What `verify_log` must decide, restated from RFC 0002 section 6.
fn expected_log_result(log: &[(ContentId, MergeRecord)]) -> Option<ContentId> {
    let (first, rest) = log.split_first()?;
    if first.1.decision != Decision::Baseline {
        return None;
    }
    let mut previous = first;
    for entry in rest {
        let r = &entry.1;
        if r.decision == Decision::Baseline
            || r.previous_record_id != Some(previous.0)
            || r.world_before != Some(previous.1.world_after)
        {
            return None;
        }
        previous = entry;
    }
    Some(previous.1.world_after)
}

fn check_log(bytes: &[u8]) {
    let Ok(Value::Array(items)) = serde_json::from_slice::<Value>(bytes) else {
        return;
    };
    let mut log = Vec::new();
    for item in items {
        let Some([id, record]) = item
            .as_array()
            .and_then(|a| <&[Value; 2]>::try_from(&a[..]).ok())
        else {
            return;
        };
        let Some(id) = id.as_str().and_then(|s| ContentId::parse(s).ok()) else {
            return;
        };
        let Ok(record) = serde_json::from_value::<MergeRecord>(record.clone()) else {
            return;
        };
        if let Ok(canonical) = cjson::to_canonical(&record) {
            let _ = MergeRecord::from_bytes(&canonical);
        }
        log.push((id, record));
    }
    let expected = expected_log_result(&log);
    match verify_log(&log) {
        Ok(world) => assert_eq!(Some(world), expected),
        Err(_) => assert_eq!(expected, None),
    }
}

fn id_or_null(rng: &mut Rng, ids: &[ContentId]) -> Value {
    if rng.chance(20) {
        Value::Null
    } else {
        Value::String(rng.pick(ids).to_string())
    }
}

#[test]
fn world_log() {
    let log = sample_log();
    let ids: Vec<ContentId> = log.iter().map(|(id, _)| *id).collect();
    let worlds: Vec<ContentId> = log.iter().map(|(_, r)| r.world_after).collect();
    let base: Value = log
        .iter()
        .map(|(id, r)| {
            serde_json::json!([id.to_string(), serde_json::to_value(r).expect("serialises")])
        })
        .collect();
    fuzzing::run(
        Target::new("world-log", 10_000),
        |rng| {
            let mut v = base.clone();
            for _ in 0..rng.count(4) {
                if rng.chance(15) {
                    fuzzing::edit_value(rng, &mut v, None);
                    continue;
                }
                let Some(entries) = v.as_array_mut() else {
                    break;
                };
                if entries.is_empty() {
                    break;
                }
                let i = rng.below(entries.len());
                match rng.below(8) {
                    0 => {
                        entries.remove(i);
                    }
                    1 => {
                        let copy = entries[i].clone();
                        let at = rng.below(entries.len() + 1);
                        entries.insert(at, copy);
                    }
                    2 => {
                        let j = rng.below(entries.len());
                        entries.swap(i, j);
                    }
                    3 => entries.reverse(),
                    4 => {
                        if let Some(slot) = entries[i].get_mut(0) {
                            *slot = Value::String(rng.pick(&ids).to_string());
                        }
                    }
                    k => {
                        let (key, value) = match k {
                            5 => ("previous_record_id", id_or_null(rng, &ids)),
                            6 => ("world_before", id_or_null(rng, &worlds)),
                            _ => (
                                "decision",
                                Value::from(*rng.pick(&["baseline", "merged", "takedown"])),
                            ),
                        };
                        if let Some(record) = entries[i].get_mut(1).and_then(Value::as_object_mut) {
                            record.insert(key.into(), value);
                        }
                    }
                }
            }
            serde_json::to_vec(&v).expect("a JSON value serialises")
        },
        check_log,
    );
}

// ------------------------------------------------------------- regressions --
//
// Inputs that once broke a reader, minimised. Each must stay an error.

/// One primitive whose POSITION and NORMAL are the same accessor: 24 bytes
/// decoded from a 12-byte BIN chunk. Accessors that share data let a small
/// file make the reader copy the same bytes again and again (a 37 KB file
/// made it copy 786 MB; the copies grow with the product of the node,
/// primitive and vertex counts until an allocation aborts the process).
/// Found by review while writing the glb fuzz target.
const GLB_SHARED_ACCESSOR: &[u8] = b"glTF\x02\x00\x00\x00\xb0\x01\x00\x00\x88\x01\x00\x00JSON{\
        \"accessors\":[{\"bufferView\":0,\"componentType\":5126,\
        \"count\":1,\"type\":\"VEC3\"},{\"bufferView\":1,\"componentType\":5125,\
        \"count\":0,\"type\":\"SCALAR\"}],\"bufferViews\":[{\"byteLength\":12,\
        \"byteOffset\":0},{\"byteLength\":0,\"byteOffset\":12}],\
        \"materials\":[{\"name\":\"road\"}],\"meshes\":[{\"primitives\":[\
        {\"attributes\":{\"NORMAL\":0,\"POSITION\":0},\"indices\":1,\
        \"material\":0,\"mode\":4}]}],\"nodes\":[{\"mesh\":0,\
        \"name\":\"a:b\"}]}\x20\x20\x0c\x00\x00\x00BIN\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";

/// Two nodes showing the same mesh: its data decoded once per node (found
/// with the accessor case above).
const GLB_SHARED_MESH: &[u8] = b"glTF\x02\x00\x00\x004\x02\x00\x00\x00\x02\x00\x00JSON{\
        \"accessors\":[{\"bufferView\":0,\"componentType\":5126,\
        \"count\":1,\"type\":\"VEC3\"},{\"bufferView\":1,\"componentType\":5126,\
        \"count\":1,\"type\":\"VEC3\"},{\"bufferView\":2,\"componentType\":5125,\
        \"count\":0,\"type\":\"SCALAR\"}],\"bufferViews\":[{\"byteLength\":12,\
        \"byteOffset\":0},{\"byteLength\":12,\"byteOffset\":12},\
        {\"byteLength\":0,\"byteOffset\":24}],\"materials\":[\
        {\"name\":\"road\"}],\"meshes\":[{\"primitives\":[{\"attributes\":{\
        \"NORMAL\":1,\"POSITION\":0},\"indices\":2,\"material\":0,\
        \"mode\":4}]}],\"nodes\":[{\"mesh\":0,\"name\":\"a:b\"},\
        {\"mesh\":0,\"name\":\"a:b\"}]}\x20\x20\x18\x00\x00\x00BIN\x00\
        \x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";

fn glb_error(bytes: &[u8]) -> String {
    match glb::read(bytes) {
        Err(op_world::Error::Glb(m)) => m,
        other => panic!("expected a glb error, got {other:?}"),
    }
}

#[test]
fn regression_glb_accessors_cannot_decode_the_bin_chunk_twice() {
    assert_eq!(GLB_SHARED_ACCESSOR.len(), 432);
    assert_eq!(
        glb_error(GLB_SHARED_ACCESSOR),
        "accessors read more data than the BIN chunk holds"
    );
}

#[test]
fn regression_glb_nodes_cannot_share_a_mesh() {
    assert_eq!(GLB_SHARED_MESH.len(), 564);
    assert_eq!(glb_error(GLB_SHARED_MESH), "node 1 must show mesh 1");
}

/// A bundle whose `change.json` has one unknown, 5000-letter key. The
/// reader's error quotes the key, the gate quoted the error in G1, and the
/// report broke its own rule for text (at most 4096 characters), so the
/// gate could not write a report at all. Found by the gate target, with a
/// manifest whose `collider` was `none` and 4096 more letters.
#[test]
fn regression_gate_reports_an_overlong_unknown_field() {
    let change = format!("{{\"{}\":1}}", "a".repeat(5000));
    let dir = common::temp_dir("regression-gate-overlong");
    let bundle = dir.join("bundle");
    std::fs::create_dir_all(&bundle).expect("bundle directory");
    std::fs::write(bundle.join("change.json"), &change).expect("write change.json");
    let world = fuzzing::repo_root().join("fixtures/world");
    let report = gate::run(&world, &bundle, &dir.join("candidate")).expect("the gate runs");
    let bytes = report.to_bytes().expect("the report is valid");
    assert_eq!(CheckReport::from_bytes(&bytes).expect("reads back"), report);
    let g1 = &report.checks[0];
    assert_eq!(g1.outcome, op_world::contrib::Outcome::Fail);
    assert_eq!(g1.details.len(), 1);
    assert!(g1.details[0].starts_with("change.json: invalid JSON: unknown field `aaa"));
    assert_eq!(g1.details[0].chars().count(), rules::MAX_TEXT_CHARS);
    std::fs::remove_dir_all(&dir).ok();
}

/// A glb whose JSON chunk is one long array of zeros. Parsing builds a tree
/// about 17 times the size of the text before any check can refuse it: a
/// 66 MB file took 1.1 GB in the client, the gate and validate. JSON chunks
/// are now at most 1 MiB. Found by review.
#[test]
fn regression_glb_json_chunk_is_bounded() {
    let mut json = b"{\"a\":[".to_vec();
    json.extend_from_slice("0,".repeat(600_000).as_bytes());
    json.extend_from_slice(b"0]}");
    let bytes = fuzzing::glb_pack(&json, &[]);
    assert_eq!(bytes.len(), 1_200_040);
    assert_eq!(glb_error(&bytes), "JSON chunk longer than 1048576 bytes");
}

/// Store and bundle reads took whatever a path named: a directory, a FIFO
/// (which blocks), a link to /dev/zero (which never ends) or a 300 MB
/// change.json, read whole before the gate refused it. Every read now takes
/// only a regular file, not a link, of at most 64 MiB. Found by review.
#[test]
fn regression_store_reads_refuse_irregular_and_oversized_files() {
    use op_world::store::{MAX_FILE_BYTES, read_file};
    let dir = common::temp_dir("regression-store-files");
    let store = Store::new(&dir);
    std::fs::create_dir_all(store.world_path()).expect("a directory as world.json");
    let err = store.read_world().expect_err("not a file").to_string();
    assert!(err.ends_with("world.json: not a regular file"), "{err}");
    let id = ContentId::of(b"{}");
    std::fs::create_dir_all(store.object_path(&id, ObjectKind::Json)).expect("a directory");
    assert_eq!(
        store
            .read_object(&id, ObjectKind::Json)
            .expect_err("not a file")
            .to_string(),
        format!("I/O error: object {id}: not a regular file")
    );

    // Longer than any valid file: refused before anything is read (the
    // file is sparse where the filesystem allows).
    let bundle = dir.join("bundle");
    std::fs::create_dir_all(&bundle).expect("bundle directory");
    std::fs::File::create(bundle.join("change.json"))
        .and_then(|f| f.set_len(MAX_FILE_BYTES + 1))
        .expect("a long change.json");
    let world = fuzzing::repo_root().join("fixtures/world");
    let err = gate::run(&world, &bundle, &dir.join("candidate")).expect_err("refused");
    assert_eq!(
        err.to_string(),
        "I/O error: change.json: larger than 67108864 bytes"
    );
    let eleven = dir.join("eleven");
    std::fs::write(&eleven, b"01234567890").expect("write");
    assert_eq!(read_file(&eleven, 11).expect("11 bytes"), b"01234567890");
    assert_eq!(
        read_file(&eleven, 10).expect_err("12 bytes").to_string(),
        "larger than 10 bytes"
    );

    #[cfg(unix)]
    {
        // A FIFO would block the read; a link to /dev/zero would never end;
        // a link to a regular file is refused too (it could point outside
        // the store, and errors quote what they read).
        let made = std::process::Command::new("mkfifo")
            .arg(dir.join("fifo"))
            .status()
            .expect("run mkfifo");
        assert!(made.success(), "mkfifo failed");
        for (link, target) in [("zero", "/dev/zero"), ("link", "eleven")] {
            std::os::unix::fs::symlink(target, dir.join(link)).expect("symlink");
        }
        for name in ["fifo", "zero", "link"] {
            let err = read_file(&dir.join(name), MAX_FILE_BYTES).expect_err(name);
            assert_eq!(err.to_string(), "not a regular file", "{name}");
        }
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// A world whose tile lists one large mesh thousands of times (under
/// different feature ids). Validation read, hashed and parsed the mesh again
/// for every listing: 400 listings of a 744 KB mesh took 2 s, and a 64 MiB
/// mesh listed from a 1 MB tile index would take about 20 minutes. Each
/// object is now read and checked at most twice; every listing still gets
/// its own errors and counts. Found by review.
#[test]
fn regression_validation_reads_a_repeated_mesh_once() {
    let source = Store::new(fuzzing::repo_root().join("fixtures/world"));
    let (_, mut world) = source.read_world().expect("fixture world");
    let index_bytes = source
        .read_object(&world.tiles[0].tile_id, ObjectKind::Json)
        .expect("tile index");
    let mut index = TileIndex::from_bytes(&index_bytes).expect("valid tile index");
    let largest = index
        .assets
        .iter()
        .max_by_key(|a| {
            source
                .read_object(&a.content_id, ObjectKind::Glb)
                .map_or(0, |b| b.len())
        })
        .expect("assets")
        .clone();
    let dir = common::temp_dir("regression-validate-repeated");
    let store = Store::new(&dir);
    for (id, kind) in [
        (largest.manifest_id, ObjectKind::Json),
        (largest.content_id, ObjectKind::Glb),
    ] {
        let bytes = source.read_object(&id, kind).expect("fixture object");
        store.write_object(&bytes, kind).expect("write object");
    }
    const LISTINGS: usize = 4000;
    index.assets = (0..LISTINGS)
        .map(|k| {
            let mut a = largest.clone();
            a.feature_id = format!("{}-{k:04}", largest.feature_id);
            a
        })
        .collect();
    world.tiles.truncate(1);
    world.tiles[0].tile_id = store
        .write_object(&index.to_bytes().expect("valid index"), ObjectKind::Json)
        .expect("write index");
    world.tiles[0].bounds_mm = largest.bounds_mm;
    std::fs::write(store.world_path(), world.to_bytes().expect("valid world")).expect("world");

    // Reading the mesh for every listing took about 17 s here, 0.3 s once.
    // A timeout, not a clock: no wall-clock time in this workspace's code.
    let (done, report) = std::sync::mpsc::channel();
    let validated = store.clone();
    std::thread::spawn(move || done.send(validate_store(&validated)));
    let report = report
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("validation finishes within 5 s");
    assert_eq!(report.assets, LISTINGS);
    let mesh = glb::read(
        &store
            .read_object(&largest.content_id, ObjectKind::Glb)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(report.triangles, LISTINGS * mesh.triangle_count());
    let differ = report
        .errors
        .iter()
        .filter(|e| e.ends_with("feature_id in the tile index differs from the manifest"))
        .count();
    assert_eq!(differ, LISTINGS, "{:?}", &report.errors[..3]);
    std::fs::remove_dir_all(&dir).ok();
}
