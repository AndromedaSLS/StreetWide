//! Merging change sets into a world store and writing world-log records
//! (RFC 0002 section 6).
//!
//! These functions work on local directories; where the canonical store and
//! the world log are hosted is decided separately (ADR-0010). A merge never
//! leaves the store half-changed by a refused change set: the gate runs, the
//! record is built and validated, and with `--log` the log is checked to
//! take the record ([`WorldLog::check_append`]); then the new objects are
//! copied in, and `world.json` is written last.
//!
//! One merge at a time: a merge holds a lock file in the store
//! ([`STORE_LOCK`]) and, with `--log`, one in the log ([`LOG_LOCK`]).
//! [`MergeArgs::run`] takes both before it reads the store or the log and
//! keeps them until it has written the store and appended the record;
//! [`merge_with`] alone holds the store's until it has written `world.json`.
//! A second merge finds a lock file and stops before it writes anything.
//! Two merges that both started from the same world would otherwise leave
//! the store and the log naming different worlds. An empty path names the
//! current directory here, as it does for [`Store`] and [`WorldLog`], so
//! its lock is taken there.
//!
//! - [`merge`] runs `op_world`'s gate (G1 to G8). [`merge_with`] takes the
//!   gate as a function, so `op_replay merge` can run the complete gate (G1
//!   to G10, ADR-0009) and merge change sets that touch colliders.
//! - [`WorldLog`] keeps a world log in a local directory.
//! - [`MergeArgs`] is what `op_world merge` and `op_replay merge` share: the
//!   same options, checks and outputs, with a different gate.
//!
//! A change set authored by [`INGEST_AUTHOR`], a baseline refresh, merges
//! only on the world named in its `base_world_id` (RFC 0002 sections 2
//! and 7, question 15): every merge refuses it on another world, with or
//! without `--require-base`, which applies the same rule to any change set
//! ([`require_base`]). It is a merge rule, not a gate check.
//!
//! Bundles, previous records and world logs may come from elsewhere, so
//! every file is read with [`read_file`]: a regular file of at most
//! [`MAX_FILE_BYTES`], never a link, a FIFO or a device. A world log holds
//! no links at all (see [`WorldLog`]).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use crate::contrib::{
    CONTRIBUTION_CONTRACT_VERSION, ChangeSet, CheckReport, Decision, INGEST_AUTHOR, MergeRecord,
    Outcome, Review, Verdict, verify_log,
};
use crate::gate;
use crate::id::ContentId;
use crate::manifest::AssetManifest;
use crate::store::{MAX_FILE_BYTES, ObjectKind, Store, read_file};
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
    /// that stopped before it could remove it. Something else at that path
    /// (a directory, a link) is refused with what it is; Windows reports it
    /// as access denied rather than as existing.
    fn take(dir: &Path, name: &str, what: &str) -> Result<Lock> {
        let path = dir.join(name);
        let opened = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path);
        let err = match opened {
            Ok(_) => return Ok(Lock(path)),
            Err(e) => e,
        };
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_file() => Err(Error::Invalid(format!(
                "{} exists: another merge is running on this {what}, or one stopped before it could remove the file; if no merge is running, delete the file and merge again",
                path.display()
            ))),
            Ok(meta) => Err(Error::Invalid(format!(
                "{} exists and is {}, not a lock file; if no merge is running, remove it and merge again",
                path.display(),
                kind_of(&meta)
            ))),
            Err(_) => Err(Error::Io(format!("{}: {err}", path.display()))),
        }
    }

    /// The lock `name` in `dir` (the current directory for an empty path),
    /// or none when `dir` is not a directory: the merge then fails on
    /// reading it, with its own message.
    fn take_in(dir: &Path, name: &str, what: &str) -> Result<Option<Lock>> {
        let dir = if dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            dir
        };
        if dir.is_dir() {
            Lock::take(dir, name, what).map(Some)
        } else {
            Ok(None)
        }
    }
}

