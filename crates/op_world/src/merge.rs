//! Merging change sets into a world store and writing world-log records
//! (RFC 0002 section 6).
//!
//! These functions work on local directories; where the canonical store and
//! the world log are hosted is decided separately (ADR-0010). A merge never
//! leaves the store half-changed by a refused change set: the gate runs and
//! the record is built and validated first, then the new objects are copied
//! in, and `world.json` is written last.
//!
//! One merge at a time: a merge holds a lock file in the store
//! ([`STORE_LOCK`]) and, with `--log`, one in the log ([`LOG_LOCK`]), from
//! before it reads them until it has written them. A second merge finds the
//! file and stops before it reads anything. Two merges that both started
//! from the same world would otherwise leave the store and the log naming
//! different worlds.
//!
//! - [`merge`] runs `op_world`'s gate (G1 to G8). [`merge_with`] takes the
//!   gate as a function, so `op_replay merge` can run the complete gate (G1
//!   to G10, ADR-0009) and merge change sets that touch colliders.
//! - [`WorldLog`] keeps a world log in a local directory.
//! - [`MergeArgs`] is what `op_world merge` and `op_replay merge` share: the
//!   same options, checks and outputs, with a different gate.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::contrib::{
    CONTRIBUTION_CONTRACT_VERSION, ChangeSet, CheckReport, Decision, MergeRecord, Outcome, Review,
    Verdict, verify_log,
};
use crate::gate;
use crate::id::ContentId;
use crate::manifest::AssetManifest;
use crate::store::{ObjectKind, Store};
use crate::{Error, Result};

/// The lock file a merge holds in the world store's directory.
pub const STORE_LOCK: &str = "merge.lock";
/// The lock file a merge with `--log` holds in the log's directory (next to
/// `log/`, which holds only records).
pub const LOG_LOCK: &str = "log.lock";

/// A lock file, created only if it does not exist and removed when the
/// lock is dropped.
struct Lock(PathBuf);

