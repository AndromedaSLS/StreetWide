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
//!
//! [`probe_public_ways`] and [`replay_waypoints`] are the measurements
//! behind G9 and G10. `op_replay compare-worlds` ([`crate::compare`]) uses
//! them to compare any two versions of a world, and [`full_gate`] passes
//! the complete gate to `op_world::merge::merge_with` for `op_replay merge`.

use std::path::Path;
use std::sync::Arc;

use op_kernel::ColliderMesh;
use op_kernel::geometry::{m_to_mm, sim_origin_mm};
use op_sim::probe::ProbeWorld;
use op_stream::{StoreSource, WorldIndex, collider_mesh};
use op_world::contrib::{ChangeSet, CheckReport, CheckResult, Outcome};
use op_world::glb::f32_to_mm;
use op_world::{BoundsMm, Collider, ContentId, Layer, Store, TileKey};

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

/// The complete gate ([`check_change`] with `replays`) in the form
/// `op_world::merge::merge_with` takes, so that `op_replay merge` can merge
/// change sets that touch colliders. The merge record's `check_report_id`
/// then names the full G1 to G10 report.
pub fn full_gate(
    replays: &[Replay],
) -> impl FnOnce(&Path, &Path, &Path) -> op_world::Result<CheckReport> + '_ {
    move |world_dir: &Path, bundle_dir: &Path, candidate_dir: &Path| {
        check_change(world_dir, bundle_dir, candidate_dir, replays)
            .map_err(|e| op_world::Error::Invalid(e.to_string()))
    }
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

/// A probe point on a public way: where the player capsule's feet go.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Probe {
    pub tile: TileKey,
    /// Region frame (mm).
    pub feet_mm: [i64; 3],
}

/// Probe points on the public ways near a region, tested in two worlds.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Walkability {
    /// Every probe point, in tile, asset and triangle order.
    pub probes: Vec<Probe>,
    /// The probe points that are free in the base world and blocked in the
    /// candidate world, in the same order.
    pub newly_blocked: Vec<Probe>,
}

/// G9's measurement: one probe point per triangle of the base world's
/// public ways (transport ribbons) within [`PROBE_MARGIN_MM`] of `region`,
/// tested for the player capsule in both worlds.
pub fn probe_public_ways(
    base: &WorldIndex,
    candidate: &WorldIndex,
    region: BoundsMm,
) -> Result<Walkability, Error> {
    let origin = [region.min[0], 0, region.min[2]];
    let mut probes: Vec<Probe> = Vec::new();
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
                        probes.push(Probe {
                            tile: t.key,
                            feet_mm: feet,
                        });
                    }
                }
            }
        }
    }
    if probes.is_empty() {
        return Ok(Walkability::default());
    }
    let mut area = region;
    for p in &probes {
        area = area.union(&BoundsMm {
            min: p.feet_mm,
            max: p.feet_mm,
        });
    }
    let base_world = ProbeWorld::new(&colliders(base, &area, origin)?).map_err(err)?;
    let candidate_world = ProbeWorld::new(&colliders(candidate, &area, origin)?).map_err(err)?;
    let newly_blocked = probes
        .iter()
        .filter(|p| {
            let local = [0, 1, 2].map(|k| p.feet_mm[k] - origin[k]);
            !base_world.blocked(local) && candidate_world.blocked(local)
        })
        .copied()
        .collect();
    Ok(Walkability {
        probes,
        newly_blocked,
    })
}

/// How G9 reports a newly blocked probe point.
pub fn blocked_detail(p: &Probe) -> String {
    format!(
        "tile {}: a public way is newly blocked at ({:.1}, {:.1}, {:.1}) m in the region frame",
        p.tile,
        p.feet_mm[0] as f64 / 1000.0,
        p.feet_mm[1] as f64 / 1000.0,
        p.feet_mm[2] as f64 / 1000.0
    )
}

