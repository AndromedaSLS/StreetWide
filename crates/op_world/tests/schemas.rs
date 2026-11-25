//! The JSON Schemas in `schemas/` and the Rust types must agree: every
//! fixture document validates against its schema, and every mutation below
//! is rejected by both the schema and `op_world`.

mod common;

use jsonschema::{Registry, Validator};
use op_world::{AssetManifest, Layer, PointsDocument, TileIndex, WorldManifest, cjson};
use serde_json::{Value, json};

const SCHEMAS: [(&str, &str); 5] = [
    ("urn:streetwide:schema:common:0.1", "common.schema.json"),
    (
        "urn:streetwide:schema:asset-manifest:0.1",
        "asset-manifest.schema.json",
    ),
    (
        "urn:streetwide:schema:tile-index:0.1",
        "tile-index.schema.json",
    ),
    (
        "urn:streetwide:schema:world-manifest:0.1",
        "world-manifest.schema.json",
    ),
    ("urn:streetwide:schema:points:0.1", "points.schema.json"),
];

fn load(file: &str) -> Value {
    let path = common::repo_root().join("schemas").join(file);
    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap()
}

/// Validator for one document type, with all schemas registered.
fn validator(file: &str) -> Validator {
    let mut registry = Registry::new();
    for (uri, f) in SCHEMAS {
        let schema = load(f);
        assert_eq!(schema["$id"], uri, "{f} has an unexpected $id");
        registry = registry.add(uri, schema).expect("valid URI");
    }
    let registry = registry.prepare().expect("registry prepares");
    jsonschema::options()
        .with_registry(&registry)
        .build(&load(file))
        .expect("schema compiles")
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Asset,
    Tile,
    World,
    Points,
}

impl Kind {
    fn schema(self) -> &'static str {
        match self {
            Kind::Asset => "asset-manifest.schema.json",
            Kind::Tile => "tile-index.schema.json",
            Kind::World => "world-manifest.schema.json",
            Kind::Points => "points.schema.json",
        }
    }

    /// Does `op_world` accept this value as a document of this kind?
    fn rust_accepts(self, value: &Value) -> bool {
        let Ok(bytes) = cjson::to_vec(value) else {
            return false;
        };
        match self {
            Kind::Asset => AssetManifest::from_bytes(&bytes).is_ok(),
            Kind::Tile => TileIndex::from_bytes(&bytes).is_ok(),
            Kind::World => WorldManifest::from_bytes(&bytes).is_ok(),
            Kind::Points => PointsDocument::from_bytes(&bytes).is_ok(),
        }
    }
}

/// The fixture documents, keyed by kind.
fn fixture_docs() -> Vec<(Kind, Value)> {
    let dir = common::format_dir().join("world");
    let mut docs = Vec::new();
    for f in common::files_under(&dir) {
        if !f.ends_with(".json") {
            continue;
        }
        let v: Value = serde_json::from_slice(&std::fs::read(dir.join(&f)).unwrap()).unwrap();
        let kind = match v["format"].as_str() {
            Some("streetwide.asset") => Kind::Asset,
            Some("streetwide.tile") => Kind::Tile,
            Some("streetwide.world") => Kind::World,
            Some("streetwide.points") => Kind::Points,
            other => panic!("{f}: unexpected format {other:?}"),
        };
        docs.push((kind, v));
    }
    docs
}

fn first(kind_wanted: fn(Kind) -> bool) -> Value {
    fixture_docs()
        .into_iter()
        .find(|(k, _)| kind_wanted(*k))
        .map(|(_, v)| v)
        .unwrap()
}

#[test]
fn fixture_documents_match_their_schemas() {
    let docs = fixture_docs();
    assert_eq!(
        docs.len(),
        8,
        "4 manifests, 1 tile index, 1 world, 2 points documents"
    );
    for (kind, doc) in docs {
        let v = validator(kind.schema());
        let errors: Vec<String> = v.iter_errors(&doc).map(|e| e.to_string()).collect();
        assert!(errors.is_empty(), "{kind:?}: {errors:#?}");
        assert!(kind.rust_accepts(&doc), "{kind:?} rejected by op_world");
    }
}

