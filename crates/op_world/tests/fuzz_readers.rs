//! Fuzzing of the world-format readers (roadmap M1, "Before announcing"):
//! canonical JSON and every document type with its rules, points documents
//! (RFC 0004) against their asset's rules, the glb reader,
//! the contribution documents and the world log, content ids, cell keys,
//! store paths and reads, whole-store validation, the gate on contribution
//! bundles and on refreshes with points, world log directories
//! (`WorldLog::open` and `--log` merges), `--require-base` on bundles, and
//! the merge commands' arguments.
//!
//! Every reader of untrusted bytes must return an error, never panic, hang,
//! overflow or allocate without bound; what a reader accepts must re-encode
//! to exactly the bytes it read. The engine, its environment variables
//! (`STREETWIDE_FUZZ_ITERS`, `STREETWIDE_FUZZ_SEED`) and how a failure is
//! reported are described in `fuzzing/mod.rs`. Inputs that once broke a
//! reader are pinned at the end of this file.

mod common;
mod fuzzing;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use fuzzing::{Rng, Target};
use op_world::cjson;
use op_world::contrib::{
    CONTRIBUTION_CONTRACT_VERSION, Change, ChangeSet, CheckReport, Decision, MergeRecord, Op,
    Review, Verdict, verify_log,
};
use op_world::gate;
use op_world::glb::{self, GlbMesh, Material, Node, Primitive, mm_to_f32};
use op_world::keys::{RegionKey, TileKey};
use op_world::limits::Document;
use op_world::merge::{LOG_LOCK, MergeArgs, MergeOutput, WorldLog, baseline, require_base};
use op_world::points::{Attributes, Point, PointKind, TreeAttributes};
use op_world::rules;
use op_world::sample::parse_mesh_specs;
use op_world::sample_change::{sample_points_draft, write_points_bundle};
use op_world::validate::{Report, validate_store};
use op_world::{
    AssetManifest, ContentId, ObjectKind, PointsDocument, Store, TileIndex, WorldManifest,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

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

/// `from_bytes` must accept exactly the canonical documents within the
/// limit for their kind without problems, and `problems()` must cope with
/// any canonical document.
fn consistent<T: Serialize + DeserializeOwned>(
    bytes: &[u8],
    kind: Document,
    problems: fn(&T) -> Vec<String>,
    accepts: fn(&[u8]) -> bool,
) {
    if let Ok(doc) = cjson::from_canonical::<T>(bytes) {
        assert_eq!(
            accepts(bytes),
            problems(&doc).is_empty() && kind.fits(bytes.len()),
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
    if let Ok(d) = PointsDocument::from_bytes(bytes) {
        check_points_document(&d, id);
    }
    consistent(
        bytes,
        Document::AssetManifest,
        AssetManifest::problems,
        |b| AssetManifest::from_bytes(b).is_ok(),
    );
    consistent(bytes, Document::TileIndex, TileIndex::problems, |b| {
        TileIndex::from_bytes(b).is_ok()
    });
    consistent(
        bytes,
        Document::WorldManifest,
        WorldManifest::problems,
        |b| WorldManifest::from_bytes(b).is_ok(),
    );
    consistent(bytes, Document::ChangeSet, ChangeSet::problems, |b| {
        ChangeSet::from_bytes(b).is_ok()
    });
    consistent(bytes, Document::CheckReport, CheckReport::problems, |b| {
        CheckReport::from_bytes(b).is_ok()
    });
    consistent(bytes, Document::MergeRecord, MergeRecord::problems, |b| {
        MergeRecord::from_bytes(b).is_ok()
    });
    consistent(bytes, Document::Points, PointsDocument::problems, |b| {
        PointsDocument::from_bytes(b).is_ok()
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

/// What holds of every points document the reader accepts (RFC 0004): it
/// writes back to the bytes it was read from, its points share one source
/// and have bounds, and it is a valid asset of the layer of its kinds, in
/// any tile, with that source and those bounds, but never of a mesh layer.
fn check_points_document(d: &PointsDocument, id: ContentId) {
    assert!(d.problems().is_empty());
    assert_eq!(d.id().expect("valid points document"), id);
    let summary = d.summary();
    assert_eq!(summary.count, d.points.len());
    let bounds = summary.bounds.expect("bounds");
    assert!(bounds.is_valid());
    let source = summary.source.clone().expect("one source");
    let tile = TileKey::parse("17/68646/45903").expect("a tile");
    let feature_id = format!("tile:{tile}/{source}");
    let layers: BTreeSet<op_world::Layer> = summary.kinds.iter().map(|k| k.layer()).collect();
    for layer in op_world::Layer::ALL {
        let problems = summary.asset_problems(layer, &feature_id, tile, &bounds);
        let fits = layer.has_points() && layers.iter().all(|l| *l == layer);
        assert_eq!(problems.is_empty(), fits, "{layer:?}: {problems:?}");
    }
}

/// Values of every JSON type for the points target, around the limits of
/// each attribute and with the names, kinds and rule ids it knows.
fn point_values() -> Vec<Value> {
    let mut values: Vec<Value> = [
        json!(null),
        json!(true),
        json!(0),
        json!(1),
        json!(-1),
        json!(64),
        json!(65),
        json!(999),
        json!(1000),
        json!(2999),
        json!(3000),
        json!(50_000),
        json!(50_001),
        json!(65_535),
        json!(65_536),
        json!(100_000),
        json!(100_001),
        json!(150_000),
        json!(150_001),
        json!(9_007_199_254_740_991_i64),
        json!(-9_007_199_254_740_991_i64),
        json!(""),
        json!("Tilia"),
        json!("tilia"),
        json!("Tilia cordata"),
        json!("Tilia  cordata"),
        json!("Platanus \u{d7} hispanica"),
        json!("Tilia cordata 'Greenspire'"),
        json!("deciduous"),
        json!("semi_evergreen"),
        json!("broadleaved"),
        json!("led"),
        json!("metal_halide"),
        json!("pole"),
        json!("wire"),
        json!("street"),
        json!("navigation"),
        json!("tree"),
        json!("light"),
        json!("bench"),
        json!("yaw"),
        json!("height_mm"),
        json!("mount"),
        json!("adr-0015:yaw-from-orientierung:1"),
        json!("adr-0015:yaw:0"),
        json!("streetwide:sample-trees:T-1"),
        json!("streetwide:sample-trees:T-0"),
        json!("streetwide:sample-lights:L-9"),
        json!("osm:node:1"),
        json!("osm::1"),
        json!("a:b c"),
        json!([]),
        json!(["yaw"]),
        json!(["height_mm"]),
        json!(["yaw", "yaw"]),
        json!([0, 0, 0]),
        json!([1, 2]),
        json!({}),
        json!({"light": {"yaw": "adr-0015:yaw-from-orientierung:1"}}),
        json!({"light": {}}),
        json!({"tree": {"height_mm": "adr-0016:height:1"}}),
    ]
    .into();
    values.push(json!("x".repeat(257)));
    values.push(json!(format!("a:{}", "x".repeat(254))));
    values
}

/// Every attribute name of every kind of point.
const ATTRIBUTE_NAMES: [&str; 15] = [
    "count",
    "crown_diameter_mm",
    "genus",
    "height_mm",
    "leaf_cycle",
    "leaf_type",
    "light_height_mm",
    "method",
    "mount",
    "planted_year",
    "purpose",
    "species",
    "taxon",
    "trunk_circumference_mm",
    "yaw",
];

/// A points document from the corpus with a few targeted edits: a value
/// from [`point_values`] put in an attribute, an id, a kind, an anchor,
/// `estimated` or `rules`; points copied, removed or swapped; or any edit
/// of [`fuzzing::edit_value`].
fn mutate_points(rng: &mut Rng, docs: &[&Doc], values: &[Value]) -> Vec<u8> {
    let doc = rng.pick(docs);
    let Some(mut v) = doc.value.clone() else {
        return doc.bytes.clone();
    };
    for _ in 0..rng.count(3) {
        let n = v
            .get("points")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let k = if n > 0 { rng.below(n) } else { 0 };
        let value = rng.pick(values).clone();
        // Edits go through JSON pointers: an earlier edit may have left
        // any value anywhere.
        let mut set_in = |path: &str, key: &str| {
            if let Some(object) = v.pointer_mut(path).and_then(Value::as_object_mut) {
                object.insert(key.to_string(), value.clone());
            }
        };
        match rng.below(8) {
            0 => {
                let key = ATTRIBUTE_NAMES[rng.below(ATTRIBUTE_NAMES.len())];
                set_in(&format!("/points/{k}/attributes"), key);
            }
            1 => set_in(&format!("/points/{k}"), "id"),
            2 => set_in(&format!("/points/{k}"), "kind"),
            3 => {
                if let Some(c) = v.pointer_mut(&format!("/points/{k}/anchor_mm/{}", rng.below(3))) {
                    *c = value;
                }
            }
            4 => set_in(&format!("/points/{k}"), "estimated"),
            5 => set_in("", "rules"),
            6 => {
                if let Some(points) = v.get_mut("points").and_then(Value::as_array_mut)
                    && !points.is_empty()
                {
                    let a = rng.below(points.len());
                    let b = rng.below(points.len());
                    match rng.below(3) {
                        0 => {
                            let copy = points[a].clone();
                            points.insert(b, copy);
                        }
                        1 => {
                            points.remove(a);
                        }
                        _ => points.swap(a, b),
                    }
                }
            }
            _ => fuzzing::edit_value(rng, &mut v, None),
        }
    }
    cjson::to_vec(&v).unwrap_or_else(|_| serde_json::to_vec(&v).expect("a JSON value serialises"))
}

/// Points documents (RFC 0004), mutated where their rules are: the reader
/// accepts exactly the documents without problems, and what it accepts
/// holds [`check_points_document`].
#[test]
fn points_documents() {
    let c = corpus();
    let docs: Vec<&Doc> = c
        .json
        .iter()
        .filter(|d| d.format() == Some(PointsDocument::FORMAT))
        .collect();
    assert!(docs.len() >= 2, "the golden points documents are missing");
    let values = point_values();
    fuzzing::run(
        Target::new("points", 4000),
        |rng| mutate_points(rng, &docs, &values),
        |bytes| {
            if let Ok(d) = PointsDocument::from_bytes(bytes) {
                check_points_document(&d, ContentId::of(bytes));
            }
            consistent(bytes, Document::Points, PointsDocument::problems, |b| {
                PointsDocument::from_bytes(b).is_ok()
            });
        },
    );
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
                let layered = Store::layered(&empty_root, vec![dir.to_path_buf()]);
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
/// manifest naming changed content (a mesh or a points document) also gets
/// its new `size_bytes`.
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
        // A mesh or a points document is an asset's content: its manifest
        // gets its new size.
        let content = kind == ObjectKind::Glb
            || Doc::new(old.clone()).format() == Some(PointsDocument::FORMAT);
        readdress(&mut files, &old, &new, content);
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

/// An input that makes a reader reserve what a length field claims: 1 GiB,
/// never touched.
fn reserve_1_gib(_: &mut Rng) -> Vec<u8> {
    (1u32 << 30).to_le_bytes().to_vec()
}

fn reserve_claimed(input: &[u8]) {
    let claimed = u32::from_le_bytes(input.try_into().expect("4 bytes")) as usize;
    std::hint::black_box(Vec::<u8>::with_capacity(claimed));
}

/// The engine notices a reader reserving what a length field claims, where
/// the kernel reports the process's size (Linux; found by review: it could
/// not). The reservation is 1 GiB, so it stands out from what the tests
/// running alongside hold.
#[test]
fn the_memory_check_catches_what_a_length_field_reserves() {
    if fuzzing::address_space().is_none() {
        return;
    }
    let caught = std::panic::catch_unwind(|| {
        fuzzing::run(
            Target::new("memory-check", 1).exactly(1).in_process(),
            reserve_1_gib,
            reserve_claimed,
        );
    });
    assert!(caught.is_err(), "a 1 GiB reservation went unnoticed");
}

/// The same through a child process, where every target runs on Linux: the
/// reservation fails the run there, so the child's test fails, and so does
/// the run that started it. Found by review: no test showed that the child
/// processes measure anything. Where they do not run, nothing to show.
#[test]
fn the_memory_check_works_in_a_child_process() {
    if fuzzing::address_space().is_none() || fuzzing::test_to_isolate().is_none() {
        return;
    }
    let target = Target::new("memory-check-child", 1).exactly(1);
    if fuzzing::in_child() {
        // Here the run must fail: that is what the parent checks.
        fuzzing::run(target, reserve_1_gib, reserve_claimed);
        return;
    }
    let caught = std::panic::catch_unwind(|| fuzzing::run(target, reserve_1_gib, reserve_claimed));
    let message = match &caught {
        Err(payload) => payload
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_default(),
        Ok(()) => String::new(),
    };
    assert!(
        message.contains("failed in its child process"),
        "a 1 GiB reservation in a child process went unnoticed: {message:?}"
    );
}

/// `STREETWIDE_FUZZ_ISOLATE` takes exactly 0, false or no, or 1, true or
/// yes. Found by review: any value but `0` meant child processes.
#[test]
fn isolation_settings_are_strict() {
    for (raw, setting) in [
        ("0", Some(false)),
        ("false", Some(false)),
        ("no", Some(false)),
        ("1", Some(true)),
        ("true", Some(true)),
        ("yes", Some(true)),
        ("", None),
        (" 0", None),
        ("2", None),
        ("off", None),
        ("TRUE", None),
    ] {
        assert_eq!(fuzzing::parse_isolate(raw), setting, "{raw:?}");
    }
}

/// The memory check runs only where nothing else shares the process: in a
/// target's child process, or for the engine's own tests in the test
/// process. Found by review: with `STREETWIDE_FUZZ_ISOLATE=0`, targets ran
/// in the test process with the check on, where other tests' threads and
/// allocator arenas move the process's size both ways.
#[test]
fn the_memory_check_needs_a_process_of_its_own() {
    assert!(fuzzing::checks_memory(true, true, false), "a child process");
    assert!(
        fuzzing::checks_memory(true, false, true),
        "the engine's tests"
    );
    assert!(
        !fuzzing::checks_memory(true, false, false),
        "a target in the test process"
    );
    assert!(
        !fuzzing::checks_memory(false, true, true),
        "where the kernel reports no size"
    );
}

/// Writes `files` into a store of its own, `<dir>/s<n>`, validates it and
/// deletes it. No input sees what another left, so an input that fails,
/// once shrunk, fails again in a fresh store.
fn validate_alone(dir: &Path, n: u64, files: &Files) -> Report {
    let root = dir.join(format!("s{n}"));
    std::fs::create_dir_all(&root).expect("store directory");
    let store = Store::new(&root);
    for (kind, bytes) in &files.objects {
        store.write_object(bytes, *kind).expect("write object");
    }
    std::fs::write(store.world_path(), &files.world).expect("write world.json");
    let report = validate_store(&store);
    std::fs::remove_dir_all(&root).ok();
    report
}

#[test]
fn world_store_validation() {
    let c = corpus();
    let base = format_world();
    let dir = common::temp_dir("fuzz-validate");
    let runs = AtomicU64::new(0);
    fuzzing::run(
        // Validation reads one object at a time, so what it holds follows
        // the JSON it parses: the default allowance.
        Target::new("validate", 400).parts(files_split, files_join),
        |rng| encode(&mutate_files(rng, &base, c, WorldManifest::FORMAT)),
        |input| {
            let Some(files) = decode(input) else {
                return;
            };
            let report = validate_alone(&dir, runs.fetch_add(1, Ordering::Relaxed), &files);
            if report.is_ok() {
                assert_eq!(report.world_id, Some(ContentId::of(&files.world)));
            }
        },
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Found by review: the validate target wrote every input into one store,
/// so a failing input, once shrunk, could rely on objects that earlier
/// inputs had left there, and pass in a fresh store. Each input now gets a
/// store of its own.
#[test]
fn validate_inputs_do_not_see_each_other() {
    let dir = common::temp_dir("validate-alone");
    let base = format_world();
    assert!(validate_alone(&dir, 0, &base).is_ok());
    let world_only = Files {
        world: base.world.clone(),
        objects: Vec::new(),
    };
    assert!(
        !validate_alone(&dir, 1, &world_only).is_ok(),
        "the objects of an earlier input were seen"
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

/// A baseline refresh adding three light points (RFC 0004) to the fixture
/// world's tile, on its terrain: `change.json` (as the root document), its
/// manifest and its points document.
fn points_bundle() -> Files {
    let dir = common::temp_dir("fuzz-points-bundle");
    let world = fuzzing::repo_root().join("fixtures/world");
    let mut draft = sample_points_draft(&world, op_world::Layer::StreetObjects).expect("a draft");
    write_points_bundle(&mut draft, &dir).expect("a bundle");
    let objects = common::files_under(&dir.join("objects"))
        .into_iter()
        .map(|name| {
            let bytes = std::fs::read(dir.join("objects").join(&name)).expect("an object");
            (ObjectKind::Json, bytes)
        })
        .collect();
    Files {
        world: std::fs::read(dir.join("change.json")).expect("change.json"),
        objects,
    }
}

/// A contribution bundle is untrusted input to the gate (RFC 0002), which
/// CI runs on pull requests: whatever the bundle holds, the gate must
/// produce a valid report about it (or an I/O error), never panic.
#[test]
fn contribution_bundles_through_the_gate() {
    gate_target("gate", 150, sample_bundle());
}

/// The same for a baseline refresh with points (RFC 0004): G1's reading of
/// the points document, G3's review aid, G7's points per tile and G8's
/// anchors on the terrain take whatever the bundle holds.
#[test]
fn refreshes_with_points_through_the_gate() {
    gate_target("gate-points", 150, points_bundle());
}

/// The gate on mutations of the bundle `base`, against the fixture world.
fn gate_target(name: &'static str, cases: u64, base: Files) {
    let c = corpus();
    let world = fuzzing::repo_root().join("fixtures/world");
    let dir = common::temp_dir(&format!("fuzz-{name}"));
    let runs = AtomicU64::new(0);
    fuzzing::run(
        // Besides the JSON it parses, the gate may list up to
        // FACADE_MAX_GRID_STEPS four-byte triangle numbers in G8's grid:
        // 80 MB, twice that while the lists grow. It reads the fixture world
        // one object at a time; at CI size an input takes about 3.4 MB.
        Target::new(name, cases)
            .memory(160, gate::FACADE_MAX_GRID_STEPS * 8 + (16 << 20))
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

// ------------------------------------------------- world log directories --
//
// A world log may come from elsewhere (a clone of the canonical log), so
// `WorldLog::open` and a `--log` merge read and write crafted directories:
// files mutated, truncated, grown, renamed, removed or added, and on Unix
// links and FIFOs where files and directories should be. Inputs are
// directory trees: a control byte, then the entries in the order they are
// made, each a part of its own for shrinking.

/// What a crafted directory holds at one path.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    File,
    Dir,
    /// A link. The entry's bytes name its target: a path in the same tree,
    /// or `/dev/zero`.
    Link,
    /// A FIFO (Unix only): a reader that opens one waits forever.
    Fifo,
}

const KINDS: [Kind; 4] = [Kind::File, Kind::Dir, Kind::Link, Kind::Fifo];

/// One entry of a crafted tree: its path (`/`-separated) and what is there.
#[derive(Clone, Debug)]
struct Entry {
    kind: Kind,
    path: String,
    bytes: Vec<u8>,
}

impl Entry {
    fn file(path: &str, bytes: &[u8]) -> Entry {
        Entry {
            kind: Kind::File,
            path: path.into(),
            bytes: bytes.to_vec(),
        }
    }
}

/// Length-prefixed parts as one input.
fn parts_join(parts: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for p in parts {
        out.extend_from_slice(&(p.len() as u32).to_le_bytes());
        out.extend_from_slice(p);
    }
    out
}

fn parts_split(mut input: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut parts = Vec::new();
    while !input.is_empty() {
        let len = u32::from_le_bytes(input.get(..4)?.try_into().ok()?) as usize;
        parts.push(input.get(4..4usize.checked_add(len)?)?.to_vec());
        input = &input[4 + len..];
    }
    Some(parts)
}

/// A tree as one input: the control byte `flags`, then each entry as its
/// kind, its path, a zero byte and its bytes.
fn tree_encode(flags: u8, entries: &[Entry]) -> Vec<u8> {
    let mut parts = vec![vec![flags]];
    for e in entries {
        let mut part = vec![e.kind as u8];
        part.extend_from_slice(e.path.as_bytes());
        part.push(0);
        part.extend_from_slice(&e.bytes);
        parts.push(part);
    }
    parts_join(&parts)
}

/// The flags and entries of an encoded tree. Entries whose kind or path
/// is unusable (a shrunk input can have them) are left out.
fn tree_decode(input: &[u8]) -> Option<(u8, Vec<Entry>)> {
    let parts = parts_split(input)?;
    let (control, rest) = parts.split_first()?;
    let entries = rest
        .iter()
        .filter_map(|part| {
            let (&kind, rest) = part.split_first()?;
            let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
            let path = std::str::from_utf8(&rest[..end])
                .ok()
                .filter(|p| safe_path(p))?;
            Some(Entry {
                kind: *KINDS.get(usize::from(kind))?,
                path: path.into(),
                bytes: rest.get(end + 1..).unwrap_or_default().to_vec(),
            })
        })
        .collect();
    Some((control.first().copied().unwrap_or(0), entries))
}

/// A relative path that stays in its tree and means the same file on every
/// platform: at most 6 parts of ASCII letters, digits, `.`, `_` and `-`,
/// none ending in a dot (so none is `.` or `..`) or naming a Windows device
/// such as `nul` (a few harmless names like `com` are refused too).
fn safe_path(path: &str) -> bool {
    const DEVICES: &[&str] = &["con", "prn", "aux", "nul", "com", "lpt"];
    let parts: Vec<&str> = path.split('/').collect();
    parts.len() <= 6
        && parts.iter().all(|p| {
            let stem = p.split('.').next().unwrap_or("").to_ascii_lowercase();
            let device = DEVICES
                .iter()
                .any(|d| stem == *d || (stem.len() == 4 && stem.starts_with(d)));
            !p.is_empty()
                && p.len() <= 100
                && !p.ends_with('.')
                && !device
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
}

/// `root` joined with a `/`-separated path.
fn tree_path(root: &Path, path: &str) -> PathBuf {
    path.split('/')
        .fold(root.to_path_buf(), |p, part| p.join(part))
}

/// Where a link entry points: `/dev/zero`, or a path in the tree at `root`
/// (which may not exist).
fn link_target(root: &Path, spec: &[u8]) -> PathBuf {
    if spec == b"/dev/zero" {
        return PathBuf::from("/dev/zero");
    }
    match std::str::from_utf8(spec).ok().filter(|p| safe_path(p)) {
        Some(p) => tree_path(root, p),
        None => root.join("missing"),
    }
}

/// A FIFO in `dir` for crafted trees to link to, made with `mkfifo` as the
/// store regression test does; every FIFO in a tree is a hard link to it.
#[cfg(unix)]
fn fifo_in(dir: &Path) -> Option<PathBuf> {
    let fifo = dir.join("fifo");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("run mkfifo");
    assert!(made.success(), "mkfifo failed");
    Some(fifo)
}

#[cfg(not(unix))]
fn fifo_in(_: &Path) -> Option<PathBuf> {
    None
}

/// Makes the tree under `root`, entry by entry. An entry whose path is
/// taken, or whose parent cannot be made a directory, is left out, and so is
/// one this platform cannot make: a FIFO off Unix, a link where making one
/// needs a right the test does not have (Windows). The checks look at the
/// tree as it was made, never at the entries.
fn make_tree(root: &Path, entries: &[Entry], fifo: Option<&Path>) {
    std::fs::create_dir_all(root).expect("tree root");
    for e in entries {
        let path = tree_path(root, &e.path);
        if std::fs::symlink_metadata(&path).is_ok()
            || path
                .parent()
                .is_some_and(|p| std::fs::create_dir_all(p).is_err())
        {
            continue;
        }
        let _ = match e.kind {
            Kind::File => std::fs::write(&path, &e.bytes),
            Kind::Dir => std::fs::create_dir(&path),
            Kind::Link => symlink_file(&link_target(root, &e.bytes), &path),
            Kind::Fifo => fifo.map_or(Ok(()), |f| std::fs::hard_link(f, &path)),
        };
    }
}

/// Everything under `root` as it is, links not followed: each path with
/// its kind and, for a regular file, its bytes (for a link, its target).
fn snapshot(root: &Path) -> Vec<(String, &'static str, Vec<u8>)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, &'static str, Vec<u8>)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let rel = path
                .strip_prefix(root)
                .expect("under the root")
                .to_string_lossy()
                .replace('\\', "/");
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                out.push((rel, "dir", Vec::new()));
                walk(root, &path, out);
            } else if meta.is_file() {
                out.push((rel, "file", std::fs::read(&path).unwrap_or_default()));
            } else if meta.file_type().is_symlink() {
                let target = std::fs::read_link(&path).unwrap_or_default();
                out.push((rel, "link", target.to_string_lossy().as_bytes().to_vec()));
            } else {
                out.push((rel, "other", Vec::new()));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

/// The bytes of the file `parts` under `root`, if every part on the way is
/// a directory itself and the last a regular file: no links followed, no
/// FIFO opened.
fn regular_file(root: &Path, parts: &[&str]) -> Option<Vec<u8>> {
    let mut path = root.to_path_buf();
    for (k, part) in parts.iter().enumerate() {
        path.push(part);
        let meta = std::fs::symlink_metadata(&path).ok()?;
        let wanted = if k + 1 == parts.len() {
            meta.is_file()
        } else {
            meta.is_dir()
        };
        if !wanted {
            return None;
        }
    }
    std::fs::read(&path).ok()
}

/// What `WorldLog::open` must find in the log at `root`, worked out from
/// the directory as it is (RFC 0002 section 6 and the layout of
/// `WorldLog`): each record's id, record and bytes, or `None` if the log
/// must be refused.
fn expected_log(root: &Path) -> Option<Vec<(ContentId, MergeRecord, Vec<u8>)>> {
    let mut names = Vec::new();
    if !std::fs::symlink_metadata(root.join("log")).ok()?.is_dir() {
        return None;
    }
    for entry in std::fs::read_dir(root.join("log")).ok()? {
        names.push(entry.ok()?.file_name().into_string().ok()?);
    }
    names.sort();
    if names.is_empty() {
        return None;
    }
    let mut records = Vec::new();
    for (i, name) in names.iter().enumerate() {
        if *name != format!("{i:08}.json") {
            return None;
        }
        let bytes = regular_file(root, &["log", name])?;
        let record = MergeRecord::from_bytes(&bytes).ok()?;
        records.push((ContentId::of(&bytes), record, bytes));
    }
    let chain: Vec<(ContentId, MergeRecord)> =
        records.iter().map(|(id, r, _)| (*id, r.clone())).collect();
    expected_log_result(&chain)?;
    for (_, r, _) in &records {
        if let Some(id) = r.change_id {
            let bytes = regular_file(root, &["changes", &id.hex(), "change.json"])?;
            if ContentId::of(&bytes) != id || ChangeSet::from_bytes(&bytes).is_err() {
                return None;
            }
        }
    }
    Some(records)
}

/// A world log made with the library calls behind `op_world baseline --log`
/// and `op_world merge --log`: the fixture world's baseline, the sample
/// facade merged, and the facade taken down again, back to the baseline's
/// world. Also what merging the facade once more takes, for the `--log`
/// merge target, which merges it with a stand-in for the gate.
struct LogCorpus {
    /// The log's files, by path.
    files: Vec<(String, Vec<u8>)>,
    /// `world.json` of the fixture world (w0), where the log ends.
    world: Vec<u8>,
    /// The world manifest after the facade (w1).
    world_after: Vec<u8>,
    /// The candidate world the gate built for the facade on w0, by path.
    candidate: Vec<(String, Vec<u8>)>,
    /// The gate's report on the facade on w0.
    report: Vec<u8>,
    /// The facade's `change.json`.
    change: Vec<u8>,
    /// Where merging the facade writes in the log: its change set, its
    /// report and the world after it.
    next: [String; 3],
}

fn facade_bundle() -> PathBuf {
    fuzzing::repo_root().join("fixtures/changes/facade-sample")
}

fn copy_tree(from: &Path, to: &Path) {
    for f in common::files_under(from) {
        let dst = to.join(&f);
        std::fs::create_dir_all(dst.parent().expect("parent")).expect("mkdir");
        std::fs::copy(from.join(&f), dst).expect("copy");
    }
}

fn read_tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    common::files_under(dir)
        .into_iter()
        .map(|f| {
            let bytes = std::fs::read(dir.join(&f)).expect("read");
            (f, bytes)
        })
        .collect()
}

fn reviewer() -> Review {
    Review {
        reviewer: "github:reviewer".into(),
        verdict: Verdict::Better,
        notes: "Checked.".into(),
    }
}

/// The arguments of `op_world merge <world> <bundle> --log <log>
/// --require-base`; `MergeArgs::run` takes the gate.
fn merge_args(world: &Path, bundle: &Path, log: &Path, decision: Decision) -> MergeArgs {
    MergeArgs {
        world_dir: world.into(),
        bundle_dir: bundle.into(),
        output: MergeOutput::Log(log.into()),
        decision,
        reviews: if decision == Decision::Merged {
            vec![reviewer()]
        } else {
            Vec::new()
        },
        reasons: "Fuzz.".into(),
        require_base: true,
    }
}

fn log_corpus() -> &'static LogCorpus {
    static CORPUS: OnceLock<LogCorpus> = OnceLock::new();
    CORPUS.get_or_init(|| {
        let dir = common::temp_dir("fuzz-log-corpus");
        let (world, log, kept) = (dir.join("world"), dir.join("log"), dir.join("candidate"));
        copy_tree(&fuzzing::repo_root().join("fixtures/world"), &world);
        let w0 = std::fs::read(world.join("world.json")).expect("world.json");
        let record = baseline(&world, "The ingest fixture.").expect("baseline");
        WorldLog::start(&log, &record, &w0).expect("start the log");
        merge_args(&world, &facade_bundle(), &log, Decision::Merged)
            .run(&dir.join("scratch"), |w, b, c| {
                let report = gate::run(w, b, c)?;
                copy_tree(c, &kept);
                Ok(report)
            })
            .expect("merge the facade");
        let w1 = std::fs::read(world.join("world.json")).expect("world.json");
        // The facade taken down by a change set planned on w1.
        let change = std::fs::read(facade_bundle().join("change.json")).expect("change.json");
        let added = ChangeSet::from_bytes(&change).expect("the facade").changes[0].clone();
        let takedown = ChangeSet {
            format: ChangeSet::FORMAT.into(),
            contract_version: CONTRIBUTION_CONTRACT_VERSION.into(),
            base_world_id: ContentId::of(&w1),
            title: "Remove the sample facade (fuzz corpus)".into(),
            author: "github:maintainer".into(),
            rationale: "Owner request (test).".into(),
            changes: vec![Change {
                op: Op::Remove,
                parent_version_id: added.manifest_id,
                manifest_id: None,
                ..added
            }],
        };
        let bundle = dir.join("takedown");
        std::fs::create_dir_all(&bundle).expect("bundle");
        std::fs::write(
            bundle.join("change.json"),
            takedown.to_bytes().expect("valid"),
        )
        .expect("write change.json");
        merge_args(&world, &bundle, &log, Decision::Takedown)
            .run(&dir.join("scratch"), gate::run)
            .expect("take the facade down");
        assert!(std::fs::read(world.join("world.json")).expect("world.json") == w0);
        let report = std::fs::read(facade_bundle().join("report.json")).expect("report.json");
        let next = [
            format!("changes/{}/change.json", ContentId::of(&change).hex()),
            format!("reports/{}.json", ContentId::of(&report).hex()),
            format!("worlds/{}.json", ContentId::of(&w1).hex()),
        ];
        let files = read_tree(&log);
        assert_eq!(
            files.len(),
            9,
            "3 records, 2 change sets, 2 reports, 2 worlds"
        );
        for path in &next {
            assert!(files.iter().any(|(p, _)| p == path), "{path} is in the log");
        }
        let candidate = read_tree(&kept);
        std::fs::remove_dir_all(&dir).ok();
        LogCorpus {
            files,
            world: w0,
            world_after: w1,
            candidate,
            report,
            change,
            next,
        }
    })
}

/// Stands in for the gate when the facade is merged once more: builds the
/// candidate world the gate built for the corpus and returns its report,
/// so the `--log` merge target spends its time on the log.
fn stand_in_gate(
    c: &LogCorpus,
) -> impl FnOnce(&Path, &Path, &Path) -> op_world::Result<CheckReport> + '_ {
    move |_, _, candidate| {
        for (path, bytes) in &c.candidate {
            let to = tree_path(candidate, path);
            std::fs::create_dir_all(to.parent().expect("parent")).expect("candidate");
            std::fs::write(to, bytes).expect("candidate file");
        }
        CheckReport::from_bytes(&c.report)
    }
}

fn encode_value(v: &Value) -> Vec<u8> {
    cjson::to_vec(v).unwrap_or_else(|_| serde_json::to_vec(v).expect("a JSON value serialises"))
}

fn change_id_of(record: &[u8]) -> Option<ContentId> {
    let v: Value = serde_json::from_slice(record).ok()?;
    ContentId::parse(v.get("change_id")?.as_str()?).ok()
}

/// Makes each record name the one before it and start from its world, as
/// far as the records are JSON objects, so the checks behind the chain run.
fn relink(records: &mut [Vec<u8>]) {
    for i in 1..records.len() {
        let previous_id = ContentId::of(&records[i - 1]).to_string();
        let previous_world = serde_json::from_slice::<Value>(&records[i - 1])
            .ok()
            .and_then(|v| v.get("world_after").cloned());
        let Ok(mut v) = serde_json::from_slice::<Value>(&records[i]) else {
            continue;
        };
        let Some(record) = v.as_object_mut() else {
            continue;
        };
        if record.get("decision").and_then(Value::as_str) == Some("baseline") {
            continue;
        }
        record.insert("previous_record_id".into(), Value::String(previous_id));
        if let Some(world) = previous_world {
            record.insert("world_before".into(), world);
        }
        records[i] = encode_value(&v);
    }
}

/// Edits a change set a record names and stores it under its new id, which
/// every record that named it now names.
fn readdress_change(rng: &mut Rng, records: &mut [Vec<u8>], others: &mut [Entry]) {
    let named: Vec<ContentId> = records.iter().filter_map(|r| change_id_of(r)).collect();
    if named.is_empty() {
        return;
    }
    let id = *rng.pick(&named);
    let path = format!("changes/{}/change.json", id.hex());
    let Some(k) = others.iter().position(|e| e.path == path) else {
        return;
    };
    let old = others[k].bytes.clone();
    let value = serde_json::from_slice::<Value>(&old).ok();
    let new = fuzzing::mutate_json(rng, &old, value.as_ref(), &old, value.as_ref());
    let new_id = ContentId::of(&new);
    others[k] = Entry::file(&format!("changes/{}/change.json", new_id.hex()), &new);
    for r in records.iter_mut() {
        if change_id_of(r) == Some(id)
            && let Ok(mut v) = serde_json::from_slice::<Value>(r)
            && let Some(record) = v.as_object_mut()
        {
            record.insert("change_id".into(), Value::String(new_id.to_string()));
            *r = encode_value(&v);
        }
    }
}

/// Edits the records as documents: a field, the order, a duplicate, or a
/// change set re-addressed. Usually the chain is linked again afterwards,
/// so the checks after `verify_log` run.
fn edit_records(rng: &mut Rng, records: &mut Vec<Vec<u8>>, others: &mut [Entry]) {
    if records.is_empty() {
        return;
    }
    let i = rng.below(records.len());
    match rng.below(6) {
        0 | 1 => {
            let donor = serde_json::from_slice::<Value>(rng.pick(records)).ok();
            if let Ok(mut v) = serde_json::from_slice::<Value>(&records[i]) {
                fuzzing::edit_value(rng, &mut v, donor.as_ref());
                records[i] = encode_value(&v);
            }
        }
        2 => {
            records.remove(i);
        }
        3 => {
            let copy = records[i].clone();
            let at = rng.below(records.len() + 1);
            records.insert(at, copy);
        }
        4 => {
            let j = rng.below(records.len());
            records.swap(i, j);
        }
        _ => readdress_change(rng, records, others),
    }
    if rng.chance(70) {
        relink(records);
    }
}

fn random_hex(rng: &mut Rng) -> String {
    ContentId::of(&rng.next_u64().to_le_bytes()).hex()
}

/// Another name for `path`: a near miss of a record's name, a temporary
/// file's name, another directory or another content id.
fn renamed(rng: &mut Rng, path: &str) -> String {
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    let stem = name.strip_suffix(".json").unwrap_or(name);
    let name = match rng.below(10) {
        0 | 1 => format!("{:08}.json", rng.below(5)),
        2 => format!("{:07}.json", rng.below(5)),
        3 => format!("{:09}.json", rng.below(5)),
        4 => format!("{name}.tmp"),
        5 => format!("{stem}.JSON"),
        6 => stem.to_string(),
        7 => format!("{name}~"),
        8 => format!("_{name}"),
        _ => format!("{}.json", random_hex(rng)),
    };
    let dir = match rng.below(8) {
        0 => "log".to_string(),
        1 => format!("{dir}/sub").trim_start_matches('/').to_string(),
        2 => "worlds".into(),
        3 => format!("changes/{}", random_hex(rng)),
        _ => dir.to_string(),
    };
    if dir.is_empty() {
        name
    } else {
        format!("{dir}/{name}")
    }
}

/// A link's target: a file or directory of the tree, a path that does not
/// exist, or `/dev/zero`.
fn link_spec(rng: &mut Rng, entries: &[Entry]) -> Vec<u8> {
    match rng.below(6) {
        0 => b"/dev/zero".to_vec(),
        1 => b"missing/x".to_vec(),
        2 => rng
            .pick(&["log", "changes", "reports", "worlds"])
            .as_bytes()
            .to_vec(),
        _ if entries.is_empty() => b"missing".to_vec(),
        _ => entries[rng.below(entries.len())].path.as_bytes().to_vec(),
    }
}

/// Something added to the tree: in `log/`, the lock, where the next merge
/// writes or at its temporary paths, another change set or world.
fn added_entry(rng: &mut Rng, entries: &[Entry], next: &[String; 3]) -> Entry {
    let kind = if rng.chance(60) {
        Kind::File
    } else {
        *rng.pick(&KINDS)
    };
    let path = match rng.below(8) {
        0 => format!("log/{:08}.json", rng.below(5)),
        1 => format!(
            "log/{}",
            rng.pick(&[
                "00000003.json.tmp",
                ".DS_Store",
                "README.md",
                "00000001.json~",
                "x"
            ])
        ),
        2 => LOG_LOCK.to_string(),
        3 => rng.pick(next).clone(),
        4 => format!("{}.tmp", rng.pick(next)),
        5 => format!("changes/{}/change.json", random_hex(rng)),
        6 => format!("worlds/{}.json", random_hex(rng)),
        _ => format!("{}.json", random_hex(rng)),
    };
    let bytes = match kind {
        Kind::Link => link_spec(rng, entries),
        Kind::File if !entries.is_empty() && rng.chance(60) => {
            entries[rng.below(entries.len())].bytes.clone()
        }
        Kind::File => b"{}".to_vec(),
        _ => Vec::new(),
    };
    Entry { kind, path, bytes }
}

/// Replaces a directory of the tree by a link to a copy of it under
/// `alt<n>/`: a reader that follows links would take the log as valid.
fn link_a_directory(rng: &mut Rng, entries: &mut Vec<Entry>) {
    let dirs: Vec<String> = entries
        .iter()
        .filter_map(|e| e.path.rsplit_once('/').map(|(d, _)| d.to_string()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if dirs.is_empty() {
        return;
    }
    let dir = rng.pick(&dirs).clone();
    let alt = format!("alt{}", rng.below(4));
    let prefix = format!("{dir}/");
    let mut moved = Vec::new();
    entries.retain(|e| match e.path.strip_prefix(&prefix) {
        Some(rest) => {
            moved.push(Entry {
                path: format!("{alt}/{rest}"),
                ..e.clone()
            });
            false
        }
        None => true,
    });
    entries.push(Entry {
        kind: Kind::Link,
        path: dir,
        bytes: alt.into_bytes(),
    });
    entries.extend(moved);
}

/// One edit of a tree's entries: bytes, length, name, presence or kind.
fn edit_entries(rng: &mut Rng, entries: &mut Vec<Entry>, next: &[String; 3]) {
    if entries.is_empty() {
        let added = added_entry(rng, entries, next);
        entries.push(added);
        return;
    }
    let i = rng.below(entries.len());
    match rng.below(12) {
        0 | 1 => {
            let donor = entries[rng.below(entries.len())].bytes.clone();
            fuzzing::mutate_bytes(rng, &mut entries[i].bytes, &donor);
        }
        2 => {
            let len = entries[i].bytes.len();
            entries[i].bytes.truncate(rng.below(len + 1));
        }
        3 => {
            // Grown: copies of itself, or padding, up to about 256 KiB.
            let bytes = &mut entries[i].bytes;
            if !bytes.is_empty() && rng.chance(50) {
                let times = rng.range(2, (262_144 / bytes.len()).clamp(2, 64));
                *bytes = bytes.repeat(times);
            } else {
                let n = *rng.pick(&[1, 4096, 65_536, 262_144]);
                let pad = *rng.pick(&[b' ', b'\n', 0, b'a']);
                bytes.extend(std::iter::repeat_n(pad, n));
            }
        }
        4 => entries[i].path = renamed(rng, &entries[i].path),
        5 => {
            // Removed, or with everything beside it.
            let path = entries.remove(i).path;
            if rng.chance(30)
                && let Some((dir, _)) = path.rsplit_once('/')
            {
                let prefix = format!("{dir}/");
                entries.retain(|e| !e.path.starts_with(&prefix));
            }
        }
        6 | 7 => {
            let added = added_entry(rng, entries, next);
            entries.push(added);
        }
        8 | 9 => {
            let kind = *rng.pick(&[Kind::Dir, Kind::Link, Kind::Link, Kind::Fifo]);
            let bytes = if kind == Kind::Link {
                link_spec(rng, entries)
            } else {
                Vec::new()
            };
            (entries[i].kind, entries[i].bytes) = (kind, bytes);
        }
        10 => link_a_directory(rng, entries),
        _ => {
            // Where the next merge writes: missing, and something at its
            // temporary path.
            let path = rng.pick(next).clone();
            entries.retain(|e| e.path != path);
            let kind = *rng.pick(&KINDS);
            let bytes = if kind == Kind::Link {
                link_spec(rng, entries)
            } else {
                b"left by a crash".to_vec()
            };
            entries.push(Entry {
                kind,
                path: format!("{path}.tmp"),
                bytes,
            });
        }
    }
}

/// One edit of what is where the next merge writes (`next`): other bytes,
/// another kind, missing with something at its temporary path, or its
/// directory a file or a link.
fn edit_next(rng: &mut Rng, entries: &mut Vec<Entry>, next: &[String; 3]) {
    let path = rng.pick(next).clone();
    let Some(i) = entries.iter().position(|e| e.path == path) else {
        return;
    };
    match rng.below(5) {
        0 => {
            let donor = entries[rng.below(entries.len())].bytes.clone();
            fuzzing::mutate_bytes(rng, &mut entries[i].bytes, &donor);
        }
        1 => {
            let kind = *rng.pick(&[Kind::Dir, Kind::Link, Kind::Fifo]);
            let bytes = if kind == Kind::Link {
                link_spec(rng, entries)
            } else {
                Vec::new()
            };
            (entries[i].kind, entries[i].bytes) = (kind, bytes);
        }
        2 => {
            let removed = entries.remove(i);
            if rng.chance(70) {
                let kind = *rng.pick(&KINDS);
                let bytes = if kind == Kind::Link {
                    link_spec(rng, entries)
                } else {
                    removed.bytes
                };
                entries.push(Entry {
                    kind,
                    path: format!("{path}.tmp"),
                    bytes,
                });
            }
        }
        _ => {
            // Its directory (`worlds`, `reports` or `changes/<hex>`) made a
            // file, or a link to a copy of it.
            let dir = path.rsplit_once('/').map_or("", |(d, _)| d).to_string();
            let prefix = format!("{dir}/");
            let mut moved = Vec::new();
            entries.retain(|e| match e.path.strip_prefix(&prefix) {
                Some(rest) => {
                    moved.push(Entry {
                        path: format!("alt/{rest}"),
                        ..e.clone()
                    });
                    false
                }
                None => true,
            });
            if rng.chance(50) {
                entries.push(Entry::file(&dir, b"not a directory"));
            } else {
                entries.push(Entry {
                    kind: Kind::Link,
                    path: dir,
                    bytes: b"alt".to_vec(),
                });
                entries.extend(moved);
            }
        }
    }
}

/// The corpus log, crafted: its records edited as documents (half the
/// time), then its entries edited a few times.
fn craft_log(rng: &mut Rng, c: &LogCorpus) -> Vec<Entry> {
    let mut records: Vec<Vec<u8>> = Vec::new();
    let mut others: Vec<Entry> = Vec::new();
    for (path, bytes) in &c.files {
        if path.starts_with("log/") {
            records.push(bytes.clone());
        } else {
            others.push(Entry::file(path, bytes));
        }
    }
    if rng.chance(50) {
        for _ in 0..rng.count(3) {
            edit_records(rng, &mut records, &mut others);
        }
    }
    let mut entries: Vec<Entry> = records
        .iter()
        .enumerate()
        .map(|(i, bytes)| Entry::file(&format!("log/{i:08}.json"), bytes))
        .collect();
    entries.extend(others);
    if rng.chance(85) {
        for _ in 0..rng.count(4) {
            edit_entries(rng, &mut entries, &c.next);
        }
    }
    entries
}

/// `WorldLog::open` must accept exactly the logs `expected_log` accepts,
/// with the same records, and change nothing; it refuses the others with
/// its documented errors.
fn check_log_dir(root: &Path) {
    let before = snapshot(root);
    let expected = expected_log(root);
    let opened = WorldLog::open(root);
    assert!(snapshot(root) == before, "WorldLog::open changed the log");
    match (opened, expected) {
        (Ok(log), Some(records)) => {
            let want: Vec<(ContentId, MergeRecord)> =
                records.iter().map(|(id, r, _)| (*id, r.clone())).collect();
            assert!(log.records() == want.as_slice(), "other records");
            let (_, last, bytes) = records.last().expect("a record");
            assert!(log.head() == bytes.as_slice(), "another head");
            assert_eq!(log.world_id(), last.world_after);
        }
        (Err(e), None) => assert!(
            matches!(e, op_world::Error::Invalid(_) | op_world::Error::Io(_)),
            "{e:?}"
        ),
        (Ok(_), None) => panic!("WorldLog::open accepted a log it must refuse"),
        (Err(e), Some(_)) => panic!("WorldLog::open refused a log it must accept: {e}"),
    }
}

#[test]
fn world_log_directories() {
    let c = log_corpus();
    let dir = common::temp_dir("fuzz-log-open");
    let fifo = fifo_in(&dir);
    let runs = AtomicU64::new(0);
    fuzzing::run(
        Target::new("log-open", 1500).parts(parts_split, parts_join),
        |rng| tree_encode(0, &craft_log(rng, c)),
        |input| {
            let Some((_, entries)) = tree_decode(input) else {
                return;
            };
            let root = dir.join(format!("l{}", runs.fetch_add(1, Ordering::Relaxed)));
            make_tree(&root, &entries, fifo.as_deref());
            check_log_dir(&root);
            std::fs::remove_dir_all(&root).ok();
        },
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Whether a `--log` merge may write `bytes` at `path` in the log at
/// `root`: every directory on the way is a directory or missing, and the
/// file holds exactly these bytes, or is missing with no directory at its
/// temporary path (a file, a link or a FIFO there is removed first).
fn writable(root: &Path, path: &str, bytes: &[u8]) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    let (file, dirs) = parts.split_last().expect("a path");
    let mut at = root.to_path_buf();
    for part in dirs {
        at.push(part);
        match std::fs::symlink_metadata(&at) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return false,
            Err(_) => return true,
        }
    }
    at.push(file);
    match std::fs::symlink_metadata(&at) {
        Ok(meta) => meta.is_file() && std::fs::read(&at).is_ok_and(|b| b == bytes),
        Err(_) => {
            let mut tmp = at.into_os_string();
            tmp.push(".tmp");
            !std::fs::symlink_metadata(&tmp).is_ok_and(|m| m.is_dir())
        }
    }
}

/// A `--log` merge of the facade on a crafted log, with the corpus world
/// as the store. It must be refused, changing nothing, unless the lock is
/// free, the log is valid and ends at the store's world, and everything
/// where the merge writes is what it may write over (`writable`); then the
/// store and the log are both written. Found by review: a log whose files
/// the merge could not write was found only after the store was written,
/// and the merge stopped with an undo hint.
fn check_log_merge(c: &LogCorpus, base: &Path, flags: u8, entries: &[Entry], fifo: Option<&Path>) {
    let (log, world) = (base.join("log"), base.join("world"));
    make_tree(&log, entries, fifo);
    std::fs::create_dir_all(&world).expect("store");
    std::fs::write(world.join("world.json"), &c.world).expect("world.json");
    let (w0, w1) = (ContentId::of(&c.world), ContentId::of(&c.world_after));
    let expected = expected_log(&log);
    let before = snapshot(&log);
    let lock_free = std::fs::symlink_metadata(log.join(LOG_LOCK)).is_err();
    let appends = c
        .next
        .iter()
        .zip([&c.change, &c.report, &c.world_after])
        .all(|(path, bytes)| writable(&log, path, bytes));
    let args = MergeArgs {
        require_base: flags & 1 == 1,
        ..merge_args(&world, &facade_bundle(), &log, Decision::Merged)
    };
    let result = args.run(&base.join("scratch"), stand_in_gate(c));
    let store = read_tree(&world);
    let Some(records) = expected
        .filter(|r| lock_free && appends && r.last().expect("a record").1.world_after == w0)
    else {
        let e = result.expect_err("a merge on this log is refused");
        assert!(
            matches!(e, op_world::Error::Invalid(_) | op_world::Error::Io(_)),
            "{e:?}"
        );
        assert!(
            snapshot(&log) == before,
            "a refused merge changed the log: {e}"
        );
        assert!(
            store == [("world.json".to_string(), c.world.clone())],
            "a refused merge wrote the store: {e}"
        );
        return;
    };
    let n = records.len();
    let (last_id, _, _) = records.last().expect("a record");
    let line = result.expect("a merge on this log goes through");
    assert!(store.contains(&("world.json".to_string(), c.world_after.clone())));
    assert!(
        line.contains(&format!(" (log/{n:08}.json): world {w0} -> {w1}")),
        "{line}"
    );
    let after = WorldLog::open(&log).expect("the log opens after the merge");
    assert_eq!(after.records().len(), n + 1);
    for ((id, r), (want_id, want, _)) in after.records().iter().zip(&records) {
        assert!(id == want_id && r == want, "a record changed");
    }
    let (_, record) = after.records().last().expect("the new record");
    assert_eq!(record.decision, Decision::Merged);
    assert_eq!(record.change_id, Some(ContentId::of(&c.change)));
    assert_eq!(record.check_report_id, Some(ContentId::of(&c.report)));
    assert_eq!((record.world_before, record.world_after), (Some(w0), w1));
    assert_eq!(record.previous_record_id, Some(*last_id));
    for (path, bytes) in c.next.iter().zip([&c.change, &c.report, &c.world_after]) {
        let parts: Vec<&str> = path.split('/').collect();
        assert!(
            regular_file(&log, &parts).as_ref() == Some(bytes),
            "{path} does not hold what the merge wrote"
        );
    }
}

#[test]
fn log_merges_on_crafted_logs() {
    let c = log_corpus();
    let dir = common::temp_dir("fuzz-log-merge");
    let fifo = fifo_in(&dir);
    let runs = AtomicU64::new(0);
    fuzzing::run(
        Target::new("log-merge", 400).parts(parts_split, parts_join),
        |rng| {
            let flags = u8::from(rng.chance(50));
            let mut entries = craft_log(rng, c);
            if rng.chance(40) {
                edit_next(rng, &mut entries, &c.next);
            }
            tree_encode(flags, &entries)
        },
        |input| {
            let Some((flags, entries)) = tree_decode(input) else {
                return;
            };
            let base = dir.join(format!("m{}", runs.fetch_add(1, Ordering::Relaxed)));
            check_log_merge(c, &base, flags, &entries, fifo.as_deref());
            std::fs::remove_dir_all(&base).ok();
        },
    );
    std::fs::remove_dir_all(&dir).ok();
}

// ------------------------------------------------------------ require-base --

/// `--require-base` reads a bundle's `change.json`, which may be anything:
/// it must pass exactly when it is a regular file holding a change set
/// planned on the store's world, and otherwise return the reader's error.
#[test]
fn require_base_on_crafted_bundles() {
    let c = corpus();
    let changes: Vec<&Doc> = c
        .json
        .iter()
        .filter(|d| d.format() == Some(ChangeSet::FORMAT))
        .collect();
    assert!(changes.len() >= 2);
    let world = fuzzing::repo_root().join("fixtures/world");
    let (world_id, _) = Store::new(&world).read_world().expect("fixture world");
    let dir = common::temp_dir("fuzz-require-base");
    let fifo = fifo_in(&dir);
    let runs = AtomicU64::new(0);
    fuzzing::run(
        Target::new("require-base", 2000).parts(parts_split, parts_join),
        |rng| {
            let doc = rng.pick(&changes);
            let donor = rng.pick(&changes);
            let bytes = if rng.chance(10) {
                doc.bytes.clone()
            } else {
                fuzzing::mutate_json(
                    rng,
                    &doc.bytes,
                    doc.value.as_ref(),
                    &donor.bytes,
                    donor.value.as_ref(),
                )
            };
            // A link to `copy.json`, a regular file holding a valid change
            // set: only a reader that follows links would take it.
            let (kind, bytes) = match rng.below(20) {
                0 => (Kind::Dir, Vec::new()),
                1 | 2 => (
                    Kind::Link,
                    rng.pick(&["copy.json", "/dev/zero", "missing"])
                        .as_bytes()
                        .to_vec(),
                ),
                3 => (Kind::Fifo, Vec::new()),
                _ => (Kind::File, bytes),
            };
            let name = if rng.chance(3) {
                "x.json"
            } else {
                "change.json"
            };
            let change = Entry {
                kind,
                path: name.into(),
                bytes,
            };
            tree_encode(0, &[change, Entry::file("copy.json", &doc.bytes)])
        },
        |input| {
            let Some((_, entries)) = tree_decode(input) else {
                return;
            };
            let bundle = dir.join(format!("b{}", runs.fetch_add(1, Ordering::Relaxed)));
            make_tree(&bundle, &entries, fifo.as_deref());
            let expected =
                regular_file(&bundle, &["change.json"]).map(|b| ChangeSet::from_bytes(&b));
            let result = require_base(&world, &bundle);
            std::fs::remove_dir_all(&bundle).ok();
            match expected {
                None => assert!(
                    matches!(&result, Err(op_world::Error::Io(m)) if m.starts_with("change.json: ")),
                    "{result:?}"
                ),
                Some(Err(e)) => assert_eq!(result, Err(e)),
                Some(Ok(change)) if change.base_world_id == world_id => assert_eq!(result, Ok(())),
                Some(Ok(change)) => assert_eq!(
                    result,
                    Err(op_world::Error::Invalid(format!(
                        "the store holds world {world_id}, but this change set was planned on {}; plan the refresh again",
                        change.base_world_id
                    )))
                ),
            }
        },
    );
    std::fs::remove_dir_all(&dir).ok();
}

// -------------------------------------------------------- merge arguments --

const MERGE_OPTIONS: &[&str] = &[
    "--previous",
    "--record",
    "--report",
    "--log",
    "--reason",
    "--review",
    "--takedown",
    "--require-base",
];

/// Options that are not quite the merge commands' options.
const NEAR_OPTIONS: &[&str] = &[
    "--",
    "---log",
    "--Log",
    "-log",
    "--log=l",
    "--require-bas",
    "--require-base=yes",
    "--revie",
    "--replay",
    "--takedowns",
    "--world",
    "-",
    "\u{2014}log",
];

const ARG_VALUES: &[&str] = &[
    "w", "b", "l", "p.json", "r.json", "g.json", "", " ", "-x", "x y", "C:\\w", "/w", "\u{e9}",
    "\u{feff}", "Why.",
];

const REVIEW_VALUES: &[&str] = &[
    "github:a=better",
    "github:a=better:Fine.",
    "github:b=not_better",
    "github:c=blocked:No.",
    "=better",
    "a=",
    "a",
    "a=great",
    "a==better",
    "a=better:",
    "a=better:x:y",
    "a=:x",
    "a=BETTER",
    "a = better",
];

/// Arguments for `MergeArgs::parse`: the merge commands' options, mostly in
/// a valid order, then tokens dropped, repeated, swapped or added (options,
/// near misses, values).
fn merge_argv(rng: &mut Rng) -> Vec<String> {
    fn option(argv: &mut Vec<String>, name: &str, value: &str) {
        argv.push(name.into());
        argv.push(value.into());
    }
    let mut argv: Vec<String> = vec![
        (*rng.pick(&["w", "world", "--log", ""])).into(),
        (*rng.pick(&["b", "bundle", "-", ""])).into(),
    ];
    if rng.chance(50) {
        option(&mut argv, "--log", "l");
    } else {
        for name in ["--previous", "--record", "--report"] {
            option(&mut argv, name, rng.pick(ARG_VALUES));
        }
    }
    option(&mut argv, "--reason", "Why.");
    for _ in 0..rng.below(3) {
        option(&mut argv, "--review", rng.pick(REVIEW_VALUES));
    }
    for flag in ["--takedown", "--require-base"] {
        if rng.chance(30) {
            argv.push(flag.into());
        }
    }
    for _ in 0..rng.count(6) {
        let i = rng.below(argv.len() + 1);
        let token = match rng.below(4) {
            0 => rng.pick(MERGE_OPTIONS),
            1 => rng.pick(NEAR_OPTIONS),
            2 => rng.pick(REVIEW_VALUES),
            _ => rng.pick(ARG_VALUES),
        };
        match rng.below(6) {
            0 if i < argv.len() => {
                argv.remove(i);
            }
            1 if i < argv.len() => {
                let copy = argv[i].clone();
                argv.insert(i, copy);
            }
            2 if i < argv.len() => {
                let j = rng.below(argv.len());
                argv.swap(i, j);
            }
            3 => argv.truncate(i),
            _ => argv.insert(i, (*token).into()),
        }
    }
    argv
}

/// What `MergeArgs::parse` must give, restated from `MergeArgs::USAGE` and
/// its documentation: the arguments, or `None` for a usage error.
fn expected_merge_args(args: &[String]) -> Option<MergeArgs> {
    let [world, bundle, rest @ ..] = args else {
        return None;
    };
    // No directory may start with `--` or be empty.
    if [world, bundle]
        .iter()
        .any(|d| d.starts_with("--") || d.is_empty())
    {
        return None;
    }
    let mut values: BTreeMap<&str, String> = BTreeMap::new();
    let (mut reviews, mut takedown, mut require_base) = (Vec::new(), false, false);
    let mut i = 0;
    while i < rest.len() {
        let option = rest[i].as_str();
        let value = rest.get(i + 1).filter(|v| !v.starts_with("--"));
        i += 1 + usize::from(value.is_some());
        match option {
            "--takedown" if value.is_none() => takedown = true,
            "--require-base" if value.is_none() => require_base = true,
            "--review" => {
                let (reviewer, rest) = value.map_or("", String::as_str).split_once('=')?;
                let (verdict, notes) = rest.split_once(':').unwrap_or((rest, rest));
                let verdict = match verdict {
                    "better" => Verdict::Better,
                    "not_better" => Verdict::NotBetter,
                    "blocked" => Verdict::Blocked,
                    _ => return None,
                };
                reviews.push(Review {
                    reviewer: reviewer.into(),
                    verdict,
                    notes: notes.into(),
                });
            }
            "--previous" | "--record" | "--report" | "--log" | "--reason" => {
                // Every value but the reasons is a path, and no path may be
                // empty.
                let value = value?;
                if value.is_empty() && option != "--reason" {
                    return None;
                }
                if values.insert(option, value.clone()).is_some() {
                    return None;
                }
            }
            _ => return None,
        }
    }
    let output = match values.remove("--log") {
        Some(_) if values.keys().any(|k| *k != "--reason") => return None,
        Some(log) => MergeOutput::Log(log.into()),
        None => MergeOutput::Files {
            previous: values.remove("--previous")?.into(),
            record: values.remove("--record")?.into(),
            report: values.remove("--report")?.into(),
        },
    };
    Some(MergeArgs {
        world_dir: world.into(),
        bundle_dir: bundle.into(),
        output,
        decision: if takedown {
            Decision::Takedown
        } else {
            Decision::Merged
        },
        reviews,
        reasons: values.remove("--reason")?,
        require_base,
    })
}

/// A message `MergeArgs::parse` gives for a usage error.
fn is_usage_error(m: &str) -> bool {
    let valued = ["--previous", "--record", "--report", "--log", "--reason"];
    m == format!("merge takes {}", MergeArgs::USAGE)
        || m == "<world-dir> and <bundle-dir> must not be empty"
        || ["--previous", "--record", "--report", "--log"]
            .iter()
            .any(|o| m == format!("{o} must not be empty"))
        || m == "--review is <handle>=<verdict>[:<notes>]"
        || m.starts_with("unknown verdict ")
        || m.starts_with("unknown option --")
        || m.starts_with("unexpected argument ")
        || ["--takedown", "--require-base"]
            .iter()
            .any(|o| m == format!("{o} takes no value"))
        || valued.iter().any(|o| {
            m == format!("{o} <value> is required")
                || m == format!("{o} is given twice")
                || m.starts_with(&format!("{o} cannot be combined with --log"))
        })
}

/// `MergeArgs::parse` on arguments from the real options and junk: the
/// arguments their documentation gives, or a usage error, never a panic.
#[test]
fn merge_arguments() {
    fuzzing::run(
        Target::new("merge-args", 20_000),
        |rng| merge_argv(rng).join("\0").into_bytes(),
        |input| {
            let args: Vec<String> = String::from_utf8_lossy(input)
                .split('\0')
                .map(String::from)
                .collect();
            let parsed = MergeArgs::parse(&args);
            match (&parsed, expected_merge_args(&args)) {
                (Ok(got), Some(want)) => assert_eq!(*got, want),
                (Err(m), None) => assert!(is_usage_error(m), "{m}"),
                (_, want) => panic!("{args:?}: parsed as {parsed:?}, expected {want:?}"),
            }
        },
    );
}

// ------------------------------------------------------- JSON size limits --
//
// RFC 0001 section 7: every kind of JSON document has a size limit, checked
// before it is parsed. Found by the fuzzing round: a crafted 64 MiB
// document, the largest file a store read takes, made serde_json take about
// 7 GB.

/// Text number `i` of `n` characters: sorted and unique by its number.
fn padding(i: usize, n: usize) -> String {
    format!("{i:07}{}", "x".repeat(n - 7))
}

/// Lengths of items for `doc` whose bytes are exactly `len` long. `doc`
/// writes one item per length, and an item's bytes grow by one for each
/// unit of its length, from `min` to `max`, so only small documents are
/// written to find them.
fn lengths_for(
    len: usize,
    min: usize,
    max: usize,
    doc: &dyn Fn(&[usize]) -> Vec<u8>,
) -> Vec<usize> {
    let mid = (min + max) / 2;
    let one = doc(&[mid]).len();
    let per_item = doc(&[mid, mid]).len() - one;
    let count = (len - one) / per_item + 1;
    let mut lengths = vec![mid; count];
    let mut left = len - (one + (count - 1) * per_item);
    for l in &mut lengths {
        let grow = left.min(max - *l);
        *l += grow;
        left -= grow;
    }
    assert_eq!(left, 0, "cannot make {len} bytes");
    lengths
}

/// The canonical bytes of a document of `kind`, made of items of the given
/// lengths: valid for every kind but change sets, which list one change
/// per item and so break the cap of 50 changes.
fn document_of(kind: Document, lengths: &[usize]) -> Vec<u8> {
    let texts = || lengths.iter().enumerate().map(|(i, &n)| padding(i, n));
    let fixture = |path: &str| std::fs::read(fuzzing::repo_root().join(path)).expect(path);
    let sample = "fixtures/changes/facade-sample";
    match kind {
        Document::AssetManifest => {
            let change =
                ChangeSet::from_bytes(&fixture(&format!("{sample}/change.json"))).expect("sample");
            let id = change.changes[0].manifest_id.expect("an add");
            let bundle = Store::new(fuzzing::repo_root().join(sample));
            let mut m = AssetManifest::from_bytes(
                &bundle
                    .read_object(&id, ObjectKind::Json)
                    .expect("sample manifest"),
            )
            .expect("sample manifest");
            m.provenance.sources[0].source_ids = texts().collect();
            cjson::to_canonical(&m)
        }
        Document::TileIndex => {
            let world = Store::new(fuzzing::repo_root().join("fixtures/world"));
            let (_, w) = world.read_world().expect("fixture world");
            let mut index = TileIndex::from_bytes(
                &world
                    .read_object(&w.tiles[0].tile_id, ObjectKind::Json)
                    .expect("tile index"),
            )
            .expect("fixture tile index");
            let first = index.assets[0].clone();
            index.assets = texts()
                .map(|t| op_world::TileAsset {
                    layer: op_world::Layer::Facade,
                    feature_id: format!("t:{t}"),
                    ..first.clone()
                })
                .collect();
            cjson::to_canonical(&index)
        }
        Document::WorldManifest => {
            let (_, mut w) = Store::new(fuzzing::repo_root().join("fixtures/world"))
                .read_world()
                .expect("fixture world");
            w.attribution = texts().collect();
            cjson::to_canonical(&w)
        }
        Document::ChangeSet => {
            let mut change =
                ChangeSet::from_bytes(&fixture(&format!("{sample}/change.json"))).expect("sample");
            let first = change.changes[0].clone();
            change.changes = texts()
                .map(|t| Change {
                    feature_id: format!("t:{t}"),
                    ..first.clone()
                })
                .collect();
            cjson::to_canonical(&change)
        }
        Document::CheckReport => {
            let mut report = CheckReport::from_bytes(&fixture(&format!("{sample}/report.json")))
                .expect("sample");
            report.checks[0].details = texts().collect();
            cjson::to_canonical(&report)
        }
        Document::MergeRecord => {
            let (_, mut record) = sample_log().swap_remove(0);
            record.reviews = texts()
                .enumerate()
                .map(|(i, notes)| Review {
                    reviewer: format!("github:r{i:07}"),
                    verdict: Verdict::Better,
                    notes,
                })
                .collect();
            cjson::to_canonical(&record)
        }
        Document::Points => {
            // Trees of unknown attributes, one per item, with ids of the
            // item's length after the source `a:`.
            let point = |id: String| Point {
                id,
                kind: PointKind::Tree,
                anchor_mm: [0, 0, 0],
                attributes: Attributes::Tree(TreeAttributes::default()),
                estimated: vec![],
            };
            cjson::to_canonical(&PointsDocument {
                format: PointsDocument::FORMAT.into(),
                contract_version: op_world::CONTRACT_VERSION.into(),
                points: texts().map(|t| point(format!("a:{t}"))).collect(),
                rules: BTreeMap::new(),
            })
        }
    }
    .expect("canonical")
}

/// The longest item of `document_of(kind, ...)`, so that every item stays
/// valid: feature ids and handles are at most 256 bytes, texts at most
/// 4,096 characters.
fn longest_item(kind: Document) -> usize {
    match kind {
        Document::TileIndex | Document::ChangeSet | Document::Points => 254,
        _ => rules::MAX_TEXT_CHARS,
    }
}

/// `bytes` parsed as canonical JSON of `kind`'s type, whatever their size,
/// and written again with `to_bytes`.
fn write_as(kind: Document, bytes: &[u8]) -> op_world::Result<Vec<u8>> {
    match kind {
        Document::AssetManifest => cjson::from_canonical::<AssetManifest>(bytes)?.to_bytes(),
        Document::TileIndex => cjson::from_canonical::<TileIndex>(bytes)?.to_bytes(),
        Document::WorldManifest => cjson::from_canonical::<WorldManifest>(bytes)?.to_bytes(),
        Document::ChangeSet => cjson::from_canonical::<ChangeSet>(bytes)?.to_bytes(),
        Document::CheckReport => cjson::from_canonical::<CheckReport>(bytes)?.to_bytes(),
        Document::MergeRecord => cjson::from_canonical::<MergeRecord>(bytes)?.to_bytes(),
        Document::Points => cjson::from_canonical::<PointsDocument>(bytes)?.to_bytes(),
    }
}

/// `from_bytes` of `kind` on `bytes`, without its value.
fn read_as(kind: Document, bytes: &[u8]) -> op_world::Result<()> {
    match kind {
        Document::AssetManifest => AssetManifest::from_bytes(bytes).map(drop),
        Document::TileIndex => TileIndex::from_bytes(bytes).map(drop),
        Document::WorldManifest => WorldManifest::from_bytes(bytes).map(drop),
        Document::ChangeSet => ChangeSet::from_bytes(bytes).map(drop),
        Document::CheckReport => CheckReport::from_bytes(bytes).map(drop),
        Document::MergeRecord => MergeRecord::from_bytes(bytes).map(drop),
        Document::Points => PointsDocument::from_bytes(bytes).map(drop),
    }
}

/// A valid document at the limit for its kind reads and writes, and one
/// byte more is refused before it is parsed, with a message that names the
/// kind and the limit; `to_bytes` refuses to write it. A change set cannot
/// reach its limit (50 changes of a few hundred bytes), so one at the limit
/// is refused for its rules instead, not for its size. World manifests
/// and points documents, whose limit is that of every file (64 MiB), are
/// only checked over it, below: building and reading 64 MiB here would take
/// seconds.
#[test]
fn documents_read_up_to_the_limit_for_their_kind() {
    for kind in Document::ALL {
        if kind.max_bytes() == op_world::store::MAX_FILE_BYTES {
            continue;
        }
        let limit = kind.max_bytes() as usize;
        let doc = |lengths: &[usize]| document_of(kind, lengths);
        let mut lengths = lengths_for(limit, 8, longest_item(kind), &doc);
        let at = doc(&lengths);
        assert_eq!(at.len(), limit, "{kind:?}");
        match kind {
            Document::ChangeSet => assert_eq!(
                read_as(kind, &at).unwrap_err().to_string(),
                "invalid: a change set has 1..=50 changes"
            ),
            _ => {
                read_as(kind, &at).unwrap_or_else(|e| panic!("{kind:?} at its limit: {e}"));
                assert!(write_as(kind, &at).expect("writes") == at);
            }
        }
        let k = lengths
            .iter()
            .position(|&l| l < longest_item(kind))
            .expect("an item that can grow");
        lengths[k] += 1;
        let over = doc(&lengths);
        assert_eq!(over.len(), limit + 1);
        assert_eq!(
            read_as(kind, &over).unwrap_err().to_string(),
            format!("invalid: {}", kind.too_large())
        );
        if kind != Document::ChangeSet {
            let refused = write_as(kind, &over).unwrap_err().to_string();
            assert!(refused.ends_with(&kind.too_large()), "{refused}");
        }
    }
}

/// Each kind's worst case for the parser, one byte over its limit: one-key
/// objects, which serde_json parses at about 83 bytes of memory per byte
/// (1.4 GB for a check report, 5.6 GB for a world manifest). Each is
/// refused before it is parsed, with the memory of the input alone. The
/// input's first byte says its kind.
#[test]
fn regression_documents_over_their_limit_are_refused_unparsed() {
    let next = AtomicU64::new(0);
    fuzzing::run(
        Target::new("documents-over-their-limit", 1)
            .exactly(Document::ALL.len() as u64)
            .memory(1, 1 << 20),
        |_| {
            let k = next.fetch_add(1, Ordering::Relaxed) as usize % Document::ALL.len();
            let len = Document::ALL[k].max_bytes() as usize + 1;
            let mut input = vec![k as u8, b'['];
            while input.len() + 8 < len + 1 {
                input.extend_from_slice(b"{\"a\":0},");
            }
            input.resize(len, b' ');
            input.push(b']');
            input
        },
        |input| {
            let kind = Document::ALL[usize::from(input[0])];
            let doc = &input[1..];
            assert_eq!(doc.len() as u64, kind.max_bytes() + 1);
            assert_eq!(
                read_as(kind, doc).unwrap_err().to_string(),
                format!("invalid: {}", kind.too_large())
            );
        },
    );
}

/// A file one byte over the limit for its kind is refused before it is
/// read, wherever a document of a known kind is read: files, store
/// objects, validation and the gate, which reports a change set or a
/// manifest over its limit in G1. The files are sparse where the
/// filesystem allows.
#[test]
fn regression_document_files_over_their_limit_are_refused_unread() {
    use op_world::store::read_document;
    let dir = common::temp_dir("regression-document-files");
    let long = |path: &Path, kind: Document| {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::File::create(path)
            .and_then(|f| f.set_len(kind.max_bytes() + 1))
            .expect("a long file");
    };
    for kind in Document::ALL {
        let path = dir.join(format!("{kind:?}.json"));
        long(&path, kind);
        let err = read_document(&path, kind).expect_err("too large");
        assert_eq!(err.kind(), std::io::ErrorKind::FileTooLarge);
        assert_eq!(err.to_string(), kind.too_large());
        std::fs::remove_file(&path).expect("remove");
    }

    // A bundle whose manifest is over its limit.
    let sample = fuzzing::repo_root().join("fixtures/changes/facade-sample");
    let world = fuzzing::repo_root().join("fixtures/world");
    let change = ChangeSet::from_bytes(&std::fs::read(sample.join("change.json")).expect("read"))
        .expect("sample change set");
    let c = &change.changes[0];
    let manifest_id = c.manifest_id.expect("an add");
    let bundle = dir.join("bundle");
    std::fs::create_dir_all(&bundle).expect("bundle");
    std::fs::copy(sample.join("change.json"), bundle.join("change.json")).expect("copy");
    long(
        &Store::new(&bundle).object_path(&manifest_id, ObjectKind::Json),
        Document::AssetManifest,
    );
    let manifest_error = format!(
        "I/O error: object {manifest_id}: larger than 1048576 bytes (1 MiB), the limit for an asset manifest"
    );
    assert_eq!(
        Store::new(&bundle)
            .read_document(&manifest_id, Document::AssetManifest)
            .unwrap_err()
            .to_string(),
        manifest_error
    );
    let report = gate::run(&world, &bundle, &dir.join("candidate-1")).expect("a report");
    assert_eq!(report.gate_version, "0.1.5");
    assert_eq!(report.checks[0].outcome, op_world::contrib::Outcome::Fail);
    assert_eq!(
        report.checks[0].details[0],
        format!(
            "{} {} {}: {manifest_error}",
            c.tile,
            c.layer.as_str(),
            c.feature_id
        )
    );
    assert_eq!(report.candidate_world_id, None);

    // A change set over its limit: the gate reports it in G1.
    long(&bundle.join("change.json"), Document::ChangeSet);
    let report = gate::run(&world, &bundle, &dir.join("candidate-2")).expect("a report");
    assert_eq!(report.checks[0].outcome, op_world::contrib::Outcome::Fail);
    assert_eq!(
        report.checks[0].details,
        ["change.json: invalid: larger than 8388608 bytes (8 MiB), the limit for a change set"]
    );
    assert!(
        report.checks[1..]
            .iter()
            .all(|c| c.outcome == op_world::contrib::Outcome::NotRun)
    );
    assert_eq!(
        CheckReport::from_bytes(&report.to_bytes().expect("valid")).expect("reads"),
        report
    );
    assert_eq!(
        require_base(&world, &bundle).unwrap_err().to_string(),
        "I/O error: change.json: larger than 8388608 bytes (8 MiB), the limit for a change set"
    );

    // A world whose tile index is over its limit does not validate.
    let store = dir.join("store");
    std::fs::create_dir_all(&store).expect("store");
    std::fs::copy(world.join("world.json"), store.join("world.json")).expect("copy");
    let (_, w) = Store::new(&store).read_world().expect("fixture world");
    long(
        &Store::new(&store).object_path(&w.tiles[0].tile_id, ObjectKind::Json),
        Document::TileIndex,
    );
    let report = validate_store(&Store::new(&store));
    assert_eq!(
        report.errors[0],
        format!(
            "tile {}: I/O error: object {}: larger than 16777216 bytes (16 MiB), the limit for a tile index",
            w.tiles[0].tile, w.tiles[0].tile_id
        )
    );
    std::fs::remove_dir_all(&dir).ok();
}

// ------------------------------------------------------------- regressions --
//
// Inputs that once broke a reader, minimised. Each must stay an error.

// The four inputs below were found by the log-open, log-merge,
// require-base and merge-args targets, run on Linux against the readers as
// they were before they were hardened (the targets were written with the
// fix). The two hangs are not shrunk by the engine; these are their
// essence.

/// The merge arguments `-- "" --log "" --reason ""` (the merge-args target,
/// shrunk): `--` was taken as the world directory. The two directories come
/// first, so an option there is a usage error, as in `op_replay merge`.
#[test]
fn regression_merge_args_take_no_option_as_a_directory() {
    let args: Vec<String> = String::from_utf8_lossy(b"--\x00\x00--log\x00\x00--reason\x00")
        .split('\0')
        .map(String::from)
        .collect();
    assert_eq!(
        MergeArgs::parse(&args),
        Err(format!("merge takes {}", MergeArgs::USAGE))
    );
}

/// A bundle whose `change.json` is a FIFO (the require-base target):
/// `--require-base` read it with `fs::read`, which waited forever for a
/// writer. It is refused unopened. Runs through the engine for its hang
/// watchdog.
#[cfg(unix)]
#[test]
fn regression_require_base_never_opens_a_fifo() {
    let dir = common::temp_dir("regression-require-base-fifo");
    let fifo = fifo_in(&dir);
    let world = fuzzing::repo_root().join("fixtures/world");
    fuzzing::run(
        Target::new("require-base-fifo", 1).exactly(1),
        |_| {
            let fifo = Entry {
                kind: Kind::Fifo,
                path: "change.json".into(),
                bytes: Vec::new(),
            };
            tree_encode(0, &[fifo])
        },
        |input| {
            let (_, entries) = tree_decode(input).expect("a tree");
            let bundle = dir.join("bundle");
            make_tree(&bundle, &entries, fifo.as_deref());
            assert_eq!(
                require_base(&world, &bundle),
                Err(op_world::Error::Io(
                    "change.json: not a regular file".into()
                ))
            );
            std::fs::remove_dir_all(&bundle).ok();
        },
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The corpus log with `log/00000001.json` a FIFO (the log-open target):
/// `WorldLog::open` read it with `fs::read`, which waited forever. It is
/// refused unopened. Runs through the engine for its hang watchdog.
#[cfg(unix)]
#[test]
fn regression_a_log_record_that_is_a_fifo_is_never_opened() {
    let c = log_corpus();
    let dir = common::temp_dir("regression-log-fifo");
    let fifo = fifo_in(&dir);
    fuzzing::run(
        Target::new("log-open-fifo", 1).exactly(1),
        |_| {
            let entries: Vec<Entry> = c
                .files
                .iter()
                .map(|(path, bytes)| match path.as_str() {
                    "log/00000001.json" => Entry {
                        kind: Kind::Fifo,
                        path: path.clone(),
                        bytes: Vec::new(),
                    },
                    _ => Entry::file(path, bytes),
                })
                .collect();
            tree_encode(0, &entries)
        },
        |input| {
            let (_, entries) = tree_decode(input).expect("a tree");
            let root = dir.join("log");
            make_tree(&root, &entries, fifo.as_deref());
            check_log_dir(&root);
            assert_eq!(
                WorldLog::open(&root)
                    .err()
                    .map(|e| e.to_string())
                    .as_deref(),
                Some(
                    "invalid: the world log's log/00000001.json is not a regular file; records are regular files, not links, directories or pipes"
                )
            );
            std::fs::remove_dir_all(&root).ok();
        },
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The corpus log with `worlds` a link to a directory that holds the
/// baseline's world but not the next one (the log-merge target): a `--log`
/// merge followed the link and wrote the new world into the linked
/// directory, outside the log, and its undo hint named a world it reached
/// through the link. The merge is now refused before it writes anything,
/// as the target's checks require. Runs where the platform lets a test make
/// links (not on Windows without the right).
#[test]
fn regression_a_log_merge_writes_through_no_linked_directory() {
    let c = log_corpus();
    let dir = common::temp_dir("regression-log-merge-link");
    if let Err(e) = symlink_file(&dir.join("missing"), &dir.join("probe")) {
        println!("not run: cannot make a link here ({e})");
        std::fs::remove_dir_all(&dir).ok();
        return;
    }
    let after = format!("{}.json", ContentId::of(&c.world_after).hex());
    let mut entries = vec![Entry {
        kind: Kind::Link,
        path: "worlds".into(),
        bytes: b"alt".to_vec(),
    }];
    for (path, bytes) in &c.files {
        match path.strip_prefix("worlds/") {
            Some(name) if name == after => {}
            Some(name) => entries.push(Entry::file(&format!("alt/{name}"), bytes)),
            None => entries.push(Entry::file(path, bytes)),
        }
    }
    let base = dir.join("m");
    check_log_merge(c, &base, 0, &entries, None);
    assert!(
        !base.join("log/alt").join(&after).exists(),
        "the merge wrote through the link"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// A glb of `nodes` nodes, each showing one primitive of `vertices`
/// vertices (at the origin) and one triangle, valid but for one kind of
/// sharing. With `shared_mesh`, every node shows mesh 0. Otherwise every
/// node has a mesh of its own, but all of them read one POSITION accessor,
/// as their NORMAL too. Only the reader's guards refuse it: without them,
/// it decodes the same data again for every node and attribute, and only
/// then fails the canonical-form check. Found by review while writing the
/// glb fuzz target: a 37 KB file made the reader copy 786 MB, and larger
/// ones aborted the process.
fn shared_glb(nodes: usize, vertices: usize, shared_mesh: bool) -> Vec<u8> {
    use serde_json::json;
    let floats = 12 * vertices;
    let vec3 = |view: usize| json!({"bufferView": view, "componentType": 5126, "count": vertices, "type": "VEC3"});
    let indices = |view: usize| json!({"bufferView": view, "componentType": 5125, "count": 3, "type": "SCALAR"});
    let view = |offset: usize, length: usize| json!({"byteLength": length, "byteOffset": offset});
    let mesh = |position: usize, normal: usize, index: usize| {
        json!({"primitives": [{
            "attributes": {"NORMAL": normal, "POSITION": position},
            "indices": index,
            "material": 0,
            "mode": 4
        }]})
    };
    let (accessors, views, meshes, bin) = if shared_mesh {
        (
            vec![vec3(0), vec3(1), indices(2)],
            vec![view(0, floats), view(floats, floats), view(2 * floats, 12)],
            vec![mesh(0, 1, 2)],
            2 * floats + 12,
        )
    } else {
        (
            vec![vec3(0), indices(1)],
            vec![view(0, floats), view(floats, 12)],
            vec![mesh(0, 0, 1); nodes],
            floats + 12,
        )
    };
    let nodes: Vec<Value> = (0..nodes)
        .map(|i| json!({"mesh": if shared_mesh { 0 } else { i }, "name": "a:b"}))
        .collect();
    let doc = json!({
        "accessors": accessors,
        "bufferViews": views,
        "materials": [{"name": "road"}],
        "meshes": meshes,
        "nodes": nodes
    });
    fuzzing::glb_pack(&serde_json::to_vec(&doc).expect("JSON"), &vec![0; bin])
}

fn glb_error(bytes: &[u8]) -> String {
    match glb::read(bytes) {
        Err(op_world::Error::Glb(m)) => m,
        other => panic!("expected a glb error, got {other:?}"),
    }
}

const SHARED_ACCESSOR: &str = "accessors read more data than the BIN chunk holds";
const SHARED_MESH: &str = "node 1 must show mesh 1";

/// One primitive whose POSITION and NORMAL are the same accessor: 24 bytes
/// decoded from 12 bytes of positions.
#[test]
fn regression_glb_accessors_cannot_decode_the_bin_chunk_twice() {
    assert_eq!(glb_error(&shared_glb(1, 1, false)), SHARED_ACCESSOR);
}

/// Two nodes showing the same mesh: its data decoded once per node.
#[test]
fn regression_glb_nodes_cannot_share_a_mesh() {
    assert_eq!(glb_error(&shared_glb(2, 1, true)), SHARED_MESH);
}

/// Both at scale, 5,000 nodes on 1,000 vertices: about 120 MB to decode
/// without the guards. Where the memory check is available (Linux), the
/// reader must refuse them taking little beyond their size.
#[test]
fn regression_glb_shared_accessors_at_scale() {
    let input = shared_glb(5000, 1000, false);
    assert!(input.len() < 1 << 20);
    fuzzing::run(
        Target::new("glb-shared-accessor", 1)
            .exactly(1)
            .memory(64, 1 << 20),
        |_| input.clone(),
        |bytes| assert_eq!(glb_error(bytes), SHARED_ACCESSOR),
    );
}

#[test]
fn regression_glb_shared_meshes_at_scale() {
    let input = shared_glb(5000, 1000, true);
    assert!(input.len() < 1 << 20);
    fuzzing::run(
        Target::new("glb-shared-mesh", 1)
            .exactly(1)
            .memory(64, 1 << 20),
        |_| input.clone(),
        |bytes| assert_eq!(glb_error(bytes), SHARED_MESH),
    );
}

/// A canonical document of 1,400 chains of nested objects, 127 levels
/// deep: parsing it takes about 112 bytes per input byte (a 632-byte map
/// node for each `{"a":`), and the documents target must allow that. Found
/// by review: at 64 bytes per input byte allowed, a long campaign reached
/// 99% of the allowance with valid nesting, and a document like this one
/// went over it.
#[test]
fn regression_parsing_nested_objects_fits_the_allowance() {
    let chain = format!("{}0{}", "{\"a\":".repeat(126), "}".repeat(126));
    let doc = format!("[{}]", vec![chain; 1400].join(","));
    assert_eq!(doc.len(), 1_061_201);
    assert!(cjson::from_canonical::<Value>(doc.as_bytes()).is_ok());
    fuzzing::run(
        Target::new("documents-nested", 1).exactly(1),
        |_| doc.as_bytes().to_vec(),
        check_documents,
    );
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

/// `op_world id` read each file with `fs::read`: any size, through links,
/// and waiting forever on a FIFO. It reads them as a store does now, and
/// says why it refuses one. Found by review.
#[test]
fn regression_op_world_id_reads_only_regular_files() {
    let dir = common::temp_dir("regression-id");
    let (file, folder) = (dir.join("file.json"), dir.join("folder"));
    std::fs::write(&file, b"{}").expect("write");
    std::fs::create_dir(&folder).expect("mkdir");
    let mut named = vec![(folder.clone(), "not a regular file")];
    #[cfg(unix)]
    {
        let made = std::process::Command::new("mkfifo")
            .arg(dir.join("fifo"))
            .status()
            .expect("run mkfifo");
        assert!(made.success(), "mkfifo failed");
        named.push((dir.join("fifo"), "not a regular file"));
    }
    if symlink_file(&file, &dir.join("link")).is_ok() {
        named.push((dir.join("link"), "not a regular file"));
    }
    let long = dir.join("long.json");
    std::fs::File::create(&long)
        .and_then(|f| f.set_len(op_world::store::MAX_FILE_BYTES + 1))
        .expect("a long file");
    named.push((long, "larger than 67108864 bytes"));
    for (path, why) in named {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_op_world"))
            .arg("id")
            .arg(&file)
            .arg(&path)
            .output()
            .expect("run op_world id");
        assert_eq!(out.status.code(), Some(1), "{}", path.display());
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            format!("{}  {}\n", ContentId::of(b"{}"), file.display())
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            format!("{}: {why}\n", path.display())
        );
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// Makes `link` a symbolic link to the file `target`.
#[cfg(unix)]
fn symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Makes `link` a symbolic link to the file `target` (Windows allows it
/// only with developer mode or the right to make links).
#[cfg(windows)]
fn symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

#[cfg(not(any(unix, windows)))]
fn symlink_file(_: &Path, _: &Path) -> std::io::Result<()> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}

/// A bundle can hold an object as a link (git keeps them). The layer lookup
/// followed it, chose the bundle, and the read refused the link, so the
/// world's regular copy of the same object was never read. A link is now
/// passed over like a directory or a FIFO. Found by review. Runs where the
/// platform lets a test make links (not on Windows without the right).
#[test]
fn regression_a_link_does_not_hide_an_object_below_it() {
    let dir = common::temp_dir("regression-store-link");
    let (bundle, world) = (dir.join("bundle"), dir.join("world"));
    let bytes = b"{}".as_slice();
    let id = Store::new(&world)
        .write_object(bytes, ObjectKind::Json)
        .expect("write object");
    let link = Store::new(&bundle).object_path(&id, ObjectKind::Json);
    std::fs::create_dir_all(link.parent().expect("parent")).expect("mkdir");
    let target = Store::new(&world).object_path(&id, ObjectKind::Json);
    if let Err(e) = symlink_file(&target, &link) {
        println!("not run: cannot make a link here ({e})");
        std::fs::remove_dir_all(&dir).ok();
        return;
    }
    let layered = Store::layered(&bundle, vec![world.clone()]);
    assert!(layered.has_object(&id, ObjectKind::Json));
    assert!(
        layered
            .read_object(&id, ObjectKind::Json)
            .expect("the world's copy")
            == bytes
    );
    // With no regular copy, the link is refused, never followed.
    let alone = Store::new(&bundle);
    assert!(!alone.has_object(&id, ObjectKind::Json));
    assert_eq!(
        alone
            .read_object(&id, ObjectKind::Json)
            .expect_err("a link")
            .to_string(),
        format!("I/O error: object {id}: not a regular file")
    );
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
