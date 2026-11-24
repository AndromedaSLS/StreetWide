//! Merging change sets into a world store and writing world-log records
//! (RFC 0002 section 6).
//!
//! These functions work on local directories; where the canonical store and
//! the world log are hosted is decided separately (ADR-0010). A merge never
//! leaves the store half-changed by a refused change set: the gate runs and
//! the record is built and validated first, then the new objects are copied
//! in, and `world.json` is written last.

use std::path::Path;

use crate::contrib::{
    CONTRIBUTION_CONTRACT_VERSION, ChangeSet, CheckReport, Decision, MergeRecord, Outcome, Review,
};
use crate::gate;
use crate::id::ContentId;
use crate::manifest::AssetManifest;
use crate::store::{MAX_FILE_BYTES, ObjectKind, Store, read_file};
use crate::{Error, Result};

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
/// `bundle_dir` into the world store at `world_dir`.
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
    mut reviews: Vec<Review>,
    reasons: &str,
    scratch_dir: &Path,
) -> Result<Merged> {
    if decision == Decision::Baseline {
        return Err(Error::Invalid("use baseline() to start a world log".into()));
    }
    let previous = MergeRecord::from_bytes(previous_record)?;
    let previous_id = ContentId::of(previous_record);
    let world = Store::new(world_dir);
    let (world_id, _) = world.read_world()?;
    if previous.world_after != world_id {
        return Err(Error::Invalid(format!(
            "the store holds world {world_id}, but the log's last record ends at {}",
            previous.world_after
        )));
    }

    let candidate_dir = scratch_dir.join("candidate");
    let report = gate::run(world_dir, bundle_dir, &candidate_dir)?;
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

    // Copy the new objects in: the change set's manifests and meshes, then
    // the candidate's tile indexes; `world.json` last.
    let change = ChangeSet::from_bytes(
        &read_file(&bundle_dir.join("change.json"), MAX_FILE_BYTES)
            .map_err(|e| Error::Io(format!("change.json: {e}")))?,
    )?;
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