type Mutation = (&'static str, fn(&mut Value));

#[test]
fn schema_and_rust_reject_the_same_mutations() {
    let asset_mutations: &[Mutation] = &[
        ("non-fictional interior", |m| {
            m["layer"] = json!("interior");
            m["fictional"] = json!(false);
        }),
        ("zero size", |m| m["size_bytes"] = json!(0)),
        ("bad content id", |m| m["content_id"] = json!("blake3:XYZ")),
        ("unknown field", |m| m["colour"] = json!("red")),
        ("missing parent_version_id", |m| {
            m.as_object_mut().unwrap().remove("parent_version_id");
        }),
        ("no sources", |m| m["provenance"]["sources"] = json!([])),
        ("http source url", |m| {
            m["provenance"]["sources"][0]["url"] = json!("http://example.org")
        }),
        ("bad feature id", |m| m["feature_id"] = json!("My House")),
        ("wrong tile zoom", |m| m["tile"] = json!("16/1/1")),
        ("bad licence", |m| {
            m["provenance"]["license"] = json!("CC BY ©")
        }),
        ("empty attribution", |m| {
            m["provenance"]["attribution"] = json!([])
        }),
        ("unknown collider", |m| {
            m["physics"]["collider"] = json!("mesh")
        }),
        ("short bounds", |m| m["bounds_mm"]["min"] = json!([1, 2])),
        ("unknown layer", |m| m["layer"] = json!("roof")),
        ("float size", |m| m["size_bytes"] = json!(2084.5)),
        ("wrong contract version", |m| {
            m["contract_version"] = json!("0.2.0")
        }),
    ];
    let tile_mutations: &[Mutation] = &[
        ("no assets", |t| t["assets"] = json!([])),
        ("bad region key", |t| t["region"] = json!("13/1/1")),
        ("asset without licence", |t| {
            t["assets"][0].as_object_mut().unwrap().remove("license");
        }),
        ("unknown layer", |t| {
            t["assets"][1]["layer"] = json!("river")
        }),
    ];
    let world_mutations: &[Mutation] = &[
        ("yaw out of range", |w| w["spawn"]["yaw"] = json!(70000)),
        ("name not a slug", |w| w["name"] = json!("Format Sample")),
        ("latitude beyond Web Mercator", |w| {
            w["region"]["anchor_e9"]["lat"] = json!(90_000_000_000_i64)
        }),
        ("tile id not a string", |w| {
            w["tiles"][0]["tile_id"] = json!(5)
        }),
        ("no tiles", |w| w["tiles"] = json!([])),
    ];
    // RFC 0004: what the schema and op_world both refuse in a points
    // document (the base is the light document, with an estimated yaw).
    let points_mutations: &[Mutation] = &[
        ("no points", |d| d["points"] = json!([])),
        ("unknown kind", |d| d["points"][0]["kind"] = json!("bench")),
        ("tree attributes on a light", |d| {
            d["points"][0]["kind"] = json!("tree")
        }),
        ("an omitted null", |d| {
            d["points"][1]["attributes"]
                .as_object_mut()
                .unwrap()
                .remove("mount");
        }),
        ("an unknown attribute", |d| {
            d["points"][1]["attributes"]["colour"] = json!(null)
        }),
        ("yaw out of range", |d| {
            d["points"][0]["attributes"]["yaw"] = json!(65536)
        }),
        ("negative yaw", |d| {
            d["points"][0]["attributes"]["yaw"] = json!(-1)
        }),
        ("count zero", |d| {
            d["points"][0]["attributes"]["count"] = json!(0)
        }),
        ("count over 64", |d| {
            d["points"][0]["attributes"]["count"] = json!(65)
        }),
        ("light height zero", |d| {
            d["points"][0]["attributes"]["light_height_mm"] = json!(0)
        }),
        ("unknown method", |d| {
            d["points"][0]["attributes"]["method"] = json!("plasma")
        }),
        ("unknown mount", |d| {
            d["points"][0]["attributes"]["mount"] = json!("tree")
        }),
        ("unknown purpose", |d| {
            d["points"][0]["attributes"]["purpose"] = json!("traffic")
        }),
        ("a float", |d| {
            d["points"][0]["attributes"]["light_height_mm"] = json!(5500.5)
        }),
        ("a short anchor", |d| {
            d["points"][0]["anchor_mm"] = json!([1, 2])
        }),
        ("an id without a source", |d| {
            d["points"][0]["id"] = json!("LEU105227")
        }),
        ("a space in the local id", |d| {
            d["points"][0]["id"] = json!("streetwide:sample-lights:L 1")
        }),
        ("an uppercase source", |d| {
            d["points"][0]["id"] = json!("Streetwide:sample-lights:L-1")
        }),
        ("height estimated", |d| {
            d["points"][0]["estimated"] = json!(["light_height_mm", "yaw"])
        }),
        ("mount estimated", |d| {
            d["points"][0]["estimated"] = json!(["mount", "yaw"])
        }),
        ("a rule for a tree", |d| {
            d["rules"]["tree"] = json!({"height_mm": "adr-0016:height:1"})
        }),
        ("a rule for a kind that does not exist", |d| {
            d["rules"]["bench"] = json!({"yaw": "adr-0015:yaw:1"})
        }),
        ("an empty rule set for a kind", |d| {
            d["rules"]["light"] = json!({})
        }),
        ("a rule id that is not one", |d| {
            d["rules"]["light"]["yaw"] = json!("orientierung")
        }),
        ("an unknown top-level key", |d| d["source"] = json!("osm")),
        ("wrong format", |d| d["format"] = json!("streetwide.asset")),
        ("wrong contract version", |d| {
            d["contract_version"] = json!("0.2.0")
        }),
    ];
    // The same for trees (the base is the tree document).
    let tree_mutations: &[Mutation] = &[
        ("crown zero", |d| {
            d["points"][0]["attributes"]["crown_diameter_mm"] = json!(0)
        }),
        ("crown over 100 m", |d| {
            d["points"][0]["attributes"]["crown_diameter_mm"] = json!(100_001)
        }),
        ("tree over 150 m", |d| {
            d["points"][0]["attributes"]["height_mm"] = json!(150_001)
        }),
        ("planted in 999", |d| {
            d["points"][0]["attributes"]["planted_year"] = json!(999)
        }),
        ("planted in 3000", |d| {
            d["points"][0]["attributes"]["planted_year"] = json!(3000)
        }),
        ("trunk over 50 m", |d| {
            d["points"][0]["attributes"]["trunk_circumference_mm"] = json!(50_001)
        }),
        ("lowercase genus", |d| {
            d["points"][0]["attributes"]["genus"] = json!("platanus")
        }),
        ("a genus of 65 characters", |d| {
            d["points"][0]["attributes"]["genus"] = json!(format!("A{}", "a".repeat(64)))
        }),
        ("a one-word species", |d| {
            d["points"][0]["attributes"]["species"] = json!("Platanus")
        }),
        ("a species with two spaces", |d| {
            d["points"][0]["attributes"]["species"] = json!("Platanus  hispanica")
        }),
        ("a species with a final space", |d| {
            d["points"][0]["attributes"]["species"] = json!("Platanus hispanica ")
        }),
        ("a species of 129 characters", |d| {
            d["points"][0]["attributes"]["species"] = json!(format!("Platanus {}", "x".repeat(120)))
        }),
        ("an empty taxon", |d| {
            d["points"][0]["attributes"]["taxon"] = json!("")
        }),
        ("a taxon of 257 characters", |d| {
            d["points"][0]["attributes"]["taxon"] = json!("x".repeat(257))
        }),
        ("a control character in the taxon", |d| {
            d["points"][0]["attributes"]["taxon"] = json!("Tilia\u{7}")
        }),
        ("unknown leaf cycle", |d| {
            d["points"][0]["attributes"]["leaf_cycle"] = json!("annual")
        }),
        ("unknown leaf type", |d| {
            d["points"][0]["attributes"]["leaf_type"] = json!("scale")
        }),
        ("a tree's yaw", |d| {
            d["points"][0]["attributes"]["yaw"] = json!(0)
        }),
        ("anything estimated on a tree", |d| {
            d["points"][0]["estimated"] = json!(["height_mm"])
        }),
        ("light attributes on a tree", |d| {
            d["points"][1]["kind"] = json!("light")
        }),
    ];

    for (kind, mutations, base) in [
        (
            Kind::Asset,
            asset_mutations,
            first(|k| matches!(k, Kind::Asset)),
        ),
        (
            Kind::Tile,
            tile_mutations,
            first(|k| matches!(k, Kind::Tile)),
        ),
        (
            Kind::World,
            world_mutations,
            first(|k| matches!(k, Kind::World)),
        ),
        (Kind::Points, points_mutations, points_doc("light")),
        (Kind::Points, tree_mutations, points_doc("tree")),
    ] {
        let v = validator(kind.schema());
        for (name, mutate) in mutations {
            let mut doc = base.clone();
            mutate(&mut doc);
            assert!(!v.is_valid(&doc), "{kind:?} schema accepts: {name}");
            assert!(
                !kind.rust_accepts(&doc),
                "op_world accepts {kind:?}: {name}"
            );
        }
    }
}

/// The fixture's points document whose points are of `kind`.
fn points_doc(kind: &str) -> Value {
    fixture_docs()
        .into_iter()
        .map(|(_, v)| v)
        .find(|v| v["format"] == "streetwide.points" && v["points"][0]["kind"] == kind)
        .expect("a points document")
}

/// The fixture's manifest of the asset in `layer`.
fn manifest_in(layer: &str) -> Value {
    fixture_docs()
        .into_iter()
        .map(|(_, v)| v)
        .find(|v| v["format"] == "streetwide.asset" && v["layer"] == layer)
        .expect("a manifest")
}

/// The schemas' layer list is `Layer::ALL`.
#[test]
fn schema_layers_are_the_rust_layers() {
    let common = load("common.schema.json");
    let mut names: Vec<&str> = common["$defs"]["layer"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    names.sort_unstable();
    let rust: Vec<&str> = Layer::ALL.iter().map(|l| l.as_str()).collect();
    assert_eq!(names, rust);
}

/// Water is visual only (RFC 0003): both the schemas and `op_world` accept
/// a water asset with `collider: none` and reject one with a trimesh, in
/// manifests and in tile indexes.
#[test]
fn water_assets_never_collide() {
    let mut asset = first(|k| matches!(k, Kind::Asset));
    asset["layer"] = json!("water");
    let mut tile = first(|k| matches!(k, Kind::Tile));
    // The last entry, so that the entries stay sorted.
    let last = tile["assets"].as_array().unwrap().len() - 1;
    tile["assets"][last]["layer"] = json!("water");
    for collider in ["none", "trimesh"] {
        asset["physics"]["collider"] = json!(collider);
        tile["assets"][last]["collider"] = json!(collider);
        let ok = collider == "none";
        for (kind, doc) in [(Kind::Asset, &asset), (Kind::Tile, &tile)] {
            let errors: Vec<String> = validator(kind.schema())
                .iter_errors(doc)
                .map(|e| e.to_string())
                .collect();
            assert_eq!(
                errors.is_empty(),
                ok,
                "{kind:?} schema, {collider}: {errors:#?}"
            );
            assert_eq!(kind.rust_accepts(doc), ok, "{kind:?} op_world, {collider}");
        }
        if !ok {
            // op_world says why.
            let manifest = AssetManifest::from_bytes(&cjson::to_vec(&asset).unwrap()).unwrap_err();
            let index = TileIndex::from_bytes(&cjson::to_vec(&tile).unwrap()).unwrap_err();
            for e in [manifest, index] {
                assert!(e.to_string().contains("must have collider none"), "{e}");
            }
        }
    }
}

/// The point layers are visual only, with per-tile feature ids that carry
/// the source (RFC 0004 sections 2 and 7), in manifests and in tile
/// indexes, in the schemas and in `op_world`.
#[test]
fn point_assets_never_collide_and_name_their_source() {
    let tile = first(|k| matches!(k, Kind::Tile));
    let entry = |layer: &str| {
        tile["assets"]
            .as_array()
            .unwrap()
            .iter()
            .position(|a| a["layer"] == layer)
            .unwrap()
    };
    for layer in ["street_objects", "vegetation"] {
        let manifest = manifest_in(layer);
        let k = entry(layer);
        let fid = manifest["feature_id"].clone();
        type Edit = fn(&mut Value, &str);
        let edits: &[(&str, Edit, bool)] = &[
            ("as committed", |_, _| {}, true),
            ("a trimesh", |c, _| c["collider"] = json!("trimesh"), false),
            (
                "no source",
                |c, _| c["feature_id"] = json!("tile:17/68646/45908"),
                false,
            ),
            (
                "an empty source",
                |c, _| c["feature_id"] = json!("tile:17/68646/45908/"),
                false,
            ),
            (
                "not a per-tile id",
                |c, _| c["feature_id"] = json!("osm:node:1"),
                false,
            ),
            (
                "an uppercase source",
                |c, _| c["feature_id"] = json!("tile:17/68646/45908/OSM:node"),
                false,
            ),
            (
                "another source",
                |c, _| c["feature_id"] = json!("tile:17/68646/45908/osm:node"),
                true,
            ),
        ];
        for (name, edit, ok) in edits {
            let mut m = manifest.clone();
            let mut t = tile.clone();
            let mut fields = json!({"collider": "none", "feature_id": fid});
            edit(&mut fields, layer);
            m["physics"]["collider"] = fields["collider"].clone();
            m["feature_id"] = fields["feature_id"].clone();
            t["assets"][k]["collider"] = fields["collider"].clone();
            t["assets"][k]["feature_id"] = fields["feature_id"].clone();
            for (kind, doc) in [(Kind::Asset, &m), (Kind::Tile, &t)] {
                let errors: Vec<String> = validator(kind.schema())
                    .iter_errors(doc)
                    .map(|e| e.to_string())
                    .collect();
                assert_eq!(
                    errors.is_empty(),
                    *ok,
                    "{layer}, {name}, {kind:?} schema: {errors:#?}"
                );
                // The tile index's entries stay sorted only for some ids.
                if matches!(kind, Kind::Asset) || *ok {
                    assert_eq!(
                        kind.rust_accepts(doc),
                        *ok,
                        "{layer}, {name}, {kind:?} op_world"
                    );
                }
            }
        }
        // Another tile's id fits the schema's pattern; op_world compares it
        // with the tile.
        let mut m = manifest.clone();
        m["feature_id"] = json!(format!(
            "tile:17/1/2/{}",
            fid.as_str().unwrap().rsplit('/').next().unwrap()
        ));
        assert!(validator(Kind::Asset.schema()).is_valid(&m));
        let err = AssetManifest::from_bytes(&cjson::to_vec(&m).unwrap()).unwrap_err();
        assert!(
            err.to_string()
                .contains("must be tile:17/68646/45908/<source> (RFC 0004)"),
            "{err}"
        );
    }
}

/// Rules JSON Schema cannot express; only `op_world` enforces them.
#[test]
fn rust_only_rules() {
    let tile = first(|k| matches!(k, Kind::Tile));
    let mut wrong_region = tile.clone();
    wrong_region["region"] = json!("12/1/1");
    let mut unsorted = tile.clone();
    unsorted["assets"].as_array_mut().unwrap().reverse();

    let world = first(|k| matches!(k, Kind::World));
    let mut unsorted_attr = world.clone();
    unsorted_attr["attribution"] = json!(["b", "a"]);

    // RFC 0004.
    let lights = points_doc("light");
    let mut unsorted_points = lights.clone();
    unsorted_points["points"].as_array_mut().unwrap().reverse();
    let mut duplicate_points = lights.clone();
    duplicate_points["points"][1] = duplicate_points["points"][0].clone();
    let mut two_sources = lights.clone();
    two_sources["points"][1]["id"] = json!("streetwide:urban-lights:L-2");
    let mut estimate_without_rule = lights.clone();
    estimate_without_rule["rules"] = json!({});
    let mut rule_without_estimate = lights.clone();
    rule_without_estimate["points"][0]["estimated"] = json!([]);
    let mut estimated_null = lights.clone();
    estimated_null["points"][0]["attributes"]["yaw"] = json!(null);

    for (name, kind, doc) in [
        ("tile outside its region", Kind::Tile, wrong_region),
        ("assets not sorted", Kind::Tile, unsorted),
        ("attribution not sorted", Kind::World, unsorted_attr),
        ("points not sorted", Kind::Points, unsorted_points),
        ("points with the same id", Kind::Points, duplicate_points),
        ("points of two sources", Kind::Points, two_sources),
        (
            "an estimate without a rule",
            Kind::Points,
            estimate_without_rule,
        ),
        ("a rule no point uses", Kind::Points, rule_without_estimate),
        ("an estimated null", Kind::Points, estimated_null),
    ] {
        assert!(
            validator(kind.schema()).is_valid(&doc),
            "{name}: schema should accept"
        );
        assert!(!kind.rust_accepts(&doc), "{name}: op_world should reject");
    }
}
