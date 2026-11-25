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
use op_world::store::MAX_FILE_BYTES;
use op_world::{ContentId, Error, Layer, ObjectKind, Store, TileIndex, WorldManifest, gate};

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

/// Something other than a lock file where a lock goes, such as a
/// directory, is refused with what it is, and kept. Found by review:
/// Windows reported "access denied" for a directory there, where Linux
/// said that the lock exists.
#[test]
fn a_directory_where_a_lock_goes_is_named() {
    let t = Temp::new("lock-dir");
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
    for (dir, name) in [(&world, STORE_LOCK), (&log_dir, LOG_LOCK)] {
        let lock = dir.join(name);
        std::fs::create_dir(&lock).unwrap();
        let e = args.run(&t.join("scratch"), gate::run).unwrap_err();
        assert_eq!(
            e,
            Error::Invalid(format!(
                "{} exists and is a directory, not a lock file; if no merge is running, remove it and merge again",
                lock.display()
            ))
        );
        assert!(lock.is_dir(), "what was there stays");
        std::fs::remove_dir(&lock).unwrap();
        assert!(!world.join(STORE_LOCK).exists() && !log_dir.join(LOG_LOCK).exists());
        assert_eq!(world_id(&world), w0);
        assert_eq!(common::files_under(&log_dir), before);
    }
}

/// The variables that make `empty_paths_in_a_child_process` run.
const CHILD_FACADE: &str = "STREETWIDE_TEST_EMPTY_PATHS_FACADE";
const CHILD_SCRATCH: &str = "STREETWIDE_TEST_EMPTY_PATHS_SCRATCH";

/// An empty path names the current directory, for the store and the log
/// alike, so a merge given one takes its lock there. Found by review: the
/// locks were taken only where `is_dir()` held, which it does not for an
/// empty path, so two merges given `""` from inside one store both merged.
/// The command line now refuses an empty path (an unset shell variable);
/// the library takes the lock in the current directory. A test cannot
/// change its own current directory (the tests share it), so the library
/// half runs in a child process (`empty_paths_in_a_child_process`).
#[test]
fn an_empty_path_does_not_skip_a_lock() {
    let t = Temp::new("empty");
    let world = world_copy(&t);
    // The store is its own log: the current directory serves for both.
    start_log(&world, &world);
    let w0 = world_id(&world);

    // The command line refuses "" for the store and for --log.
    std::fs::write(world.join(STORE_LOCK), b"").unwrap();
    std::fs::write(world.join(LOG_LOCK), b"").unwrap();
    for (store, log, said) in [
        ("", ".", "<world-dir> and <bundle-dir> must not be empty"),
        (".", "", "--log must not be empty"),
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_op_world"))
            .arg("merge")
            .arg(store)
            .arg(facade())
            .args([
                "--log",
                log,
                "--reason",
                "Test.",
                "--review",
                "github:a=better",
            ])
            .current_dir(&world)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2));
        assert_eq!(
            String::from_utf8_lossy(&out.stderr).trim_end(),
            format!("error: {said}")
        );
    }
    assert_eq!(world_id(&world), w0);
    std::fs::remove_file(world.join(STORE_LOCK)).unwrap();
    std::fs::remove_file(world.join(LOG_LOCK)).unwrap();

    // The library, in a child process whose current directory is the store.
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "empty_paths_in_a_child_process",
            "--nocapture",
            "--test-threads",
            "1",
        ])
        .env(CHILD_FACADE, facade())
        .env(CHILD_SCRATCH, t.join("scratch"))
        .current_dir(&world)
        .output()
        .unwrap();
    assert!(
        out.status.success() && String::from_utf8_lossy(&out.stdout).contains("empty paths: ok"),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let log = WorldLog::open(&world).unwrap();
    assert_eq!(log.records().len(), 2, "the last merge went through");
    assert_eq!(log.world_id(), world_id(&world));
    assert_ne!(world_id(&world), w0);
}