impl Lock {
    /// Takes the lock `name` in `dir`, or says why it cannot. A lock that
    /// exists is never removed here: it belongs to another merge, or to one
    /// that stopped before it could remove it.
    fn take(dir: &Path, name: &str, what: &str) -> Result<Lock> {
        let path = dir.join(name);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => Ok(Lock(path)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(Error::Invalid(format!(
                "{} exists: another merge is running on this {what}, or one stopped before it could remove the file; if no merge is running, delete the file and merge again",
                path.display()
            ))),
            Err(e) => Err(Error::Io(format!("{}: {e}", path.display()))),
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// The first record of a world log: the world in `world_dir` as it is.
pub fn baseline(world_dir: &Path, reasons: &str) -> Result<MergeRecord> {
    let (world_id, _) = Store::new(world_dir).read_world()?;
    let record = MergeRecord {
        format: MergeRecord::FORMAT.into(),
        contract_version: CONTRIBUTION_CONTRACT_VERSION.into(),
        decision: Decision::Baseline,
        change_id: None,
        world_before: None,
        world_after: world_id,
        check_report_id: None,
        reviews: Vec::new(),
        reasons: reasons.into(),
        previous_record_id: None,
    };
    record.to_bytes()?;
    Ok(record)
}

/// The result of a merge.
pub struct Merged {
    pub record: MergeRecord,
    pub record_bytes: Vec<u8>,
    pub report_bytes: Vec<u8>,
}

/// Merges (or, with [`Decision::Takedown`], takes down) the change set in
/// `bundle_dir` into the world store at `world_dir`, with `op_world`'s gate
/// ([`gate::run`], G1 to G8). A change set that touches colliders needs the
/// complete gate: see [`merge_with`].
///
/// - `previous_record` is the bytes of the log's last record; the store's
///   world must be that record's `world_after`.
/// - A merge needs the gate to pass and the reviews of RFC 0002 section 5
///   (at least one `better`, no `blocked`). A takedown needs only the
///   format and lineage checks (G1, G2) to pass.
/// - `scratch_dir` must not exist; the gate's candidate world is built
///   there.
pub fn merge(
    world_dir: &Path,
    bundle_dir: &Path,
    previous_record: &[u8],
    decision: Decision,
    reviews: Vec<Review>,
    reasons: &str,
    scratch_dir: &Path,
) -> Result<Merged> {
    merge_with(
        world_dir,
        bundle_dir,
        previous_record,
        decision,
        reviews,
        reasons,
        scratch_dir,
        gate::run,
    )
}

/// [`merge`] with the gate passed in: `gate(world_dir, bundle_dir,
/// candidate_dir)` must check the change set and build its candidate world
/// in `candidate_dir`, as [`gate::run`] does. The merge record's
/// `check_report_id` names the report it returns. `op_replay merge` passes
/// the complete gate (G1 to G10, ADR-0009).
///
/// The merge holds the store's lock ([`STORE_LOCK`]) from before it reads
/// the store until it has written `world.json`, and refuses if the lock is
/// taken. Before writing, it reads the store's world again and refuses if
/// it moved, which only a tool that ignores the lock can do.
#[allow(
    clippy::too_many_arguments,
    reason = "the arguments of merge() plus the gate"
)]
pub fn merge_with(
    world_dir: &Path,
    bundle_dir: &Path,
    previous_record: &[u8],
    decision: Decision,
    mut reviews: Vec<Review>,
    reasons: &str,
    scratch_dir: &Path,
    gate: impl FnOnce(&Path, &Path, &Path) -> Result<CheckReport>,
) -> Result<Merged> {
    if decision == Decision::Baseline {
        return Err(Error::Invalid("use baseline() to start a world log".into()));
    }
    let previous = MergeRecord::from_bytes(previous_record)?;
    let previous_id = ContentId::of(previous_record);
    let world = Store::new(world_dir);
    // A store that does not exist is reported by `read_world`.
    let _lock = if world_dir.is_dir() {
        Some(Lock::take(world_dir, STORE_LOCK, "store")?)
    } else {
        None
    };
    let (world_id, _) = world.read_world()?;
    if previous.world_after != world_id {
        return Err(Error::Invalid(format!(
            "the store holds world {world_id}, but the log's last record ends at {}",
            previous.world_after
        )));
    }

    let candidate_dir = scratch_dir.join("candidate");
    let report = gate(world_dir, bundle_dir, &candidate_dir)?;
    let ok = match decision {
        Decision::Takedown => report
            .checks
            .iter()
            .take(2)
            .all(|c| c.outcome == Outcome::Pass),
        _ => report.passed,
    };
    if !ok {
        return Err(Error::Invalid(format!(
            "change {} does not pass the gate: {}",
            report.change_id,
            failing(&report)
        )));
    }
    let candidate_id = report
        .candidate_world_id
        .ok_or_else(|| Error::Invalid("the gate built no candidate world".into()))?;
    let report_bytes = report.to_bytes()?;

    reviews.sort_by(|a, b| a.reviewer.cmp(&b.reviewer));
    let record = MergeRecord {
        format: MergeRecord::FORMAT.into(),
        contract_version: CONTRIBUTION_CONTRACT_VERSION.into(),
        decision,
        change_id: Some(report.change_id),
        world_before: Some(world_id),
        world_after: candidate_id,
        check_report_id: Some(ContentId::of(&report_bytes)),
        reviews,
        reasons: reasons.into(),
        previous_record_id: Some(previous_id),
    };
    // Validates the reviews before anything is written.
    let record_bytes = record.to_bytes()?;
    let (now, _) = world.read_world()?;
    if now != world_id {
        return Err(Error::Invalid(format!(
            "the store changed during the merge, from world {world_id} to {now}, so nothing was written"
        )));
    }

    // Copy the new objects in: the change set's manifests and meshes, then
    // the candidate's tile indexes; `world.json` last.
    let change = ChangeSet::from_bytes(&std::fs::read(bundle_dir.join("change.json"))?)?;
    let bundle = Store::layered(bundle_dir, vec![world_dir.to_path_buf()]);
    for c in &change.changes {
        let Some(manifest_id) = c.manifest_id else {
            continue;
        };
        let bytes = bundle.read_object(&manifest_id, ObjectKind::Json)?;
        let manifest = AssetManifest::from_bytes(&bytes)?;
        world.write_object(
            &bundle.read_object(&manifest.content_id, ObjectKind::Glb)?,
            ObjectKind::Glb,
        )?;
        world.write_object(&bytes, ObjectKind::Json)?;
    }
    let candidate = Store::layered(
        &candidate_dir,
        vec![bundle_dir.to_path_buf(), world_dir.to_path_buf()],
    );
    let (_, candidate_world) = candidate.read_world()?;
    for t in &candidate_world.tiles {
        if !world.has_object(&t.tile_id, ObjectKind::Json) {
            world.write_object(
                &candidate.read_object(&t.tile_id, ObjectKind::Json)?,
                ObjectKind::Json,
            )?;
        }
    }
    let written = world.write_world(&candidate_world)?;
    if written != candidate_id {
        return Err(Error::Invalid(
            "the written world differs from the gate's candidate".into(),
        ));
    }
    Ok(Merged {
        record,
        record_bytes,
        report_bytes,
    })
}

fn failing(report: &CheckReport) -> String {
    report
        .checks
        .iter()
        .filter(|c| matches!(c.outcome, Outcome::Fail | Outcome::NotRun))
        .map(|c| format!("{} {}", c.id, c.name))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `--require-base`: refuses unless the change set in `bundle_dir` was
/// planned on the world in `world_dir`, its `base_world_id`.
///
/// RFC 0002 lets a change set whose parents are all still current merge on
/// a newer world. A baseline refresh is planned as several change sets,
/// each on the world the previous one produces, so a merge in between makes
/// the rest of the plan stale: nothing would re-check a facade merged onto a
/// shell that a later set replaces. The check is opt-in. Making it
/// mandatory for refreshes is amendment A7, which is not adopted.
pub fn require_base(world_dir: &Path, bundle_dir: &Path) -> Result<()> {
    let (world_id, _) = Store::new(world_dir).read_world()?;
    let bytes = fs::read(bundle_dir.join("change.json"))
        .map_err(|e| Error::Io(format!("change.json: {e}")))?;
    let change = ChangeSet::from_bytes(&bytes)?;
    if change.base_world_id != world_id {
        return Err(Error::Invalid(format!(
            "the store holds world {world_id}, but this change set was planned on {}; plan the refresh again",
            change.base_world_id
        )));
    }
    Ok(())
}

/// The directory of merge records in a [`WorldLog`].
const RECORDS_DIR: &str = "log";
/// The highest record number with 8 digits.
const MAX_RECORD: usize = 99_999_999;

/// The file name of record `n`: 8 digits, from `00000000.json`.
fn record_name(n: usize) -> String {
    format!("{n:08}.json")
}

/// `dir` joined with `parts`, and the same path as POSIX text for messages.
fn log_path(dir: &Path, parts: &[&str]) -> (PathBuf, String) {
    let mut path = dir.to_path_buf();
    for p in parts {
        path.push(p);
    }
    (path, parts.join("/"))
}

/// Writes through a temporary file next to `path`, then renames it. A
/// temporary file left in `log/` by a crash makes [`WorldLog::open`] refuse
/// the log until someone looks at it.
fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

/// Writes a file named by its content, unless it already holds exactly
/// these bytes.
fn write_content(dir: &Path, parts: &[&str], bytes: &[u8]) -> Result<String> {
    let (path, rel) = log_path(dir, parts);
    match fs::read(&path) {
        Ok(existing) if existing == bytes => return Ok(rel),
        Ok(_) => {
            return Err(Error::Invalid(format!(
                "the world log's {rel} holds other bytes"
            )));
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(Error::Io(format!("{rel}: {e}"))),
    }
    write_atomically(&path, bytes).map_err(|e| Error::Io(format!("{rel}: {e}")))?;
    Ok(rel)
}

/// A world log kept in a local directory:
///
/// ```text
/// log/00000000.json           merge records in log order; the first is the baseline
/// changes/<hex>/change.json   the change set of each merged or takedown record
/// reports/<hex>.json          the gate's report on that change set
/// worlds/<hex>.json           the world after each record
/// ```
///
/// `<hex>` is a content id without its `blake3:` prefix, because `:` is not
/// allowed in Windows file names. A local store keeps only its current
/// `world.json`; `worlds/` keeps every world the log names.
///
/// This is the layout of ADR-0010 option A with the two changes that
/// ADR-0020 recommends for a local log. Both ADRs are Proposed, so the
/// layout is a default until the maintainer decides.
pub struct WorldLog {
    dir: PathBuf,
    records: Vec<(ContentId, MergeRecord)>,
    head: Vec<u8>,
}

impl WorldLog {
    /// Opens the world log in `dir`. The records in `log/` must be numbered
    /// from `00000000.json` with no gaps and no other files, and must form a
    /// chain ([`verify_log`]). The change set of every merged or takedown
    /// record must be in `changes/<hex>/change.json` and hash to its id, as
    /// the refresh planner requires, so that a merge never extends a log
    /// that the planner refuses.
    pub fn open(dir: &Path) -> Result<WorldLog> {
        let records_dir = dir.join(RECORDS_DIR);
        let no_records = || {
            Error::Invalid(
                "the world log has no records; start it with op_world baseline <world-dir> --log <dir> --reason <text>"
                    .into(),
            )
        };
        let entries = match fs::read_dir(&records_dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(no_records()),
            Err(e) => return Err(Error::Io(format!("{RECORDS_DIR}/: {e}"))),
        };
        let mut names = Vec::new();
        for entry in entries {
            names.push(entry?.file_name().to_string_lossy().into_owned());
        }
        if names.is_empty() {
            return Err(no_records());
        }
        names.sort();
        let mut records = Vec::with_capacity(names.len());
        let mut head = Vec::new();
        for (n, name) in names.iter().enumerate() {
            let expected = record_name(n);
            if *name != expected {
                return Err(Error::Invalid(format!(
                    "the world log has {RECORDS_DIR}/{name} where {RECORDS_DIR}/{expected} should be; records are numbered from 00000000, with no gaps and no other files"
                )));
            }
            let bytes = fs::read(records_dir.join(name))
                .map_err(|e| Error::Io(format!("{RECORDS_DIR}/{name}: {e}")))?;
            let record = MergeRecord::from_bytes(&bytes)
                .map_err(|e| Error::Invalid(format!("{RECORDS_DIR}/{name}: {e}")))?;
            records.push((ContentId::of(&bytes), record));
            head = bytes;
        }
        verify_log(&records)?;
        for (n, (_, record)) in records.iter().enumerate() {
            let Some(change_id) = record.change_id else {
                continue;
            };
            let hex = change_id.hex();
            let (path, rel) = log_path(dir, &["changes", &hex, "change.json"]);
            match fs::read(&path) {
                Ok(bytes) if ContentId::of(&bytes) == change_id => {
                    ChangeSet::from_bytes(&bytes)
                        .map_err(|e| Error::Invalid(format!("{rel}: {e}")))?;
                }
                _ => {
                    return Err(Error::Invalid(format!(
                        "the world log names change set {change_id} ({RECORDS_DIR}/{}) but {rel} is missing or has another id",
                        record_name(n)
                    )));
                }
            }
        }
        Ok(WorldLog {
            dir: dir.to_path_buf(),
            records,
            head,
        })
    }

    /// Starts a world log in `dir` with its baseline record: writes
    /// `worlds/<hex>.json` (`world` is the bytes of the world manifest the
    /// record names), then `log/00000000.json`. `log/` must be missing or
    /// empty.
    pub fn start(dir: &Path, record: &MergeRecord, world: &[u8]) -> Result<WorldLog> {
        if record.decision != Decision::Baseline {
            return Err(Error::Invalid(
                "a world log starts with a baseline record".into(),
            ));
        }
        if ContentId::of(world) != record.world_after {
            return Err(Error::Invalid(format!(
                "the world manifest is not world {}, which the baseline record names",
                record.world_after
            )));
        }
        let records_dir = dir.join(RECORDS_DIR);
        if fs::read_dir(&records_dir).is_ok_and(|mut entries| entries.next().is_some()) {
            return Err(Error::Invalid(format!(
                "the world log already has records in {RECORDS_DIR}/; a baseline record starts a new log"
            )));
        }
        let bytes = record.to_bytes()?;
        write_content(
            dir,
            &["worlds", &format!("{}.json", record.world_after.hex())],
            world,
        )?;
        let name = record_name(0);
        write_atomically(&records_dir.join(&name), &bytes)
            .map_err(|e| Error::Io(format!("{RECORDS_DIR}/{name}: {e}")))?;
        Ok(WorldLog {
            dir: dir.to_path_buf(),
            records: vec![(ContentId::of(&bytes), record.clone())],
            head: bytes,
        })
    }

    /// The records with their ids, in log order.
    pub fn records(&self) -> &[(ContentId, MergeRecord)] {
        &self.records
    }

    /// The bytes of the last record.
    pub fn head(&self) -> &[u8] {
        &self.head
    }

    /// The current world: the last record's `world_after`.
    pub fn world_id(&self) -> ContentId {
        self.records
            .last()
            .map(|(_, r)| r.world_after)
            .expect("a world log has at least its baseline record")
    }

    /// Appends a merge or takedown: its change set (`change`, the bytes of
    /// `change.json`), the gate's report, and the world after it (`world`,
    /// the bytes of the store's new `world.json`), then its record as the
    /// next `log/NNNNNNNN.json`, which is written last. Returns the record's
    /// path in the log, such as `log/00000001.json`.
    pub fn append(&mut self, merged: &Merged, change: &[u8], world: &[u8]) -> Result<String> {
        let record = &merged.record;
        let (head_id, head) = self
            .records
            .last()
            .ok_or_else(|| Error::Invalid("the world log is empty".into()))?;
        if record.previous_record_id != Some(*head_id)
            || record.world_before != Some(head.world_after)
        {
            return Err(Error::Invalid(
                "the record does not continue the world log".into(),
            ));
        }
        let (Some(change_id), Some(report_id)) = (record.change_id, record.check_report_id) else {
            return Err(Error::Invalid(
                "only merged and takedown records are appended; a baseline starts a log".into(),
            ));
        };
        for (what, bytes, id) in [
            ("change set", change, change_id),
            ("report", merged.report_bytes.as_slice(), report_id),
            ("world manifest", world, record.world_after),
        ] {
            if ContentId::of(bytes) != id {
                return Err(Error::Invalid(format!(
                    "the {what} is not {id}, which the record names"
                )));
            }
        }
        let n = self.records.len();
        if n > MAX_RECORD {
            return Err(Error::Invalid(
                "the world log is full: record numbers have 8 digits".into(),
            ));
        }
        write_content(
            &self.dir,
            &["changes", &change_id.hex(), "change.json"],
            change,
        )?;
        write_content(
            &self.dir,
            &["reports", &format!("{}.json", report_id.hex())],
            &merged.report_bytes,
        )?;
        write_content(
            &self.dir,
            &["worlds", &format!("{}.json", record.world_after.hex())],
            world,
        )?;
        let name = record_name(n);
        let (path, rel) = log_path(&self.dir, &[RECORDS_DIR, &name]);
        if path.exists() {
            return Err(Error::Invalid(format!(
                "{rel} appeared during the merge; did another merge run at the same time?"
            )));
        }
        write_atomically(&path, &merged.record_bytes)
            .map_err(|e| Error::Io(format!("{rel}: {e}")))?;
        self.records
            .push((ContentId::of(&merged.record_bytes), record.clone()));
        self.head = merged.record_bytes.clone();
        Ok(rel)
    }
}

/// Where a merge command reads the previous record from, and where it
/// writes the new record and the gate's report.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MergeOutput {
    /// `--previous <record> --record <file> --report <file>`.
    Files {
        previous: PathBuf,
        record: PathBuf,
        report: PathBuf,
    },
    /// `--log <dir>`: the log's last record is the previous one, and the
    /// merge is appended to the log ([`WorldLog::append`]).
    Log(PathBuf),
}

/// The arguments of the merge commands, `op_world merge` and
/// `op_replay merge` ([`MergeArgs::USAGE`]).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MergeArgs {
    pub world_dir: PathBuf,
    pub bundle_dir: PathBuf,
    pub output: MergeOutput,
    pub decision: Decision,
    pub reviews: Vec<Review>,
    pub reasons: String,
    /// `--require-base`: see [`require_base`].
    pub require_base: bool,
}

/// `--review <handle>=<verdict>[:<notes>]`.
fn parse_review(text: &str) -> std::result::Result<Review, String> {
    let (reviewer, rest) = text
        .split_once('=')
        .ok_or("--review is <handle>=<verdict>[:<notes>]")?;
    let (verdict, notes) = rest.split_once(':').unwrap_or((rest, rest));
    let verdict = match verdict {
        "better" => Verdict::Better,
        "not_better" => Verdict::NotBetter,
        "blocked" => Verdict::Blocked,
        other => {
            return Err(format!(
                "unknown verdict {other} (better, not_better, blocked)"
            ));
        }
    };
    Ok(Review {
        reviewer: reviewer.into(),
        verdict,
        notes: notes.into(),
    })
}

impl MergeArgs {
    /// The merge commands' arguments, after the command's name.
    pub const USAGE: &'static str = "<world-dir> <bundle-dir> (--previous <record> --record <file> --report <file> | --log <dir>) --reason <text> --review <handle>=<verdict>[:<notes>]... [--takedown] [--require-base]";