/// What is at a path that should be a regular file, in words.
fn kind_of(meta: &fs::Metadata) -> &'static str {
    let kind = meta.file_type();
    if kind.is_symlink() {
        "a link"
    } else if kind.is_dir() {
        "a directory"
    } else if kind.is_file() {
        "a file"
    } else {
        "neither a file nor a directory"
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
    /// The bundle's `change.json` as the merge read it: the change set the
    /// gate checked, which hashes to the record's `change_id`.
    pub change_bytes: Vec<u8>,
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
/// - A change set authored by [`INGEST_AUTHOR`] must have been planned on
///   the store's world, its `base_world_id` (RFC 0002 section 7).
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
/// it moved, which only a tool that ignores the lock can do. It also reads
/// the bundle's `change.json` once more, to copy its objects, and refuses
/// if it is not the change set the gate checked, and it refuses a
/// candidate world in `candidate_dir` that is not the one the gate's report
/// names: nothing is written then.
///
/// A change set authored by [`INGEST_AUTHOR`] whose `base_world_id` is not
/// the store's world is refused before the gate runs, and the change set
/// the gate checked is held to the same rule before anything is written,
/// in case `change.json` changed in between (RFC 0002 section 7). A
/// `change.json` that cannot be read as a change set is left to the gate,
/// which reports it.
#[allow(
    clippy::too_many_arguments,
    reason = "the arguments of merge() plus the gate"
)]
pub fn merge_with(
    world_dir: &Path,
    bundle_dir: &Path,
    previous_record: &[u8],
    decision: Decision,
    reviews: Vec<Review>,
    reasons: &str,
    scratch_dir: &Path,
    gate: impl FnOnce(&Path, &Path, &Path) -> Result<CheckReport>,
) -> Result<Merged> {
    let _lock = Lock::take_in(world_dir, STORE_LOCK, "store")?;
    merge_locked(
        world_dir,
        bundle_dir,
        previous_record,
        decision,
        reviews,
        reasons,
        scratch_dir,
        gate,
        |_, _| Ok(()),
    )
}

