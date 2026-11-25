//! `op_replay merge`: `op_world merge` with the complete gate (G1 to G10),
//! so that a change set that touches colliders can be merged, with
//! `--require-base` and `--log`.

mod common;

use common::{
    Temp, better, check_log, facade_sample, facade_shell, far_removal, far_shells, fixture_world,
    op_replay, read, removal, sample_path_file, sample_replay, world_copy, world_id,
};
use op_replay::gate::{check_change, full_gate, replay_name};
use op_world::contrib::{CheckReport, Decision, INGEST_AUTHOR, Outcome};
use op_world::merge::{MergeArgs, MergeOutput, WorldLog, baseline};
use op_world::validate::validate_store;
use op_world::{ContentId, Error, Store, gate};

/// `op_world baseline <world> --log <log>`, through the library.
fn start_log(world: &std::path::Path, log: &std::path::Path) {
    let record = baseline(world, "The ingest fixture.").unwrap();
    WorldLog::start(log, &record, &read(world.join("world.json"))).unwrap();
}

fn merge_args(
    world: &std::path::Path,
    bundle: &std::path::Path,
    log: &std::path::Path,
    require_base: bool,
) -> MergeArgs {
    MergeArgs {
        world_dir: world.to_path_buf(),
        bundle_dir: bundle.to_path_buf(),
        output: MergeOutput::Log(log.to_path_buf()),
        decision: Decision::Merged,
        reviews: vec![better()],
        reasons: "Test.".into(),
        require_base,
    }
}

fn outcome(r: &CheckReport, id: &str) -> Outcome {
    r.checks.iter().find(|c| c.id == id).unwrap().outcome
}

#[test]
fn the_far_building_removal_needs_the_full_gate() {
    let t = Temp::new("merge-far");
    let world = world_copy(&t, "world");
    let log = t.join("log");
    start_log(&world, &log);
    let w0 = world_id(&world);
    let bundle = t.join("far");
    far_removal(&world, &bundle);
    let replays = [sample_replay()];

    // `op_world merge`: its gate cannot run G9 and G10 for a collider.
    let refused = merge_args(&world, &bundle, &log, true).run(&t.join("scratch-1"), gate::run);
    let e = refused.err().unwrap().to_string();
    assert!(
        e.ends_with("does not pass the gate: G9 walkability, G10 simulation"),
        "{e}"
    );
    assert_eq!(world_id(&world), w0);
    assert_eq!(check_log(&log).len(), 1, "a refused merge writes nothing");

    // `op_replay merge` runs them, and they pass.
    let line = merge_args(&world, &bundle, &log, true)
        .run(&t.join("scratch-2"), full_gate(&replays))
        .unwrap();
    let records = check_log(&log);
    let record = &records[1];
    assert!(
        line.contains(&format!(
            " (log/00000001.json): world {w0} -> {}",
            record.world_after
        )),
        "{line}"
    );
    assert_eq!(world_id(&world), record.world_after);
    let v = validate_store(&Store::new(&world));
    assert!(v.errors.is_empty(), "{:?}", v.errors);
    assert_eq!(v.assets, 44, "one of the 45 fixture assets is gone");

    // The record names the full report: the one `op_replay check-change`
    // gives on the world before, with G9 and G10 run.
    let report_id = record.check_report_id.unwrap();
    let report_bytes = read(log.join(format!("reports/{}.json", report_id.hex())));
    assert_eq!(ContentId::of(&report_bytes), report_id);
    let full = check_change(&fixture_world(), &bundle, &t.join("candidate"), &replays).unwrap();
    assert_eq!(full.to_bytes().unwrap(), report_bytes);
    assert_eq!(
        (outcome(&full, "G9"), outcome(&full, "G10")),
        (Outcome::Pass, Outcome::Pass)
    );
    assert!(full.passed);
}

#[test]
fn a_set_that_fails_g10_is_refused() {
    // The removal of the house the sample walk runs into (full_gate.rs):
    // G9 passes, G10 fails, so `op_replay merge` refuses it.
    let t = Temp::new("merge-g10");
    let world = world_copy(&t, "world");
    let log = t.join("log");
    start_log(&world, &log);
    let w0 = world_id(&world);
    let bundle = t.join("house");
    removal(
        &world,
        &["swisstopo:swissbuildings3d:f1e4f3ab-6046-439a-b6ba-bd74c711bee6"],
        INGEST_AUTHOR,
        &bundle,
    );
    let refused = merge_args(&world, &bundle, &log, true)
        .run(&t.join("scratch"), full_gate(&[sample_replay()]));
    let e = refused.err().unwrap().to_string();
    assert!(e.ends_with("does not pass the gate: G10 simulation"), "{e}");
    assert_eq!(world_id(&world), w0);
    assert_eq!(check_log(&log).len(), 1, "a refused merge writes nothing");
}

