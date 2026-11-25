//! `op_replay compare-worlds`: compares two versions of a world with the
//! measurements behind the gate's G9 (walkability) and G10 (replays), for
//! review.
//!
//! The gate judges a change set against its own base world only. A
//! baseline refresh is merged as several change sets, so nothing judges
//! the sets together: a building in a late set over a way that an early
//! set changed, or replay drift that adds up over many sets. Comparing the
//! world before the refresh with the world after it shows both.
//!
//! - **Region:** the bounding box, in the region frame, of every asset
//!   that differs between the two worlds: listed in one world only, or
//!   listed differently. Like the gate, one box covers all of them.
//! - **When it simulates:** like the gate, only if one of those assets has
//!   a collider in either world. Otherwise nothing the player can touch
//!   has changed.
//! - **Findings:** probe points on the public ways of the world before
//!   that are newly blocked in the world after (G9's test), and replay
//!   waypoints more than 2 m off their path in the world before (G10's
//!   test). Unlike the gate, every off waypoint is listed, not just the
//!   first.
//! - **Report:** this tool's own canonical JSON, not a `streetwide.check`
//!   document. The findings are for review, not gate outcomes, so the
//!   command exits 0 when it finds something.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use op_stream::WorldIndex;
use op_world::{BoundsMm, Collider, ContentId, Layer, Store, TileAsset, TileKey, cjson};
use serde::Serialize;

use crate::gate::{
    PROBE_LIFT_MM, PROBE_MARGIN_MM, Probe, REPLAY_REACH_MM, WAYPOINT_EVERY, WAYPOINT_TOLERANCE_M,
    Walkability, Waypoint, blocked_detail, probe_public_ways, replay_name, replay_waypoints,
};
use crate::{Error, Replay};

/// The `report` field of the JSON report.
pub const REPORT: &str = "streetwide world comparison";
/// Version of the JSON report's layout.
pub const REPORT_VERSION: &str = "0.1.0";
/// The printed text lists at most this many lines per finding kind; the
/// JSON report lists everything.
const PRINTED: usize = 20;

/// One replay played in both worlds.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ReplayDrift {
    /// The content id of the replay file.
    pub replay_id: ContentId,
    /// The name G10 gives it.
    pub name: String,
    /// Its waypoints in both worlds; `None` if it was not played because no
    /// collider differs or its path stays farther than [`REPLAY_REACH_MM`]
    /// from the changes.
    pub waypoints: Option<Vec<Waypoint>>,
}

impl ReplayDrift {
    /// The waypoints more than 2 m off the path in the world before.
    pub fn missed(&self) -> Vec<Waypoint> {
        self.waypoints
            .iter()
            .flatten()
            .filter(|w| w.missed())
            .copied()
            .collect()
    }
}

/// What `compare-worlds` found.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Comparison {
    pub before_world_id: ContentId,
    pub after_world_id: ContentId,
    /// Assets, by tile, layer and feature id, that differ between the
    /// worlds.
    pub changed_assets: usize,
    /// How many of them have a collider in either world.
    pub changed_colliders: usize,
    /// Region-frame bounds of every version of the changed assets.
    pub region: Option<BoundsMm>,
    /// G9's measurement over the region; `None` if no collider differs.
    pub walkability: Option<Walkability>,
    /// Every replay given, sorted by id.
    pub replays: Vec<ReplayDrift>,
}

type Key<'a> = (TileKey, Layer, &'a str);

/// Every asset of a world with its tile's origin.
fn assets(index: &WorldIndex) -> BTreeMap<Key<'_>, ([i64; 3], &TileAsset)> {
    let mut out = BTreeMap::new();
    for t in index.tiles() {
        for a in &t.assets {
            out.insert((t.key, a.layer, a.feature_id.as_str()), (t.origin_mm, a));
        }
    }
    out
}

