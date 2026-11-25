//! Point layers (RFC 0004): the points document reader's rejections,
//! whole-world validation of point assets, the gate's rules for
//! `vegetation` and `street_objects` (G1, G3, G7, G8 at gate 0.1.4), and a
//! merge that copies a points document into the store.
//!
//! The gate tests use [`sample_points_draft`]: a baseline refresh adding
//! three fictional points, anchored on vertices of the ingest fixture's
//! terrain, to its tile.

mod common;

use std::path::Path;

use op_world::contrib::{Change, CheckReport, Decision, Op, Outcome, Review, Verdict};
use op_world::gate::{self, ANCHOR_TOLERANCE_MM, GATE_VERSION, TILE_MAX_POINTS};
use op_world::manifest::{AssetManifest, BoundsMm, Collider, Layer};
use op_world::merge::{baseline, merge};
use op_world::points::{PointKind, PointsDocument};
use op_world::sample_change::{
    PointsDraft, SAMPLE_POINTS, sample_points_draft, write_points_bundle,
};
use op_world::validate::validate_store;
use op_world::{ContentId, ObjectKind, Store, TileAsset, TileIndex, TileKey, cjson};
use serde_json::{Value, json};

// ------------------------------------------------------------- the reader --

/// The golden points document of `layer`, as a JSON value.
fn golden(layer: Layer) -> Value {
    let id = common::expected_points_ids()[&layer];
    let bytes = common::fixture_store()
        .read_object(&id, ObjectKind::Json)
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// What `PointsDocument::from_bytes` says about `doc` written canonically
/// (or `cjson`'s refusal to write it).
fn refusal(doc: &Value) -> String {
    match cjson::to_vec(doc) {
        Ok(bytes) => PointsDocument::from_bytes(&bytes)
            .expect_err("refused")
            .to_string(),
        Err(e) => e.to_string(),
    }
}

/// The mutations RFC 0004's "Tests and checks" names for the reader, and
/// more, each refused with op_world's message.
/// `tools/worldfmt/tests/test_points.py` expects the same messages.
#[test]
fn the_reader_refuses_what_rfc_0004_rules_out() {
    let lights = golden(Layer::StreetObjects);
    let trees = golden(Layer::Vegetation);
    type Mutation = fn(&mut Value);
    let cases: &[(&str, &Value, Mutation, &str)] = &[
        (
            "unsorted ids",
            &trees,
            |d| d["points"].as_array_mut().unwrap().reverse(),
            "invalid: points document: points must be sorted by id without duplicates",
        ),
        (
            "a duplicate id",
            &trees,
            |d| d["points"][1]["id"] = d["points"][0]["id"].clone(),
            "invalid: points document: points must be sorted by id without duplicates",
        ),
        (
            "two sources",
            &trees,
            |d| d["points"][1]["id"] = json!("streetwide:urban-trees:T-2"),
            "invalid: points document: every point must have the same source (its id without the last part), not \"streetwide:sample-trees\", \"streetwide:urban-trees\"",
        ),
        (
            "an id without a source",
            &trees,
            |d| d["points"][1]["id"] = json!("tree-2"),
            "invalid: points document: point \"tree-2\": id must be <source>:<local id>, at most 256 characters (RFC 0004 section 2)",
        ),
        (
            "a light's attributes on a tree",
            &trees,
            |d| {
                d["points"][0]["attributes"] =
                    golden(Layer::StreetObjects)["points"][1]["attributes"].clone()
            },
            "invalid: points document: point \"streetwide:sample-trees:T-1\": attributes must be those of a tree: crown_diameter_mm, genus, height_mm, leaf_cycle, leaf_type, planted_year, species, taxon, trunk_circumference_mm",
        ),
        (
            "an estimate without a rule",
            &lights,
            |d| d["rules"] = json!({}),
            "invalid: points document: point \"streetwide:sample-lights:L-1\": estimated \"yaw\" has no rule in rules.light",
        ),
        (
            "a rule no point uses",
            &lights,
            |d| d["points"][0]["estimated"] = json!([]),
            "invalid: points document: rules.light.yaw: no light point has an estimated yaw",
        ),
        (
            "an estimated null",
            &lights,
            |d| d["points"][0]["attributes"]["yaw"] = json!(null),
            "invalid: points document: point \"streetwide:sample-lights:L-1\": estimated \"yaw\" is null",
        ),
        (
            "an estimated mount",
            &lights,
            |d| d["points"][0]["estimated"] = json!(["mount", "yaw"]),
            "invalid: points document: point \"streetwide:sample-lights:L-1\": \"mount\" may not be estimated for a light",
        ),
        (
            "an estimated tree height",
            &trees,
            |d| d["points"][0]["estimated"] = json!(["height_mm"]),
            "invalid: points document: point \"streetwide:sample-trees:T-1\": \"height_mm\" may not be estimated for a tree",
        ),
        (
            "estimates out of order",
            &lights,
            |d| {
                d["points"][0]["estimated"] = json!(["yaw", "yaw"]);
            },
            "invalid: points document: point \"streetwide:sample-lights:L-1\": estimated must be sorted and unique",
        ),
        (
            "a rule id that is not one",
            &lights,
            |d| d["rules"]["light"]["yaw"] = json!("orientierung"),
            "invalid: points document: rules.light.yaw: \"orientierung\" is not a rule id (adr-<4 digits>:<name>:<version>)",
        ),
        (
            "a rule for a kind that does not exist",
            &lights,
            |d| d["rules"]["bench"] = json!({"yaw": "adr-0015:yaw:1"}),
            "invalid: points document: rules: \"bench\" is not a kind of point",
        ),
        (
            "an empty rule set",
            &trees,
            |d| d["rules"]["tree"] = json!({}),
            "invalid: points document: rules.tree must name at least one estimated attribute",
        ),
        (
            "a yaw of a whole turn",
            &lights,
            |d| d["points"][0]["attributes"]["yaw"] = json!(65_536),
            "invalid: points document: point \"streetwide:sample-lights:L-1\": yaw must be 0..=65535",
        ),
        (
            "a crown of 0 mm",
            &trees,
            |d| d["points"][0]["attributes"]["crown_diameter_mm"] = json!(0),
            "invalid: points document: point \"streetwide:sample-trees:T-1\": crown_diameter_mm must be 1..=100000",
        ),
        (
            "a lowercase genus",
            &trees,
            |d| d["points"][0]["attributes"]["genus"] = json!("platanus"),
            "invalid: points document: point \"streetwide:sample-trees:T-1\": genus must match ^[A-Z][a-z]+$ (at most 64 characters)",
        ),
        (
            "a one-word species",
            &trees,
            |d| d["points"][0]["attributes"]["species"] = json!("Platanus"),
            "invalid: points document: point \"streetwide:sample-trees:T-1\": species must be a Latin binomial of at most 128 characters",
        ),
        (
            "no points",
            &trees,
            |d| d["points"] = json!([]),
            "invalid: points document: a points document has 1..=65536 points",
        ),
        (
            "another format",
            &trees,
            |d| d["format"] = json!("streetwide.asset"),
            "invalid: points document: format must be \"streetwide.points\"",
        ),
        (
            "another contract version",
            &trees,
            |d| d["contract_version"] = json!("0.2.0"),
            "invalid: points document: unsupported contract_version \"0.2.0\"",
        ),
        (
            "an omitted null",
            &trees,
            |d| {
                d["points"][1]["attributes"]
                    .as_object_mut()
                    .unwrap()
                    .remove("taxon");
            },
            "not canonical JSON: document omits fields that must be written explicitly",
        ),
        (
            "a float",
            &trees,
            |d| d["points"][0]["attributes"]["height_mm"] = json!(18500.5),
            "invalid: number 18500.5 is not an integer; canonical JSON allows integers only",
        ),
    ];
    for (name, base, mutate, expected) in cases {
        let mut doc = (*base).clone();
        mutate(&mut doc);
        assert_eq!(refusal(&doc), *expected, "{name}");
    }
    // Not canonical bytes.
    let bytes = cjson::to_vec(&trees).unwrap();
    let mut spaced = bytes.clone();
    spaced.insert(1, b' ');
    assert!(
        PointsDocument::from_bytes(&spaced)
            .unwrap_err()
            .to_string()
            .starts_with("not canonical JSON")
    );
    // An unknown kind, or an attribute of no kind, does not parse.
    for edit in [
        |d: &mut Value| d["points"][0]["kind"] = json!("bench"),
        |d: &mut Value| d["points"][0]["attributes"]["colour"] = json!(null),
    ] {
        let mut doc = trees.clone();
        edit(&mut doc);
        assert!(
            refusal(&doc).starts_with("invalid JSON"),
            "{}",
            refusal(&doc)
        );
    }
}

/// The writer refuses what the reader would: `to_bytes` checks the rules.
#[test]
fn the_writer_refuses_invalid_documents() {
    let mut doc: PointsDocument = serde_json::from_value(golden(Layer::StreetObjects)).unwrap();
    assert!(doc.to_bytes().is_ok());
    doc.points.reverse();
    assert_eq!(
        doc.to_bytes().unwrap_err().to_string(),
        "invalid: points document: points must be sorted by id without duplicates"
    );
    doc.points.reverse();
    doc.points.clear();
    assert!(doc.to_bytes().is_err());
}

// ------------------------------------------------------ whole-world checks --

/// A point asset for [`world_with`].
struct PointAsset {
    tile: TileKey,
    layer: Layer,
    feature_id: String,
    doc: Vec<u8>,
    bounds: BoundsMm,
    size: u64,
    kind: ObjectKind,
}

impl PointAsset {
    /// `doc` as an asset in `layer` and `tile`, as op_world writes one.
    fn of(tile: &str, layer: Layer, doc: &PointsDocument) -> PointAsset {
        let bytes = cjson::to_canonical(doc).unwrap();
        PointAsset {
            tile: TileKey::parse(tile).unwrap(),
            layer,
            feature_id: format!("tile:{tile}/{}", doc.source().unwrap()),
            size: bytes.len() as u64,
            doc: bytes,
            bounds: doc.bounds().unwrap(),
            kind: ObjectKind::Json,
        }
    }
}

const SAMPLE_TILE: &str = "17/68646/45908";
/// A second tile of the sample's region, for ids across tiles.
const OTHER_TILE: &str = "17/68647/45908";

fn sample_doc(layer: Layer) -> PointsDocument {
    serde_json::from_value(golden(layer)).unwrap()
}

/// A copy of the format fixture whose point assets are `points` instead of
/// its own: their documents, manifests (as the fixture's, with these
/// fields), tile indexes and world.
fn world_with(name: &str, points: Vec<PointAsset>) -> common::TempDir {
    let dir = common::temp_dir(name);
    let src = common::format_dir().join("world");
    for f in common::files_under(&src) {
        let to = dir.join(&f);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(src.join(&f), to).unwrap();
    }
    let store = Store::new(dir.path());
    let (_, mut world) = store.read_world().unwrap();
    let index = TileIndex::from_bytes(
        &store
            .read_object(&world.tiles[0].tile_id, ObjectKind::Json)
            .unwrap(),
    )
    .unwrap();
    let template = AssetManifest::from_bytes(
        &store
            .read_object(
                &index
                    .assets
                    .iter()
                    .find(|a| a.layer == Layer::Vegetation)
                    .unwrap()
                    .manifest_id,
                ObjectKind::Json,
            )
            .unwrap(),
    )
    .unwrap();
    let mut indexes: std::collections::BTreeMap<TileKey, TileIndex> = Default::default();
    let mut sample = index.clone();
    sample.assets.retain(|a| !a.layer.has_points());
    indexes.insert(sample.tile, sample);
    for p in points {
        let content_id = store.write_object(&p.doc, p.kind).unwrap();
        let manifest = AssetManifest {
            content_id,
            size_bytes: p.size,
            layer: p.layer,
            feature_id: p.feature_id.clone(),
            tile: p.tile,
            bounds_mm: p.bounds,
            ..template.clone()
        };
        let manifest_id = store
            .write_object(&manifest.to_bytes().unwrap(), ObjectKind::Json)
            .unwrap();
        let index = indexes.entry(p.tile).or_insert_with(|| TileIndex {
            tile: p.tile,
            region: p.tile.region(),
            origin_mm: [index.origin_mm[0] + 300_000, 0, index.origin_mm[2]],
            assets: vec![],
            ..index.clone()
        });
        index.assets.push(TileAsset {
            layer: p.layer,
            feature_id: p.feature_id,
            manifest_id,
            content_id,
            license: template.provenance.license.clone(),
            collider: Collider::None,
            bounds_mm: p.bounds,
        });
    }
    world.tiles.clear();
    for (tile, index) in &mut indexes {
        index
            .assets
            .sort_by(|a, b| (a.layer, &a.feature_id).cmp(&(b.layer, &b.feature_id)));
        let tile_id = store
            .write_object(&index.to_bytes().unwrap(), ObjectKind::Json)
            .unwrap();
        world.tiles.push(op_world::world::WorldTile {
            tile: *tile,
            tile_id,
            origin_mm: index.origin_mm,
            bounds_mm: index.bounds().unwrap(),
        });
    }
    store.write_world(&world).unwrap();
    dir
}

fn errors_of(dir: &Path) -> Vec<String> {
    validate_store(&Store::new(dir)).errors
}

#[test]
fn validation_of_worlds_with_points() {
    let trees = sample_doc(Layer::Vegetation);
    let lights = sample_doc(Layer::StreetObjects);
    let both = || {
        vec![
            PointAsset::of(SAMPLE_TILE, Layer::Vegetation, &trees),
            PointAsset::of(SAMPLE_TILE, Layer::StreetObjects, &lights),
        ]
    };
    // The fixture as it is, rebuilt.
    let dir = world_with("points-valid", both());
    let report = validate_store(&Store::new(dir.path()));
    assert!(report.is_ok(), "{:#?}", report.errors);
    assert_eq!(report.points, 4);
    assert_eq!(report.world_id, Some(common::expected_world_id()));

    let one = |error: &str, assets: Vec<PointAsset>| {
        let dir = world_with("points-invalid", assets);
        let errors = errors_of(dir.path());
        assert!(
            errors.iter().any(|e| e.contains(error)),
            "expected {error:?} in {errors:#?}"
        );
    };
    // Trees in the lights' layer.
    let mut wrong = both();
    wrong[1] = PointAsset::of(SAMPLE_TILE, Layer::StreetObjects, &trees);
    one(
        "points of kind tree belong in the vegetation layer, not street_objects",
        wrong,
    );
    // A feature id naming another source.
    let mut other = both();
    other[0].feature_id = format!("tile:{SAMPLE_TILE}/osm:node");
    one(
        "the points' source must be \"osm:node\", the source of feature id \"tile:17/68646/45908/osm:node\"",
        other,
    );
    // Bounds that are not the anchors'.
    let mut wide = both();
    wide[0].bounds.max[1] += 1;
    one("bounds_mm are not the exact bounds of the anchors", wide);
    // A size that is not the document's.
    let mut size = both();
    size[1].size += 1;
    one("size_bytes does not match the points document", size);
    // A points document stored as a mesh is not found as a document.
    let mut stored_as_glb = trees.clone();
    stored_as_glb.points[0].anchor_mm[0] += 1;
    let mut glb = both();
    glb[0] = PointAsset::of(SAMPLE_TILE, Layer::Vegetation, &stored_as_glb);
    glb[0].kind = ObjectKind::Glb;
    one(
        "vegetation tile:17/68646/45908/streetwide:sample-trees: points document: I/O error",
        glb,
    );
    // A document that is not valid.
    let mut unsorted = both();
    let mut value = golden(Layer::Vegetation);
    value["points"].as_array_mut().unwrap().reverse();
    unsorted[0].doc = cjson::to_vec(&value).unwrap();
    unsorted[0].size = unsorted[0].doc.len() as u64;
    one("points must be sorted by id without duplicates", unsorted);
    // The same ids in two tiles of one layer.
    let mut twice = both();
    twice.push(PointAsset::of(OTHER_TILE, Layer::Vegetation, &trees));
    one(
        "another vegetation asset lists the same points; point ids are unique per layer",
        twice,
    );
    let mut moved = trees.clone();
    moved.points[1].anchor_mm[0] += 1;
    let mut overlap = both();
    overlap.push(PointAsset::of(OTHER_TILE, Layer::Vegetation, &moved));
    one(
        "point \"streetwide:sample-trees:T-1\" and 1 more already listed in the vegetation layer",
        overlap,
    );
    // Ids may repeat across layers, and other sources in another tile are
    // fine.
    let mut renamed = trees.clone();
    for p in &mut renamed.points {
        p.id = p.id.replace("sample-trees", "sample-trees-2");
    }
    let mut fine = both();
    fine.push(PointAsset::of(OTHER_TILE, Layer::Vegetation, &renamed));
    let dir = world_with("points-two-tiles", fine);
    let report = validate_store(&Store::new(dir.path()));
    assert!(report.is_ok(), "{:#?}", report.errors);
    assert_eq!((report.tiles, report.points), (2, 6));
}

// ---------------------------------------------------------------- the gate --

fn fixture_world() -> std::path::PathBuf {
    common::repo_root().join("fixtures/world")
}

/// Writes `draft` as a bundle and runs the gate on it against `world`.
fn gate_on(world: &Path, draft: &mut PointsDraft, name: &str) -> CheckReport {
    let bundle = common::temp_dir(&format!("points-bundle-{name}"));
    write_points_bundle(draft, &bundle).unwrap();
    let candidate = common::temp_path(&format!("points-candidate-{name}"), "candidate");
    gate::run(world, &bundle, &candidate).expect("the gate runs")
}

fn draft(layer: Layer) -> PointsDraft {
    sample_points_draft(&fixture_world(), layer).unwrap()
}

/// The draft's bounds after its anchors were edited.
fn rebound(d: &mut PointsDraft) {
    d.manifest.bounds_mm = d.doc.bounds().unwrap();
}

/// A contribution instead of a refresh.
fn contributed(d: &mut PointsDraft) {
    d.change.author = "github:alice".into();
}

fn check<'a>(report: &'a CheckReport, id: &str) -> &'a op_world::contrib::CheckResult {
    report.checks.iter().find(|c| c.id == id).expect("check")
}

