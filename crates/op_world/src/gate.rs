//! The contribution gate (RFC 0002 section 4).
//!
//! [`run`] applies a change set to the canonical world, builds the candidate
//! world (new tile indexes and world manifest layered over the bundle and
//! the canonical store) and runs the checks G1 to G10. Everything it does is
//! deterministic, and its messages name assets and ids, never local paths,
//! so the same inputs give the same report bytes on every machine.
//!
//! Checks that need the simulation (G9 walkability, G10 replays) cannot run
//! here (`op_world` does not depend on `op_sim`, ADR-0002). When a change
//! set touches colliders they are reported as `not_run`;
//! `op_replay check-change` runs the complete gate and fills them in
//! (ADR-0009).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::cjson::is_control;
use crate::contrib::{
    CONTRIBUTION_CONTRACT_VERSION, ChangeSet, CheckReport, CheckResult, INGEST_AUTHOR, Op, Outcome,
};
use crate::glb::{self, GlbMesh};
use crate::id::ContentId;
use crate::keys::TileKey;
use crate::limits::Document;
use crate::manifest::{AssetManifest, BoundsMm, Collider, Layer};
use crate::rules::{MAX_TEXT_CHARS, is_text};
use crate::store::{MAX_FILE_BYTES, ObjectKind, Store, read_file};
use crate::tile::{TileAsset, TileIndex};
use crate::validate::validate_store;
use crate::{Error, Result};

/// Bumped whenever the gate's output for some change set changes (0.1.1:
/// the water layer, RFC 0003; 0.1.2: bounded readers and G8, the fuzzing
/// round; 0.1.3: the size limits of JSON documents, RFC 0001 section 7).
pub const GATE_VERSION: &str = "0.1.3";

/// Licence of contributed content (ADR-0001).
pub const CONTRIBUTION_LICENSE: &str = "CC-BY-SA-4.0";
/// Licences that contributed assets may build on.
pub const ALLOWED_SOURCE_LICENSES: [&str; 4] = [
    "CC-BY-SA-4.0",
    "CC-BY-4.0",
    "CC0-1.0",
    "LicenseRef-swisstopo-OGD",
];
pub const CONTRIBUTIONS_DATASET: &str = "StreetWide contributions";
pub const COLLECTIVE_CREDIT: &str = "The StreetWide contributors";
/// Sources that are never acceptable (proprietary imagery and maps).
pub const FORBIDDEN_SOURCE_WORDS: [&str; 4] = ["google", "apple", "bing", "here.com"];

// Budgets and tolerances (RFC 0002 G7, G8).
pub const FACADE_MAX_TRIANGLES: usize = 50_000;
pub const FACADE_MAX_BYTES: u64 = 4 << 20;
pub const SHELL_MAX_TRIANGLES: usize = 100_000;
pub const TILE_MAX_TRIANGLES: usize = 1_000_000;
pub const TILE_MAX_COLLIDER_TRIANGLES: usize = 300_000;
pub const FACADE_MAX_DISTANCE_M: f64 = 0.5;
/// A facade may reach this far above its shell's highest point (mm).
pub const FACADE_ROOF_TOLERANCE_MM: i64 = 10;
/// Most grid steps G8 may take to measure one facade against its shell.
/// The facade's vertices fall in 2 m cells, and within a cell in 2 m height
/// buckets; each shell triangle is listed in the cells and buckets within
/// its reach. A grid step visits a column of cells, a cell or a bucket, and
/// each cell or bucket visit lists the triangle once, a 4-byte entry, so
/// the limit bounds the grid's memory: 80 MB of entries, up to twice that
/// while the lists grow. The sample facade takes 2,627 grid steps (312
/// vertices, a shell of 195 triangles). A facade at the G7 maximum (49,800
/// triangles) on that building with its roof divided into 76,800 triangles
/// takes about 440,000.
pub const FACADE_MAX_GRID_STEPS: u64 = 20_000_000;
/// Most vertex-to-triangle distances G8 may compute for one facade. A
/// vertex below the roof is placed as soon as one triangle is in reach, so
/// it searches its own bucket first, starting with the triangles last found
/// in reach there. It is measured against every triangle of its cell only
/// when none in its bucket is in reach, or when it is above the roof. The
/// sample takes 455 distances, and the facade at the G7 maximum above about
/// 1.1 million. Meshes crafted to need more are refused instead of keeping
/// the gate busy for minutes: 100 million distances take a few seconds.
pub const FACADE_MAX_DISTANCE_STEPS: u64 = 100_000_000;
pub const SHELL_FOOTPRINT_TOLERANCE_MM: i64 = 1_000;

const NAMES: [&str; 10] = [
    "format",
    "lineage",
    "layer policy",
    "licence",
    "provenance",
    "privacy",
    "budgets",
    "placement",
    "walkability",
    "simulation",
];

/// Collects the outcome of one check. The overall outcome is the most
/// serious one noted: fail, then not run, then review, then pass.
struct Check {
    outcomes: BTreeSet<Outcome>,
    details: Vec<String>,
}

impl Check {
    fn note(&mut self, outcome: Outcome, detail: impl Into<String>) {
        self.outcomes.insert(outcome);
        self.details.push(report_text(detail.into()));
    }

    fn pass(&mut self) {
        self.outcomes.insert(Outcome::Pass);
    }

    fn outcome(&self) -> Outcome {
        [
            Outcome::Fail,
            Outcome::NotRun,
            Outcome::Review,
            Outcome::Pass,
        ]
        .into_iter()
        .find(|o| self.outcomes.contains(o))
        .unwrap_or(Outcome::NotApplicable)
    }
}

/// `detail` as report text (non-empty, at most [`MAX_TEXT_CHARS`]
/// characters, no control characters). Details quote what a bundle holds,
/// such as an error naming an overlong value, and a crafted bundle must not
/// stop the gate from writing its report. Text is kept as it is; otherwise
/// control characters are escaped and an overlong detail is cut short.
fn report_text(detail: String) -> String {
    if is_text(&detail) {
        return detail;
    }
    let mut out = String::new();
    for c in detail.chars() {
        if is_control(c) {
            out.extend(c.escape_unicode());
        } else {
            out.push(c);
        }
    }
    if out.is_empty() {
        out.push('-');
    }
    if out.chars().count() > MAX_TEXT_CHARS {
        out = out.chars().take(MAX_TEXT_CHARS - 1).collect();
        out.push('\u{2026}');
    }
    out
}

struct Checks([Check; 10]);

impl Checks {
    fn new() -> Self {
        Checks(std::array::from_fn(|_| Check {
            outcomes: BTreeSet::new(),
            details: Vec::new(),
        }))
    }

    /// Check `G<n>`.
    fn g(&mut self, n: usize) -> &mut Check {
        &mut self.0[n - 1]
    }

    fn report(
        self,
        change_id: ContentId,
        world_id: ContentId,
        candidate: Option<ContentId>,
    ) -> CheckReport {
        let checks: Vec<CheckResult> = self
            .0
            .into_iter()
            .enumerate()
            .map(|(i, c)| CheckResult {
                id: format!("G{}", i + 1),
                name: NAMES[i].into(),
                outcome: c.outcome(),
                details: c.details,
            })
            .collect();
        let passed = checks.iter().all(|c| {
            matches!(
                c.outcome,
                Outcome::Pass | Outcome::NotApplicable | Outcome::Review
            )
        });
        CheckReport {
            format: CheckReport::FORMAT.into(),
            contract_version: CONTRIBUTION_CONTRACT_VERSION.into(),
            gate_version: GATE_VERSION.into(),
            change_id,
            world_id,
            candidate_world_id: candidate,
            passed,
            checks,
        }
    }

    /// Marks every check that has noted nothing yet as not run.
    fn not_run_rest(&mut self, why: &str) {
        for c in &mut self.0 {
            if c.outcomes.is_empty() {
                c.note(Outcome::NotRun, why);
            }
        }
    }
}

/// A new asset version proposed by the change set.
struct Proposed {
    manifest_id: ContentId,
    manifest: AssetManifest,
    mesh: GlbMesh,
}

