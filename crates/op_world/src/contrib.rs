//! Contribution documents (RFC 0002): change sets, check reports and merge
//! records. All are canonical JSON (RFC 0001 section 5) named by their
//! content id, with a size limit for each kind (RFC 0001 section 7, see
//! [`crate::limits`]).

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::cjson;
use crate::id::ContentId;
use crate::keys::TileKey;
use crate::limits::Document;
use crate::manifest::Layer;
use crate::rules::{is_feature_id, is_sorted_unique, is_text};
use crate::{Error, Result};

/// Version of the contribution contract (RFC 0002).
pub const CONTRIBUTION_CONTRACT_VERSION: &str = "0.1.0";
/// Most changes in one change set.
pub const MAX_CHANGES: usize = 50;
/// Author of baseline refreshes produced by `tools/ingest`.
pub const INGEST_AUTHOR: &str = "streetwide:ingest";

/// Parses a document of kind `kind`: bytes over its limit are refused
/// before they are parsed, then the document must be canonical and have no
/// `problems`.
fn parse<T: Serialize + DeserializeOwned>(
    bytes: &[u8],
    kind: Document,
    problems: impl Fn(&T) -> Vec<String>,
) -> Result<T> {
    kind.check(bytes)?;
    let doc: T = cjson::from_canonical(bytes)?;
    let p = problems(&doc);
    if p.is_empty() {
        Ok(doc)
    } else {
        Err(Error::Invalid(p.join("; ")))
    }
}

/// The canonical bytes of a document of kind `kind` with these `problems`,
/// which must be none, and at most its limit long.
fn write<T: Serialize>(doc: &T, kind: Document, problems: Vec<String>) -> Result<Vec<u8>> {
    if !problems.is_empty() {
        return Err(Error::Invalid(problems.join("; ")));
    }
    let bytes = cjson::to_canonical(doc)?;
    kind.check(&bytes)?;
    Ok(bytes)
}

/// What a change does to one asset.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Op {
    Add,
    Remove,
    Replace,
}

/// One asset change.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub op: Op,
    pub tile: TileKey,
    pub layer: Layer,
    pub feature_id: String,
    /// The version the change expects to replace or remove; `null` to add.
    pub parent_version_id: Option<ContentId>,
    /// The new manifest; `null` to remove.
    pub manifest_id: Option<ContentId>,
}

impl Change {
    /// The order changes are listed in: tile key, layer and feature id, as
    /// strings in byte order.
    pub fn sort_key(&self) -> (String, &'static str, &str) {
        (self.tile.to_string(), self.layer.as_str(), &self.feature_id)
    }
}

/// A proposal to change the canonical world (RFC 0002 section 2).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeSet {
    pub format: String,
    pub contract_version: String,
    pub base_world_id: ContentId,
    pub title: String,
    /// Public handle, e.g. `github:octocat`.
    pub author: String,
    pub rationale: String,
    pub changes: Vec<Change>,
}

impl ChangeSet {
    pub const FORMAT: &'static str = "streetwide.change";

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        parse(bytes, Document::ChangeSet, Self::problems)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        write(self, Document::ChangeSet, self.problems())
    }

    pub fn id(&self) -> Result<ContentId> {
        Ok(ContentId::of(&self.to_bytes()?))
    }

    pub fn problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        if self.format != Self::FORMAT {
            p.push(format!("format must be {:?}", Self::FORMAT));
        }
        if self.contract_version != CONTRIBUTION_CONTRACT_VERSION {
            p.push(format!(
                "unknown contribution contract version {:?}",
                self.contract_version
            ));
        }
        for (name, v) in [("title", &self.title), ("rationale", &self.rationale)] {
            if !is_text(v) {
                p.push(format!("{name} must be non-empty text"));
            }
        }
        if !is_feature_id(&self.author) {
            p.push("author must be a handle such as github:<login>".into());
        }
        if self.changes.is_empty() || self.changes.len() > MAX_CHANGES {
            p.push(format!("a change set has 1..={MAX_CHANGES} changes"));
        }
        let keys: Vec<_> = self.changes.iter().map(Change::sort_key).collect();
        if !is_sorted_unique(&keys) {
            p.push("changes must be sorted by (tile, layer, feature_id) with no duplicates".into());
        }
        for c in &self.changes {
            let what = format!("{} {} {}", c.tile, c.layer.as_str(), c.feature_id);
            if !is_feature_id(&c.feature_id) {
                p.push(format!("{what}: invalid feature_id"));
            }
            let ok = match c.op {
                Op::Add => c.parent_version_id.is_none() && c.manifest_id.is_some(),
                Op::Replace => c.parent_version_id.is_some() && c.manifest_id.is_some(),
                Op::Remove => c.parent_version_id.is_some() && c.manifest_id.is_none(),
            };
            if !ok {
                p.push(format!(
                    "{what}: add needs a manifest and no parent, replace both, remove a parent and no manifest"
                ));
            }
        }
        p
    }
}

/// The outcome of one gate check.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Pass,
    Fail,
    /// The check does not apply to this change set.
    NotApplicable,
    /// The check applies but could not run (its tooling does not exist yet).
    NotRun,
    /// Left to reviewers (RFC 0002 section 5).
    Review,
}

/// One gate check in a report.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckResult {
    /// `G1` to `G10` (RFC 0002 section 4).
    pub id: String,
    pub name: String,
    pub outcome: Outcome,
    pub details: Vec<String>,
}