fn outcome(report: &CheckReport, id: &str) -> Outcome {
    check(report, id).outcome
}

fn has_detail(report: &CheckReport, id: &str, text: &str) -> bool {
    check(report, id).details.iter().any(|d| d.contains(text))
}

#[test]
fn a_refresh_adding_points_is_reviewed_and_passes() {
    for layer in [Layer::Vegetation, Layer::StreetObjects] {
        let mut d = draft(layer);
        let r = gate_on(&fixture_world(), &mut d, layer.as_str());
        assert_eq!(r.gate_version, "0.1.4");
        assert_eq!(GATE_VERSION, "0.1.4");
        assert!(r.passed, "{r:#?}");
        let expect = |id, o| assert_eq!(outcome(&r, id), o, "{layer:?} {id}: {r:#?}");
        for id in ["G1", "G2", "G7", "G8"] {
            expect(id, Outcome::Pass);
        }
        expect("G3", Outcome::Review);
        for id in ["G4", "G5", "G6", "G9", "G10"] {
            expect(id, Outcome::NotApplicable);
        }
        // G3's review aid: the points of the asset and of the tile.
        let what = format!(
            "17/68646/45903 {} tile:17/68646/45903/{}",
            layer.as_str(),
            d.doc.source().unwrap()
        );
        let ids: Vec<&str> = d.doc.points.iter().map(|p| p.id.as_str()).collect();
        assert!(
            has_detail(
                &r,
                "G3",
                &format!(
                    "{what}: 3 points (was 0): 3 added, 0 removed, 0 moved, 0 changed; added: {}",
                    ids.join(", ")
                )
            ),
            "{r:#?}"
        );
        assert!(
            has_detail(
                &r,
                "G3",
                "tile 17/68646/45903: 3 points in vegetation and street_objects after the change set (was 0)"
            ),
            "{r:#?}"
        );
        assert_eq!(d.doc.points.len(), SAMPLE_POINTS);
    }
}