    /// Parses `<world-dir> <bundle-dir>` and the options of
    /// [`MergeArgs::USAGE`]. `--log` replaces `--previous`, `--record` and
    /// `--report`, so it cannot be combined with them; without it, all
    /// three are required. Errors are usage messages.
    pub fn parse(args: &[String]) -> std::result::Result<MergeArgs, String> {
        let [world_dir, bundle_dir, options @ ..] = args else {
            return Err(format!("merge takes {}", Self::USAGE));
        };
        let mut values: BTreeMap<&str, String> = BTreeMap::new();
        let (mut reviews, mut takedown, mut require_base) = (Vec::new(), false, false);
        let mut it = options.iter().peekable();
        while let Some(option) = it.next() {
            let value = match it.peek() {
                Some(v) if !v.starts_with("--") => it.next().cloned(),
                _ => None,
            };
            match option.as_str() {
                o @ ("--takedown" | "--require-base") => {
                    if value.is_some() {
                        return Err(format!("{o} takes no value"));
                    }
                    if o == "--takedown" {
                        takedown = true;
                    } else {
                        require_base = true;
                    }
                }
                "--review" => reviews.push(parse_review(value.as_deref().unwrap_or(""))?),
                o @ ("--previous" | "--record" | "--report" | "--log" | "--reason") => {
                    let value = value.ok_or(format!("{o} <value> is required"))?;
                    if values.insert(o, value).is_some() {
                        return Err(format!("{o} is given twice"));
                    }
                }
                o if o.starts_with("--") => return Err(format!("unknown option {o}")),
                other => return Err(format!("unexpected argument {other}")),
            }
        }
        fn take(
            values: &mut BTreeMap<&str, String>,
            name: &str,
        ) -> std::result::Result<String, String> {
            values
                .remove(name)
                .ok_or(format!("{name} <value> is required"))
        }
        let output = match values.remove("--log") {
            Some(log) => {
                if let Some(o) = ["--previous", "--record", "--report"]
                    .into_iter()
                    .find(|o| values.contains_key(o))
                {
                    return Err(format!(
                        "{o} cannot be combined with --log, which takes the previous record from the log and writes the new record and the report into it"
                    ));
                }
                MergeOutput::Log(PathBuf::from(log))
            }
            None => MergeOutput::Files {
                previous: PathBuf::from(take(&mut values, "--previous")?),
                record: PathBuf::from(take(&mut values, "--record")?),
                report: PathBuf::from(take(&mut values, "--report")?),
            },
        };
        let reasons = take(&mut values, "--reason")?;
        Ok(MergeArgs {
            world_dir: PathBuf::from(world_dir),
            bundle_dir: PathBuf::from(bundle_dir),
            output,
            decision: if takedown {
                Decision::Takedown
            } else {
                Decision::Merged
            },
            reviews,
            reasons,
            require_base,
        })
    }