/// The child half of `an_empty_path_does_not_skip_a_lock`: does nothing
/// unless that test started it, in a store that is its own log.
#[test]
fn empty_paths_in_a_child_process() {
    let (Some(facade), Some(scratch)) = (
        std::env::var_os(CHILD_FACADE),
        std::env::var_os(CHILD_SCRATCH),
    ) else {
        return;
    };
    let (facade, scratch) = (PathBuf::from(facade), PathBuf::from(scratch));
    let here = Path::new("");
    let exists = |name: &str, what: &str| {
        Error::Invalid(format!(
            "{} exists: another merge is running on this {what}, or one stopped before it could remove the file; if no merge is running, delete the file and merge again",
            Path::new(".").join(name).display()
        ))
    };
    let previous = read("log/00000000.json");
    let w0 = world_id(Path::new("."));

    std::fs::write(STORE_LOCK, b"").unwrap();
    let e = merge_with(
        here,
        &facade,
        &previous,
        Decision::Merged,
        vec![better()],
        "Test.",
        &scratch,
        gate::run,
    )
    .err();
    assert_eq!(e, Some(exists(STORE_LOCK, "store")));
    let args = merge_args(
        here,
        &facade,
        MergeOutput::Log(PathBuf::new()),
        Decision::Merged,
        true,
    );
    assert_eq!(
        args.run(&scratch, gate::run).err(),
        Some(exists(STORE_LOCK, "store"))
    );
    std::fs::remove_file(STORE_LOCK).unwrap();
    std::fs::write(LOG_LOCK, b"").unwrap();
    assert_eq!(
        args.run(&scratch, gate::run).err(),
        Some(exists(LOG_LOCK, "world log"))
    );
    assert_eq!(world_id(Path::new(".")), w0);
    std::fs::remove_file(LOG_LOCK).unwrap();
    args.run(&scratch, gate::run).unwrap();
    assert!(Path::new("log/00000001.json").is_file());
    assert!(!Path::new(STORE_LOCK).exists() && !Path::new(LOG_LOCK).exists());
    println!("empty paths: ok");
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

/// A log that cannot take the record (here a file where the new world goes
/// that holds other bytes) is found before the store is written, so
/// nothing is written. Found by review: the store was written first, and
/// the merge then stopped with an undo hint.
#[test]
fn a_log_that_cannot_take_the_record_leaves_the_store_unchanged() {
    let t = Temp::new("refused-log");
    let world = world_copy(&t);
    let log_dir = t.join("log");
    start_log(&world, &log_dir);
    let w0 = world_id(&world);
    let after = gate::run(&world, &facade(), &t.join("candidate"))
        .unwrap()
        .candidate_world_id
        .unwrap();
    let clash = log_dir.join(format!("worlds/{}.json", after.hex()));
    std::fs::write(&clash, b"not this world").unwrap();
    let before = common::files_under(&log_dir);
    let objects = common::files_under(&world.join("objects"));
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
        Error::Invalid(format!(
            "the world log's worlds/{}.json holds other bytes",
            after.hex()
        ))
    );
    assert_eq!(world_id(&world), w0);
    assert_eq!(common::files_under(&world.join("objects")), objects);
    assert_eq!(common::files_under(&log_dir), before);
    assert!(!world.join(STORE_LOCK).exists() && !log_dir.join(LOG_LOCK).exists());

    std::fs::remove_file(&clash).unwrap();
    args.run(&t.join("scratch-2"), gate::run).unwrap();
    assert_eq!(check_log(&log_dir), after);
}

/// Makes `link` a link to the directory `target`: a junction on Windows,
/// which needs no privilege (`mklink /J`), else a symbolic link.
fn link_directory(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        let made = Command::new("cmd")
            .arg("/c")
            .arg("mklink")
            .arg("/J")
            .arg(link)
            .arg(target)
            .output()?;
        if made.status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(
                String::from_utf8_lossy(&made.stderr).into_owned(),
            ))
        }
    }
    #[cfg(not(windows))]
    {
        symlink(target, link)
    }
}