#[test]
fn g3_sends_point_fixes_to_the_source() {
    for layer in [Layer::Vegetation, Layer::StreetObjects] {
        let mut d = draft(layer);
        contributed(&mut d);
        let r = gate_on(&fixture_world(), &mut d, &format!("g3-{}", layer.as_str()));
        assert_eq!(outcome(&r, "G3"), Outcome::Fail, "{r:#?}");
        assert!(!r.passed);
        assert!(
            has_detail(
                &r,
                "G3",
                &format!(
                    "the {} layer does not accept contributions; fix it in the source (the open data it comes from), and the next baseline refresh brings the fix in",
                    layer.as_str()
                )
            ),
            "{r:#?}"
        );
        assert_eq!(outcome(&r, "G9"), Outcome::NotApplicable);
    }
}

#[test]
fn g1_reads_a_points_asset_against_its_manifest() {
    // Lights in the vegetation layer.
    let mut d = draft(Layer::StreetObjects);
    d.manifest.layer = Layer::Vegetation;
    d.change.changes[0].layer = Layer::Vegetation;
    let r = gate_on(&fixture_world(), &mut d, "g1-kind");
    assert_eq!(outcome(&r, "G1"), Outcome::Fail);
    assert!(
        has_detail(
            &r,
            "G1",
            "points of kind light belong in the street_objects layer, not vegetation"
        ),
        "{r:#?}"
    );
    // Bounds that are not the anchors'.
    let mut d = draft(Layer::Vegetation);
    d.manifest.bounds_mm.min[0] -= 1;
    let r = gate_on(&fixture_world(), &mut d, "g1-bounds");
    assert!(has_detail(
        &r,
        "G1",
        "bounds_mm are not the exact bounds of the anchors"
    ));
    // A trimesh collider: the manifest itself is refused.
    let mut d = draft(Layer::Vegetation);
    d.manifest.physics.collider = Collider::Trimesh;
    let bundle = common::temp_dir("points-bundle-g1-collider");
    let err = write_points_bundle(&mut d, &bundle).unwrap_err();
    assert!(
        err.to_string()
            .contains("vegetation assets must have collider none (visual only)"),
        "{err}"
    );
}