/// Compares the world in `before_dir` with the world in `after_dir`.
/// `replays` must have been recorded in the world before.
pub fn compare_worlds(
    before_dir: &Path,
    after_dir: &Path,
    replays: &[Replay],
) -> Result<Comparison, Error> {
    let world_error = |e: op_stream::StreamError| Error::World(e.to_string());
    let (before_store, after_store) = (Store::new(before_dir), Store::new(after_dir));
    let before = WorldIndex::from_store(before_store.clone()).map_err(world_error)?;
    let after = WorldIndex::from_store(after_store.clone()).map_err(world_error)?;
    if before.world().region != after.world().region {
        return Err(Error::World(format!(
            "the worlds use different region frames ({}, {}); compare-worlds compares two versions of one world",
            before.world().region.key,
            after.world().region.key
        )));
    }
    let mut sorted: BTreeMap<ContentId, &Replay> = BTreeMap::new();
    for r in replays {
        if r.header.world_id != before.world_id() {
            return Err(Error::World(format!(
                "{} was recorded in world {}, not in the world before ({})",
                replay_name(r),
                r.header.world_id,
                before.world_id()
            )));
        }
        sorted.insert(ContentId::of(&r.to_bytes()?), r);
    }

    let (old, new) = (assets(&before), assets(&after));
    let keys: BTreeSet<&Key> = old.keys().chain(new.keys()).collect();
    let (mut changed_assets, mut changed_colliders) = (0, 0);
    let mut region: Option<BoundsMm> = None;
    for key in keys {
        let versions = [old.get(key), new.get(key)];
        if versions[0] == versions[1] {
            continue;
        }
        changed_assets += 1;
        if versions
            .iter()
            .flatten()
            .any(|(_, a)| a.collider == Collider::Trimesh)
        {
            changed_colliders += 1;
        }
        for (origin, a) in versions.into_iter().flatten() {
            let b = a.bounds_mm.translated(*origin);
            region = Some(region.map_or(b, |r| r.union(&b)));
        }
    }

    let simulated = region.filter(|_| changed_colliders > 0);
    let walkability = match simulated {
        Some(r) => Some(probe_public_ways(&before, &after, r)?),
        None => None,
    };
    let mut drifts = Vec::with_capacity(sorted.len());
    for (replay_id, r) in sorted {
        let waypoints = match simulated {
            Some(region) => replay_waypoints(&before_store, &after_store, region, r)?,
            None => None,
        };
        drifts.push(ReplayDrift {
            replay_id,
            name: replay_name(r),
            waypoints,
        });
    }
    Ok(Comparison {
        before_world_id: before.world_id(),
        after_world_id: after.world_id(),
        changed_assets,
        changed_colliders,
        region,
        walkability,
        replays: drifts,
    })
}

/// The distance between two points (mm), rounded down.
fn distance_mm(a: [i64; 3], b: [i64; 3]) -> i64 {
    let squares: u128 = (0..3)
        .map(|k| {
            let d = u128::from(a[k].abs_diff(b[k]));
            d * d
        })
        .sum();
    i64::try_from(squares.isqrt()).unwrap_or(i64::MAX)
}

#[derive(Serialize)]
struct ToolDoc {
    command: &'static str,
    version: &'static str,
}

/// The gate's constants that the findings depend on.
#[derive(Serialize)]
struct RulesDoc {
    probe_margin_mm: i64,
    probe_lift_mm: i64,
    replay_reach_mm: i64,
    waypoint_every_ticks: u64,
    waypoint_tolerance_mm: i64,
}

#[derive(Serialize)]
struct ProbeDoc {
    tile: TileKey,
    feet_mm: [i64; 3],
}

#[derive(Serialize)]
struct WalkabilityDoc {
    probes: usize,
    newly_blocked: Vec<ProbeDoc>,
}

#[derive(Serialize)]
struct OffDoc {
    tick: u64,
    before_mm: [i64; 3],
    after_mm: [i64; 3],
    off_mm: i64,
}

#[derive(Serialize)]
struct ReplayDoc {
    replay_id: ContentId,
    name: String,
    played: bool,
    waypoints: usize,
    off: Vec<OffDoc>,
}

#[derive(Serialize)]
struct ReportDoc {
    report: &'static str,
    report_version: &'static str,
    tool: ToolDoc,
    rules: RulesDoc,
    before_world_id: ContentId,
    after_world_id: ContentId,
    changed_assets: usize,
    changed_colliders: usize,
    region_mm: Option<BoundsMm>,
    walkability: Option<WalkabilityDoc>,
    replays: Vec<ReplayDoc>,
    findings: usize,
}

