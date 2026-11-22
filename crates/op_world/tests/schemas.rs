//! The JSON Schemas in `schemas/` and the Rust types must agree: every
//! fixture document validates against its schema, and every mutation below
//! is rejected by both the schema and `op_world`.

mod common;

use jsonschema::{Registry, Validator};
use op_world::{AssetManifest, TileIndex, WorldManifest, cjson};
use serde_json::{Value, json};

const SCHEMAS: [(&str, &str); 4] = [
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
}

impl Kind {
    fn schema(self) -> &'static str {
        match self {
            Kind::Asset => "asset-manifest.schema.json",
            Kind::Tile => "tile-index.schema.json",
            Kind::World => "world-manifest.schema.json",
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
    assert_eq!(docs.len(), 4, "2 manifests, 1 tile index, 1 world");
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

    for (name, kind, doc) in [
        ("tile outside its region", Kind::Tile, wrong_region),
        ("assets not sorted", Kind::Tile, unsorted),
        ("attribution not sorted", Kind::World, unsorted_attr),
    ] {
        assert!(
            validator(kind.schema()).is_valid(&doc),
            "{name}: schema should accept"
        );
        assert!(!kind.rust_accepts(&doc), "{name}: op_world should reject");
    }
}