/// Runs the gate on the change set in `bundle_dir` (`change.json` plus an
/// `objects/` directory) against the world in `world_dir`. The candidate
/// world is written to `candidate_dir`, which must not exist yet.
pub fn run(world_dir: &Path, bundle_dir: &Path, candidate_dir: &Path) -> Result<CheckReport> {
    let world = Store::new(world_dir);
    let (world_id, base) = world.read_world()?;
    // Read up to the limit of every file, so that a change set over its own
    // limit gets a report: `ChangeSet::from_bytes` refuses it in G1 before
    // parsing it.
    let change_path = bundle_dir.join("change.json");
    let change_bytes = read_file(&change_path, MAX_FILE_BYTES)
        .map_err(|e| Error::Io(format!("change.json: {e}")))?;
    let change_id = ContentId::of(&change_bytes);
    if candidate_dir.exists() {
        return Err(Error::Io(
            "the candidate directory must not exist yet".into(),
        ));
    }
    let mut checks = Checks::new();
    let change = match ChangeSet::from_bytes(&change_bytes) {
        Ok(c) => c,
        Err(e) => {
            checks.g(1).note(Outcome::Fail, format!("change.json: {e}"));
            checks.not_run_rest("the change set could not be read");
            return Ok(checks.report(change_id, world_id, None));
        }
    };
    let bundle = Store::layered(bundle_dir, vec![world_dir.to_path_buf()]);
    let baseline = change.author == INGEST_AUTHOR;

    // The base world's tiles.
    let mut tiles: BTreeMap<TileKey, (usize, TileIndex)> = BTreeMap::new();
    for (i, wt) in base.tiles.iter().enumerate() {
        let index = TileIndex::from_bytes(&world.read_document(&wt.tile_id, Document::TileIndex)?)?;
        tiles.insert(wt.tile, (i, index));
    }

    // Per change: lineage, the proposed manifests and meshes.
    let mut proposed: BTreeMap<usize, Proposed> = BTreeMap::new();
    let mut current_assets: BTreeMap<usize, TileAsset> = BTreeMap::new();
    for (i, c) in change.changes.iter().enumerate() {
        let what = format!("{} {} {}", c.tile, c.layer.as_str(), c.feature_id);
        let Some((_, index)) = tiles.get(&c.tile) else {
            checks.g(2).note(
                Outcome::Fail,
                format!("{what}: tile not in the world (adding tiles is not supported)"),
            );
            continue;
        };
        let current = index
            .assets
            .iter()
            .find(|a| a.layer == c.layer && a.feature_id == c.feature_id);
        match (c.op, current) {
            (Op::Add, None) => checks.g(2).pass(),
            (Op::Add, Some(_)) => checks.g(2).note(
                Outcome::Fail,
                format!("{what}: already exists; use replace"),
            ),
            (_, Some(cur)) if Some(cur.manifest_id) == c.parent_version_id => checks.g(2).pass(),
            (_, cur) => checks.g(2).note(
                Outcome::Fail,
                format!(
                    "{what}: parent {} is not the current version ({}); rebase the change set",
                    c.parent_version_id
                        .map_or("null".into(), |id| id.to_string()),
                    cur.map_or("none".into(), |a| a.manifest_id.to_string())
                ),
            ),
        }
        if let Some(cur) = current {
            current_assets.insert(i, cur.clone());
        }
        let Some(manifest_id) = c.manifest_id else {
            continue;
        };
        let loaded = bundle
            .read_document(&manifest_id, Document::AssetManifest)
            .and_then(|b| AssetManifest::from_bytes(&b))
            .and_then(|m| {
                let bytes = bundle.read_object(&m.content_id, ObjectKind::Glb)?;
                if bytes.len() as u64 != m.size_bytes {
                    return Err(Error::Invalid("size_bytes does not match the mesh".into()));
                }
                let mesh = glb::read(&bytes)?;
                if mesh.bounds_mm() != Some(m.bounds_mm) {
                    return Err(Error::Invalid("bounds_mm does not match the mesh".into()));
                }
                Ok((m, mesh))
            });
        match loaded {
            Ok((manifest, mesh)) => {
                if (manifest.tile, manifest.layer, manifest.feature_id.as_str())
                    != (c.tile, c.layer, c.feature_id.as_str())
                {
                    checks.g(2).note(
                        Outcome::Fail,
                        format!("{what}: the manifest describes another asset"),
                    );
                }
                if manifest.parent_version_id != c.parent_version_id {
                    checks.g(2).note(
                        Outcome::Fail,
                        format!("{what}: the manifest's parent_version_id differs from the change"),
                    );
                }
                proposed.insert(
                    i,
                    Proposed {
                        manifest_id,
                        manifest,
                        mesh,
                    },
                );
            }
            Err(e) => checks.g(1).note(Outcome::Fail, format!("{what}: {e}")),
        }
    }

    check_policy(&mut checks, &change, &current_assets, &proposed, baseline);

    // Build the candidate world.
    let candidate = Store::layered(
        candidate_dir,
        vec![bundle_dir.to_path_buf(), world_dir.to_path_buf()],
    );
    let candidate_id = match build_candidate(&candidate, &base, &tiles, &change, &proposed) {
        Ok(id) => Some(id),
        Err(e) => {
            checks
                .g(1)
                .note(Outcome::Fail, format!("candidate world: {e}"));
            None
        }
    };
    if candidate_id.is_some() {
        let report = validate_store(&candidate);
        if report.errors.is_empty() {
            checks.g(1).pass();
        }
        for e in report.errors {
            checks.g(1).note(Outcome::Fail, e);
        }
    }

    match candidate_id {
        Some(_) => {
            if let Err(e) = check_budgets_and_placement(
                &mut checks,
                &candidate,
                &change,
                &current_assets,
                &proposed,
            ) {
                checks
                    .g(7)
                    .note(Outcome::Fail, format!("candidate world: {e}"));
            }
        }
        None => {
            for g in [7, 8] {
                checks
                    .g(g)
                    .note(Outcome::NotRun, "the candidate world could not be built");
            }
        }
    }

    // G9, G10: only collider changes reach the simulation.
    let touches_colliders = change.changes.iter().enumerate().any(|(i, _)| {
        current_assets
            .get(&i)
            .is_some_and(|a| a.collider == Collider::Trimesh)
            || proposed
                .get(&i)
                .is_some_and(|p| p.manifest.physics.collider == Collider::Trimesh)
    });
    for g in [9, 10] {
        if touches_colliders {
            checks.g(g).note(
                Outcome::NotRun,
                "the change set touches colliders; the simulation-based checks are not implemented yet",
            );
        }
    }
    Ok(checks.report(change_id, world_id, candidate_id))
}