/// [`merge_with`] under a store lock the caller holds. `before_write(merged,
/// world)` runs once everything is checked, just before the store is
/// written, with the merge's outputs and the bytes of the new
/// `world.json`; an error there stops the merge with nothing written.
#[allow(
    clippy::too_many_arguments,
    reason = "the arguments of merge_with() plus the check before writing"
)]
fn merge_locked(
    world_dir: &Path,
    bundle_dir: &Path,
    previous_record: &[u8],
    decision: Decision,
    mut reviews: Vec<Review>,
    reasons: &str,
    scratch_dir: &Path,
    gate: impl FnOnce(&Path, &Path, &Path) -> Result<CheckReport>,
    before_write: impl FnOnce(&Merged, &[u8]) -> Result<()>,
) -> Result<Merged> {
    if decision == Decision::Baseline {
        return Err(Error::Invalid("use baseline() to start a world log".into()));
    }
    let previous = MergeRecord::from_bytes(previous_record)?;
    let previous_id = ContentId::of(previous_record);
    // A store that does not exist is reported by `read_world`.
    let world = Store::new(world_dir);
    let (world_id, _) = world.read_world()?;
    if previous.world_after != world_id {
        return Err(Error::Invalid(format!(
            "the store holds world {world_id}, but the log's last record ends at {}",
            previous.world_after
        )));
    }
    // A baseline refresh planned on another world is refused before the
    // gate runs. The change set the gate checks is held to the rule below.
    if let Some(change) = read_file(&bundle_dir.join("change.json"), MAX_FILE_BYTES)
        .ok()
        .and_then(|bytes| ChangeSet::from_bytes(&bytes).ok())
    {
        check_base(world_id, &change, false)?;
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

    let change_bytes = read_file(&bundle_dir.join("change.json"), MAX_FILE_BYTES)
        .map_err(|e| Error::Io(format!("change.json: {e}")))?;
    if ContentId::of(&change_bytes) != report.change_id {
        return Err(Error::Invalid(format!(
            "the bundle's change.json changed during the merge (the gate checked change {}), so nothing was written",
            report.change_id
        )));
    }
    let change = ChangeSet::from_bytes(&change_bytes)?;
    check_base(world_id, &change, false)?;
    // The candidate world as the gate left it in `candidate_dir`: it must
    // be the one the gate's report names, or something changed it since.
    let candidate = Store::layered(
        &candidate_dir,
        vec![bundle_dir.to_path_buf(), world_dir.to_path_buf()],
    );
    let (built, candidate_world) = candidate.read_world()?;
    let world_bytes = candidate_world.to_bytes()?;
    if built != candidate_id || ContentId::of(&world_bytes) != candidate_id {
        return Err(Error::Invalid(format!(
            "the candidate world in {} is not the gate's candidate {candidate_id}; it changed after the gate built it, so nothing was written",
            candidate_dir.display()
        )));
    }
    let merged = Merged {
        record,
        record_bytes,
        report_bytes,
        change_bytes,
    };
    before_write(&merged, &world_bytes)?;

    // Copy the new objects in: the change set's manifests and meshes, then
    // the candidate's tile indexes; `world.json` last.
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
    Ok(merged)
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
/// shell that a later set replaces. So a change set authored by
/// [`INGEST_AUTHOR`] merges only on its base world, and every merge holds
/// it to that rule without the flag (RFC 0002 sections 2 and 7, question
/// 15, amendment A7). The flag applies the rule to any change set.
pub fn require_base(world_dir: &Path, bundle_dir: &Path) -> Result<()> {
    let (world_id, _) = Store::new(world_dir).read_world()?;
    let bytes = read_file(&bundle_dir.join("change.json"), MAX_FILE_BYTES)
        .map_err(|e| Error::Io(format!("change.json: {e}")))?;
    let change = ChangeSet::from_bytes(&bytes)?;
    check_base(world_id, &change, true)
}

/// The base-world rule: refuses `change` if the store's world, `world_id`,
/// is not its `base_world_id`, when the change set is a baseline refresh
/// (authored by [`INGEST_AUTHOR`], RFC 0002 section 7) or when `required`
/// (`--require-base`).
fn check_base(world_id: ContentId, change: &ChangeSet, required: bool) -> Result<()> {
    if (required || change.author == INGEST_AUTHOR) && change.base_world_id != world_id {
        return Err(Error::Invalid(format!(
            "the store holds world {world_id}, but this change set was planned on {}; plan the refresh again",
            change.base_world_id
        )));
    }
    Ok(())
}

/// The directory of merge records in a [`WorldLog`].
const RECORDS_DIR: &str = "log";

/// A file a merge writes into a log: its path, as parts, and its bytes.
type LogFile<'a> = (Vec<String>, &'a [u8]);
/// The highest record number with 8 digits.
const MAX_RECORD: usize = 99_999_999;

/// The file name of record `n`: 8 digits, from `00000000.json`.
fn record_name(n: usize) -> String {
    format!("{n:08}.json")
}

/// `log` is there but is not a directory: a file, or a link (to a
/// directory or not), which is not followed.
fn not_a_records_dir() -> Error {
    Error::Invalid(format!(
        "the world log's {RECORDS_DIR} is not a directory; a world log holds its records in a directory {RECORDS_DIR}/, not in a link to one"
    ))
}

/// `dir` joined with `parts`, and the same path as POSIX text for messages.
fn log_path(dir: &Path, parts: &[&str]) -> (PathBuf, String) {
    let mut path = dir.to_path_buf();
    for p in parts {
        path.push(p);
    }
    (path, parts.join("/"))
}

/// Checks that every directory on the way to the file `parts` in the log
/// directory `dir` is a directory itself, not a link to one. A directory
/// that does not exist ends the check: the file is missing, and a write
/// creates it.
fn check_dirs(dir: &Path, parts: &[&str]) -> io::Result<()> {
    let mut path = dir.to_path_buf();
    for k in 0..parts.len().saturating_sub(1) {
        path.push(parts[k]);
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        if !meta.is_dir() {
            let what = if meta.file_type().is_symlink() {
                "a link"
            } else {
                "not a directory"
            };
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} is {what}", parts[..=k].join("/")),
            ));
        }
    }
    Ok(())
}

