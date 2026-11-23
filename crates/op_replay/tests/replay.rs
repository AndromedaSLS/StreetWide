//! The replay format, playback and the committed sample replay
//! (`fixtures/replays/sample.oprep` in `fixtures/world`).

use std::path::PathBuf;

use jsonschema::Registry;
use op_replay::format::{CHECKPOINT_INTERVAL, checkpoint_ticks};
use op_replay::synth::{self, Meter};
use op_replay::{Error, Recorder, Replay, Verdict, check, open_world, record};
use op_world::cjson;
use serde_json::{Value, json};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sample_bytes() -> Vec<u8> {
    std::fs::read(repo().join("fixtures/replays/sample.oprep")).expect("sample replay")
}

fn sample() -> Replay {
    Replay::from_bytes(&sample_bytes()).expect("sample replay parses")
}

#[test]
fn the_sample_replay_reproduces_its_hash_trail() {
    let replay = sample();
    let mut source = open_world(&replay.header, &repo().join("fixtures/world")).unwrap();
    let verdict = check(&replay, &mut source).unwrap();
    assert_eq!(
        verdict,
        Verdict::Match(replay.checkpoints.last().unwrap().hash)
    );
}

#[test]
fn the_sample_regenerates_byte_for_byte_and_covers_the_script() {
    let mut meter = Meter::default();
    let replay = record(
        &repo().join("fixtures/world"),
        synth::SEED,
        None,
        synth::sample_inputs(),
        Recorder {
            name: "op_replay synth".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
        |sim| meter.observe(sim),
    )
    .unwrap();
    assert_eq!(
        replay.to_bytes().unwrap(),
        sample_bytes(),
        "regenerate with `op_replay synth`"
    );
    let r = meter.report();
    assert_eq!(r.ticks, 1200);
    assert!(r.walked_m > 30.0, "{r:?}");
    assert!(r.blocked_ticks >= 30, "ran into a wall: {r:?}");
    assert!((35..=50).contains(&r.airborne_ticks), "jumped: {r:?}");
    assert!(r.climbed_m > 1.5, "walked uphill: {r:?}");
    assert!(r.headings >= 3, "turned: {r:?}");
}

#[test]
fn a_changed_input_is_caught_at_the_next_checkpoint() {
    let mut replay = sample();
    replay.inputs[700].forward = 0; // the 701st tick
    let mut source = open_world(&replay.header, &repo().join("fixtures/world")).unwrap();
    match check(&replay, &mut source).unwrap() {
        Verdict::Diverged { tick, .. } => assert_eq!(tick, 720),
        v => panic!("not caught: {v:?}"),
    }
}

#[test]
fn the_world_and_simulation_version_must_match() {
    let replay = sample();
    let wrong = open_world(&replay.header, &repo().join("fixtures/format/world"));
    assert!(matches!(wrong, Err(Error::World(_))));

    let mut old = replay.clone();
    old.header.sim_contract_version = "0.0.1".into();
    let mut source = open_world(&replay.header, &repo().join("fixtures/world")).unwrap();
    assert!(matches!(
        check(&old, &mut source),
        Err(Error::SimVersion { .. })
    ));
}

#[test]
fn checkpoints_are_every_interval_and_at_the_end() {
    assert_eq!(checkpoint_ticks(120, 60), vec![60, 120]);
    assert_eq!(checkpoint_ticks(130, 60), vec![60, 120, 130]);
    assert_eq!(checkpoint_ticks(5, 60), vec![5]);
    assert_eq!(
        sample().checkpoints.len(),
        1200 / CHECKPOINT_INTERVAL as usize
    );
}

#[test]
fn malformed_files_are_rejected() {
    let good = sample_bytes();
    assert_eq!(sample().to_bytes().unwrap(), good, "round trip");
    let header_len = u32::from_le_bytes(good[12..16].try_into().unwrap()) as usize;
    let inputs_at = 16 + header_len;

    let mut cases: Vec<(&str, Vec<u8>)> = Vec::new();
    let mut b = good.clone();
    b[0] = b'X';
    cases.push(("magic", b));
    let mut b = good.clone();
    b[8] = 2;
    cases.push(("file version", b));
    cases.push(("truncated", good[..good.len() - 1].to_vec()));
    let mut b = good.clone();
    b.push(0);
    cases.push(("trailing byte", b));
    let mut b = good.clone();
    b[inputs_at + 7] = 1;
    cases.push(("reserved input byte", b));
    let mut b = good.clone();
    b[inputs_at + 6] = 0x10;
    cases.push(("unknown button", b));
    let mut b = good.clone();
    let first_checkpoint = inputs_at + 1200 * 8 + 4;
    b[first_checkpoint] = 61;
    cases.push(("checkpoint tick", b));
    let mut b = good.clone();
    let header = &mut b[16..inputs_at];
    let colon = header.iter().position(|&c| c == b':').unwrap();
    header[colon + 1] = b' ';
    cases.push(("non-canonical header", b));

    for (what, bytes) in cases {
        assert!(
            Replay::from_bytes(&bytes).is_err(),
            "accepted a file with a bad {what}"
        );
    }
}

fn header_validator() -> jsonschema::Validator {
    let load = |f: &str| -> Value {
        serde_json::from_slice(&std::fs::read(repo().join("schemas").join(f)).unwrap()).unwrap()
    };
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
        .build(&load("replay-header.schema.json"))
        .unwrap()
}

#[test]
fn header_schema_and_rust_agree() {
    let validator = header_validator();
    let header: Value =
        serde_json::from_slice(&cjson::to_canonical(&sample().header).unwrap()).unwrap();
    assert!(
        validator.is_valid(&header),
        "{:?}",
        validator
            .iter_errors(&header)
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
    );

    let mutations: Vec<(&str, Value)> = vec![
        ("seed", json!(9_007_199_254_740_992u64)),
        ("tick_hz", json!(30)),
        ("ticks", json!(0)),
        ("world_name", json!("Grossmünster")),
        ("format", json!("streetwide.world")),
        ("contract_version", json!("9.9.9")),
        ("checkpoint_interval", json!(0)),
    ];
    for (field, value) in mutations {
        let mut doc = header.clone();
        doc[field] = value;
        assert!(!validator.is_valid(&doc), "schema accepted {field}");
        let rust = serde_json::from_value::<op_replay::ReplayHeader>(doc)
            .map(|h| h.problems().is_empty())
            .unwrap_or(false);
        assert!(!rust, "Rust accepted {field}");
    }
    let mut extra = header.clone();
    extra["note"] = json!("x");
    assert!(!validator.is_valid(&extra));
    assert!(serde_json::from_value::<op_replay::ReplayHeader>(extra).is_err());
}