fn probe_doc(p: &Probe) -> ProbeDoc {
    ProbeDoc {
        tile: p.tile,
        feet_mm: p.feet_mm,
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

impl Comparison {
    /// Newly blocked probe points plus missed waypoints.
    pub fn findings(&self) -> usize {
        self.walkability
            .as_ref()
            .map_or(0, |w| w.newly_blocked.len())
            + self.replays.iter().map(|r| r.missed().len()).sum::<usize>()
    }

    /// The JSON report: canonical JSON with no local paths, so the same
    /// worlds and replays give the same bytes on every machine.
    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let doc = ReportDoc {
            report: REPORT,
            report_version: REPORT_VERSION,
            tool: ToolDoc {
                command: "op_replay compare-worlds",
                version: env!("CARGO_PKG_VERSION"),
            },
            rules: RulesDoc {
                probe_margin_mm: PROBE_MARGIN_MM,
                probe_lift_mm: PROBE_LIFT_MM,
                replay_reach_mm: REPLAY_REACH_MM,
                waypoint_every_ticks: WAYPOINT_EVERY,
                waypoint_tolerance_mm: (WAYPOINT_TOLERANCE_M * 1000.0) as i64,
            },
            before_world_id: self.before_world_id,
            after_world_id: self.after_world_id,
            changed_assets: self.changed_assets,
            changed_colliders: self.changed_colliders,
            region_mm: self.region,
            walkability: self.walkability.as_ref().map(|w| WalkabilityDoc {
                probes: w.probes.len(),
                newly_blocked: w.newly_blocked.iter().map(probe_doc).collect(),
            }),
            replays: self
                .replays
                .iter()
                .map(|r| ReplayDoc {
                    replay_id: r.replay_id,
                    name: r.name.clone(),
                    played: r.waypoints.is_some(),
                    waypoints: r.waypoints.as_ref().map_or(0, Vec::len),
                    off: r
                        .missed()
                        .iter()
                        .map(|w| OffDoc {
                            tick: w.tick,
                            before_mm: w.base_mm,
                            after_mm: w.candidate_mm,
                            off_mm: distance_mm(w.base_mm, w.candidate_mm),
                        })
                        .collect(),
                })
                .collect(),
            findings: self.findings(),
        };
        cjson::to_canonical(&doc).map_err(|e| Error::World(e.to_string()))
    }

    /// The findings as text, one line each, as the command prints them.
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        let worlds = format!("worlds {} -> {}", self.before_world_id, self.after_world_id);
        if self.changed_assets == 0 {
            out.push(format!("{worlds}: no asset differs"));
        } else {
            out.push(format!(
                "{worlds}: {}, {} of them with a collider",
                plural(self.changed_assets, "asset differs", "assets differ"),
                self.changed_colliders
            ));
        }
        match &self.walkability {
            None if self.changed_assets > 0 => out.push(
                "no changed asset has a collider, so no probe point or replay can change".into(),
            ),
            None => {}
            Some(w) if w.probes.is_empty() => {
                out.push("walkability: no public way within 2 m of the changes".into());
            }
            Some(w) => {
                out.push(format!(
                    "walkability: {} on public ways, {} newly blocked",
                    plural(w.probes.len(), "probe point", "probe points"),
                    w.newly_blocked.len()
                ));
                for p in w.newly_blocked.iter().take(PRINTED) {
                    out.push(format!("  {}", blocked_detail(p)));
                }
                if w.newly_blocked.len() > PRINTED {
                    out.push(format!("  and {} more", w.newly_blocked.len() - PRINTED));
                }
            }
        }
        for r in &self.replays {
            let Some(waypoints) = &r.waypoints else {
                out.push(match self.walkability {
                    None => format!("{}: not played, as no collider differs", r.name),
                    Some(_) => format!(
                        "{}: not played, as its path stays more than {} m from the changes",
                        r.name,
                        REPLAY_REACH_MM / 1000
                    ),
                });
                continue;
            };
            let missed = r.missed();
            if missed.is_empty() {
                out.push(format!(
                    "{}: all {} within 2 m of its path in the world before",
                    r.name,
                    plural(waypoints.len(), "waypoint", "waypoints")
                ));
                continue;
            }
            out.push(format!(
                "{}: {} of {} waypoints more than 2 m off its path in the world before",
                r.name,
                missed.len(),
                waypoints.len()
            ));
            for w in missed.iter().take(PRINTED) {
                out.push(format!("  tick {}: {:.1} m off", w.tick, w.off_m()));
            }
            if missed.len() > PRINTED {
                out.push(format!("  and {} more", missed.len() - PRINTED));
            }
        }
        out.push(format!(
            "{}, for review",
            plural(self.findings(), "finding", "findings")
        ));
        out
    }
}