/// The bytes of the file `parts` in the log directory `dir`: a regular file
/// of at most `limit` bytes, under directories that are not links (see
/// [`WorldLog`]).
fn read_in_log(dir: &Path, parts: &[&str], limit: u64) -> io::Result<Vec<u8>> {
    check_dirs(dir, parts)?;
    read_file(&log_path(dir, parts).0, limit)
}

/// The temporary path [`write_atomically`] writes `path` through.
fn tmp_path(path: &Path) -> PathBuf {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    PathBuf::from(tmp)
}

/// Writes through a temporary file next to `path`, then renames it.
/// Whatever is at the temporary path (a file left by a crash, a link, a
/// FIFO) is removed first and the temporary file is created anew, so the
/// write never follows a link or waits on a FIFO; a directory there stops
/// the write. If the write or the rename fails, the temporary file is
/// removed again. A temporary file left in `log/` by a crash makes
/// [`WorldLog::open`] refuse the log until someone looks at it, so records
/// are never written over one.
fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = tmp_path(path);
    match fs::remove_file(&tmp) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let written = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .and_then(|mut file| file.write_all(bytes))
        .and_then(|()| fs::rename(&tmp, path));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// Whether the file `parts` of the log in `dir`, named by its content, must
/// be written to hold `bytes`: false when it already holds exactly these
/// bytes. Anything else there is refused: other bytes, or something that
/// is not a regular file (a directory, a link, a FIFO), which is never
/// opened or followed, and so is a directory at its temporary path, which
/// [`write_atomically`] could not remove. At most `bytes.len()` bytes are
/// read: a longer file cannot hold these bytes.
fn must_write(dir: &Path, parts: &[&str], bytes: &[u8]) -> Result<bool> {
    let (path, rel) = log_path(dir, parts);
    let io_error = |e: io::Error| Error::Io(format!("{rel}: {e}"));
    let other = || Error::Invalid(format!("the world log's {rel} holds other bytes"));
    check_dirs(dir, parts).map_err(io_error)?;
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return match fs::symlink_metadata(tmp_path(&path)) {
                Ok(meta) if meta.is_dir() => Err(Error::Invalid(format!(
                    "the world log has a directory at {rel}.tmp, where a merge writes {rel} through a temporary file"
                ))),
                _ => Ok(true),
            };
        }
        Err(e) => return Err(io_error(e)),
        Ok(meta) if meta.is_file() && meta.len() != bytes.len() as u64 => return Err(other()),
        Ok(_) => {}
    }
    match read_file(&path, bytes.len() as u64) {
        Ok(existing) if existing == bytes => Ok(false),
        Ok(_) => Err(other()),
        Err(e) => Err(io_error(e)),
    }
}

/// Writes a file named by its content, unless it already holds exactly
/// these bytes ([`must_write`] says what is refused).
fn write_content(dir: &Path, parts: &[&str], bytes: &[u8]) -> Result<String> {
    let (path, rel) = log_path(dir, parts);
    if must_write(dir, parts, bytes)? {
        write_atomically(&path, bytes).map_err(|e| Error::Io(format!("{rel}: {e}")))?;
    }
    Ok(rel)
}

/// Whether the log in `dir` keeps world `id` in `worlds/<hex>.json`, as a
/// regular file that hashes to it.
fn keeps_world(dir: &Path, id: ContentId) -> bool {
    let name = format!("{}.json", id.hex());
    read_in_log(dir, &["worlds", &name], MAX_FILE_BYTES).is_ok_and(|b| ContentId::of(&b) == id)
}

