//! `merge_with` (the gate as a parameter), the world log kept in a
//! directory (`--log`), `--require-base`, and the arguments that
//! `op_world merge` and `op_replay merge` share
//! (`op_world::merge::{WorldLog, MergeArgs}`).

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use op_world::contrib::{
    Change, ChangeSet, CheckReport, Decision, MergeRecord, Op, Outcome, Review, Verdict, verify_log,
};
use op_world::merge::{
    LOG_LOCK, MergeArgs, MergeOutput, STORE_LOCK, WorldLog, baseline, merge_with, require_base,
};
use op_world::{ContentId, Error, Layer, ObjectKind, Store, TileIndex, gate};

/// A fresh temporary directory, deleted again at the end of the test.
struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Temp {
        Temp(common::temp_dir(&format!("log-{name}")))
    }

    fn join(&self, path: &str) -> PathBuf {
        self.0.join(path)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn read(path: impl AsRef<Path>) -> Vec<u8> {
    let path = path.as_ref();
    std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A private copy of the ingest fixture to merge into.
fn world_copy(t: &Temp) -> PathBuf {
    let from = common::repo_root().join("fixtures/world");
    let to = t.join("world");
    for f in common::files_under(&from) {
        let dst = to.join(&f);
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        std::fs::copy(from.join(&f), dst).unwrap();
    }
    to
}

fn world_id(world: &Path) -> ContentId {
    Store::new(world).read_world().unwrap().0
}

fn facade() -> PathBuf {
    common::repo_root().join("fixtures/changes/facade-sample")
}

fn better() -> Review {
    Review {
        reviewer: "github:reviewer".into(),
        verdict: Verdict::Better,
        notes: "Checked against the RFC 0002 facade checklist.".into(),
    }
}

/// `op_world baseline <world> --log <log>`, through the library.
fn start_log(world: &Path, log: &Path) -> WorldLog {
    let record = baseline(world, "The ingest fixture.").unwrap();
    WorldLog::start(log, &record, &read(Store::new(world).world_path())).unwrap()
}

fn merge_args(
    world: &Path,
    bundle: &Path,
    output: MergeOutput,
    decision: Decision,
    require_base: bool,
) -> MergeArgs {
    MergeArgs {
        world_dir: world.to_path_buf(),
        bundle_dir: bundle.to_path_buf(),
        output,
        decision,
        reviews: if decision == Decision::Merged {
            vec![better()]
        } else {
            Vec::new()
        },
        reasons: "Test.".into(),
        require_base,
    }
}

/// A change set planned on world `base` that takes the facade in `world`
/// down again.
fn takedown_bundle(world: &Path, base: ContentId, dir: &Path) -> PathBuf {
    let store = Store::new(world);
    let (_, w) = store.read_world().unwrap();
    let index = TileIndex::from_bytes(
        &store
            .read_object(&w.tiles[0].tile_id, ObjectKind::Json)
            .unwrap(),
    )
    .unwrap();
    let facade = index
        .assets
        .iter()
        .find(|a| a.layer == Layer::Facade)
        .unwrap();
    let change = ChangeSet {
        format: ChangeSet::FORMAT.into(),
        contract_version: "0.1.0".into(),
        base_world_id: base,
        title: "Remove the test facade".into(),
        author: "github:maintainer".into(),
        rationale: "Owner request (test).".into(),
        changes: vec![Change {
            op: Op::Remove,
            tile: w.tiles[0].tile,
            layer: Layer::Facade,
            feature_id: facade.feature_id.clone(),
            parent_version_id: Some(facade.manifest_id),
            manifest_id: None,
        }],
    };
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("change.json"), change.to_bytes().unwrap()).unwrap();
    dir.to_path_buf()
}

/// Checks a log directory file by file, independently of `WorldLog`: the
/// records are `log/00000000.json` onwards, every record's change set,
/// report and world are there under their hex ids, and nothing else is.
/// Returns the current world.
fn check_log(dir: &Path) -> ContentId {
    let files = common::files_under(dir);
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
    verify_log(&records).unwrap()
}

#[test]
fn merge_with_records_the_report_of_the_gate_it_is_given() {
    let t = Temp::new("with");
    let world = world_copy(&t);
    let w0 = world_id(&world);
    let base = baseline(&world, "The ingest fixture.")
        .unwrap()
        .to_bytes()
        .unwrap();
    // A gate that fails G7 on top of `op_world`'s gate: refused, and the
    // store is untouched.
    let failing = |w: &Path, b: &Path, c: &Path| -> op_world::Result<CheckReport> {
        let mut r = gate::run(w, b, c)?;
        let g7 = r.checks.iter_mut().find(|c| c.id == "G7").unwrap();
        g7.outcome = Outcome::Fail;
        g7.details.push("refused by the test's gate".into());
        r.passed = false;
        Ok(r)
    };
    let refused = merge_with(
        &world,
        &facade(),
        &base,
        Decision::Merged,
        vec![better()],
        "Test.",
        &t.join("scratch-1"),
        failing,
    );
    assert!(refused.err().is_some_and(|e| {
        e.to_string()
            .ends_with("does not pass the gate: G7 budgets")
    }),);
    assert_eq!(world_id(&world), w0);

    // A gate that adds a detail: the record names that report.
    let noting = |w: &Path, b: &Path, c: &Path| -> op_world::Result<CheckReport> {
        let mut r = gate::run(w, b, c)?;
        r.checks[0].details.push("noted by the test's gate".into());
        Ok(r)
    };
    let merged = merge_with(
        &world,
        &facade(),
        &base,
        Decision::Merged,
        vec![better()],
        "Test.",
        &t.join("scratch-2"),
        noting,
    )
    .unwrap();
    let report = CheckReport::from_bytes(&merged.report_bytes).unwrap();
    assert_eq!(
        report.checks[0].details.last().map(String::as_str),
        Some("noted by the test's gate")
    );
    assert_eq!(
        merged.record.check_report_id,
        Some(ContentId::of(&merged.report_bytes))
    );
    assert_eq!(Some(world_id(&world)), report.candidate_world_id);
}

#[test]
fn a_log_directory_keeps_every_merge_and_takedown() {
    let t = Temp::new("keeps");
    let world = world_copy(&t);
    let log_dir = t.join("log");
    let w0 = world_id(&world);
    let log = start_log(&world, &log_dir);
    assert_eq!((log.records().len(), log.world_id()), (1, w0));
    assert_eq!(
        common::files_under(&log_dir),
        [
            "log/00000000.json".to_string(),
            format!("worlds/{}.json", w0.hex())
        ]
    );

    let line = merge_args(
        &world,
        &facade(),
        MergeOutput::Log(log_dir.clone()),
        Decision::Merged,
        true,
    )
    .run(&t.join("scratch-1"), gate::run)
    .unwrap();
    let log = WorldLog::open(&log_dir).unwrap();
    let (record_id, record) = log.records()[1].clone();
    let w1 = record.world_after;
    assert_eq!(
        line,
        format!("record {record_id} (log/00000001.json): world {w0} -> {w1}")
    );
    assert_eq!(log.head(), read(log_dir.join("log/00000001.json")));
    assert_eq!(w1, world_id(&world));
    // The four files: the record, the change set as merged, the gate's
    // report (the committed one, since the copy is the fixture world) and
    // the world after.
    let change = read(facade().join("change.json"));
    assert_eq!(record.change_id, Some(ContentId::of(&change)));
    let hex = |id: Option<ContentId>| id.unwrap().hex();
    assert_eq!(
        read(log_dir.join(format!("changes/{}/change.json", hex(record.change_id)))),
        change
    );
    assert_eq!(
        read(log_dir.join(format!("reports/{}.json", hex(record.check_report_id)))),
        read(facade().join("report.json"))
    );
    assert_eq!(
        read(log_dir.join(format!("worlds/{}.json", w1.hex()))),
        read(world.join("world.json"))
    );

    let takedown = takedown_bundle(&world, w1, &t.join("takedown"));
    let line = merge_args(
        &world,
        &takedown,
        MergeOutput::Log(log_dir.clone()),
        Decision::Takedown,
        true,
    )
    .run(&t.join("scratch-2"), gate::run)
    .unwrap();
    assert!(line.contains(" (log/00000002.json): world "), "{line}");
    assert_eq!(world_id(&world), w0, "the takedown restores the baseline");
    assert_eq!(check_log(&log_dir), w0);
    // Three records, two change sets, two reports, two worlds.
    assert_eq!(common::files_under(&log_dir).len(), 9);
    assert_eq!(WorldLog::open(&log_dir).unwrap().world_id(), w0);
}

#[test]
fn require_base_refuses_a_change_set_planned_on_another_world() {
    let t = Temp::new("base");
    let world = world_copy(&t);
    let w0 = world_id(&world);
    std::fs::write(
        t.join("0.json"),
        baseline(&world, "The ingest fixture.")
            .unwrap()
            .to_bytes()
            .unwrap(),
    )
    .unwrap();
    let files = |previous: &str, n: &str| MergeOutput::Files {
        previous: t.join(previous),
        record: t.join(&format!("{n}.json")),
        report: t.join(&format!("{n}-report.json")),
    };

    // The facade was planned on the fixture world, which the store holds.
    require_base(&world, &facade()).unwrap();
    merge_args(
        &world,
        &facade(),
        files("0.json", "1"),
        Decision::Merged,
        true,
    )
    .run(&t.join("scratch-1"), gate::run)
    .unwrap();
    let w1 = world_id(&world);

    // A takedown planned before the facade was merged is stale.
    let stale = takedown_bundle(&world, w0, &t.join("stale"));
    let refused = merge_args(
        &world,
        &stale,
        files("1.json", "2"),
        Decision::Takedown,
        true,
    )
    .run(&t.join("scratch-2"), gate::run);
    assert_eq!(
        refused.err(),
        Some(Error::Invalid(format!(
            "the store holds world {w1}, but this change set was planned on {w0}; plan the refresh again"
        )))
    );
    assert_eq!(world_id(&world), w1);
    assert!(!t.join("2.json").exists() && !t.join("2-report.json").exists());

    // The flag is opt-in: without it, RFC 0002 lets a change set whose
    // parents are still current merge on a newer world.
    merge_args(
        &world,
        &stale,
        files("1.json", "2"),
        Decision::Takedown,
        false,
    )
    .run(&t.join("scratch-3"), gate::run)
    .unwrap();
    assert_eq!(world_id(&world), w0);
}

#[test]
fn a_refused_merge_leaves_the_log_and_the_store_unchanged() {
    let t = Temp::new("refused");
    let world = world_copy(&t);
    let log_dir = t.join("log");
    start_log(&world, &log_dir);
    let before = common::files_under(&log_dir);
    let mut args = merge_args(
        &world,
        &facade(),
        MergeOutput::Log(log_dir.clone()),
        Decision::Merged,
        false,
    );
    args.reviews[0].verdict = Verdict::NotBetter;
    let refused = args.run(&t.join("scratch"), gate::run);
    assert!(refused.is_err(), "a merge needs a review judging it better");
    assert_eq!(common::files_under(&log_dir), before);
    assert_eq!(
        world_id(&world),
        WorldLog::open(&log_dir).unwrap().world_id()
    );
}

#[test]
fn a_merge_holds_a_lock_on_the_store_and_on_the_log() {
    let t = Temp::new("lock");
    let world = world_copy(&t);
    let log_dir = t.join("log");
    start_log(&world, &log_dir);
    let w0 = world_id(&world);
    let before = common::files_under(&log_dir);
    let args = merge_args(
        &world,
        &facade(),
        MergeOutput::Log(log_dir.clone()),
        Decision::Merged,
        true,
    );

    // A lock that another merge holds: refused before anything is read,
    // and the other merge's lock stays.
    for (dir, name, what) in [
        (&world, STORE_LOCK, "store"),
        (&log_dir, LOG_LOCK, "world log"),
    ] {
        let lock = dir.join(name);
        std::fs::write(&lock, b"").unwrap();
        let e = args.run(&t.join("scratch"), gate::run).unwrap_err();
        assert_eq!(
            e,
            Error::Invalid(format!(
                "{} exists: another merge is running on this {what}, or one stopped before it could remove the file; if no merge is running, delete the file and merge again",
                lock.display()
            ))
        );
        assert!(lock.is_file(), "the other merge's lock stays");
        std::fs::remove_file(&lock).unwrap();
        assert_eq!(world_id(&world), w0);
        assert_eq!(common::files_under(&log_dir), before);
    }

    // The gate runs with both locks taken; after the merge, both are gone.
    let log_lock = log_dir.join(LOG_LOCK);
    let locked = |w: &Path, b: &Path, c: &Path| -> op_world::Result<CheckReport> {
        assert!(w.join(STORE_LOCK).is_file() && log_lock.is_file());
        gate::run(w, b, c)
    };
    args.run(&t.join("scratch"), locked).unwrap();
    assert!(!world.join(STORE_LOCK).exists() && !log_lock.exists());
    assert_eq!(check_log(&log_dir), world_id(&world));

    // A log that does not exist is the log's error, not the lock's.
    let missing = merge_args(
        &world,
        &facade(),
        MergeOutput::Log(t.join("no-log")),
        Decision::Merged,
        true,
    )
    .run(&t.join("scratch"), gate::run)
    .unwrap_err();
    assert!(
        missing.to_string().contains("the world log has no records"),
        "{missing}"
    );

    // A merge that the gate refuses removes its own locks too.
    let mut again = args.clone();
    again.require_base = false;
    let e = again.run(&t.join("scratch"), gate::run).unwrap_err();
    assert!(e.to_string().contains("does not pass the gate"), "{e}");
    assert!(!world.join(STORE_LOCK).exists() && !log_lock.exists());
}

#[test]
fn a_store_that_changes_during_the_merge_is_not_written() {
    // A tool that ignores the lock writes world.json while the gate runs.
    let t = Temp::new("moved");
    let world = world_copy(&t);
    let previous = baseline(&world, "The ingest fixture.")
        .unwrap()
        .to_bytes()
        .unwrap();
    let w0 = world_id(&world);
    let objects = common::files_under(&world.join("objects"));
    let racing = |w: &Path, b: &Path, c: &Path| -> op_world::Result<CheckReport> {
        let report = gate::run(w, b, c)?;
        std::fs::copy(c.join("world.json"), w.join("world.json")).unwrap();
        Ok(report)
    };
    let e = merge_with(
        &world,
        &facade(),
        &previous,
        Decision::Merged,
        vec![better()],
        "Test.",
        &t.join("scratch"),
        racing,
    )
    .err()
    .unwrap();
    let w1 = world_id(&world);
    assert_ne!(w1, w0);
    assert_eq!(
        e,
        Error::Invalid(format!(
            "the store changed during the merge, from world {w0} to {w1}, so nothing was written"
        ))
    );
    assert_eq!(common::files_under(&world.join("objects")), objects);
    assert!(!world.join(STORE_LOCK).exists());
}

#[test]
fn a_log_that_cannot_be_written_says_how_to_undo_the_merge() {
    let t = Temp::new("undo");
    let world = world_copy(&t);
    let log_dir = t.join("log");
    start_log(&world, &log_dir);
    let w0 = world_id(&world);
    // The world after the merge, then a file in its place in the log that
    // holds other bytes.
    let after = gate::run(&world, &facade(), &t.join("candidate"))
        .unwrap()
        .candidate_world_id
        .unwrap();
    let clash = log_dir.join(format!("worlds/{}.json", after.hex()));
    std::fs::write(&clash, b"not this world").unwrap();
    let args = merge_args(
        &world,
        &facade(),
        MergeOutput::Log(log_dir.clone()),
        Decision::Merged,
        true,
    );
    let e = args.run(&t.join("scratch-1"), gate::run).unwrap_err();
    assert_eq!(
        e,
        Error::Io(format!(
            "the store now holds world {after}, but the world log was not updated (invalid: the world log's worlds/{}.json holds other bytes); to undo the merge, copy the log's worlds/{}.json over the store's world.json",
            after.hex(),
            w0.hex()
        ))
    );
    assert_eq!(world_id(&world), after);
    assert!(WorldLog::open(&log_dir).unwrap().records().len() == 1);

    // Undo as the message says, then merge again.
    std::fs::copy(
        log_dir.join(format!("worlds/{}.json", w0.hex())),
        world.join("world.json"),
    )
    .unwrap();
    assert_eq!(world_id(&world), w0);
    std::fs::remove_file(&clash).unwrap();
    args.run(&t.join("scratch-2"), gate::run).unwrap();
    assert_eq!(check_log(&log_dir), after);
}

#[test]
fn the_undo_hint_names_only_a_world_the_log_keeps() {
    // A log whose world before the merge is not in worlds/ (it is optional
    // there, as for a baseline record written with --record).
    let t = Temp::new("undo-gone");
    let world = world_copy(&t);
    let log_dir = t.join("log");
    start_log(&world, &log_dir);
    let w0 = world_id(&world);
    std::fs::remove_file(log_dir.join(format!("worlds/{}.json", w0.hex()))).unwrap();
    let after = gate::run(&world, &facade(), &t.join("candidate"))
        .unwrap()
        .candidate_world_id
        .unwrap();
    std::fs::write(
        log_dir.join(format!("worlds/{}.json", after.hex())),
        b"not this world",
    )
    .unwrap();
    let e = merge_args(
        &world,
        &facade(),
        MergeOutput::Log(log_dir.clone()),
        Decision::Merged,
        true,
    )
    .run(&t.join("scratch"), gate::run)
    .unwrap_err();
    assert_eq!(
        e,
        Error::Io(format!(
            "the store now holds world {after}, but the world log was not updated (invalid: the world log's worlds/{}.json holds other bytes); to undo the merge, put back the store's world.json of world {w0}",
            after.hex()
        ))
    );
}

#[test]
fn a_broken_log_is_refused() {
    let t = Temp::new("broken");
    let world = world_copy(&t);
    let refusal = |dir: &Path| match WorldLog::open(dir) {
        Ok(_) => panic!("{} opens", dir.display()),
        Err(e) => e.to_string(),
    };
    assert!(
        refusal(&t.join("missing"))
            .contains("the world log has no records; start it with op_world baseline")
    );

    let log_dir = t.join("log");
    start_log(&world, &log_dir);
    WorldLog::open(&log_dir).unwrap();
    let again = baseline(&world, "Again.").unwrap();
    let started = WorldLog::start(&log_dir, &again, &read(world.join("world.json")));
    assert!(
        started.is_err_and(|e| e.to_string().contains("already has records")),
        "a baseline starts a new log only"
    );

    let records = log_dir.join("log");
    std::fs::copy(records.join("00000000.json"), records.join("00000002.json")).unwrap();
    assert!(
        refusal(&log_dir).contains("has log/00000002.json where log/00000001.json should be"),
        "a gap"
    );
    std::fs::rename(records.join("00000002.json"), records.join("00000001.json")).unwrap();
    assert!(
        refusal(&log_dir).contains("world log entry 1 (blake3:"),
        "a second baseline breaks the chain"
    );
    std::fs::rename(
        records.join("00000001.json"),
        records.join("00000001.json.tmp"),
    )
    .unwrap();
    assert!(
        refusal(&log_dir).contains("has log/00000001.json.tmp where log/00000001.json should be"),
        "a file left by a crash"
    );
    std::fs::remove_file(records.join("00000001.json.tmp")).unwrap();
    assert_eq!(WorldLog::open(&log_dir).unwrap().records().len(), 1);

    // A merged record whose change set is missing, or another one: the
    // refresh planner refuses such a log, so a merge must not extend it.
    merge_args(
        &world,
        &facade(),
        MergeOutput::Log(log_dir.clone()),
        Decision::Merged,
        true,
    )
    .run(&t.join("scratch"), gate::run)
    .unwrap();
    let change = read(facade().join("change.json"));
    let id = ContentId::of(&change);
    let kept = log_dir.join(format!("changes/{}/change.json", id.hex()));
    let missing = format!(
        "invalid: the world log names change set {id} (log/00000001.json) but changes/{}/change.json is missing or has another id",
        id.hex()
    );
    std::fs::write(&kept, b"{}").unwrap();
    assert_eq!(refusal(&log_dir), missing);
    std::fs::remove_file(&kept).unwrap();
    assert_eq!(refusal(&log_dir), missing);
    std::fs::write(&kept, &change).unwrap();
    assert_eq!(WorldLog::open(&log_dir).unwrap().records().len(), 2);
}

fn parse(args: &[&str]) -> Result<MergeArgs, String> {
    MergeArgs::parse(&args.iter().map(|a| a.to_string()).collect::<Vec<_>>())
}

#[test]
fn the_merge_arguments() {
    let files = parse(&[
        "w",
        "b",
        "--previous",
        "p.json",
        "--record",
        "r.json",
        "--report",
        "g.json",
        "--reason",
        "Why.",
        "--review",
        "github:a=better:Fine.",
        "--review",
        "github:b=not_better",
    ])
    .unwrap();
    assert_eq!(
        files.output,
        MergeOutput::Files {
            previous: "p.json".into(),
            record: "r.json".into(),
            report: "g.json".into()
        }
    );
    assert_eq!(
        (files.decision, files.require_base, files.reasons.as_str()),
        (Decision::Merged, false, "Why.")
    );
    assert_eq!(
        files
            .reviews
            .iter()
            .map(|r| (r.reviewer.as_str(), r.verdict, r.notes.as_str()))
            .collect::<Vec<_>>(),
        [
            ("github:a", Verdict::Better, "Fine."),
            ("github:b", Verdict::NotBetter, "not_better")
        ]
    );
    let log = parse(&[
        "w",
        "b",
        "--log",
        "l",
        "--takedown",
        "--require-base",
        "--reason",
        "Why.",
    ])
    .unwrap();
    assert_eq!(log.output, MergeOutput::Log("l".into()));
    assert_eq!((log.decision, log.require_base), (Decision::Takedown, true));

    let refused: [(&[&str], &str); 9] = [
        (
            &["w", "b", "--log", "l", "--previous", "p", "--reason", "x"],
            "--previous cannot be combined with --log",
        ),
        (
            &["w", "b", "--report", "g", "--log", "l", "--reason", "x"],
            "--report cannot be combined with --log",
        ),
        (
            &[
                "w",
                "b",
                "--previous",
                "p",
                "--record",
                "r",
                "--reason",
                "x",
            ],
            "--report <value> is required",
        ),
        (&["w", "b", "--log", "l"], "--reason <value> is required"),
        (
            &["w", "b", "--log", "l", "--reason", "x", "--require-bas"],
            "unknown option --require-bas",
        ),
        (
            &[
                "w",
                "b",
                "--log",
                "l",
                "--reason",
                "x",
                "--require-base",
                "yes",
            ],
            "--require-base takes no value",
        ),
        (
            &["w", "b", "--log", "l", "--reason", "x", "--reason", "y"],
            "--reason is given twice",
        ),
        (
            &[
                "w",
                "b",
                "--log",
                "l",
                "--reason",
                "x",
                "--review",
                "github:a=great",
            ],
            "unknown verdict great",
        ),
        (&["w", "b", "x", "--log", "l"], "unexpected argument x"),
    ];
    for (args, error) in refused {
        let e = parse(args).unwrap_err();
        assert!(e.starts_with(error), "{args:?}: {e}");
    }
}

/// Runs the `op_world` binary: exit code, stdout, stderr.
fn op_world(args: &[&dyn AsRef<std::ffi::OsStr>]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_op_world"))
        .args(args.iter().map(|a| a.as_ref()))
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

#[test]
fn the_command_line_keeps_a_log() {
    let t = Temp::new("cli");
    let world = world_copy(&t);
    let log = t.join("log");
    let w0 = world_id(&world);

    let (code, out, err) = op_world(&[
        &"baseline",
        &world,
        &"--log",
        &log,
        &"--reason",
        &"The ingest fixture.",
    ]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.starts_with("baseline record blake3:")
            && out.contains(&format!(" (log/00000000.json) for world {w0}")),
        "{out}"
    );
    let (code, _, err) = op_world(&[&"baseline", &world, &"--log", &log, &"--reason", &"Again."]);
    assert_eq!(code, 2);
    assert!(err.contains("already has records"), "{err}");

    let (code, out, err) = op_world(&[
        &"merge",
        &world,
        &facade(),
        &"--log",
        &log,
        &"--require-base",
        &"--reason",
        &"Test facade.",
        &"--review",
        &"github:reviewer=better:Checked.",
    ]);
    assert_eq!(code, 0, "{err}");
    let w1 = world_id(&world);
    assert!(
        out.contains(&format!(" (log/00000001.json): world {w0} -> {w1}")),
        "{out}"
    );

    // Planned on the world before the facade: refused with the flag.
    let stale = takedown_bundle(&world, w0, &t.join("stale"));
    let (code, _, err) = op_world(&[
        &"merge",
        &world,
        &stale,
        &"--log",
        &log,
        &"--takedown",
        &"--require-base",
        &"--reason",
        &"Owner request (test).",
    ]);
    assert_eq!(code, 2);
    assert_eq!(
        err.trim_end(),
        format!(
            "error: invalid: the store holds world {w1}, but this change set was planned on {w0}; plan the refresh again"
        )
    );
    let (code, _, err) = op_world(&[
        &"merge",
        &world,
        &stale,
        &"--log",
        &log,
        &"--previous",
        &t.join("0.json"),
        &"--reason",
        &"x",
    ]);
    assert_eq!(code, 2);
    assert!(
        err.contains("--previous cannot be combined with --log"),
        "{err}"
    );
    let (code, _, err) = op_world(&[
        &"baseline",
        &world,
        &"--record",
        &t.join("0.json"),
        &"--log",
        &log,
        &"--reason",
        &"x",
    ]);
    assert_eq!(code, 2);
    assert!(
        err.contains("--record cannot be combined with --log"),
        "{err}"
    );
    let (code, _, err) = op_world(&[]);
    assert_eq!(code, 2);
    assert!(
        err.contains("op_world merge <world-dir> <bundle-dir> (--previous <record> --record <file> --report <file> | --log <dir>)")
            && err.contains("[--require-base]")
            && err.contains("op_world baseline <world-dir> (--record <file> | --log <dir>)"),
        "{err}"
    );
    assert_eq!(check_log(&log), w1, "the refused merges wrote nothing");
}