    /// Runs the merge with `gate` (see [`merge_with`]) and writes its
    /// outputs. Checks the world log (with `--log`) and the base world
    /// (with `--require-base`) before the gate runs. With `--log`, the merge
    /// holds the log's lock ([`LOG_LOCK`]) from before it opens the log
    /// until it has appended the record. `scratch_dir` is a directory of its
    /// own for the candidate world: it is deleted before and after the
    /// merge. Returns the line the commands print.
    pub fn run(
        &self,
        scratch_dir: &Path,
        gate: impl FnOnce(&Path, &Path, &Path) -> Result<CheckReport>,
    ) -> Result<String> {
        enum Target<'a> {
            Log(WorldLog, &'a Path),
            Files { record: &'a Path, report: &'a Path },
        }
        // A log directory that does not exist has no lock to take, and
        // `WorldLog::open` says how to start one.
        let _lock = match &self.output {
            MergeOutput::Log(dir) if dir.is_dir() => Some(Lock::take(dir, LOG_LOCK, "world log")?),
            _ => None,
        };
        let (mut target, previous) = match &self.output {
            MergeOutput::Log(dir) => {
                let log = WorldLog::open(dir)?;
                let previous = log.head().to_vec();
                (Target::Log(log, dir), previous)
            }
            MergeOutput::Files {
                previous,
                record,
                report,
            } => (
                Target::Files { record, report },
                fs::read(previous).map_err(|e| Error::Io(format!("--previous: {e}")))?,
            ),
        };
        if self.require_base {
            require_base(&self.world_dir, &self.bundle_dir)?;
        }
        let _ = fs::remove_dir_all(scratch_dir);
        let result = merge_with(
            &self.world_dir,
            &self.bundle_dir,
            &previous,
            self.decision,
            self.reviews.clone(),
            &self.reasons,
            scratch_dir,
            gate,
        );
        let _ = fs::remove_dir_all(scratch_dir);
        let merged = result?;
        let before = merged
            .record
            .world_before
            .map(|w| w.to_string())
            .unwrap_or_default();
        let after = merged.record.world_after;
        // The store holds the new world from here on, as in ADR-0010,
        // where the bucket is written before the log. A failure now says
        // so, and how to undo the merge where that is possible.
        let place = match &mut target {
            Target::Log(log, dir) => {
                let mut append = || -> Result<String> {
                    let change = fs::read(self.bundle_dir.join("change.json"))
                        .map_err(|e| Error::Io(format!("change.json: {e}")))?;
                    let world = fs::read(Store::new(&self.world_dir).world_path())
                        .map_err(|e| Error::Io(format!("world.json: {e}")))?;
                    log.append(&merged, &change, &world)
                };
                let rel = append().map_err(|e| {
                    // The log still ends at `before`: no other merge could
                    // append while this one held the log's lock.
                    let kept = format!(
                        "worlds/{}.json",
                        before.trim_start_matches(ContentId::PREFIX)
                    );
                    let undo = if dir.join(&kept).is_file() {
                        format!("copy the log's {kept} over the store's world.json")
                    } else {
                        format!("put back the store's world.json of world {before}")
                    };
                    Error::Io(format!(
                        "the store now holds world {after}, but the world log was not updated ({e}); to undo the merge, {undo}"
                    ))
                })?;
                format!(" ({rel})")
            }
            Target::Files { record, report } => {
                fs::write(report, &merged.report_bytes)
                    .and_then(|()| fs::write(record, &merged.record_bytes))
                    .map_err(|e| {
                        Error::Io(format!(
                            "the store now holds world {after}, but the record and the report were not both written: {e}"
                        ))
                    })?;
                String::new()
            }
        };
        Ok(format!(
            "record {}{place}: world {before} -> {after}",
            ContentId::of(&merged.record_bytes)
        ))
    }
}