/// G3 layer policy, G4 licence, G5 provenance, G6 privacy.
fn check_policy(
    checks: &mut Checks,
    change: &ChangeSet,
    current: &BTreeMap<usize, TileAsset>,
    proposed: &BTreeMap<usize, Proposed>,
    baseline: bool,
) {
    for (i, c) in change.changes.iter().enumerate() {
        let what = format!("{} {} {}", c.tile, c.layer.as_str(), c.feature_id);
        // G3.
        if baseline {
            checks.g(3).note(
                Outcome::Review,
                format!("{what}: baseline refresh by {INGEST_AUTHOR}; a maintainer reviews it"),
            );
        } else {
            // The collider a contribution to this layer must have, or why the
            // layer takes no contributions (RFC 0002 section 3).
            let expected = match c.layer {
                Layer::Facade => Ok(Collider::None),
                Layer::Shell | Layer::Terrain => Ok(Collider::Trimesh),
                // Water comes from open data (RFC 0003): errors are fixed
                // there, and the next baseline refresh brings the fixes in.
                Layer::Water => Err(format!(
                    "{what}: the water layer does not accept contributions; fix it in the source (OpenStreetMap), and the next baseline refresh brings the fix in"
                )),
                Layer::Furniture | Layer::Interactivity | Layer::Interior | Layer::Transport => {
                    Err(format!(
                        "{what}: the {} layer does not accept contributions yet",
                        c.layer.as_str()
                    ))
                }
            };
            match expected {
                Err(why) => checks.g(3).note(Outcome::Fail, why),
                Ok(collider) => {
                    if current
                        .get(&i)
                        .is_some_and(|a| a.license.contains("ODbL-1.0"))
                    {
                        checks.g(3).note(
                            Outcome::Fail,
                            format!("{what}: derived from OpenStreetMap; fix it in OpenStreetMap instead"),
                        );
                    } else if proposed
                        .get(&i)
                        .is_some_and(|p| p.manifest.physics.collider != collider)
                    {
                        checks.g(3).note(
                            Outcome::Fail,
                            format!(
                                "{what}: {} assets must have collider {:?}",
                                c.layer.as_str(),
                                collider
                            ),
                        );
                    } else {
                        checks.g(3).pass();
                    }
                }
            }
        }
        // G6: facades show real places; reviewers check the privacy rules.
        if c.layer == Layer::Facade && c.op != Op::Remove {
            checks.g(6).note(
                Outcome::Review,
                format!("{what}: check the facade against the privacy rules (RFC 0002 G6)"),
            );
        }
        let Some(p) = proposed.get(&i) else { continue };
        let prov = &p.manifest.provenance;
        if baseline {
            continue;
        }
        // G4.
        let tokens: BTreeSet<&str> = prov.license.split(" AND ").collect();
        let mut wanted: BTreeSet<&str> = prov.sources.iter().map(|s| s.license.as_str()).collect();
        wanted.insert(CONTRIBUTION_LICENSE);
        let mut ok = true;
        for t in tokens
            .iter()
            .filter(|t| !ALLOWED_SOURCE_LICENSES.contains(t))
        {
            checks.g(4).note(
                Outcome::Fail,
                format!("{what}: licence {t} cannot be combined with contributed content"),
            );
            ok = false;
        }
        if tokens != wanted {
            checks.g(4).note(
                Outcome::Fail,
                format!(
                    "{what}: the licence must be {CONTRIBUTION_LICENSE} combined with every source's licence ({})",
                    wanted.iter().copied().collect::<Vec<_>>().join(" AND ")
                ),
            );
            ok = false;
        }
        for s in &prov.sources {
            let text = format!("{} {}", s.dataset, s.url).to_lowercase();
            if let Some(w) = FORBIDDEN_SOURCE_WORDS.iter().find(|w| text.contains(*w)) {
                checks.g(4).note(
                    Outcome::Fail,
                    format!("{what}: source {:?} looks proprietary ({w})", s.dataset),
                );
                ok = false;
            }
        }
        if ok {
            checks.g(4).pass();
        }
        // G5.
        let mut ok = true;
        if !prov
            .sources
            .iter()
            .any(|s| s.dataset == CONTRIBUTIONS_DATASET && s.source_ids.contains(&change.author))
        {
            checks.g(5).note(
                Outcome::Fail,
                format!(
                    "{what}: needs a {CONTRIBUTIONS_DATASET:?} source listing the author {}",
                    change.author
                ),
            );
            ok = false;
        }
        let credits: BTreeSet<&str> = prov.attribution.iter().map(String::as_str).collect();
        for needed in std::iter::once(COLLECTIVE_CREDIT)
            .chain(prov.sources.iter().map(|s| s.attribution.as_str()))
        {
            if !credits.contains(needed) {
                checks.g(5).note(
                    Outcome::Fail,
                    format!("{what}: attribution must include {needed:?}"),
                );
                ok = false;
            }
        }
        if ok {
            checks.g(5).pass();
        }
    }
}

/// Writes the candidate's tile indexes and world manifest; returns its id.
fn build_candidate(
    candidate: &Store,
    base: &crate::world::WorldManifest,
    tiles: &BTreeMap<TileKey, (usize, TileIndex)>,
    change: &ChangeSet,
    proposed: &BTreeMap<usize, Proposed>,
) -> Result<ContentId> {
    let mut world = base.clone();
    let mut edited: BTreeMap<TileKey, TileIndex> = BTreeMap::new();
    for (i, c) in change.changes.iter().enumerate() {
        let (_, base_index) = tiles
            .get(&c.tile)
            .ok_or_else(|| Error::Invalid(format!("tile {} is not in the world", c.tile)))?;
        let index = edited.entry(c.tile).or_insert_with(|| base_index.clone());
        index
            .assets
            .retain(|a| !(a.layer == c.layer && a.feature_id == c.feature_id));
        if c.op != Op::Remove {
            let p = proposed.get(&i).ok_or_else(|| {
                Error::Invalid(format!(
                    "{} {}: no readable manifest",
                    c.layer.as_str(),
                    c.feature_id
                ))
            })?;
            let m = &p.manifest;
            index.assets.push(TileAsset {
                layer: m.layer,
                feature_id: m.feature_id.clone(),
                manifest_id: p.manifest_id,
                content_id: m.content_id,
                license: m.provenance.license.clone(),
                collider: m.physics.collider,
                bounds_mm: m.bounds_mm,
            });
        }
    }
    for (key, index) in &mut edited {
        index
            .assets
            .sort_by(|a, b| (a.layer, &a.feature_id).cmp(&(b.layer, &b.feature_id)));
        let bounds = index
            .bounds()
            .ok_or_else(|| Error::Invalid(format!("tile {key} would be empty")))?;
        let tile_id = candidate.write_object(&index.to_bytes()?, ObjectKind::Json)?;
        let (pos, _) = tiles[key];
        world.tiles[pos].tile_id = tile_id;
        world.tiles[pos].bounds_mm = bounds;
    }
    // The world's credits are the union of every asset's credits.
    let mut credits = BTreeSet::new();
    for wt in &world.tiles {
        let index = match edited.get(&wt.tile) {
            Some(i) => i.clone(),
            None => {
                TileIndex::from_bytes(&candidate.read_document(&wt.tile_id, Document::TileIndex)?)?
            }
        };
        for a in &index.assets {
            let m = AssetManifest::from_bytes(
                &candidate.read_document(&a.manifest_id, Document::AssetManifest)?,
            )?;
            credits.extend(m.provenance.attribution);
        }
    }
    world.attribution = credits.into_iter().collect();
    candidate.write_world(&world)
}

fn tile_triangles(candidate: &Store, index: &TileIndex) -> Result<(usize, usize)> {
    let (mut all, mut colliders) = (0, 0);
    for a in &index.assets {
        let n =
            glb::read(&candidate.read_object(&a.content_id, ObjectKind::Glb)?)?.triangle_count();
        all += n;
        if a.collider == Collider::Trimesh {
            colliders += n;
        }
    }
    Ok((all, colliders))
}