/// The reviewer's case: `worlds` in the log is a link (here a junction on
/// Windows) to a directory outside the log. Nothing was written through it,
/// but the merge wrote the store, then stopped and left the store to be
/// put back by hand. It is now refused before anything is written.
#[test]
fn a_log_with_a_linked_directory_leaves_the_store_unchanged() {
    let t = Temp::new("linked-worlds");
    let world = world_copy(&t);
    let log_dir = t.join("log");
    start_log(&world, &log_dir);
    let w0 = world_id(&world);
    let (worlds, outside) = (log_dir.join("worlds"), t.join("outside"));
    std::fs::rename(&worlds, &outside).unwrap();
    if let Err(e) = link_directory(&outside, &worlds) {
        println!("not run: cannot make a link to a directory here ({e})");
        return;
    }
    let after = gate::run(&world, &facade(), &t.join("candidate"))
        .unwrap()
        .candidate_world_id
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
        Error::Io(format!("worlds/{}.json: worlds is a link", after.hex()))
    );
    assert_eq!(world_id(&world), w0, "the store is as it was");
    assert_eq!(
        common::files_under(&outside).len(),
        1,
        "nothing written through the link"
    );
    // A link, not the directory it names (on Windows, a junction goes with
    // remove_dir).
    std::fs::remove_dir(&worlds)
        .or_else(|_| std::fs::remove_file(&worlds))
        .unwrap();
}

/// What makes writing a file into a log fail after the log was checked,
/// until it is dropped.
enum Blocked {
    /// Windows: the file's temporary path, kept open without sharing, so
    /// the merge cannot remove it.
    #[cfg(windows)]
    Open(Option<std::fs::File>, PathBuf),
    /// Elsewhere: the file's directory, made read-only.
    #[cfg(unix)]
    ReadOnly(PathBuf),
}

impl Drop for Blocked {
    fn drop(&mut self) {
        match self {
            #[cfg(windows)]
            Blocked::Open(file, tmp) => {
                drop(file.take());
                let _ = std::fs::remove_file(tmp);
            }
            #[cfg(unix)]
            Blocked::ReadOnly(dir) => {
                use std::os::unix::fs::PermissionsExt as _;
                let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755));
            }
        }
    }
}

/// Makes a merge fail to write `path`, a file it writes into a log
/// through `<path>.tmp`, after the log has been checked; None where this
/// process cannot be stopped that way (a superuser ignores permissions).
fn block_writes(path: &Path) -> Option<Blocked> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(0)
            .open(&tmp)
            .ok()?;
        Some(Blocked::Open(Some(file), tmp))
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = path.parent()?.to_path_buf();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).ok()?;
        let blocked = Blocked::ReadOnly(dir.clone());
        let probe = dir.join("probe");
        if std::fs::write(&probe, b"").is_ok() {
            let _ = std::fs::remove_file(&probe);
            return None;
        }
        Some(blocked)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        None
    }
}

/// A write into the log that fails after the store was written (the disk
/// is full, the file is in use) leaves the store ahead of the log: the
/// error says so and how to undo the merge, naming a world from the log
/// only where the log keeps it (it is optional there, as for a baseline
/// record written with --record).
#[test]
fn a_log_that_fails_to_be_written_says_how_to_undo_the_merge() {
    for keeps in [true, false] {
        let t = Temp::new(&format!("undo-{keeps}"));
        let world = world_copy(&t);
        let log_dir = t.join("log");
        start_log(&world, &log_dir);
        let w0 = world_id(&world);
        let kept = log_dir.join(format!("worlds/{}.json", w0.hex()));
        if !keeps {
            std::fs::remove_file(&kept).unwrap();
        }
        let after = gate::run(&world, &facade(), &t.join("candidate"))
            .unwrap()
            .candidate_world_id
            .unwrap();
        let args = merge_args(
            &world,
            &facade(),
            MergeOutput::Log(log_dir.clone()),
            Decision::Merged,
            true,
        );
        let Some(blocked) = block_writes(&log_dir.join(format!("worlds/{}.json", after.hex())))
        else {
            println!("not run: this process writes into a read-only directory");
            return;
        };
        let e = args.run(&t.join("scratch-1"), gate::run).unwrap_err();
        drop(blocked);
        let undo = if keeps {
            format!(
                "copy the log's worlds/{}.json over the store's world.json",
                w0.hex()
            )
        } else {
            format!("put back the store's world.json of world {w0}")
        };
        let message = e.to_string();
        assert!(
            message.starts_with(&format!(
                "I/O error: the store now holds world {after}, but the world log was not updated (I/O error: worlds/{}.json: ",
                after.hex()
            )) && message.ends_with(&format!("); to undo the merge, {undo}")),
            "{message}"
        );
        assert_eq!(world_id(&world), after);
        assert_eq!(WorldLog::open(&log_dir).unwrap().records().len(), 1);
        assert!(!world.join(STORE_LOCK).exists() && !log_dir.join(LOG_LOCK).exists());

        // Undo as the message says, then merge again.
        let from = if keeps {
            kept.clone()
        } else {
            common::repo_root().join("fixtures/world/world.json")
        };
        std::fs::copy(from, world.join("world.json")).unwrap();
        assert_eq!(world_id(&world), w0);
        args.run(&t.join("scratch-2"), gate::run).unwrap();
        let log = WorldLog::open(&log_dir).unwrap();
        assert_eq!((log.records().len(), log.world_id()), (2, after));
    }
}

