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
use crate::manifest::{AssetManifest, BoundsMm, Collider, Layer};
use crate::rules::{MAX_TEXT_CHARS, is_text};
use crate::store::{MAX_FILE_BYTES, ObjectKind, Store, read_file};
use crate::tile::{TileAsset, TileIndex};
use crate::validate::validate_store;
use crate::{Error, Result};

/// Bumped whenever the gate's output for some change set changes (0.1.1:
/// the water layer, RFC 0003).
pub const GATE_VERSION: &str = "0.1.1";

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
/// Most steps G8 may take to measure one facade against its shell: grid
/// steps (finding the facade cells each shell triangle reaches) and
/// vertex-to-triangle distances together. The sample facade takes about
/// 3,600 (312 vertices, a shell of 195 triangles), and a vertex within
/// reach of the shell usually stops at one of the first triangles it meets.
/// Meshes crafted to need more are refused instead of keeping the gate busy
/// for minutes; the limit also bounds the grid's lists (one 4-byte entry
/// per grid step, 80 MB at most).
pub const FACADE_MAX_CHECK_STEPS: u64 = 20_000_000;
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
        let index = TileIndex::from_bytes(&world.read_object(&wt.tile_id, ObjectKind::Json)?)?;
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
            .read_object(&manifest_id, ObjectKind::Json)
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
            None => TileIndex::from_bytes(&candidate.read_object(&wt.tile_id, ObjectKind::Json)?)?,
        };
        for a in &index.assets {
            let m = AssetManifest::from_bytes(
                &candidate.read_object(&a.manifest_id, ObjectKind::Json)?,
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
            TileIndex::from_bytes(&candidate.read_object(&wt.tile_id, ObjectKind::Json)?)?,
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
                    let shell_mesh =
                        glb::read(&candidate.read_object(&shell.content_id, ObjectKind::Glb)?)?;
                    let shell_triangles = shell_mesh.triangle_count();
                    let measured = (shell_triangles <= SHELL_MAX_TRIANGLES).then(|| {
                        facade_distance(
                            &p.mesh,
                            &shell_mesh,
                            shell.bounds_mm,
                            FACADE_MAX_CHECK_STEPS,
                        )
                    });
                    match measured {
                        None => checks.g(8).note(
                            Outcome::NotRun,
                            format!(
                                "{what}: not checked, the shell has {shell_triangles} triangles, more than the G7 budget of {SHELL_MAX_TRIANGLES}"
                            ),
                        ),
                        Some(None) => checks.g(8).note(
                            Outcome::Fail,
                            format!(
                                "{what}: too complex to check, more than {FACADE_MAX_CHECK_STEPS} steps to measure the facade against the shell; simplify the facade"
                            ),
                        ),
                        Some(Some((0, _))) => checks.g(8).pass(),
                        Some(Some((far, worst))) => checks.g(8).note(
                            Outcome::Fail,
                            format!(
                                "{what}: {far} vertices are more than {FACADE_MAX_DISTANCE_M} m from the shell or more than {FACADE_ROOF_TOLERANCE_MM} mm above its highest point (worst {worst:.3} m)"
                            ),
                        ),
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

/// How many facade vertices are too far from the shell or above its roof,
/// and the worst distance (m); `None` if finding out takes more than
/// `max_steps` steps (see [`FACADE_MAX_CHECK_STEPS`]).
///
/// The shell's triangles are put in a grid of 2 m cells, each listed in
/// every cell within [`FACADE_MAX_DISTANCE_M`] of its bounding box, and a
/// vertex is measured against the triangles listed in its cell.
fn facade_distance(
    facade: &GlbMesh,
    shell: &GlbMesh,
    shell_bounds: BoundsMm,
    max_steps: u64,
) -> Option<(usize, f64)> {
    const CELL: f64 = 2.0;
    let tris = triangles(shell);
    let cell = |x: f64| (x / CELL).floor() as i64;
    let mut steps = 0u64;
    // Only the cells the facade's vertices fall in are ever looked up, so
    // each shell triangle is listed in those of them within its reach,
    // found column by column (x, then z). Listing it in every cell it
    // reaches would cost as much as its area (a contributed shell can be
    // kilometres wide), and one range over (x, z) pairs would visit whole
    // columns of the facade.
    // Triangle numbers: a shell within its budget has far fewer than 2^32.
    let mut columns: BTreeMap<i64, BTreeMap<i64, Vec<u32>>> = BTreeMap::new();
    for v in facade
        .nodes
        .iter()
        .flat_map(|n| &n.primitives)
        .flat_map(|p| &p.positions)
    {
        columns
            .entry(cell(f64::from(v[0])))
            .or_default()
            .entry(cell(f64::from(v[2])))
            .or_default();
    }
    for (i, t) in (0u32..).zip(&tris) {
        let lo =
            |k: usize| t.iter().map(|v| v[k]).fold(f64::INFINITY, f64::min) - FACADE_MAX_DISTANCE_M;
        let hi = |k: usize| {
            t.iter().map(|v| v[k]).fold(f64::NEG_INFINITY, f64::max) + FACADE_MAX_DISTANCE_M
        };
        let (x0, x1, z0, z1) = (cell(lo(0)), cell(hi(0)), cell(lo(2)), cell(hi(2)));
        if x0 > x1 || z0 > z1 {
            continue; // not for finite positions, which glb::read guarantees
        }
        for (_, column) in columns.range_mut(x0..=x1) {
            steps += 1;
            for (_, ids) in column.range_mut(z0..=z1) {
                steps += 1;
                ids.push(i);
            }
        }
        if steps > max_steps {
            return None;
        }
    }
    // Heights are compared in exact millimetres (glb positions are
    // millimetre-snapped), so a vertex level with the roof is not "above" it.
    let roof_mm = shell_bounds.max[1] + FACADE_ROOF_TOLERANCE_MM;
    // Vertices with no shell triangle in reach count as this far away.
    const NOWHERE: f64 = 1.0e6;
    let (mut far, mut worst) = (0usize, 0.0f64);
    for p in facade.nodes.iter().flat_map(|n| &n.primitives) {
        for v in &p.positions {
            let y_mm = glb::f32_to_mm(v[1]).unwrap_or(i64::MAX);
            let above = y_mm > roof_mm;
            let v = v.map(f64::from);
            let mut d = NOWHERE;
            let listed = columns
                .get(&cell(v[0]))
                .and_then(|column| column.get(&cell(v[2])));
            for &i in listed.into_iter().flatten() {
                steps += 1;
                d = d.min(point_triangle_distance(v, &tris[i as usize]));
                // In reach: the exact distance no longer matters, unless
                // the vertex is above the roof (then it counts towards the
                // worst distance).
                if d <= FACADE_MAX_DISTANCE_M && !above {
                    break;
                }
            }
            if steps > max_steps {
                return None;
            }
            let above_m = (y_mm.saturating_sub(roof_mm)) as f64 / 1000.0;
            if d > FACADE_MAX_DISTANCE_M || above {
                far += 1;
                worst = worst.max(d.max(above_m));
            }
        }
    }
    Some((far, worst))
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
        assert_eq!(
            facade_distance(&facade, &shell, bounds, 100),
            Some((0, 0.0))
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

    fn assert_same(new: Option<(usize, f64)>, old: (usize, f64)) {
        let new = new.expect("measured within the step budget");
        assert!(
            new.0 == old.0 && new.1.to_bits() == old.1.to_bits(),
            "{new:?} differs from {old:?}"
        );
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
        // 10,000 facade vertices at x = 1 m (column 0), one per cell along z.
        let facade = facade_at((0..10_002).map(|k| [1000, 0, 2000 * k]));
        let bounds = shell.bounds_mm().expect("bounds");
        let old = facade_distance_by_area(&facade, &shell, bounds);
        // About one step per triangle; the range over (x, z) pairs took
        // 10,000 per triangle.
        assert_same(facade_distance(&facade, &shell, bounds, 30_000), old);
    }

    /// Found by review, the same cause: long, thin shell triangles across
    /// every column of the facade, far from it along z.
    #[test]
    fn regression_facade_distance_with_long_thin_triangles() {
        // 100 columns of 200 cells each.
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
        let bounds = shell.bounds_mm().expect("bounds");
        let old = facade_distance_by_area(&facade, &shell, bounds);
        // One step per column per triangle; the range over (x, z) pairs
        // took 20,000 per triangle.
        assert_same(facade_distance(&facade, &shell, bounds, 200_000), old);
    }

    /// Found by review: stacked shell triangles made every facade vertex
    /// meet every triangle, so G8's work grew with their product however
    /// the grid was built. It is now bounded.
    #[test]
    fn regression_facade_distance_work_is_bounded() {
        // 2,000 copies of a 20 m triangle; 2,000 facade vertices 0.6 m
        // below it, so each is measured against every copy.
        let shell = stacked(
            [[0, 10_000, 0], [20_000, 10_000, 0], [0, 10_000, 20_000]],
            2000,
        );
        let facade = facade_at((0..2001).map(|k| [1000 + k % 9, 9_400, 1000 + k % 7]));
        let bounds = shell.bounds_mm().expect("bounds");
        assert_eq!(facade_distance(&facade, &shell, bounds, 1_000_000), None);
        // With room for the 4 million steps it needs, the same answer as
        // the grid by area.
        let old = facade_distance_by_area(&facade, &shell, bounds);
        assert_eq!(old.0, 2001);
        assert_same(facade_distance(&facade, &shell, bounds, 5_000_000), old);
        // Vertices on the triangle stop at the first copy.
        let on = facade_at((0..2001).map(|k| [1000 + k % 9, 10_000, 1000 + k % 7]));
        let old = facade_distance_by_area(&on, &shell, bounds);
        assert_eq!(old, (0, 0.0));
        assert_same(facade_distance(&on, &shell, bounds, 10_000), old);
    }

    #[test]
    fn facade_distance_decides_as_the_grid_by_area() {
        // SplitMix64, seeded: random shells and facades a few tens of
        // metres across, with vertices near, far from and above the shell.
        let mut state = 0x6A09_E667_F3BC_C908_u64;
        let mut next = move |n: u64| {
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            (z ^ (z >> 31)) % n
        };
        let mut close = 0;
        for case in 0..500 {
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
            let bounds = shell.bounds_mm().expect("bounds");
            let new = facade_distance(&facade, &shell, bounds, u64::MAX).expect("measured");
            let old = facade_distance_by_area(&facade, &shell, bounds);
            assert!(
                new.0 == old.0 && new.1.to_bits() == old.1.to_bits(),
                "case {case}: {new:?} differs from {old:?}"
            );
            if new.0 < facade_points.len() {
                close += 1;
            }
        }
        // The cases must include facades with vertices on the shell.
        assert!(close >= 50, "only {close} cases with a vertex in reach");
    }

    /// As above with dense shells: many triangles per cell, so a vertex in
    /// reach stops early, and facade vertices on, near, below and above
    /// the shell's triangles.
    #[test]
    fn facade_distance_decides_as_the_grid_by_area_on_dense_shells() {
        let mut state = 0xBB67_AE85_84CA_A73B_u64;
        let mut next = move |n: u64| {
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            (z ^ (z >> 31)) % n
        };
        let (mut early, mut far_cases) = (0, 0);
        for case in 0..200 {
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
            let bounds = shell.bounds_mm().expect("bounds");
            let new = facade_distance(&facade, &shell, bounds, u64::MAX).expect("measured");
            let old = facade_distance_by_area(&facade, &shell, bounds);
            assert!(
                new.0 == old.0 && new.1.to_bits() == old.1.to_bits(),
                "case {case}: {new:?} differs from {old:?}"
            );
            early += usize::from(new.0 < facade_points.len());
            far_cases += usize::from(new.0 > 0);
        }
        assert!(early >= 100 && far_cases >= 50, "{early} {far_cases}");
    }

    /// Steps G8 takes for the committed sample facade, which pins the
    /// claim in [`FACADE_MAX_CHECK_STEPS`]'s documentation.
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
        let measure = |steps| facade_distance(&facade, &shell_mesh, shell.bounds_mm, steps);
        // 3,617 steps: 312 vertices against a shell of 195 triangles.
        assert_eq!(measure(FACADE_MAX_CHECK_STEPS), Some((0, 0.0)));
        assert_eq!(measure(4_000), Some((0, 0.0)));
        assert_eq!(measure(3_000), None);
    }
}