/// G7 budgets and G8 placement, on the candidate world.
fn check_budgets_and_placement(
    checks: &mut Checks,
    candidate: &Store,
    change: &ChangeSet,
    current: &BTreeMap<usize, TileAsset>,
    proposed: &BTreeMap<usize, Proposed>,
) -> Result<()> {
    let (_, world) = candidate.read_world()?;
    let mut indexes = BTreeMap::new();
    for wt in &world.tiles {
        indexes.insert(
            wt.tile,
            TileIndex::from_bytes(&candidate.read_document(&wt.tile_id, Document::TileIndex)?)?,
        );
    }
    for (i, c) in change.changes.iter().enumerate() {
        let what = format!("{} {} {}", c.tile, c.layer.as_str(), c.feature_id);
        let Some(p) = proposed.get(&i) else { continue };
        // G7 per asset.
        let triangles = p.mesh.triangle_count();
        let limit = match c.layer {
            Layer::Facade => Some((FACADE_MAX_TRIANGLES, Some(FACADE_MAX_BYTES))),
            Layer::Shell => Some((SHELL_MAX_TRIANGLES, None)),
            Layer::Furniture
            | Layer::Interactivity
            | Layer::Interior
            | Layer::Terrain
            | Layer::Transport
            | Layer::Water => None,
        };
        let mut over_budget = true;
        match limit {
            Some((tris, _)) if triangles > tris => checks.g(7).note(
                Outcome::Fail,
                format!("{what}: {triangles} triangles, at most {tris}"),
            ),
            Some((_, Some(b))) if p.manifest.size_bytes > b => checks.g(7).note(
                Outcome::Fail,
                format!("{what}: {} bytes, at most {b}", p.manifest.size_bytes),
            ),
            _ => {
                over_budget = false;
                checks.g(7).pass();
            }
        }
        // G8.
        let index = &indexes[&c.tile];
        match c.layer {
            Layer::Facade => match index
                .assets
                .iter()
                .find(|a| a.layer == Layer::Shell && a.feature_id == c.feature_id)
            {
                None => checks.g(8).note(
                    Outcome::Fail,
                    format!("{what}: the building has no shell in this tile"),
                ),
                // G7 has refused it already; measuring it would only cost time.
                Some(_) if over_budget => checks.g(8).note(
                    Outcome::NotRun,
                    format!("{what}: not checked, the facade is over its G7 budget"),
                ),
                Some(shell) => {
                    // G7 checks the shells this change set proposes, and
                    // refuses one over its budget: measuring against it
                    // would only cost time. Any other shell, such as a
                    // canonical one, is measured within G8's step limits.
                    let refused = proposed
                        .values()
                        .find(|s| s.manifest_id == shell.manifest_id)
                        .map(|s| s.mesh.triangle_count())
                        .filter(|&n| n > SHELL_MAX_TRIANGLES);
                    match refused {
                        Some(n) => checks.g(8).note(
                            Outcome::NotRun,
                            format!(
                                "{what}: not checked, the shell this change set proposes has {n} triangles, more than the G7 budget of {SHELL_MAX_TRIANGLES}"
                            ),
                        ),
                        None => {
                            check_facade_placement(checks.g(8), &what, candidate, shell, &p.mesh)?
                        }
                    }
                }
            },
            Layer::Shell => match current.get(&i) {
                None => checks.g(8).note(
                    Outcome::Review,
                    format!("{what}: a new building; reviewers check the evidence"),
                ),
                Some(old) => {
                    let (a, b) = (old.bounds_mm, p.manifest.bounds_mm);
                    let moved = [0, 2]
                        .iter()
                        .map(|&k| (a.min[k] - b.min[k]).abs().max((a.max[k] - b.max[k]).abs()))
                        .max()
                        .unwrap_or(0);
                    if moved <= SHELL_FOOTPRINT_TOLERANCE_MM {
                        checks.g(8).pass();
                    } else {
                        checks.g(8).note(
                            Outcome::Review,
                            format!("{what}: the footprint moves by {moved} mm; reviewers check the evidence"),
                        );
                    }
                }
            },
            Layer::Terrain => match current.get(&i) {
                Some(old) => {
                    let old_mesh =
                        glb::read(&candidate.read_object(&old.content_id, ObjectKind::Glb)?)?;
                    let missing = missing_boundary_vertices(&old_mesh, &p.mesh);
                    if missing == 0 {
                        checks.g(8).pass();
                    } else {
                        checks.g(8).note(
                            Outcome::Fail,
                            format!("{what}: {missing} edge vertices differ from the old terrain; tiles would crack"),
                        );
                    }
                }
                None => checks.g(8).note(
                    Outcome::Review,
                    format!("{what}: new terrain; reviewers check the evidence"),
                ),
            },
            // No placement rule.
            Layer::Furniture
            | Layer::Interactivity
            | Layer::Interior
            | Layer::Transport
            | Layer::Water => {}
        }
    }
    // G7 per affected tile.
    let affected: BTreeSet<TileKey> = change.changes.iter().map(|c| c.tile).collect();
    for key in affected {
        let (all, colliders) = tile_triangles(candidate, &indexes[&key])?;
        if all > TILE_MAX_TRIANGLES {
            checks.g(7).note(
                Outcome::Fail,
                format!("tile {key}: {all} triangles, at most {TILE_MAX_TRIANGLES}"),
            );
        }
        if colliders > TILE_MAX_COLLIDER_TRIANGLES {
            checks.g(7).note(
                Outcome::Fail,
                format!("tile {key}: {colliders} collider triangles, at most {TILE_MAX_COLLIDER_TRIANGLES}"),
            );
        }
    }
    Ok(())
}

/// G8 for a facade within its budget: measures it against `shell` in the
/// candidate world.
fn check_facade_placement(
    check: &mut Check,
    what: &str,
    candidate: &Store,
    shell: &TileAsset,
    facade: &GlbMesh,
) -> Result<()> {
    let shell_mesh = glb::read(&candidate.read_object(&shell.content_id, ObjectKind::Glb)?)?;
    match facade_distance(facade, &shell_mesh, shell.bounds_mm, FACADE_STEP_LIMITS).0 {
        Err(TooComplex::Grid) => check.note(
            Outcome::Fail,
            format!(
                "{what}: too complex to check: measuring the facade against its shell takes more than {FACADE_MAX_GRID_STEPS} grid steps"
            ),
        ),
        Err(TooComplex::Distances) => check.note(
            Outcome::Fail,
            format!(
                "{what}: too complex to check: measuring the facade against its shell takes more than {FACADE_MAX_DISTANCE_STEPS} vertex-to-triangle distances"
            ),
        ),
        Ok((0, _)) => check.pass(),
        Ok((far, worst)) => check.note(
            Outcome::Fail,
            format!(
                "{what}: {far} vertices are more than {FACADE_MAX_DISTANCE_M} m from the shell or more than {FACADE_ROOF_TOLERANCE_MM} mm above its highest point (worst {worst:.3} m)"
            ),
        ),
    }
    Ok(())
}

type Tri = [[f64; 3]; 3];

fn triangles(mesh: &GlbMesh) -> Vec<Tri> {
    let mut out = Vec::new();
    for p in mesh.nodes.iter().flat_map(|n| &n.primitives) {
        let v = |i: u32| p.positions[i as usize].map(f64::from);
        for t in p.indices.as_chunks::<3>().0 {
            out.push([v(t[0]), v(t[1]), v(t[2])]);
        }
    }
    out
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Distance from `p` to triangle `t` (Ericson, Real-Time Collision
/// Detection, 5.1.5).
fn point_triangle_distance(p: [f64; 3], t: &Tri) -> f64 {
    let [a, b, c] = *t;
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let at = |s: f64, u: f64| {
        [
            a[0] + ab[0] * s + ac[0] * u,
            a[1] + ab[1] * s + ac[1] * u,
            a[2] + ab[2] * s + ac[2] * u,
        ]
    };
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    let q = if d1 <= 0.0 && d2 <= 0.0 {
        a
    } else {
        let bp = sub(p, b);
        let (d3, d4) = (dot(ab, bp), dot(ac, bp));
        let cp = sub(p, c);
        let (d5, d6) = (dot(ab, cp), dot(ac, cp));
        let vc = d1 * d4 - d3 * d2;
        let vb = d5 * d2 - d1 * d6;
        let va = d3 * d6 - d5 * d4;
        if d3 >= 0.0 && d4 <= d3 {
            b
        } else if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
            at(d1 / (d1 - d3), 0.0)
        } else if d6 >= 0.0 && d5 <= d6 {
            c
        } else if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
            at(0.0, d2 / (d2 - d6))
        } else if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
            let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
            [
                b[0] + (c[0] - b[0]) * w,
                b[1] + (c[1] - b[1]) * w,
                b[2] + (c[2] - b[2]) * w,
            ]
        } else {
            let denom = 1.0 / (va + vb + vc);
            at(vb * denom, vc * denom)
        }
    };
    let d = sub(p, q);
    dot(d, d).sqrt()
}

/// Steps G8 takes to measure one facade against its shell, or its limits
/// (see [`FACADE_MAX_GRID_STEPS`] and [`FACADE_MAX_DISTANCE_STEPS`]).
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct Steps {
    grid: u64,
    distances: u64,
}

/// G8's step limits.
const FACADE_STEP_LIMITS: Steps = Steps {
    grid: FACADE_MAX_GRID_STEPS,
    distances: FACADE_MAX_DISTANCE_STEPS,
};