/// A merge that stopped after it wrote the store and before it appended its
/// record (killed, or the machine stopped) leaves the store ahead of the
/// log. The next merge says so, and how to undo it, before anything else,
/// --require-base included. Found by review: the message said only that
/// the store and the log differ, and with --require-base that the plan was
/// stale, so planning again failed as well.
#[test]
fn a_merge_that_stopped_before_its_record_is_named() {
    let t = Temp::new("stopped");
    let world = world_copy(&t);
    let log_dir = t.join("log");
    start_log(&world, &log_dir);
    let w0 = world_id(&world);
    let args = merge_args(
        &world,
        &facade(),
        MergeOutput::Log(log_dir.clone()),
        Decision::Merged,
        true,
    );
    args.run(&t.join("scratch-1"), gate::run).unwrap();
    let w1 = world_id(&world);
    // What a merge killed just before it wrote its record leaves behind.
    std::fs::remove_file(log_dir.join("log/00000001.json")).unwrap();
    let stopped = format!(
        "the store holds world {w1}, but the log's last record ends at {w0}, and the log keeps world {w1} in worlds/ without a record that names it: a merge stopped after it wrote the store and before it appended its record. To undo that merge, copy the log's worlds/{}.json over the store's world.json, then merge again",
        w0.hex()
    );
    // The same set again (planned on w0), and the next one (planned on w1).
    let takedown = takedown_bundle(&world, w1, &t.join("takedown"));
    let next = merge_args(
        &world,
        &takedown,
        MergeOutput::Log(log_dir.clone()),
        Decision::Takedown,
        true,
    );
    for (n, a) in [&args, &next].into_iter().enumerate() {
        let e = a
            .run(&t.join(&format!("scratch-{n}")), gate::run)
            .unwrap_err();
        assert_eq!(e, Error::Invalid(stopped.clone()));
    }
    // Without its trace in worlds/, the message still says what to do.
    let trace = log_dir.join(format!("worlds/{}.json", w1.hex()));
    std::fs::rename(&trace, t.join("aside.json")).unwrap();
    let e = next.run(&t.join("scratch-3"), gate::run).unwrap_err();
    assert_eq!(
        e,
        Error::Invalid(format!(
            "the store holds world {w1}, but the log's last record ends at {w0}. If a merge stopped after it wrote the store and before it appended its record, copy the log's worlds/{}.json over the store's world.json and merge again; otherwise check that --log is this store's log",
            w0.hex()
        ))
    );
    std::fs::rename(t.join("aside.json"), &trace).unwrap();
    assert_eq!(world_id(&world), w1);

    // Undo as the message says, then merge again.
    std::fs::copy(
        log_dir.join(format!("worlds/{}.json", w0.hex())),
        world.join("world.json"),
    )
    .unwrap();
    args.run(&t.join("scratch-4"), gate::run).unwrap();
    assert_eq!(check_log(&log_dir), w1);
}