#[test]
fn a_facade_merged_between_two_refresh_sets_stops_the_next_one() {
    let t = Temp::new("merge-between");
    let world = world_copy(&t, "world");
    let log = t.join("log");
    start_log(&world, &log);
    let replays = [sample_replay()];
    let facade = facade_shell();
    let far: Vec<String> = far_shells()
        .into_iter()
        .filter(|f| *f != facade)
        .take(2)
        .collect();
    assert_eq!(
        far.len(),
        2,
        "two buildings away from the walk and the facade"
    );

    // A refresh planned as two sets: the second on the world the first one
    // makes.
    let set1 = t.join("set-1");
    let planned = removal(&world, &[far[0].as_str()], INGEST_AUTHOR, &set1);
    assert_eq!(planned.base_world_id, world_id(&world));
    merge_args(&world, &set1, &log, true)
        .run(&t.join("scratch-1"), full_gate(&replays))
        .unwrap();
    let w1 = world_id(&world);
    let set2 = t.join("set-2");
    removal(&world, &[far[1].as_str()], INGEST_AUTHOR, &set2);

    // A facade is merged in between (planned on the fixture world; its
    // parent, none, is still current).
    merge_args(&world, &facade_sample(), &log, false)
        .run(&t.join("scratch-2"), full_gate(&replays))
        .unwrap();
    let w2 = world_id(&world);

    let refused =
        merge_args(&world, &set2, &log, true).run(&t.join("scratch-3"), full_gate(&replays));
    assert_eq!(
        refused.err(),
        Some(Error::Invalid(format!(
            "the store holds world {w2}, but this change set was planned on {w1}; plan the refresh again"
        )))
    );
    assert_eq!(world_id(&world), w2);
    assert_eq!(check_log(&log).len(), 3, "the refused merge wrote nothing");

    // Without the flag, RFC 0002 lets the stale set through, because its
    // parent is still current. Amendment A7 would refuse it without the
    // flag; it is not adopted.
    merge_args(&world, &set2, &log, false)
        .run(&t.join("scratch-4"), full_gate(&replays))
        .unwrap();
    let records = check_log(&log);
    assert_eq!(records.len(), 4);
    assert_eq!(records[3].world_before, Some(w2));
}

#[test]
fn the_command_line_merges_with_the_full_gate() {
    let t = Temp::new("merge-cli");
    let world = world_copy(&t, "world");
    let log = t.join("log");
    start_log(&world, &log);
    let w0 = world_id(&world);
    let bundle = t.join("far");
    far_removal(&world, &bundle);
    let merge: [&dyn AsRef<std::ffi::OsStr>; 13] = [
        &"merge",
        &world,
        &bundle,
        &"--log",
        &log,
        &"--require-base",
        &"--replay",
        &sample_path_file(),
        &"--reason",
        &"Demolished (test).",
        &"--review",
        &"github:reviewer=better:Checked.",
        &"--replay",
    ];
    let (code, _, err) = op_replay(&merge);
    assert_eq!(code, 2);
    assert_eq!(err.trim_end(), "error: --replay needs a value");
    let (code, out, err) = op_replay(&merge[..12]);
    assert_eq!(code, 0, "{err}");
    let w1 = world_id(&world);
    assert!(
        out.starts_with("record blake3:")
            && out.contains(&format!(" (log/00000001.json): world {w0} -> {w1}")),
        "{out}"
    );
    // The same set again. The sample replay was recorded in the world
    // before, so G10 would ignore it: the merge is refused rather than
    // skipping G10.
    let (code, _, err) = op_replay(&merge[..12]);
    assert_eq!(code, 2);
    assert_eq!(
        err.trim_end(),
        format!(
            "error: --replay {}: {} was recorded in world {w0}, not in the store's world {w1}, so G10 would ignore it; move it to this world with op_replay rerecord, or leave it out",
            sample_path_file().display(),
            replay_name(&sample_replay())
        )
    );
    // Without the replay, the set was planned on the world before.
    let without: Vec<&dyn AsRef<std::ffi::OsStr>> = merge[..12]
        .iter()
        .enumerate()
        .filter(|(i, _)| !(6..8).contains(i))
        .map(|(_, a)| *a)
        .collect();
    let (code, _, err) = op_replay(&without);
    assert_eq!(code, 2);
    assert_eq!(
        err.trim_end(),
        format!(
            "error: invalid: the store holds world {w1}, but this change set was planned on {w0}; plan the refresh again"
        )
    );
    assert_eq!(check_log(&log).len(), 2);

    // `--previous`, `--record` and `--report`, as with `op_world merge`.
    let other = world_copy(&t, "other");
    let first = t.join("0.json");
    std::fs::write(
        &first,
        baseline(&other, "The ingest fixture.")
            .unwrap()
            .to_bytes()
            .unwrap(),
    )
    .unwrap();
    let (record, report) = (t.join("1.json"), t.join("1-report.json"));
    let (code, out, err) = op_replay(&[
        &"merge",
        &other,
        &bundle,
        &"--previous",
        &first,
        &"--record",
        &record,
        &"--report",
        &report,
        &"--reason",
        &"Demolished (test).",
        &"--review",
        &"github:reviewer=better:Checked.",
        &"--replay",
        &sample_path_file(),
    ]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        out.trim_end(),
        format!(
            "record {}: world {w0} -> {w1}",
            ContentId::of(&read(&record))
        )
    );
    let r = op_world::contrib::MergeRecord::from_bytes(&read(&record)).unwrap();
    assert_eq!(r.check_report_id, Some(ContentId::of(&read(&report))));

    // Usage errors.
    let (code, _, err) = op_replay(&[&"merge", &world, &bundle, &"--log", &log]);
    assert_eq!(code, 2);
    assert_eq!(err.trim_end(), "error: --reason <value> is required");
    let (code, _, err) = op_replay(&[&"merge", &world]);
    assert_eq!(code, 2);
    assert!(
        err.contains("op_replay merge <world-dir> <bundle-dir> (--previous <record> --record <file> --report <file> | --log <dir>)")
            && err.contains("[--require-base] [--replay <file>]..."),
        "{err}"
    );
}
