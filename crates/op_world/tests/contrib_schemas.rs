//! The contribution schemas (`schemas/change-set`, `check-report`,
//! `merge-record`) and the Rust types must agree: committed documents
//! validate, and every mutation below is rejected by both.

mod common;

use jsonschema::{Registry, Validator};
use op_world::ContentId;
use op_world::cjson;
use op_world::contrib::{ChangeSet, CheckReport, Decision, MergeRecord, Review, Verdict};
use serde_json::{Value, json};

fn load(file: &str) -> Value {
    serde_json::from_slice(&std::fs::read(common::repo_root().join("schemas").join(file)).unwrap())
        .unwrap()
}

fn validator(file: &str) -> Validator {
    let registry = Registry::new()
        .add(
            "urn:streetwide:schema:common:0.1",
            load("common.schema.json"),
        )
        .unwrap()
        .prepare()
        .unwrap();
    jsonschema::options()
        .with_registry(&registry)
        .build(&load(file))
        .unwrap()
}

fn sample(file: &str) -> Value {
    let path = common::repo_root()
        .join("fixtures/changes/facade-sample")
        .join(file);
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// Asserts that both the schema and Rust reject `doc`.
fn both_reject(v: &Validator, doc: &Value, rust: impl Fn(&[u8]) -> bool, what: &str) {
    assert!(!v.is_valid(doc), "the schema accepted {what}");
    let accepted = cjson::to_vec(doc).map(|b| rust(&b)).unwrap_or(false);
    assert!(!accepted, "Rust accepted {what}");
}

#[test]
fn change_sets() {
    let v = validator("change-set.schema.json");
    let doc = sample("change.json");
    assert!(
        v.is_valid(&doc),
        "{:?}",
        v.iter_errors(&doc)
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
    );
    let rust = |b: &[u8]| ChangeSet::from_bytes(b).is_ok();
    assert!(rust(&cjson::to_vec(&doc).unwrap()));

    let mut mutations: Vec<(&str, Value)> = Vec::new();
    let mut d = doc.clone();
    d["author"] = json!("someone@example.org");
    mutations.push(("an e-mail address as author", d));
    let mut d = doc.clone();
    d["changes"][0]["parent_version_id"] = d["base_world_id"].clone();
    mutations.push(("an add with a parent", d));
    let mut d = doc.clone();
    d["changes"][0]["op"] = json!("rename");
    mutations.push(("an unknown op", d));
    let mut d = doc.clone();
    d["changes"] = json!([]);
    mutations.push(("no changes", d));
    let mut d = doc.clone();
    let one = d["changes"][0].clone();
    d["changes"] = Value::Array(vec![one; 51]);
    mutations.push(("51 changes", d));
    let mut d = doc.clone();
    d["note"] = json!("x");
    mutations.push(("an unknown field", d));
    let mut d = doc.clone();
    d["changes"][0]["layer"] = json!("river");
    mutations.push(("an unknown layer", d));
    for (what, m) in &mutations {
        both_reject(&v, m, rust, what);
    }

    // A change to the water layer is well formed: a baseline refresh makes
    // such changes, and the gate (G3) refuses contributed ones.
    let mut d = doc.clone();
    let tile = d["changes"][0]["tile"].as_str().unwrap().to_string();
    d["changes"][0]["layer"] = json!("water");
    d["changes"][0]["feature_id"] = json!(format!("tile:{tile}"));
    assert!(v.is_valid(&d));
    assert!(rust(&cjson::to_vec(&d).unwrap()));
}

#[test]
fn check_reports() {
    let v = validator("check-report.schema.json");
    let doc = sample("report.json");
    assert!(v.is_valid(&doc));
    let rust = |b: &[u8]| CheckReport::from_bytes(b).is_ok();
    let mut d = doc.clone();
    d["checks"][0]["outcome"] = json!("maybe");
    both_reject(&v, &d, rust, "an unknown outcome");
    let mut d = doc.clone();
    d["format"] = json!("streetwide.change");
    both_reject(&v, &d, rust, "the wrong format");
}

fn record_value(decision: Decision) -> Value {
    let id = |s: &str| Some(ContentId::of(s.as_bytes()));
    let linked = decision != Decision::Baseline;
    let r = MergeRecord {
        format: MergeRecord::FORMAT.into(),
        contract_version: "0.1.0".into(),
        decision,
        change_id: if linked { id("change") } else { None },
        world_before: if linked { id("w0") } else { None },
        world_after: ContentId::of(b"w1"),
        check_report_id: if linked { id("report") } else { None },
        reviews: vec![Review {
            reviewer: "github:reviewer".into(),
            verdict: Verdict::Better,
            notes: "Matches the building.".into(),
        }],
        reasons: "test".into(),
        previous_record_id: if linked { id("previous") } else { None },
    };
    serde_json::from_slice(&r.to_bytes().unwrap()).unwrap()
}

#[test]
fn merge_records() {
    let v = validator("merge-record.schema.json");
    let rust = |b: &[u8]| MergeRecord::from_bytes(b).is_ok();
    for decision in [Decision::Baseline, Decision::Merged, Decision::Takedown] {
        let doc = record_value(decision);
        assert!(v.is_valid(&doc), "{decision:?}");
        assert!(rust(&cjson::to_vec(&doc).unwrap()));
    }
    let mut d = record_value(Decision::Merged);
    d["change_id"] = Value::Null;
    both_reject(&v, &d, rust, "a merge without its change set");
    let mut d = record_value(Decision::Baseline);
    d["previous_record_id"] = json!(ContentId::of(b"x").to_string());
    both_reject(&v, &d, rust, "a baseline with a previous record");
    let mut d = record_value(Decision::Merged);
    d["reviews"][0]["verdict"] = json!("great");
    both_reject(&v, &d, rust, "an unknown verdict");
    let mut d = record_value(Decision::Merged);
    d["reasons"] = json!("");
    both_reject(&v, &d, rust, "empty reasons");
}