/// The step limit that measuring a facade would go over.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TooComplex {
    Grid,
    Distances,
}

/// What G8 finds for one facade: how many of its vertices are too far from
/// the shell or above its roof, and the worst distance (m); or the step
/// limit that finding out would go over.
type Placement = std::result::Result<(usize, f64), TooComplex>;

/// Counts one step; false once the count is over `limit`.
fn step(count: &mut u64, limit: u64) -> bool {
    *count += 1;
    *count <= limit
}

/// One 2 m cell of G8's grid, holding facade vertices.
#[derive(Default)]
struct GridCell {
    /// The shell triangles within reach of the cell, in shell order.
    listed: Vec<u32>,
    /// The same, per 2 m height bucket holding facade vertices.
    by_height: BTreeMap<i64, Bucket>,
}

/// A 2 m height bucket of a cell of G8's grid.
#[derive(Default)]
struct Bucket {
    /// The shell triangles within reach of the bucket, in shell order.
    listed: Vec<u32>,
    /// The last few of them found in reach of a vertex, the latest first.
    recent: Vec<u32>,
}

/// How many triangles a bucket remembers as found in reach. Nearby facade
/// vertices are usually in reach of the same few shell triangles, so they
/// are tried first: for a facade at the G7 maximum on the sample's building
/// with its roof in 76,800 triangles, this cuts the distances G8 computes
/// from 90 million to 1.1 million.
const RECENT: usize = 8;

/// Where `facade` is in relation to `shell` ([`Placement`]), and the steps
/// taken.
///
/// The facade's vertices fall in 2 m cells, and within a cell in 2 m height
/// buckets. Each shell triangle is listed in the cells and buckets within
/// [`FACADE_MAX_DISTANCE_M`] of its bounding box. A vertex below the roof
/// needs one triangle in reach, and such a triangle is always listed in the
/// vertex's bucket, so the bucket is searched first: the triangles last
/// found in reach there, then all of them. A vertex with none in reach
/// there, or above the roof, is measured against every triangle listed in
/// its cell. The result is exactly that of measuring every vertex against
/// every triangle listed in its cell.
fn facade_distance(
    facade: &GlbMesh,
    shell: &GlbMesh,
    shell_bounds: BoundsMm,
    limits: Steps,
) -> (Placement, Steps) {
    const CELL: f64 = 2.0;
    let tris = triangles(shell);
    let cell = |x: f64| (x / CELL).floor() as i64;
    let mut steps = Steps::default();
    let positions = || {
        facade
            .nodes
            .iter()
            .flat_map(|n| &n.primitives)
            .flat_map(|p| &p.positions)
    };
    // Only the cells and buckets the facade's vertices fall in are ever
    // looked up, so each shell triangle is listed in those of them within
    // its reach, found column by column (x, then z, then height). Listing
    // it in every cell it reaches would cost as much as its area (a
    // contributed shell can be kilometres wide), and one range over (x, z)
    // pairs would visit whole columns of the facade.
    // Triangle numbers fit in 4 bytes: a glb of at most 64 MiB holds fewer
    // than 6 million triangles.
    let mut columns: BTreeMap<i64, BTreeMap<i64, GridCell>> = BTreeMap::new();
    for v in positions() {
        columns
            .entry(cell(f64::from(v[0])))
            .or_default()
            .entry(cell(f64::from(v[2])))
            .or_default()
            .by_height
            .entry(cell(f64::from(v[1])))
            .or_default();
    }
    for (i, t) in (0u32..).zip(&tris) {
        let lo =
            |k: usize| t.iter().map(|v| v[k]).fold(f64::INFINITY, f64::min) - FACADE_MAX_DISTANCE_M;
        let hi = |k: usize| {
            t.iter().map(|v| v[k]).fold(f64::NEG_INFINITY, f64::max) + FACADE_MAX_DISTANCE_M
        };
        let [(x0, x1), (y0, y1), (z0, z1)] = [0, 1, 2].map(|k| (cell(lo(k)), cell(hi(k))));
        if x0 > x1 || y0 > y1 || z0 > z1 {
            continue; // not for finite positions, which glb::read guarantees
        }
        for (_, column) in columns.range_mut(x0..=x1) {
            if !step(&mut steps.grid, limits.grid) {
                return (Err(TooComplex::Grid), steps);
            }
            for (_, c) in column.range_mut(z0..=z1) {
                if !step(&mut steps.grid, limits.grid) {
                    return (Err(TooComplex::Grid), steps);
                }
                c.listed.push(i);
                for (_, bucket) in c.by_height.range_mut(y0..=y1) {
                    if !step(&mut steps.grid, limits.grid) {
                        return (Err(TooComplex::Grid), steps);
                    }
                    bucket.listed.push(i);
                }
            }
        }
    }
    // Heights are compared in exact millimetres (glb positions are
    // millimetre-snapped), so a vertex level with the roof is not "above" it.
    let roof_mm = shell_bounds.max[1] + FACADE_ROOF_TOLERANCE_MM;
    // Vertices with no shell triangle in reach count as this far away.
    const NOWHERE: f64 = 1.0e6;
    let (mut far, mut worst) = (0usize, 0.0f64);
    for v in positions() {
        let y_mm = glb::f32_to_mm(v[1]).unwrap_or(i64::MAX);
        let above = y_mm > roof_mm;
        let v = v.map(f64::from);
        let here = columns
            .get_mut(&cell(v[0]))
            .and_then(|column| column.get_mut(&cell(v[2])));
        let (listed, bucket) = match here {
            Some(c) => (c.listed.as_slice(), c.by_height.get_mut(&cell(v[1]))),
            None => (&[][..], None),
        };
        let mut d = NOWHERE;
        // Below the roof, the exact distance no longer matters once one
        // triangle is in reach, and such a triangle is listed in the
        // vertex's bucket: that is searched first. Above the roof, the
        // distance counts towards the worst one.
        let mut measured: &[u32] = &[];
        if let Some(b) = bucket.filter(|_| !above) {
            let mut found = None;
            for (k, &i) in b.recent.iter().enumerate() {
                if !step(&mut steps.distances, limits.distances) {
                    return (Err(TooComplex::Distances), steps);
                }
                if point_triangle_distance(v, &tris[i as usize]) <= FACADE_MAX_DISTANCE_M {
                    found = Some(k);
                    break;
                }
            }
            if let Some(k) = found {
                b.recent[..=k].rotate_right(1);
                continue;
            }
            let mut hit = None;
            for &i in &b.listed {
                if !step(&mut steps.distances, limits.distances) {
                    return (Err(TooComplex::Distances), steps);
                }
                let di = point_triangle_distance(v, &tris[i as usize]);
                d = d.min(di);
                if di <= FACADE_MAX_DISTANCE_M {
                    hit = Some(i);
                    break;
                }
            }
            if let Some(i) = hit {
                b.recent.insert(0, i);
                b.recent.truncate(RECENT);
                continue;
            }
            measured = &b.listed;
        }
        // Then the rest of the cell. Its list holds the bucket's triangles
        // in the same order, and the minimum does not depend on the order
        // (`f64::min` drops NaN, and no distance is -0.0).
        let mut done = measured.iter().peekable();
        for i in listed {
            if done.next_if_eq(&i).is_some() {
                continue;
            }
            if !step(&mut steps.distances, limits.distances) {
                return (Err(TooComplex::Distances), steps);
            }
            d = d.min(point_triangle_distance(v, &tris[*i as usize]));
        }
        let above_m = (y_mm.saturating_sub(roof_mm)) as f64 / 1000.0;
        if d > FACADE_MAX_DISTANCE_M || above {
            far += 1;
            worst = worst.max(d.max(above_m));
        }
    }
    (Ok((far, worst)), steps)
}