#[test]
fn g8_anchors_lie_on_the_terrain() {
    for (lift, refresh, contribution) in [
        (ANCHOR_TOLERANCE_MM, Outcome::Pass, Outcome::Pass),
        (-ANCHOR_TOLERANCE_MM, Outcome::Pass, Outcome::Pass),
        (ANCHOR_TOLERANCE_MM + 1, Outcome::Review, Outcome::Fail),
        (-2_500, Outcome::Review, Outcome::Fail),
    ] {
        for contribute in [false, true] {
            let mut d = draft(Layer::Vegetation);
            if contribute {
                contributed(&mut d);
            }
            for p in &mut d.doc.points {
                p.anchor_mm[1] += lift;
            }
            rebound(&mut d);
            let r = gate_on(&fixture_world(), &mut d, &format!("g8-{lift}-{contribute}"));
            let wanted = if contribute { contribution } else { refresh };
            assert_eq!(outcome(&r, "G8"), wanted, "{lift} mm: {r:#?}");
            if wanted != Outcome::Pass {
                assert!(
                    has_detail(
                        &r,
                        "G8",
                        &format!(
                            "3 of 3 anchors are more than 50 mm from the terrain surface (worst {} mm): ",
                            lift.abs()
                        )
                    ),
                    "{r:#?}"
                );
            }
        }
    }
}