/// A candidate world that changed after the gate built it (the scratch
/// directory is in a temporary directory that others may write) is refused
/// before anything is written. Found by review: the store was written with
/// whatever world was there, and only then compared with the gate's.
#[test]
fn a_candidate_that_changed_after_the_gate_is_not_written() {
    let t = Temp::new("swapped");
    let world = world_copy(&t);
    let previous = baseline(&world, "The ingest fixture.")
        .unwrap()
        .to_bytes()
        .unwrap();
    let candidate = gate::run(&world, &facade(), &t.join("probe"))
        .unwrap()
        .candidate_world_id
        .unwrap();
    let world_json = read(world.join("world.json"));
    let objects = common::files_under(&world.join("objects"));
    let swapping = |w: &Path, b: &Path, c: &Path| -> op_world::Result<CheckReport> {
        let report = gate::run(w, b, c)?;
        let mut other = WorldManifest::from_bytes(&read(c.join("world.json")))?;
        other.name.push_str("-swapped");
        std::fs::write(c.join("world.json"), other.to_bytes()?).unwrap();
        Ok(report)
    };
    let scratch = t.join("scratch");
    let e = merge_with(
        &world,
        &facade(),
        &previous,
        Decision::Merged,
        vec![better()],
        "Test.",
        &scratch,
        swapping,
    )
    .err();
    assert_eq!(
        e,
        Some(Error::Invalid(format!(
            "the candidate world in {} is not the gate's candidate {candidate}; it changed after the gate built it, so nothing was written",
            scratch.join("candidate").display()
        )))
    );
    assert_eq!(read(world.join("world.json")), world_json);
    assert_eq!(common::files_under(&world.join("objects")), objects);
    assert!(!world.join(STORE_LOCK).exists());
}

/// The merge makes its scratch directory itself. Something there that it
/// cannot remove (in a shared temporary directory, a directory another
/// user made) is refused rather than written into; here, a file.
#[test]
fn a_scratch_directory_the_merge_cannot_make_is_refused() {
    let t = Temp::new("scratch");
    let world = world_copy(&t);
    let log_dir = t.join("log");
    start_log(&world, &log_dir);
    let w0 = world_id(&world);
    let scratch = t.join("scratch");
    std::fs::write(&scratch, b"not a directory").unwrap();
    let e = merge_args(
        &world,
        &facade(),
        MergeOutput::Log(log_dir.clone()),
        Decision::Merged,
        true,
    )
    .run(&scratch, gate::run)
    .unwrap_err();
    assert!(
        e.to_string().starts_with(&format!(
            "I/O error: cannot make the scratch directory {} anew: ",
            scratch.display()
        )),
        "{e}"
    );
    assert_eq!(world_id(&world), w0);
    assert_eq!(read(&scratch), b"not a directory");
    assert_eq!(WorldLog::open(&log_dir).unwrap().records().len(), 1);
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

/// Makes `link` a symbolic link to `target` (Windows allows it only with
/// developer mode or the right to make links).
#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}

#[cfg(not(any(unix, windows)))]
fn symlink(_: &Path, _: &Path) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}

/// Makes a FIFO at `path`, as the store regression test does. A reader
/// that opens it waits forever, so a test that reaches one hangs.
#[cfg(unix)]
fn mkfifo(path: &Path) {
    let made = Command::new("mkfifo").arg(path).status().unwrap();
    assert!(made.success(), "mkfifo failed");
}

/// Moves `path` (in the log in `dir`) out of the log, puts `make(path,
/// moved)` in its place, returns what opening the log says, and puts `path`
/// back.
fn refusal_with(dir: &Path, path: &Path, make: impl FnOnce(&Path, &Path)) -> String {
    let name = path.file_name().unwrap().to_string_lossy();
    let aside = dir.parent().unwrap().join(format!("aside-{name}"));
    std::fs::rename(path, &aside).unwrap();
    make(path, &aside);
    let said = match WorldLog::open(dir) {
        Ok(_) => panic!("{} opens with {} replaced", dir.display(), path.display()),
        Err(e) => e.to_string(),
    };
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(path).unwrap(),
        Ok(_) => std::fs::remove_file(path).unwrap(),
        Err(_) => {}
    }
    std::fs::rename(&aside, path).unwrap();
    WorldLog::open(dir).unwrap();
    said
}