/// G9: probe points on the transport ribbons near the change, free in the
/// base world, must stay free in the candidate world.
fn walkability(
    base: &WorldIndex,
    candidate: &WorldIndex,
    region: BoundsMm,
) -> Result<CheckResult, Error> {
    let w = probe_public_ways(base, candidate, region)?;
    if w.probes.is_empty() {
        return Ok(check(
            "G9",
            "walkability",
            Outcome::Pass,
            vec!["no public way within 2 m of the change".into()],
        ));
    }
    if w.newly_blocked.is_empty() {
        Ok(check(
            "G9",
            "walkability",
            Outcome::Pass,
            vec![format!(
                "{} probe points on public ways stay free",
                w.probes.len()
            )],
        ))
    } else {
        let n = w.newly_blocked.len();
        let mut details: Vec<String> = w.newly_blocked.iter().take(5).map(blocked_detail).collect();
        details.push(format!(
            "{n} of {} probe points newly blocked",
            w.probes.len()
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

/// One waypoint of a replay played in two worlds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Waypoint {
    pub tick: u64,
    /// The player's feet in the base world, region frame (mm).
    pub base_mm: [i64; 3],
    /// The player's feet in the candidate world at the same tick.
    pub candidate_mm: [i64; 3],
}

impl Waypoint {
    /// How far apart the two positions are (m), as G10 measures it.
    pub fn off_m(&self) -> f64 {
        (0..3)
            .map(|k| {
                let m = (self.base_mm[k] - self.candidate_mm[k]) as f64 / 1000.0;
                m * m
            })
            .sum::<f64>()
            .sqrt()
    }

    /// The candidate misses this waypoint: it is more than
    /// [`WAYPOINT_TOLERANCE_M`] off (G10).
    pub fn missed(&self) -> bool {
        self.off_m() > WAYPOINT_TOLERANCE_M
    }
}

/// The name G10 gives a replay: the start of its final state hash.
pub fn replay_name(replay: &Replay) -> String {
    format!(
        "replay {}",
        replay
            .checkpoints
            .last()
            .map_or("?".into(), |c| c.hash.hex()[..16].to_string())
    )
}

/// G10's measurement: plays `replay` in the base world and, if its path
/// comes within [`REPLAY_REACH_MM`] of `region`, in the candidate world as
/// well, and pairs the waypoints taken every [`WAYPOINT_EVERY`] ticks.
/// `None` if the path stays farther away: such a replay cannot show the
/// change.
pub fn replay_waypoints(
    base: &Store,
    candidate: &Store,
    region: BoundsMm,
    replay: &Replay,
) -> Result<Option<Vec<Waypoint>>, Error> {
    let origin = sim_origin_mm(replay.header.spawn.position_mm);
    let mut base_source = StoreSource::from_store(base.clone(), origin).map_err(err)?;
    let path = waypoints(replay, &mut base_source)?;
    let close = path.iter().any(|(_, p)| {
        [0, 2].iter().all(|&k| {
            region.min[k] - REPLAY_REACH_MM <= p[k] && p[k] <= region.max[k] + REPLAY_REACH_MM
        })
    });
    if !close {
        return Ok(None);
    }
    let mut candidate_source = StoreSource::from_store(candidate.clone(), origin).map_err(err)?;
    let moved = waypoints(replay, &mut candidate_source)?;
    Ok(Some(
        path.iter()
            .zip(&moved)
            .map(|((tick, a), (_, b))| Waypoint {
                tick: *tick,
                base_mm: *a,
                candidate_mm: *b,
            })
            .collect(),
    ))
}

/// G10: replays passing near the change must still reach their waypoints.
fn replay_paths(
    world_dir: &Path,
    candidate_store: &Store,
    world_id: ContentId,
    region: BoundsMm,
    replays: &[Replay],
) -> Result<CheckResult, Error> {
    let base_store = Store::new(world_dir);
    let (mut details, mut failed, mut ran) = (Vec::new(), false, 0);
    for r in replays.iter().filter(|r| r.header.world_id == world_id) {
        let name = replay_name(r);
        let Some(path) = replay_waypoints(&base_store, candidate_store, region, r)? else {
            continue;
        };
        ran += 1;
        match path.iter().find(|w| w.missed()) {
            Some(w) => {
                failed = true;
                details.push(format!(
                    "{name}: at tick {} the player is {:.1} m off the recorded path",
                    w.tick,
                    w.off_m()
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