#[test]
fn g8_fails_anchors_with_no_terrain_below() {
    let mut d = draft(Layer::Vegetation);
    // One anchor a kilometre east: outside the tile and its terrain.
    d.doc.points[2].anchor_mm[0] += 1_000_000;
    rebound(&mut d);
    let r = gate_on(&fixture_world(), &mut d, "g8-nowhere");
    assert_eq!(outcome(&r, "G8"), Outcome::Fail, "{r:#?}");
    assert!(has_detail(
        &r,
        "G8",
        &format!(
            "1 of 3 anchors have no terrain below; a point lies in its tile, on its terrain: {}",
            d.doc.points[2].id
        )
    ));
    assert!(!r.passed, "a refresh fails too");
}

/// The fixture tile's terrain: its manifest and manifest id.
fn fixture_terrain(world: &Path) -> (ContentId, AssetManifest) {
    let store = Store::new(world);
    let (_, w) = store.read_world().unwrap();
    let index = TileIndex::from_bytes(
        &store
            .read_object(&w.tiles[0].tile_id, ObjectKind::Json)
            .unwrap(),
    )
    .unwrap();
    let t = index
        .assets
        .iter()
        .find(|a| a.layer == Layer::Terrain)
        .unwrap();
    let m =
        AssetManifest::from_bytes(&store.read_object(&t.manifest_id, ObjectKind::Json).unwrap())
            .unwrap();
    (t.manifest_id, m)
}