/// A world log can come from elsewhere (git keeps links), so it may hold
/// only directories and regular files of at most MAX_FILE_BYTES. Anything
/// else is refused, never followed or opened. Found by review: records and
/// change sets were read whole with `fs::read`, which followed links,
/// waited forever on a FIFO and took any size.
#[test]
fn a_log_holds_only_directories_and_regular_files() {
    let t = Temp::new("kinds");
    let world = world_copy(&t);
    let dir = t.join("log");
    start_log(&world, &dir);
    merge_args(
        &world,
        &facade(),
        MergeOutput::Log(dir.clone()),
        Decision::Merged,
        true,
    )
    .run(&t.join("scratch"), gate::run)
    .unwrap();
    let record = dir.join("log/00000001.json");
    let hex = ContentId::of(&read(facade().join("change.json"))).hex();
    let change = dir.join(format!("changes/{hex}/change.json"));
    let not_a_record = "invalid: the world log's log/00000001.json is not a regular file; records are regular files, not links, directories or pipes";
    let not_a_dir = "invalid: the world log's log is not a directory; a world log holds its records in a directory log/, not in a link to one";
    let make_dir = |p: &Path, _: &Path| std::fs::create_dir(p).unwrap();
    let make_file = |p: &Path, _: &Path| std::fs::write(p, b"{}").unwrap();

    assert_eq!(refusal_with(&dir, &record, make_dir), not_a_record);
    assert_eq!(refusal_with(&dir, &dir.join("log"), make_file), not_a_dir);
    assert_eq!(
        refusal_with(&dir, &change, make_dir),
        format!("I/O error: changes/{hex}/change.json: not a regular file")
    );
    assert_eq!(
        refusal_with(&dir, &dir.join("changes"), make_file),
        format!("I/O error: changes/{hex}/change.json: changes is not a directory")
    );
    // Longer than any file a store read takes: refused unread (the file
    // is sparse where the filesystem allows).
    let oversized = |p: &Path, _: &Path| {
        std::fs::File::create(p)
            .and_then(|f| f.set_len(MAX_FILE_BYTES + 1))
            .unwrap();
    };
    assert_eq!(
        refusal_with(&dir, &record, oversized),
        "I/O error: log/00000001.json: larger than 67108864 bytes"
    );

    // Links to valid copies: a reader that followed them would accept the
    // log.
    let probe = t.join("probe");
    match symlink(&record, &probe) {
        Err(e) => println!("links not checked: cannot make a link here ({e})"),
        Ok(()) => {
            std::fs::remove_file(&probe).unwrap();
            let link_to_copy = |p: &Path, aside: &Path| {
                let name = p.file_name().unwrap().to_string_lossy();
                let copy = t.join(&format!("copy-{name}"));
                if aside.is_dir() {
                    common::files_under(aside).iter().for_each(|f| {
                        let to = copy.join(f);
                        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
                        std::fs::copy(aside.join(f), to).unwrap();
                    });
                } else {
                    std::fs::copy(aside, &copy).unwrap();
                }
                symlink(&copy, p).unwrap();
            };
            assert_eq!(refusal_with(&dir, &record, link_to_copy), not_a_record);
            assert_eq!(
                refusal_with(&dir, &dir.join("log"), link_to_copy),
                not_a_dir
            );
            assert_eq!(
                refusal_with(&dir, &change, link_to_copy),
                format!("I/O error: changes/{hex}/change.json: not a regular file")
            );
            assert_eq!(
                refusal_with(&dir, change.parent().unwrap(), link_to_copy),
                format!("I/O error: changes/{hex}/change.json: changes/{hex} is a link")
            );
        }
    }

    // FIFOs: opening one would wait forever.
    #[cfg(unix)]
    {
        let make_fifo = |p: &Path, _: &Path| mkfifo(p);
        assert_eq!(refusal_with(&dir, &record, make_fifo), not_a_record);
        assert_eq!(
            refusal_with(&dir, &change, make_fifo),
            format!("I/O error: changes/{hex}/change.json: not a regular file")
        );
    }
}

/// `--previous` and the bundle's `change.json` for `--require-base` are
/// read like every store read, and the error names what was read. Found by
/// review: both were read whole with `fs::read`.
#[test]
fn the_previous_record_and_the_bundle_are_read_as_regular_files() {
    let t = Temp::new("previous");
    let world = world_copy(&t);
    let previous = t.join("previous");
    std::fs::create_dir(&previous).unwrap();
    let e = merge_args(
        &world,
        &facade(),
        MergeOutput::Files {
            previous: previous.clone(),
            record: t.join("1.json"),
            report: t.join("1-report.json"),
        },
        Decision::Merged,
        false,
    )
    .run(&t.join("scratch"), gate::run)
    .unwrap_err();
    assert_eq!(
        e.to_string(),
        format!(
            "I/O error: --previous {}: not a regular file",
            previous.display()
        )
    );
    let bundle = t.join("bundle");
    std::fs::create_dir_all(bundle.join("change.json")).unwrap();
    assert_eq!(
        require_base(&world, &bundle).unwrap_err().to_string(),
        "I/O error: change.json: not a regular file"
    );
    std::fs::remove_dir(bundle.join("change.json")).unwrap();
    match symlink(&facade().join("change.json"), &bundle.join("change.json")) {
        Err(e) => println!("links not checked: cannot make a link here ({e})"),
        Ok(()) => assert_eq!(
            require_base(&world, &bundle).unwrap_err().to_string(),
            "I/O error: change.json: not a regular file"
        ),
    }
}

