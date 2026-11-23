//! The complete contribution gate (RFC 0002 section 4, ADR-0009): the
//! world-data checks G1–G8 from `op_world`, plus the checks that need the
//! simulation:
//!
//! - **G9 walkability:** probe points on the public ways (transport
//!   ribbons) near the change must stay free for the player capsule;
//! - **G10 replays:** registered replays that pass near the change must
//!   still reach their waypoints in the candidate world.
//!
//! Like `op_world`'s gate, everything here is deterministic and the report
//! names tiles and positions, never local paths.

use std::path::Path;
use std::sync::Arc;

use op_kernel::ColliderMesh;
use op_kernel::geometry::{m_to_mm, sim_origin_mm};
use op_sim::probe::ProbeWorld;
use op_stream::{StoreSource, WorldIndex, collider_mesh};
use op_world::contrib::{ChangeSet, CheckReport, CheckResult, Outcome};
use op_world::glb::f32_to_mm;
use op_world::{BoundsMm, Collider, Layer, Store};

use crate::{Error, Replay, play};

/// Probes are taken this far around the changed assets (mm, horizontal).
pub const PROBE_MARGIN_MM: i64 = 2_000;
/// Probe feet sit this far above the ribbon surface (mm).
pub const PROBE_LIFT_MM: i64 = 20;
/// A replay is affected if one of its waypoints is this close to the
/// change (mm, horizontal).
pub const REPLAY_REACH_MM: i64 = 160_000;
/// A waypoint counts as reached within this distance (m).
pub const WAYPOINT_TOLERANCE_M: f64 = 2.0;
/// Waypoints are taken every this many ticks.
pub const WAYPOINT_EVERY: u64 = 60;
/// Colliders this far around the probes take part (mm): the reach of the
/// enclosure rays plus a margin.
const COLLIDER_MARGIN_MM: i64 = 35_000;

fn err(e: impl std::fmt::Display) -> Error {
    Error::World(e.to_string())
}

/// Runs the full gate on the change set in `bundle_dir` against the world in
/// `world_dir`, writing the candidate world to `candidate_dir` (which must
/// not exist). `replays` are the replays registered for the world; those
/// recorded in another world are ignored.
pub fn check_change(
    world_dir: &Path,
    bundle_dir: &Path,
    candidate_dir: &Path,
    replays: &[Replay],
) -> Result<CheckReport, Error> {
    let mut report = op_world::gate::run(world_dir, bundle_dir, candidate_dir).map_err(err)?;
    let pending = |r: &CheckReport| {
        r.checks
            .iter()
            .any(|c| (c.id == "G9" || c.id == "G10") && c.outcome == Outcome::NotRun)
    };
    if report.candidate_world_id.is_none() || !pending(&report) {
        return Ok(report);
    }
    let base_store = Store::new(world_dir);
    let candidate_store = Store::layered(
        candidate_dir,
        vec![bundle_dir.to_path_buf(), world_dir.to_path_buf()],
    );
    let base = WorldIndex::from_store(base_store.clone()).map_err(err)?;
    let candidate = WorldIndex::from_store(candidate_store.clone()).map_err(err)?;
    let change =
        ChangeSet::from_bytes(&std::fs::read(bundle_dir.join("change.json")).map_err(err)?)
            .map_err(err)?;
    let Some(region) = changed_region(&base, &candidate, &change) else {
        return Ok(report);
    };

    let g9 = walkability(&base, &candidate, region)?;
    let g10 = replay_paths(
        world_dir,
        &candidate_store,
        base.world_id(),
        region,
        replays,
    )?;
    for c in &mut report.checks {
        match c.id.as_str() {
            "G9" => *c = g9.clone(),
            "G10" => *c = g10.clone(),
            _ => {}
        }
    }
    report.passed = report.checks.iter().all(|c| {
        matches!(
            c.outcome,
            Outcome::Pass | Outcome::NotApplicable | Outcome::Review
        )
    });
    report.to_bytes().map_err(err)?;
    Ok(report)
}

fn union(a: Option<BoundsMm>, b: BoundsMm) -> Option<BoundsMm> {
    Some(a.map_or(b, |a| a.union(&b)))
}