/// Adds to the draft's bundle a replacement of the tile's terrain with the
/// same mesh, licensed as derived from OpenStreetMap.
fn with_odbl_terrain(world: &Path, d: &mut PointsDraft, bundle: &Path) {
    let (parent, mut m) = fixture_terrain(world);
    m.parent_version_id = Some(parent);
    m.provenance.license = "ODbL-1.0 AND LicenseRef-swisstopo-OGD".into();
    let id = Store::new(bundle)
        .write_object(&m.to_bytes().unwrap(), ObjectKind::Json)
        .unwrap();
    d.change.changes.push(Change {
        op: Op::Replace,
        tile: m.tile,
        layer: Layer::Terrain,
        feature_id: m.feature_id.clone(),
        parent_version_id: Some(parent),
        manifest_id: Some(id),
    });
    d.change
        .changes
        .sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
}

#[test]
fn g8_checks_anchors_against_non_osm_ground() {
    let mut d = draft(Layer::StreetObjects);
    let bundle = common::temp_dir("points-bundle-g8-odbl");
    write_points_bundle(&mut d, &bundle).unwrap();
    with_odbl_terrain(&fixture_world(), &mut d, &bundle);
    std::fs::write(bundle.join("change.json"), d.change.to_bytes().unwrap()).unwrap();
    let candidate = common::temp_path("points-candidate-g8-odbl", "candidate");
    let r = gate::run(&fixture_world(), &bundle, &candidate).unwrap();
    assert_eq!(outcome(&r, "G1"), Outcome::Pass, "{r:#?}");
    assert_eq!(outcome(&r, "G8"), Outcome::Fail, "{r:#?}");
    assert!(
        has_detail(
            &r,
            "G8",
            "3 of 3 anchors lie over terrain licensed ODbL-1.0; anchors are checked against non-OSM ground"
        ),
        "{r:#?}"
    );
}