/// Vertices on the old mesh's open edges (edges used by one triangle) that
/// the new mesh does not have at exactly the same position.
fn missing_boundary_vertices(old: &GlbMesh, new: &GlbMesh) -> usize {
    let key = |v: [f32; 3]| v.map(f32::to_bits);
    let mut boundary = BTreeSet::new();
    for p in old.nodes.iter().flat_map(|n| &n.primitives) {
        let mut edges: BTreeMap<(u32, u32), u32> = BTreeMap::new();
        for t in p.indices.as_chunks::<3>().0 {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *edges.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        for ((a, b), n) in edges {
            if n == 1 {
                boundary.insert(key(p.positions[a as usize]));
                boundary.insert(key(p.positions[b as usize]));
            }
        }
    }
    let have: BTreeSet<_> = new
        .nodes
        .iter()
        .flat_map(|n| &n.primitives)
        .flat_map(|p| p.positions.iter().map(|v| key(*v)))
        .collect();
    boundary.difference(&have).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glb::{Material, Node, Primitive, mm_to_f32};

    /// The G8 grid as first written, each triangle listed in every cell it
    /// reaches: work and memory grow with the shell's area. Kept to check
    /// that the bounded grid decides exactly the same.
    fn facade_distance_by_area(
        facade: &GlbMesh,
        shell: &GlbMesh,
        shell_bounds: BoundsMm,
    ) -> (usize, f64) {
        const CELL: f64 = 2.0;
        let tris = triangles(shell);
        let cell = |x: f64| (x / CELL).floor() as i64;
        let mut grid: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
        for (i, t) in tris.iter().enumerate() {
            let lo = |k: usize| {
                t.iter().map(|v| v[k]).fold(f64::INFINITY, f64::min) - FACADE_MAX_DISTANCE_M
            };
            let hi = |k: usize| {
                t.iter().map(|v| v[k]).fold(f64::NEG_INFINITY, f64::max) + FACADE_MAX_DISTANCE_M
            };
            for gx in cell(lo(0))..=cell(hi(0)) {
                for gz in cell(lo(2))..=cell(hi(2)) {
                    grid.entry((gx, gz)).or_default().push(i);
                }
            }
        }
        let roof_mm = shell_bounds.max[1] + FACADE_ROOF_TOLERANCE_MM;
        const NOWHERE: f64 = 1.0e6;
        let (mut far, mut worst) = (0usize, 0.0f64);
        for p in facade.nodes.iter().flat_map(|n| &n.primitives) {
            for v in &p.positions {
                let y_mm = glb::f32_to_mm(v[1]).unwrap_or(i64::MAX);
                let v = v.map(f64::from);
                let d = grid
                    .get(&(cell(v[0]), cell(v[2])))
                    .map(|ids| {
                        ids.iter()
                            .map(|&i| point_triangle_distance(v, &tris[i]))
                            .fold(NOWHERE, f64::min)
                    })
                    .unwrap_or(NOWHERE);
                let above_m = (y_mm.saturating_sub(roof_mm)) as f64 / 1000.0;
                if d > FACADE_MAX_DISTANCE_M || y_mm > roof_mm {
                    far += 1;
                    worst = worst.max(d.max(above_m));
                }
            }
        }
        (far, worst)
    }

    #[test]
    fn report_text_keeps_text_and_repairs_the_rest() {
        let fine = "tile 17/1/2: \u{e9} \"quoted\"".to_string();
        assert_eq!(report_text(fine.clone()), fine);
        let long = "x".repeat(MAX_TEXT_CHARS);
        assert_eq!(report_text(long.clone()), long);
        let longer = report_text("\u{e9}".repeat(MAX_TEXT_CHARS + 1));
        assert_eq!(longer.chars().count(), MAX_TEXT_CHARS);
        assert!(longer.ends_with('\u{2026}') && is_text(&longer));
        assert_eq!(report_text("a\u{1}b\u{7f}".into()), "a\\u{1}b\\u{7f}");
        assert_eq!(report_text(String::new()), "-");
    }

    /// One node, one primitive with these millimetre positions.
    fn mesh(positions_mm: &[[i64; 3]], indices: Vec<u32>) -> GlbMesh {
        GlbMesh {
            nodes: vec![Node {
                name: "a:b".into(),
                primitives: vec![Primitive {
                    material: Material::Wall,
                    positions: positions_mm.iter().map(|p| p.map(mm_to_f32)).collect(),
                    normals: vec![[0.0, 1.0, 0.0]; positions_mm.len()],
                    indices,
                }],
            }],
        }
    }

    /// Nodes of primitives, each a material and millimetre positions taken
    /// three by three as triangles. Shells list their primitives in palette
    /// order, so roof triangles come first.
    fn shell_of(nodes: Vec<Vec<(Material, Vec<[i64; 3]>)>>) -> GlbMesh {
        GlbMesh {
            nodes: nodes
                .into_iter()
                .enumerate()
                .map(|(k, primitives)| Node {
                    name: format!("a:{k}"),
                    primitives: primitives
                        .into_iter()
                        .map(|(material, points)| Primitive {
                            material,
                            indices: (0..points.len() as u32 / 3 * 3).collect(),
                            normals: vec![[0.0, 1.0, 0.0]; points.len()],
                            positions: points.iter().map(|p| p.map(mm_to_f32)).collect(),
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    const UNLIMITED: Steps = Steps {
        grid: u64::MAX,
        distances: u64::MAX,
    };

    /// SplitMix64, seeded: a stream of numbers below `n`.
    fn numbers(seed: u64) -> impl FnMut(u64) -> u64 {
        let mut state = seed;
        move |n| {
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            (z ^ (z >> 31)) % n
        }
    }

    /// G8 without limits gives exactly what the grid by area gives, which
    /// is returned.
    fn decides_as_by_area(facade: &GlbMesh, shell: &GlbMesh, what: &str) -> (usize, f64) {
        let bounds = shell.bounds_mm().expect("bounds");
        let (new, _) = facade_distance(facade, shell, bounds, UNLIMITED);
        let new = new.expect("measured without limits");
        let old = facade_distance_by_area(facade, shell, bounds);
        assert!(
            new.0 == old.0 && new.1.to_bits() == old.1.to_bits(),
            "{what}: {new:?} differs from {old:?}"
        );
        old
    }

    /// Found by review while fuzzing the readers: one contributed shell
    /// triangle 2000 km across made G8 list it in 10^12 grid cells, so the
    /// gate ran for days or ran out of memory.
    #[test]
    fn regression_facade_distance_does_not_grow_with_the_shell_area() {
        let h = 1_000_000_000; // 1000 km in mm
        let shell = mesh(&[[-h, 0, -h], [h, 0, -h], [-h, 0, h]], vec![0, 1, 2]);
        let facade = mesh(
            &[[-1000, 0, -1000], [0, 0, -1000], [-1000, 0, 0]],
            vec![0, 1, 2],
        );
        let bounds = shell.bounds_mm().expect("bounds");
        // Two columns, three cells and three buckets; one distance per
        // vertex.
        assert_eq!(
            facade_distance(&facade, &shell, bounds, UNLIMITED),
            (
                Ok((0, 0.0)),
                Steps {
                    grid: 8,
                    distances: 3
                }
            )
        );
    }

    /// `count` copies of one triangle.
    fn stacked(corners_mm: [[i64; 3]; 3], count: usize) -> GlbMesh {
        let positions: Vec<[i64; 3]> = corners_mm.iter().copied().cycle().take(3 * count).collect();
        let indices = (0..positions.len() as u32).collect();
        mesh(&positions, indices)
    }

    /// Facade vertices at these millimetre positions, as degenerate
    /// triangles (facade_distance looks at vertices only).
    fn facade_at(points_mm: impl Iterator<Item = [i64; 3]>) -> GlbMesh {
        let positions: Vec<[i64; 3]> = points_mm.collect();
        let indices = (0..positions.len() as u32 / 3 * 3).collect();
        mesh(&positions, indices)
    }

    /// Found by review: the facade's cells were one range of (x, z) pairs,
    /// which visits every cell of a column whatever its z. Small shell
    /// triangles whose reach crosses a column boundary, far from the facade
    /// along z, visited every facade cell of that column: 100,000 triangles
    /// against 150,000 cells kept the gate busy for minutes.
    #[test]
    fn regression_facade_distance_crossing_a_column_boundary() {
        // 10,000 triangles at x = 0..1 m (reach -0.5..1.5 m: columns -1
        // and 0), 1 km beyond the facade in z.
        let shell = stacked(
            [[0, 0, -1_000_000], [1000, 0, -1_000_000], [0, 0, -999_000]],
            10_000,
        );
        // 10,002 facade vertices at x = 1 m (column 0), one per cell along z.
        let facade = facade_at((0..10_002).map(|k| [1000, 0, 2000 * k]));
        assert_eq!(
            decides_as_by_area(&facade, &shell, "column"),
            (10_002, 1.0e6)
        );
        // One step per triangle (its one column); the range over (x, z)
        // pairs took 10,000 per triangle.
        let bounds = shell.bounds_mm().expect("bounds");
        assert_eq!(
            facade_distance(&facade, &shell, bounds, UNLIMITED).1,
            Steps {
                grid: 10_000,
                distances: 0
            }
        );
    }

    /// Found by review, the same cause: long, thin shell triangles across
    /// every column of the facade, far from it along z.
    #[test]
    fn regression_facade_distance_with_long_thin_triangles() {
        // 100 columns of 201 cells each.
        let facade =
            facade_at((0..100).flat_map(|x| (0..201).map(move |z| [2000 * x, 0, 2000 * z])));
        let shell = stacked(
            [
                [-10_000, 0, -1_000_000],
                [210_000, 0, -1_000_000],
                [-10_000, 0, -999_990],
            ],
            1000,
        );
        decides_as_by_area(&facade, &shell, "long and thin");
        // One step per column per triangle; the range over (x, z) pairs
        // took 20,000 per triangle.
        let bounds = shell.bounds_mm().expect("bounds");
        let grid = Steps {
            grid: 100_000,
            distances: 0,
        };
        assert_eq!(facade_distance(&facade, &shell, bounds, UNLIMITED).1, grid);
        // The grid stops at its limit.
        let limits = Steps {
            grid: 99_999,
            distances: u64::MAX,
        };
        assert_eq!(
            facade_distance(&facade, &shell, bounds, limits),
            (Err(TooComplex::Grid), grid)
        );
    }

    /// Found by review: stacked shell triangles made every facade vertex
    /// meet every triangle, so G8's work grew with their product however
    /// the grid was built. It is now bounded, and a crafted worst case is
    /// refused as soon as it reaches the limit.
    #[test]
    fn regression_facade_distance_work_is_bounded() {
        // 2,000 copies of a 20 m triangle; 2,001 facade vertices 0.6 m
        // below it, so each is measured against every copy, once.
        let shell = stacked(
            [[0, 10_000, 0], [20_000, 10_000, 0], [0, 10_000, 20_000]],
            2000,
        );
        let facade = facade_at((0..2001).map(|k| [1000 + k % 9, 9_400, 1000 + k % 7]));
        let bounds = shell.bounds_mm().expect("bounds");
        let limits = Steps {
            grid: u64::MAX,
            distances: 1_000_000,
        };
        assert_eq!(
            facade_distance(&facade, &shell, bounds, limits),
            (
                Err(TooComplex::Distances),
                Steps {
                    grid: 6_000,
                    distances: 1_000_001
                }
            )
        );
        // With room for the 4 million distances it needs, the same answer
        // as the grid by area.
        let old = decides_as_by_area(&facade, &shell, "stacked");
        assert_eq!(old.0, 2001);
        assert_eq!(
            facade_distance(&facade, &shell, bounds, UNLIMITED).1,
            Steps {
                grid: 6_000,
                distances: 2001 * 2000
            }
        );
        // Vertices on the triangle stop at the first copy.
        let on = facade_at((0..2001).map(|k| [1000 + k % 9, 10_000, 1000 + k % 7]));
        assert_eq!(decides_as_by_area(&on, &shell, "on"), (0, 0.0));
        assert_eq!(
            facade_distance(&on, &shell, bounds, UNLIMITED).1.distances,
            2001
        );
    }

    /// Found by review: a shell lists its roof triangles before its walls,
    /// and a wall vertex went through every roof triangle of its cell
    /// before it met a wall. Now only those of its height bucket.
    #[test]
    fn a_wall_vertex_does_not_go_through_the_roof() {
        // A flat roof of 2 x 1250 triangles over 10 m x 10 m, 6 m up, then
        // one wall.
        let roof = (0..1250)
            .flat_map(|_| {
                [
                    [[0, 6000, 0], [10_000, 6000, 0], [0, 6000, 10_000]],
                    [[10_000, 6000, 0], [10_000, 6000, 10_000], [0, 6000, 10_000]],
                ]
            })
            .flatten()
            .collect();
        let wall = vec![[0, 0, 0], [10_000, 0, 0], [0, 6000, 0]];
        let shell = shell_of(vec![vec![(Material::Roof, roof), (Material::Wall, wall)]]);
        // 300 vertices 30 mm off the wall, from 0.3 to 3.29 m high.
        let facade = facade_at((0..300).map(|k| [1000 + 3 * k, 300 + 10 * k, -30]));
        assert_eq!(decides_as_by_area(&facade, &shell, "wall"), (0, 0.0));
        let bounds = shell.bounds_mm().expect("bounds");
        // The roof reaches the buckets from 4 m up, so each vertex meets the
        // wall first: one distance each, where its cell's list put 2,500
        // roof triangles first.
        assert_eq!(
            facade_distance(&facade, &shell, bounds, UNLIMITED)
                .1
                .distances,
            300
        );
    }

    #[test]
    fn facade_distance_decides_as_the_grid_by_area() {
        // Random shells and facades a few tens of metres across, with
        // vertices near, far from and above the shell.
        let mut next = numbers(0x6A09_E667_F3BC_C908);
        let mut close = 0;
        for case in 0..2000 {
            let (shell_count, facade_count) = (3 * (1 + next(8)), 3 * (1 + next(10)));
            let mut points = |count: u64, half_mm: u64, high_mm: u64| -> Vec<[i64; 3]> {
                (0..count)
                    .map(|_| {
                        [
                            next(2 * half_mm) as i64 - half_mm as i64,
                            next(high_mm) as i64,
                            next(2 * half_mm) as i64 - half_mm as i64,
                        ]
                    })
                    .collect()
            };
            let shell_points = points(shell_count, 30_000, 20_000);
            let facade_points = points(facade_count, 35_000, 22_000);
            let shell = mesh(&shell_points, (0..shell_points.len() as u32).collect());
            let facade = mesh(&facade_points, (0..facade_points.len() as u32).collect());
            let (far, _) = decides_as_by_area(&facade, &shell, &format!("case {case}"));
            close += usize::from(far < facade_points.len());
        }
        // The cases must include facades with vertices on the shell.
        assert!(close >= 200, "only {close} cases with a vertex in reach");
    }

    /// As above with dense shells: many triangles per cell, so a vertex in
    /// reach stops early, and facade vertices on, near, below and above
    /// the shell's triangles.
    #[test]
    fn facade_distance_decides_as_the_grid_by_area_on_dense_shells() {
        let mut next = numbers(0xBB67_AE85_84CA_A73B);
        let (mut early, mut far_cases) = (0, 0);
        for case in 0..1000 {
            let shell_points: Vec<[i64; 3]> = (0..3 * (20 + next(200)))
                .map(|_| [next(8000) as i64, next(6000) as i64, next(8000) as i64])
                .collect();
            let shell = mesh(&shell_points, (0..shell_points.len() as u32).collect());
            // Facade vertices near shell vertices, moved by up to 1.2 m.
            let facade_points: Vec<[i64; 3]> = (0..3 * (1 + next(40)))
                .map(|_| {
                    let base = shell_points[next(shell_points.len() as u64) as usize];
                    base.map(|c| c + next(2400) as i64 - 1200)
                })
                .collect();
            let facade = mesh(&facade_points, (0..facade_points.len() as u32).collect());
            let (far, _) = decides_as_by_area(&facade, &shell, &format!("case {case}"));
            early += usize::from(far < facade_points.len());
            far_cases += usize::from(far > 0);
        }
        assert!(early >= 500 && far_cases >= 250, "{early} {far_cases}");
    }

    /// As above with shells shaped like buildings, anywhere around the
    /// origin (negative coordinates too): one or two nodes, each with a
    /// roof of many small triangles listed first, then walls, then ground.
    /// Facade vertices lie near the walls at any height, near the roof,
    /// and at or just above the highest point allowed.
    #[test]
    fn facade_distance_decides_as_the_grid_by_area_on_buildings() {
        let mut next = numbers(0x3C6E_F372_FE94_F82B);
        let (mut placed, mut misplaced, mut above) = (0, 0, 0);
        for case in 0..1000 {
            let mut nodes = Vec::new();
            let mut walls: Vec<([i64; 3], [i64; 3], i64)> = Vec::new();
            let mut roof_points = Vec::new();
            for _ in 0..1 + next(2) {
                let (x0, z0) = (next(40_000) as i64 - 20_000, next(40_000) as i64 - 20_000);
                let (w, d, h) = (
                    2000 + next(10_000) as i64,
                    2000 + next(10_000) as i64,
                    2500 + next(12_000) as i64,
                );
                let corners = [[x0, z0], [x0 + w, z0], [x0 + w, z0 + d], [x0, z0 + d]];
                let mut roof = Vec::new();
                for _ in 0..10 + next(150) {
                    let (cx, cz) = (x0 + next(w as u64) as i64, z0 + next(d as u64) as i64);
                    let cy = h + next(3000) as i64;
                    for _ in 0..3 {
                        let p = [
                            cx + next(1500) as i64 - 750,
                            cy + next(400) as i64 - 200,
                            cz + next(1500) as i64 - 750,
                        ];
                        roof.push(p);
                        roof_points.push(p);
                    }
                }
                let mut wall = Vec::new();
                for side in 0..4 {
                    let ([ax, az], [bx, bz]) = (corners[side], corners[(side + 1) % 4]);
                    wall.extend([[ax, 0, az], [bx, 0, bz], [bx, h, bz]]);
                    wall.extend([[ax, 0, az], [bx, h, bz], [ax, h, az]]);
                    walls.push(([ax, 0, az], [bx, 0, bz], h));
                }
                let ground = vec![[x0, 0, z0], [x0 + w, 0, z0], [x0, 0, z0 + d]];
                nodes.push(vec![
                    (Material::Roof, roof),
                    (Material::Wall, wall),
                    (Material::Ground, ground),
                ]);
            }
            let shell = shell_of(nodes);
            let top = shell.bounds_mm().expect("bounds").max[1];
            let mut facade_points = Vec::new();
            for _ in 0..3 * (1 + next(60)) {
                let p = match next(10) {
                    // Near a wall: anywhere along it, off it by up to 0.8 m.
                    0..=6 => {
                        let (a, b, h) = walls[next(walls.len() as u64) as usize];
                        let t = next(1001) as i64;
                        [
                            a[0] + (b[0] - a[0]) * t / 1000 + next(1600) as i64 - 800,
                            next((h + 4000) as u64) as i64,
                            a[2] + (b[2] - a[2]) * t / 1000 + next(1600) as i64 - 800,
                        ]
                    }
                    // Near the roof.
                    7 | 8 => {
                        let r = roof_points[next(roof_points.len() as u64) as usize];
                        r.map(|c| c + next(1400) as i64 - 700)
                    }
                    // At the highest point allowed, or 1 mm above it.
                    _ => {
                        let r = roof_points[next(roof_points.len() as u64) as usize];
                        [r[0], top + 10 + next(2) as i64, r[2]]
                    }
                };
                facade_points.push(p);
            }
            let facade = facade_at(facade_points.iter().copied());
            let (far, _) = decides_as_by_area(&facade, &shell, &format!("case {case}"));
            placed += usize::from(far < facade_points.len());
            misplaced += usize::from(far > 0);
            above += usize::from(facade_points.iter().any(|p| p[1] > top + 10));
        }
        assert!(
            placed >= 900 && misplaced >= 500 && above >= 300,
            "{placed} {misplaced} {above}"
        );
    }

    /// Fixed cases at the edges: triangles on and beside cell and bucket
    /// boundaries, negative coordinates, vertices exactly 0.5 m away (along
    /// x, z and y), on cell lines, and at the highest point allowed.
    #[test]
    fn facade_distance_decides_as_the_grid_by_area_at_the_edges() {
        let mut cases = 0;
        for tx in [-4000, -2000, -1000, -500, 0, 1500, 2000] {
            for tz in [-4000, -2001, -2000, -1999, 0, 2000] {
                for ty in [-2500, -500, 1500, 2000, 2499, 2500] {
                    // A horizontal roof triangle, listed first, then a wall.
                    let shell = shell_of(vec![vec![
                        (
                            Material::Roof,
                            vec![[tx, ty, tz], [tx + 1000, ty, tz], [tx, ty, tz + 1000]],
                        ),
                        (
                            Material::Wall,
                            vec![
                                [tx, ty - 3000, tz],
                                [tx + 1000, ty - 3000, tz],
                                [tx, ty, tz],
                            ],
                        ),
                    ]]);
                    let mut points = Vec::new();
                    for dx in [-501, -500, -499, 0, 1000, 1499, 1500, 1501] {
                        for dz in [-501, -500, 0, 499, 500, 501] {
                            for dy in [-3501, -3500, -2000, -501, -500, -499, 0, 10, 11, 499, 500] {
                                points.push([tx + dx, ty + dy, tz + dz]);
                            }
                        }
                    }
                    let facade = facade_at(points.into_iter());
                    decides_as_by_area(&facade, &shell, &format!("{tx} {ty} {tz}"));
                    cases += 1;
                }
            }
        }
        assert_eq!(cases, 252);
    }

    /// Steps G8 takes for the committed sample facade, which pins the
    /// claims in the documentation of [`FACADE_MAX_GRID_STEPS`] and
    /// [`FACADE_MAX_DISTANCE_STEPS`].
    #[test]
    fn the_sample_facade_takes_few_steps() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let world = Store::new(root.join("fixtures/world"));
        let bundle = Store::layered(
            root.join("fixtures/changes/facade-sample"),
            vec![root.join("fixtures/world")],
        );
        let change = ChangeSet::from_bytes(
            &std::fs::read(root.join("fixtures/changes/facade-sample/change.json")).unwrap(),
        )
        .unwrap();
        let c = &change.changes[0];
        let manifest = AssetManifest::from_bytes(
            &bundle
                .read_object(&c.manifest_id.unwrap(), ObjectKind::Json)
                .unwrap(),
        )
        .unwrap();
        let facade = glb::read(
            &bundle
                .read_object(&manifest.content_id, ObjectKind::Glb)
                .unwrap(),
        )
        .unwrap();
        let (_, w) = world.read_world().unwrap();
        let wt = w.tiles.iter().find(|t| t.tile == c.tile).unwrap();
        let index =
            TileIndex::from_bytes(&world.read_object(&wt.tile_id, ObjectKind::Json).unwrap())
                .unwrap();
        let shell = index
            .assets
            .iter()
            .find(|a| a.layer == Layer::Shell && a.feature_id == c.feature_id)
            .unwrap();
        let shell_mesh = glb::read(
            &world
                .read_object(&shell.content_id, ObjectKind::Glb)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(decides_as_by_area(&facade, &shell_mesh, "sample"), (0, 0.0));
        let measure = |limits| facade_distance(&facade, &shell_mesh, shell.bounds_mm, limits);
        // 312 vertices against a shell of 195 triangles.
        let needed = measure(FACADE_STEP_LIMITS).1;
        assert_eq!(
            needed,
            Steps {
                grid: 2627,
                distances: 455
            }
        );
        assert_eq!(measure(needed).0, Ok((0, 0.0)));
        let fewer = |grid, distances| measure(Steps { grid, distances }).0;
        assert_eq!(
            fewer(needed.grid - 1, needed.distances),
            Err(TooComplex::Grid)
        );
        assert_eq!(
            fewer(needed.grid, needed.distances - 1),
            Err(TooComplex::Distances)
        );
    }
}