/// Region-frame bounds of every asset the change set removes, replaces or
/// adds.
fn changed_region(
    base: &WorldIndex,
    candidate: &WorldIndex,
    change: &ChangeSet,
) -> Option<BoundsMm> {
    let mut region = None;
    for c in &change.changes {
        for index in [base, candidate] {
            let Some(tile) = index.tiles().iter().find(|t| t.key == c.tile) else {
                continue;
            };
            for a in tile
                .assets
                .iter()
                .filter(|a| a.layer == c.layer && a.feature_id == c.feature_id)
            {
                region = union(region, a.bounds_mm.translated(tile.origin_mm));
            }
        }
    }
    region
}

fn near(b: &BoundsMm, region: &BoundsMm, margin: i64) -> bool {
    [0, 2]
        .iter()
        .all(|&k| b.min[k] <= region.max[k] + margin && region.min[k] - margin <= b.max[k])
}

fn check(id: &str, name: &str, outcome: Outcome, details: Vec<String>) -> CheckResult {
    CheckResult {
        id: id.into(),
        name: name.into(),
        outcome,
        details,
    }
}

/// Collider meshes of `index` near `area`, in a sim frame at `origin`.
fn colliders(
    index: &WorldIndex,
    area: &BoundsMm,
    origin: [i64; 3],
) -> Result<Vec<Arc<ColliderMesh>>, Error> {
    let mut out = Vec::new();
    for t in index.tiles() {
        for a in t.assets.iter().filter(|a| a.collider == Collider::Trimesh) {
            if near(
                &a.bounds_mm.translated(t.origin_mm),
                area,
                COLLIDER_MARGIN_MM,
            ) {
                let glb = index.load_mesh(&a.content_id).map_err(err)?;
                let offset = [0, 1, 2].map(|k| t.origin_mm[k] - origin[k]);
                out.push(Arc::new(collider_mesh(&glb, offset).map_err(err)?));
            }
        }
    }
    Ok(out)
}

/// G9: probe points on the transport ribbons near the change, free in the
/// base world, must stay free in the candidate world.
fn walkability(
    base: &WorldIndex,
    candidate: &WorldIndex,
    region: BoundsMm,
) -> Result<CheckResult, Error> {
    let origin = [region.min[0], 0, region.min[2]];
    let mut probes: Vec<([i64; 3], String)> = Vec::new();
    for t in base.tiles() {
        for a in t
            .assets
            .iter()
            .filter(|a| a.layer == Layer::Transport && a.collider == Collider::None)
        {
            if !near(
                &a.bounds_mm.translated(t.origin_mm),
                &region,
                PROBE_MARGIN_MM,
            ) {
                continue;
            }
            let glb = base.load_mesh(&a.content_id).map_err(err)?;
            for p in glb.nodes.iter().flat_map(|n| &n.primitives) {
                for tri in p.indices.as_chunks::<3>().0 {
                    let mut c = [0i64; 3];
                    for &i in tri {
                        for k in 0..3 {
                            c[k] += f32_to_mm(p.positions[i as usize][k]).unwrap_or(0);
                        }
                    }
                    let feet = [
                        t.origin_mm[0] + c[0] / 3,
                        t.origin_mm[1] + c[1] / 3 + PROBE_LIFT_MM,
                        t.origin_mm[2] + c[2] / 3,
                    ];
                    let inside = [0, 2].iter().all(|&k| {
                        region.min[k] - PROBE_MARGIN_MM <= feet[k]
                            && feet[k] <= region.max[k] + PROBE_MARGIN_MM
                    });
                    if inside {
                        probes.push((feet, t.key.to_string()));
                    }
                }
            }
        }
    }
    if probes.is_empty() {
        return Ok(check(
            "G9",
            "walkability",
            Outcome::Pass,
            vec!["no public way within 2 m of the change".into()],
        ));
    }
    let mut area = region;
    for (feet, _) in &probes {
        area = area.union(&BoundsMm {
            min: *feet,
            max: *feet,
        });
    }
    let base_world = ProbeWorld::new(&colliders(base, &area, origin)?).map_err(err)?;
    let candidate_world = ProbeWorld::new(&colliders(candidate, &area, origin)?).map_err(err)?;
    let mut blocked = Vec::new();
    for (feet, tile) in &probes {
        let local = [0, 1, 2].map(|k| feet[k] - origin[k]);
        if !base_world.blocked(local) && candidate_world.blocked(local) {
            blocked.push(format!(
                "tile {tile}: a public way is newly blocked at ({:.1}, {:.1}, {:.1}) m in the region frame",
                feet[0] as f64 / 1000.0,
                feet[1] as f64 / 1000.0,
                feet[2] as f64 / 1000.0
            ));
        }
    }
    if blocked.is_empty() {
        Ok(check(
            "G9",
            "walkability",
            Outcome::Pass,
            vec![format!(
                "{} probe points on public ways stay free",
                probes.len()
            )],
        ))
    } else {
        let n = blocked.len();
        let mut details: Vec<String> = blocked.into_iter().take(5).collect();
        details.push(format!(
            "{n} of {} probe points newly blocked",
            probes.len()
        ));
        Ok(check("G9", "walkability", Outcome::Fail, details))
    }
}