#[test]
fn g7_counts_points_per_tile() {
    for contribute in [false, true] {
        let mut d = draft(Layer::Vegetation);
        if contribute {
            contributed(&mut d);
        }
        let first = d.doc.points[0].clone();
        d.doc.points = (0..=TILE_MAX_POINTS)
            .map(|k| op_world::points::Point {
                id: format!("streetwide:sample-trees:P-{k:05}"),
                ..first.clone()
            })
            .collect();
        rebound(&mut d);
        let r = gate_on(&fixture_world(), &mut d, &format!("g7-{contribute}"));
        let wanted = if contribute {
            Outcome::Fail
        } else {
            Outcome::Review
        };
        assert_eq!(outcome(&r, "G7"), wanted, "{r:#?}");
        assert!(has_detail(
            &r,
            "G7",
            "tile 17/68646/45903: 5001 points in vegetation and street_objects, at most 5000"
        ));
        assert_eq!(outcome(&r, "G8"), Outcome::Pass);
    }
}

fn review() -> Review {
    Review {
        reviewer: "github:reviewer".into(),
        verdict: Verdict::Better,
        notes: "Checked the points (test).".into(),
    }
}

/// A private copy of the ingest fixture with the sample trees merged in,
/// by `op_world::merge`, which copies the points document into the store.
/// Returns the directory and the merge record's bytes.
fn world_with_trees(name: &str) -> (common::TempPath, Vec<u8>, PointsDraft) {
    world_with_many_trees(name, SAMPLE_POINTS)
}

/// [`world_with_trees`] with `count` trees: copies of the first at its
/// anchor beyond the sample's.
fn world_with_many_trees(name: &str, count: usize) -> (common::TempPath, Vec<u8>, PointsDraft) {
    let world = common::temp_path(name, "world");
    let src = fixture_world();
    for f in common::files_under(&src) {
        let to = world.join(&f);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(src.join(&f), to).unwrap();
    }
    let base = baseline(&world, "The ingest fixture.")
        .unwrap()
        .to_bytes()
        .unwrap();
    let mut d = draft(Layer::Vegetation);
    let first = d.doc.points[0].clone();
    for k in SAMPLE_POINTS..count {
        d.doc.points.push(op_world::points::Point {
            id: format!("streetwide:sample-trees:Q-{k:05}"),
            ..first.clone()
        });
    }
    let bundle = common::temp_dir(&format!("{name}-bundle"));
    write_points_bundle(&mut d, &bundle).unwrap();
    let scratch = common::temp_path(&format!("{name}-scratch"), "scratch");
    let merged = merge(
        &world,
        &bundle,
        &base,
        Decision::Merged,
        vec![review()],
        "Sample trees (test).",
        &scratch,
    )
    .expect("the refresh merges");
    (world, merged.record_bytes, d)
}

#[test]
fn a_merge_copies_the_points_document() {
    let (world, _, d) = world_with_trees("points-merge");
    let report = validate_store(&Store::new(&world));
    assert!(report.is_ok(), "{:#?}", report.errors);
    assert_eq!(report.points, SAMPLE_POINTS);
    let id = d.doc.id().unwrap();
    let store = Store::new(&world);
    assert!(store.has_object(&id, ObjectKind::Json));
    assert!(!store.has_object(&id, ObjectKind::Glb));
}

/// A tile whose points were accepted over the budget, by review, does not
/// fail a contribution to another of its layers.
#[test]
fn g7_counts_points_only_where_they_change() {
    let (world, _, _) = world_with_many_trees("points-over-budget", TILE_MAX_POINTS + 1);
    assert!(validate_store(&Store::new(&world)).is_ok());
    let candidate = common::temp_path("points-over-budget-facade", "candidate");
    let r = gate::run(
        &world,
        &common::repo_root().join("fixtures/changes/facade-sample"),
        &candidate,
    )
    .unwrap();
    assert_eq!(outcome(&r, "G7"), Outcome::Pass, "{r:#?}");
    assert!(r.passed, "{r:#?}");
}