/// The gate's report on one change set (RFC 0002 section 4).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckReport {
    pub format: String,
    pub contract_version: String,
    pub gate_version: String,
    pub change_id: ContentId,
    /// The world the change set was applied to.
    pub world_id: ContentId,
    /// The world the change set would produce, if it could be built.
    pub candidate_world_id: Option<ContentId>,
    /// True if every check passed, does not apply, or is left to review.
    pub passed: bool,
    pub checks: Vec<CheckResult>,
}

impl CheckReport {
    pub const FORMAT: &'static str = "streetwide.check";

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        parse(bytes, Document::CheckReport, Self::problems)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        write(self, Document::CheckReport, self.problems())
    }

    pub fn problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        if self.format != Self::FORMAT {
            p.push(format!("format must be {:?}", Self::FORMAT));
        }
        if self.contract_version != CONTRIBUTION_CONTRACT_VERSION {
            p.push(format!(
                "unknown contribution contract version {:?}",
                self.contract_version
            ));
        }
        if !is_text(&self.gate_version) {
            p.push("gate_version must be text".into());
        }
        for c in &self.checks {
            if !is_text(&c.id) || !is_text(&c.name) || !c.details.iter().all(|d| is_text(d)) {
                p.push(format!(
                    "check {:?}: id, name and details must be text",
                    c.id
                ));
            }
        }
        let passed = self.checks.iter().all(|c| {
            matches!(
                c.outcome,
                Outcome::Pass | Outcome::NotApplicable | Outcome::Review
            )
        });
        if self.passed != passed {
            p.push(
                "passed must say whether every check passed, does not apply or is left to review"
                    .into(),
            );
        }
        p
    }
}

/// What a merge record decided.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    /// The first record: the initial world, built by the ingest.
    Baseline,
    Merged,
    /// Removal or revert for privacy, licence or legal reasons.
    Takedown,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Better,
    NotBetter,
    Blocked,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Review {
    /// Public handle, e.g. `github:<login>`.
    pub reviewer: String,
    pub verdict: Verdict,
    pub notes: String,
}

/// One entry of the world log (RFC 0002 section 6).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergeRecord {
    pub format: String,
    pub contract_version: String,
    pub decision: Decision,
    /// The change set; `null` for the baseline.
    pub change_id: Option<ContentId>,
    /// The world before; `null` for the baseline.
    pub world_before: Option<ContentId>,
    pub world_after: ContentId,
    /// The gate's report; `null` for the baseline.
    pub check_report_id: Option<ContentId>,
    /// Sorted by reviewer.
    pub reviews: Vec<Review>,
    pub reasons: String,
    /// The previous record; `null` for the baseline.
    pub previous_record_id: Option<ContentId>,
}

impl MergeRecord {
    pub const FORMAT: &'static str = "streetwide.merge";

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        parse(bytes, Document::MergeRecord, Self::problems)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        write(self, Document::MergeRecord, self.problems())
    }

    pub fn problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        if self.format != Self::FORMAT {
            p.push(format!("format must be {:?}", Self::FORMAT));
        }
        if self.contract_version != CONTRIBUTION_CONTRACT_VERSION {
            p.push(format!(
                "unknown contribution contract version {:?}",
                self.contract_version
            ));
        }
        if !is_text(&self.reasons) {
            p.push("reasons must be non-empty text".into());
        }
        let baseline = self.decision == Decision::Baseline;
        let links = [
            self.change_id.is_none(),
            self.world_before.is_none(),
            self.check_report_id.is_none(),
            self.previous_record_id.is_none(),
        ];
        if baseline && !links.iter().all(|n| *n) {
            p.push(
                "a baseline record has no change, previous world, report or previous record".into(),
            );
        }
        if !baseline && links.iter().any(|n| *n) {
            p.push("merged and takedown records name the change, both worlds, the report and the previous record".into());
        }
        let reviewers: Vec<&str> = self.reviews.iter().map(|r| r.reviewer.as_str()).collect();
        if !is_sorted_unique(&reviewers) {
            p.push("reviews must be sorted by reviewer, one per reviewer".into());
        }
        for r in &self.reviews {
            if !is_feature_id(&r.reviewer) || !is_text(&r.notes) {
                p.push(format!(
                    "review by {:?}: reviewer must be a handle and notes text",
                    r.reviewer
                ));
            }
        }
        if self.decision == Decision::Merged
            && !self.reviews.iter().any(|r| r.verdict == Verdict::Better)
        {
            p.push("a merge needs at least one review judging the change better".into());
        }
        if self.decision == Decision::Merged
            && self.reviews.iter().any(|r| r.verdict == Verdict::Blocked)
        {
            p.push("a blocked change cannot be merged".into());
        }
        p
    }
}

/// Checks that merge records form a world log: a baseline first, then each
/// record continuing from the previous one. Returns the current world.
pub fn verify_log(records: &[(ContentId, MergeRecord)]) -> Result<ContentId> {
    let mut previous: Option<(&ContentId, &MergeRecord)> = None;
    for (i, (id, r)) in records.iter().enumerate() {
        let fail = |m: &str| Err(Error::Invalid(format!("world log entry {i} ({id}): {m}")));
        match previous {
            None if r.decision != Decision::Baseline => {
                return fail("the log must start with a baseline");
            }
            None => {}
            Some(_) if r.decision == Decision::Baseline => {
                return fail("only the first record is a baseline");
            }
            Some((prev_id, prev)) => {
                if r.previous_record_id != Some(*prev_id) {
                    return fail("previous_record_id does not name the previous record");
                }
                if r.world_before != Some(prev.world_after) {
                    return fail("world_before is not the previous record's world_after");
                }
            }
        }
        previous = Some((id, r));
    }
    previous
        .map(|(_, r)| r.world_after)
        .ok_or_else(|| Error::Invalid("the world log is empty".into()))
}