/// Feet positions every [`WAYPOINT_EVERY`] ticks, in the region frame (mm).
fn waypoints(replay: &Replay, source: &mut StoreSource) -> Result<Vec<(u64, [i64; 3])>, Error> {
    let origin = sim_origin_mm(replay.header.spawn.position_mm);
    let mut out = Vec::new();
    play(&replay.header, &replay.inputs, source, |sim| {
        if sim.ticks() % WAYPOINT_EVERY == 0 {
            let f = sim.player().feet().map(m_to_mm);
            out.push((
                sim.ticks(),
                [f[0] + origin[0], f[1] + origin[1], f[2] + origin[2]],
            ));
        }
    })?;
    Ok(out)
}

/// G10: replays passing near the change must still reach their waypoints.
fn replay_paths(
    world_dir: &Path,
    candidate_store: &Store,
    world_id: op_world::ContentId,
    region: BoundsMm,
    replays: &[Replay],
) -> Result<CheckResult, Error> {
    let (mut details, mut failed, mut ran) = (Vec::new(), false, 0);
    for r in replays.iter().filter(|r| r.header.world_id == world_id) {
        let name = format!(
            "replay {}",
            r.checkpoints
                .last()
                .map_or("?".into(), |c| c.hash.hex()[..16].to_string())
        );
        let origin = sim_origin_mm(r.header.spawn.position_mm);
        let mut base_source = StoreSource::open(world_dir, origin).map_err(err)?;
        let path = waypoints(r, &mut base_source)?;
        let close = path.iter().any(|(_, p)| {
            [0, 2].iter().all(|&k| {
                region.min[k] - REPLAY_REACH_MM <= p[k] && p[k] <= region.max[k] + REPLAY_REACH_MM
            })
        });
        if !close {
            continue;
        }
        ran += 1;
        let mut candidate_source =
            StoreSource::from_store(candidate_store.clone(), origin).map_err(err)?;
        let moved = waypoints(r, &mut candidate_source)?;
        let off = path.iter().zip(&moved).find_map(|((tick, a), (_, b))| {
            let d = (0..3)
                .map(|k| {
                    let m = (a[k] - b[k]) as f64 / 1000.0;
                    m * m
                })
                .sum::<f64>()
                .sqrt();
            (d > WAYPOINT_TOLERANCE_M).then_some((*tick, d))
        });
        match off {
            Some((tick, d)) => {
                failed = true;
                details.push(format!(
                    "{name}: at tick {tick} the player is {d:.1} m off the recorded path"
                ));
            }
            None => details.push(format!("{name}: all {} waypoints reached", path.len())),
        }
    }
    let outcome = if ran == 0 {
        details.push("no registered replay passes near the change".into());
        Outcome::NotApplicable
    } else if failed {
        Outcome::Fail
    } else {
        Outcome::Pass
    };
    Ok(check("G10", "simulation", outcome, details))
}