/// What a `--log` merge does when something is at the temporary path it
/// writes the new world through (`worlds/<hex>.json.tmp`): a file left by a
/// crash is replaced, a link is removed rather than written through, a
/// FIFO is removed rather than opened (which would wait forever), and a
/// directory, which cannot be removed that way, stops the merge before it
/// writes anything. Found by review: the temporary file was opened with
/// `fs::write`, which follows links.
#[test]
fn a_merge_writes_through_no_link_and_into_no_fifo() {
    let mut cases = vec!["file", "directory"];
    if cfg!(unix) {
        cases.extend(["link", "fifo"]);
    }
    for case in cases {
        let t = Temp::new(&format!("tmp-{case}"));
        let world = world_copy(&t);
        let dir = t.join("log");
        start_log(&world, &dir);
        let w0 = world_id(&world);
        let w1 = gate::run(&world, &facade(), &t.join("candidate"))
            .unwrap()
            .candidate_world_id
            .unwrap();
        let tmp = dir.join(format!("worlds/{}.json.tmp", w1.hex()));
        let outside = t.join("outside.txt");
        std::fs::write(&outside, b"not the log's").unwrap();
        match case {
            "file" => std::fs::write(&tmp, b"left by a crash").unwrap(),
            "directory" => std::fs::create_dir(&tmp).unwrap(),
            #[cfg(unix)]
            "link" => symlink(&outside, &tmp).unwrap(),
            #[cfg(unix)]
            "fifo" => mkfifo(&tmp),
            _ => unreachable!(),
        }
        let result = merge_args(
            &world,
            &facade(),
            MergeOutput::Log(dir.clone()),
            Decision::Merged,
            true,
        )
        .run(&t.join("scratch"), gate::run);
        assert_eq!(read(&outside), b"not the log's", "{case}");
        if case == "directory" {
            assert_eq!(
                result.unwrap_err(),
                Error::Invalid(format!(
                    "the world log has a directory at worlds/{0}.json.tmp, where a merge writes worlds/{0}.json through a temporary file",
                    w1.hex()
                ))
            );
            assert_eq!(world_id(&world), w0, "nothing was written");
            assert_eq!(WorldLog::open(&dir).unwrap().records().len(), 1);
        } else {
            result.unwrap();
            assert_eq!(world_id(&world), w1, "{case}");
            assert!(std::fs::symlink_metadata(&tmp).is_err(), "{case}");
            assert_eq!(check_log(&dir), w1, "{case}");
        }
    }
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

    let refused: [(&[&str], &str); 15] = [
        // The directories come first: an option there is a usage error, as
        // in `op_replay merge`. Found while writing the merge-args fuzz
        // target: `--log` was taken as the world directory.
        (
            &["--log", "l", "w", "b", "--reason", "x"],
            "merge takes <world-dir> <bundle-dir> (",
        ),
        // No path may be empty (an unset shell variable): an empty path
        // names the current directory. Found by review: the locks were
        // skipped for it (see `an_empty_path_does_not_skip_a_lock`).
        (
            &["", "b", "--log", "l", "--reason", "x"],
            "<world-dir> and <bundle-dir> must not be empty",
        ),
        (
            &["w", "", "--log", "l", "--reason", "x"],
            "<world-dir> and <bundle-dir> must not be empty",
        ),
        (
            &["w", "b", "--log", "", "--reason", "x"],
            "--log must not be empty",
        ),
        (
            &[
                "w",
                "b",
                "--previous",
                "p",
                "--record",
                "",
                "--report",
                "g",
                "--reason",
                "x",
            ],
            "--record must not be empty",
        ),
        (
            &["w", "--takedown", "--log", "l", "--reason", "x"],
            "merge takes <world-dir> <bundle-dir> (",
        ),
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