/// How to put world `id` back into the store: from the log in `dir` when
/// it keeps that world, else in words.
fn put_back(dir: &Path, id: ContentId) -> String {
    if keeps_world(dir, id) {
        format!(
            "copy the log's worlds/{}.json over the store's world.json",
            id.hex()
        )
    } else {
        format!("put back the store's world.json of world {id}")
    }
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
///
/// A log may come from elsewhere (git keeps links), so inside its directory
/// every directory must be a directory and every file a regular file of at
/// most [`MAX_FILE_BYTES`]. A link, a FIFO, a device or a directory where a
/// file should be is refused, and so is a link where a directory should be:
/// none is ever opened or followed, for reading or for writing. The log's
/// own directory, the `--log` argument, may be a link.
pub struct WorldLog {
    dir: PathBuf,
    records: Vec<(ContentId, MergeRecord)>,
    head: Vec<u8>,
}

impl WorldLog {
    /// Opens the world log in `dir`. The records in `log/` must be numbered
    /// from `00000000.json` with no gaps and no other entries, and must form
    /// a chain ([`verify_log`]). The change set of every merged or takedown
    /// record must be in `changes/<hex>/change.json` and hash to its id, as
    /// the refresh planner requires, so that a merge never extends a log
    /// that the planner refuses.
    ///
    /// `log/` must be a directory, not a link to one. Its entries are
    /// sorted by name and taken as they are, without following links: a
    /// regular file with the next record's name is read and checked; any
    /// other name (a temporary file left by a crash, for one) and anything
    /// that is not a regular file (a directory, a link, a FIFO, a socket, a
    /// device) is refused unopened, and the message names it. Each change
    /// set is read once, however many records name it.
    pub fn open(dir: &Path) -> Result<WorldLog> {
        let records_dir = dir.join(RECORDS_DIR);
        let no_records = || {
            Error::Invalid(
                "the world log has no records; start it with op_world baseline <world-dir> --log <dir> --reason <text>"
                    .into(),
            )
        };
        let listing = |e: io::Error| Error::Io(format!("{RECORDS_DIR}/: {e}"));
        match fs::symlink_metadata(&records_dir) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(not_a_records_dir()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(no_records()),
            Err(e) => return Err(listing(e)),
        }
        let mut entries = Vec::new();
        for entry in fs::read_dir(&records_dir).map_err(listing)? {
            let entry = entry.map_err(listing)?;
            // The entry itself: a link here is a link, whatever it names.
            let kind = entry.file_type().map_err(listing)?;
            entries.push((entry.file_name().to_string_lossy().into_owned(), kind));
        }
        if entries.is_empty() {
            return Err(no_records());
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let mut records = Vec::with_capacity(entries.len());
        let mut head = Vec::new();
        for (n, (name, kind)) in entries.iter().enumerate() {
            let expected = record_name(n);
            if *name != expected {
                return Err(Error::Invalid(format!(
                    "the world log has {RECORDS_DIR}/{name} where {RECORDS_DIR}/{expected} should be; records are numbered from 00000000, with no gaps and no other files"
                )));
            }
            if !kind.is_file() {
                return Err(Error::Invalid(format!(
                    "the world log's {RECORDS_DIR}/{name} is not a regular file; records are regular files, not links, directories or pipes"
                )));
            }
            let bytes = read_file(&records_dir.join(name), MAX_FILE_BYTES)
                .map_err(|e| Error::Io(format!("{RECORDS_DIR}/{name}: {e}")))?;
            let record = MergeRecord::from_bytes(&bytes)
                .map_err(|e| Error::Invalid(format!("{RECORDS_DIR}/{name}: {e}")))?;
            records.push((ContentId::of(&bytes), record));
            head = bytes;
        }
        verify_log(&records)?;
        let mut checked = BTreeSet::new();
        for (n, (_, record)) in records.iter().enumerate() {
            let Some(change_id) = record.change_id else {
                continue;
            };
            if !checked.insert(change_id) {
                continue;
            }
            let hex = change_id.hex();
            let parts = ["changes", hex.as_str(), "change.json"];
            let rel = parts.join("/");
            let missing = || {
                Error::Invalid(format!(
                    "the world log names change set {change_id} ({RECORDS_DIR}/{}) but {rel} is missing or has another id",
                    record_name(n)
                ))
            };
            let bytes = match read_in_log(dir, &parts, MAX_FILE_BYTES) {
                Ok(bytes) if ContentId::of(&bytes) == change_id => bytes,
                Ok(_) => return Err(missing()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(missing()),
                Err(e) => return Err(Error::Io(format!("{rel}: {e}"))),
            };
            ChangeSet::from_bytes(&bytes).map_err(|e| Error::Invalid(format!("{rel}: {e}")))?;
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
        let listing = |e: io::Error| Error::Io(format!("{RECORDS_DIR}/: {e}"));
        match fs::symlink_metadata(&records_dir) {
            Ok(meta) if meta.is_dir() => {
                if fs::read_dir(&records_dir)
                    .map_err(listing)?
                    .next()
                    .is_some()
                {
                    return Err(Error::Invalid(format!(
                        "the world log already has records in {RECORDS_DIR}/; a baseline record starts a new log"
                    )));
                }
            }
            Ok(_) => return Err(not_a_records_dir()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(listing(e)),
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

    /// What [`WorldLog::append`] writes for `merged`: the change set, the
    /// report and the world, in that order, and the record's file name.
    /// Checks that the record continues the log and names these bytes, and
    /// that the record's path is free (anything there, even a link to
    /// nothing, or anything at its temporary path).
    fn appended<'a>(
        &self,
        merged: &'a Merged,
        change: &'a [u8],
        world: &'a [u8],
    ) -> Result<(Vec<LogFile<'a>>, String)> {
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
        let name = record_name(n);
        let (path, rel) = log_path(&self.dir, &[RECORDS_DIR, &name]);
        // `open` took every entry of `log/`, so only a tool that ignores the
        // lock can have put something at these paths since.
        for (taken, rel) in [
            (path.clone(), rel.clone()),
            (tmp_path(&path), format!("{rel}.tmp")),
        ] {
            if fs::symlink_metadata(&taken).is_ok() {
                return Err(Error::Invalid(format!(
                    "{rel} appeared during the merge; did another merge run at the same time?"
                )));
            }
        }
        let files = vec![
            (
                vec!["changes".into(), change_id.hex(), "change.json".into()],
                change,
            ),
            (
                vec!["reports".into(), format!("{}.json", report_id.hex())],
                merged.report_bytes.as_slice(),
            ),
            (
                vec![
                    "worlds".into(),
                    format!("{}.json", record.world_after.hex()),
                ],
                world,
            ),
        ];
        Ok((files, name))
    }

    /// Checks, without writing anything, that [`WorldLog::append`] can
    /// append `merged` with these bytes: the record continues the log and
    /// names them, its path is free, and each file it writes is missing or
    /// already holds exactly its bytes, as [`WorldLog::append`] requires. A
    /// merge with `--log` runs this before it writes the store, so a log
    /// that cannot take the merge leaves the store as it was.
    pub fn check_append(&self, merged: &Merged, change: &[u8], world: &[u8]) -> Result<()> {
        let (files, _) = self.appended(merged, change, world)?;
        for (parts, bytes) in &files {
            let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
            must_write(&self.dir, &parts, bytes)?;
        }
        Ok(())
    }

    /// Appends a merge or takedown: its change set (`change`, the bytes of
    /// `change.json`), the gate's report, and the world after it (`world`,
    /// the bytes of the store's new `world.json`), then its record as the
    /// next `log/NNNNNNNN.json`, which is written last. Returns the record's
    /// path in the log, such as `log/00000001.json`.
    pub fn append(&mut self, merged: &Merged, change: &[u8], world: &[u8]) -> Result<String> {
        let (files, name) = self.appended(merged, change, world)?;
        for (parts, bytes) in &files {
            let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
            write_content(&self.dir, &parts, bytes)?;
        }
        let (path, rel) = log_path(&self.dir, &[RECORDS_DIR, &name]);
        // Anything there, even a link to nothing.
        if fs::symlink_metadata(&path).is_ok() {
            return Err(Error::Invalid(format!(
                "{rel} appeared during the merge; did another merge run at the same time?"
            )));
        }
        write_atomically(&path, &merged.record_bytes)
            .map_err(|e| Error::Io(format!("{rel}: {e}")))?;
        self.records
            .push((ContentId::of(&merged.record_bytes), merged.record.clone()));
        self.head = merged.record_bytes.clone();
        Ok(rel)
    }

    /// Why the store, which holds world `store`, cannot take a merge with
    /// this log, which ends at another world. A merge that stopped between
    /// writing the store and appending its record leaves exactly this: the
    /// log keeps the store's world in `worlds/`, but no record names it.
    /// The message then says so, and how to undo that merge.
    fn behind(&self, store: ContentId) -> Error {
        let head = self.world_id();
        let mut message =
            format!("the store holds world {store}, but the log's last record ends at {head}");
        let named = self.records.iter().any(|(_, r)| r.world_after == store);
        if !named && keeps_world(&self.dir, store) {
            message.push_str(&format!(
                ", and the log keeps world {store} in worlds/ without a record that names it: a merge stopped after it wrote the store and before it appended its record. To undo that merge, {}, then merge again",
                put_back(&self.dir, head)
            ));
        } else {
            message.push_str(&format!(
                ". If a merge stopped after it wrote the store and before it appended its record, {} and merge again; otherwise check that --log is this store's log",
                put_back(&self.dir, head)
            ));
        }
        Error::Invalid(message)
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
    /// `--require-base`: see [`require_base`]. A change set authored by
    /// [`INGEST_AUTHOR`] is held to that rule without it.
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
    /// three are required. The two directories come first, so neither may
    /// start with `--`, and an option's value may not either. No path may
    /// be empty: an unset shell variable gives an empty argument, which
    /// would name the current directory. Errors are usage messages.
    pub fn parse(args: &[String]) -> std::result::Result<MergeArgs, String> {
        let [world_dir, bundle_dir, options @ ..] = args else {
            return Err(format!("merge takes {}", Self::USAGE));
        };
        if world_dir.starts_with("--") || bundle_dir.starts_with("--") {
            return Err(format!("merge takes {}", Self::USAGE));
        }
        if world_dir.is_empty() || bundle_dir.is_empty() {
            return Err("<world-dir> and <bundle-dir> must not be empty".into());
        }
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
                    if value.is_empty() && o != "--reason" {
                        return Err(format!("{o} must not be empty"));
                    }
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
    /// outputs. Before the gate runs, it checks the world log (with
    /// `--log`), that the store holds the world the previous record ends
    /// at, and the base world: with `--require-base` for any change set,
    /// and without it for one authored by [`INGEST_AUTHOR`]. With `--log`,
    /// it also checks that the log can take the record before it writes
    /// the store ([`WorldLog::check_append`]).
    ///
    /// The merge takes the log's lock ([`LOG_LOCK`], with `--log`) and the
    /// store's ([`STORE_LOCK`]) before it reads either, and keeps them until
    /// it has written the store and the record. `scratch_dir` is for the
    /// candidate world: whatever is there is removed, and the merge makes
    /// the directory anew and refuses to go on if it cannot (one made by
    /// someone else in a shared temporary directory, which cannot be
    /// removed, is never written into). It is removed again after the
    /// merge. Returns the line the commands print.
    pub fn run(
        &self,
        scratch_dir: &Path,
        gate: impl FnOnce(&Path, &Path, &Path) -> Result<CheckReport>,
    ) -> Result<String> {
        enum Target<'a> {
            Log(WorldLog),
            Files { record: &'a Path, report: &'a Path },
        }
        // A log directory that does not exist has no lock to take, and
        // `WorldLog::open` says how to start one; a store that does not
        // exist is reported when it is read.
        let _log_lock = match &self.output {
            MergeOutput::Log(dir) => Lock::take_in(dir, LOG_LOCK, "world log")?,
            MergeOutput::Files { .. } => None,
        };
        let _store_lock = Lock::take_in(&self.world_dir, STORE_LOCK, "store")?;
        let (mut target, previous) = match &self.output {
            MergeOutput::Log(dir) => {
                let log = WorldLog::open(dir)?;
                let previous = log.head().to_vec();
                (Target::Log(log), previous)
            }
            MergeOutput::Files {
                previous,
                record,
                report,
            } => (
                Target::Files { record, report },
                read_file(previous, MAX_FILE_BYTES)
                    .map_err(|e| Error::Io(format!("--previous {}: {e}", previous.display())))?,
            ),
        };
        // The store must hold the world the previous record ends at, which
        // `merge_with` checks too. Checked here first, before
        // `--require-base`, so that a log left behind by an interrupted
        // merge is reported as such.
        let ends_at = MergeRecord::from_bytes(&previous)?.world_after;
        let (store_world, _) = Store::new(&self.world_dir).read_world()?;
        if store_world != ends_at {
            return Err(match &target {
                Target::Log(log) => log.behind(store_world),
                Target::Files { .. } => Error::Invalid(format!(
                    "the store holds world {store_world}, but the log's last record ends at {ends_at}"
                )),
            });
        }
        if self.require_base {
            require_base(&self.world_dir, &self.bundle_dir)?;
        }
        let _ = fs::remove_dir_all(scratch_dir);
        if let Some(parent) = scratch_dir.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        fs::create_dir(scratch_dir).map_err(|e| {
            Error::Io(format!(
                "cannot make the scratch directory {} anew: {e}",
                scratch_dir.display()
            ))
        })?;
        let result = merge_locked(
            &self.world_dir,
            &self.bundle_dir,
            &previous,
            self.decision,
            self.reviews.clone(),
            &self.reasons,
            scratch_dir,
            gate,
            |merged, world| match &target {
                Target::Log(log) => log.check_append(merged, &merged.change_bytes, world),
                Target::Files { .. } => Ok(()),
            },
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
            Target::Log(log) => {
                let mut append = || -> Result<String> {
                    let path = Store::new(&self.world_dir).world_path();
                    let world = read_file(&path, MAX_FILE_BYTES)
                        .map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
                    log.append(&merged, &merged.change_bytes, &world)
                };
                let rel = append().map_err(|e| {
                    // The log still ends at `before`: no other merge could
                    // append while this one held the log's lock.
                    let undo = match merged.record.world_before {
                        Some(w) => put_back(&log.dir, w),
                        None => format!("put back the store's world.json of world {before}"),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Found by review: a write that failed (here the rename, onto a
    /// directory) left its temporary file behind, which in `log/` makes
    /// `WorldLog::open` refuse the whole log. It is removed again.
    #[test]
    fn a_failed_write_leaves_no_temporary_file() {
        let dir = std::env::temp_dir().join(format!("op_world-merge-tmp-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("log/00000001.json");
        fs::create_dir_all(path.join("inside")).unwrap();
        assert!(write_atomically(&path, b"{}").is_err());
        assert!(fs::symlink_metadata(tmp_path(&path)).is_err());
        assert_eq!(
            fs::read_dir(dir.join("log")).unwrap().count(),
            1,
            "only the directory that was there"
        );
        fs::remove_dir_all(&dir).ok();
    }
}