#[test]
fn g3_lists_the_points_a_refresh_changes() {
    let (world, _, mut d) = world_with_trees("points-review");
    let manifest_id = d.change.changes[0].manifest_id.unwrap();
    // Drop P-0, move P-1, change P-2, add P-3.
    d.doc.points.remove(0);
    d.doc.points[0].anchor_mm[1] += 10;
    if let op_world::points::Attributes::Tree(t) = &mut d.doc.points[1].attributes {
        t.planted_year = Some(1990);
    }
    let mut added = d.doc.points[1].clone();
    added.id = "streetwide:sample-trees:P-3".into();
    d.doc.points.push(added);
    rebound(&mut d);
    d.change.base_world_id = Store::new(&world).read_world().unwrap().0;
    d.change.changes[0].op = Op::Replace;
    d.change.changes[0].parent_version_id = Some(manifest_id);
    d.manifest.parent_version_id = Some(manifest_id);
    let r = gate_on(&world, &mut d, "review");
    assert!(r.passed, "{r:#?}");
    assert!(
        has_detail(
            &r,
            "G3",
            "17/68646/45903 vegetation tile:17/68646/45903/streetwide:sample-trees: 3 points (was 3): 1 added, 1 removed, 1 moved, 1 changed; added: streetwide:sample-trees:P-3; removed: streetwide:sample-trees:P-0; moved: streetwide:sample-trees:P-1; changed: streetwide:sample-trees:P-2"
        ),
        "{r:#?}"
    );
    assert!(has_detail(
        &r,
        "G3",
        "tile 17/68646/45903: 3 points in vegetation and street_objects after the change set (was 3)"
    ));
    assert_eq!(
        outcome(&r, "G8"),
        Outcome::Pass,
        "a 10 mm move stays on the ground"
    );
}

#[test]
fn g8_lists_the_anchors_a_terrain_change_strands() {
    let (world, _, d) = world_with_trees("points-strand");
    let ids: Vec<&str> = d.doc.points.iter().map(|p| p.id.as_str()).collect();
    let base_world_id = Store::new(&world).read_world().unwrap().0;
    let (parent, terrain) = fixture_terrain(&world);
    let mut change = d.change.clone();
    change.base_world_id = base_world_id;
    change.changes = vec![];
    let run = |change: &mut op_world::contrib::ChangeSet, bundle: &Path, name: &str| {
        std::fs::write(bundle.join("change.json"), change.to_bytes().unwrap()).unwrap();
        let candidate = common::temp_path(&format!("points-candidate-{name}"), "candidate");
        gate::run(&world, bundle, &candidate).unwrap()
    };
    // A terrain licensed as derived from OpenStreetMap: every anchor is
    // stranded, for review.
    let bundle = common::temp_dir("points-bundle-strand-odbl");
    let mut m = terrain.clone();
    m.parent_version_id = Some(parent);
    m.provenance.license = "ODbL-1.0 AND LicenseRef-swisstopo-OGD".into();
    let id = Store::new(&bundle)
        .write_object(&m.to_bytes().unwrap(), ObjectKind::Json)
        .unwrap();
    change.changes = vec![Change {
        op: Op::Replace,
        tile: m.tile,
        layer: Layer::Terrain,
        feature_id: m.feature_id.clone(),
        parent_version_id: Some(parent),
        manifest_id: Some(id),
    }];
    let r = run(&mut change, &bundle, "strand-odbl");
    assert_eq!(outcome(&r, "G8"), Outcome::Review, "{r:#?}");
    assert!(
        has_detail(
            &r,
            "G8",
            &format!(
                "17/68646/45903 terrain tile:17/68646/45903: the terrain strands 3 of the 3 anchors of vegetation tile:17/68646/45903/streetwide:sample-trees (no terrain below, over terrain licensed ODbL-1.0, or more than 50 mm off): {}",
                ids.join(", ")
            )
        ),
        "{r:#?}"
    );
    // The terrain removed: no terrain below any anchor.
    let bundle = common::temp_dir("points-bundle-strand-removed");
    change.changes = vec![Change {
        op: Op::Remove,
        tile: terrain.tile,
        layer: Layer::Terrain,
        feature_id: terrain.feature_id.clone(),
        parent_version_id: Some(parent),
        manifest_id: None,
    }];
    let r = run(&mut change, &bundle, "strand-removed");
    assert_eq!(outcome(&r, "G8"), Outcome::Review, "{r:#?}");
    assert!(has_detail(
        &r,
        "G8",
        "the terrain strands 3 of the 3 anchors"
    ));
}

#[test]
fn the_kinds_belong_to_their_layers() {
    assert_eq!(PointKind::Tree.layer(), Layer::Vegetation);
    assert_eq!(PointKind::Light.layer(), Layer::StreetObjects);
}
