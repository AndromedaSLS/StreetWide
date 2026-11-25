//! A benchmark of one stop for the crowd prototype:
//! `--proto-crowd-bench <out.json>` with `--place`, `--date` and `--time`
//! (ADR-0018's method). The window is 1920 x 1080 with vsync off, on the
//! high preset. The run waits until the place is reached, every mesh in
//! range is shown and the crowd is loaded, draws the crowd at tick 0 facing
//! the place's heading, takes the `--screenshot` there (or at
//! `--proto-crowd-shot-tick`, pausing the turn), then turns the camera once
//! round at eye height over 20 seconds (1,200 ticks; the crowd's tick counts
//! from the end of loading, and the turn's angle follows the tick), writes
//! a report as JSON and exits.
//!
//! With `--proto-crowd-checks` it counts the prototype's checks at every
//! tick, for all four ways of avoiding each other (none, built into the
//! function, client offsets, both), from where everyone is at that whole
//! tick, so two runs count the same. Cost runs leave the checks out.
//!
//! Frame time is the wall-clock time between frames; the main update's CPU
//! time runs from the start of `First` to the end of `Last`; GPU time per
//! pass comes from Bevy's `RenderDiagnosticsPlugin`; the crowd's own CPU
//! time is measured inside its systems.

use std::collections::{BTreeMap, BTreeSet};
use std::f32::consts::TAU;
use std::path::PathBuf;
use std::time::Instant;

use bevy::diagnostic::DiagnosticsStore;
use bevy::prelude::*;
use bevy::render::renderer::RenderAdapterInfo;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::window::PrimaryWindow;
use serde_json::{Value, json};

use super::population::{self, people_hash};
use super::{CamPose, CrowdState, CrowdTick, Key, NEAR_TIER_M, eye_of};
use crate::Options;
use crate::controls::Look;
use crate::scene::{Quality, StreamStats};
use crate::session::Session;
use crate::sky::{Clock, SunState};
use crate::travel::Travel;

/// The turn: 20 seconds of ticks.
pub const TURN_TICKS: u64 = 1_200;
/// Settling after loading, at tick 0.
const SETTLE_S: f64 = 4.0;
const SETTLE_FRAMES: u32 = 200;
/// Frames between asking for a screenshot and going on.
const SHOT_FRAMES: u32 = 30;
/// Overlap: bodies closer than this between centres (m), within
/// [`NEAR_TIER_M`] of the camera and in view.
const OVERLAP_M: f64 = 0.4;
/// Inside a shell: feet more than this inside (m).
const INSIDE_M: f64 = 0.1;
/// A jump: moving more than this in one tick while in view (m).
const JUMP_M: f32 = 0.25;

/// The benchmark's tick advances before the crowd works it out.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct BenchTick;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Loading,
    Settling { frames: u32 },
    Shot { frames: u32 },
    Turn,
    Paused { frames: u32 },
    Done,
}

#[derive(Resource)]
struct Bench {
    out: PathBuf,
    started: Instant,
    settle_start: Option<Instant>,
    phase: Phase,
    loaded_s: f64,
    yaw0: f32,
    tick: u64,
    shot_taken: bool,
    frame_ms: Vec<f64>,
    main_ms: Vec<f64>,
    crowd_ms: Vec<f64>,
    render: BTreeMap<String, Vec<f64>>,
    gpu_total_ms: Vec<f64>,
    seen: Option<bevy::platform::time::Instant>,
}

/// The main update's CPU time.
#[derive(Resource, Default)]
struct MainTime {
    start: Option<Instant>,
    last_ms: f64,
}

/// The crowd's CPU time since the last frame was recorded (ms).
#[derive(Resource, Default)]
pub struct CrowdSpent(pub f64);

pub fn build(app: &mut App) {
    let checks = app
        .world()
        .get_resource::<Options>()
        .is_some_and(|o| o.crowd.checks);
    app.add_plugins(bevy::render::diagnostic::RenderDiagnosticsPlugin)
        .init_resource::<MainTime>()
        .init_resource::<CrowdSpent>()
        .add_systems(Startup, setup)
        .add_systems(First, main_start)
        .add_systems(Last, main_end)
        .add_systems(Update, control.before(crate::travel::TravelSystems))
        .add_systems(FixedUpdate, advance.in_set(BenchTick));
    if checks {
        app.init_resource::<Checks>();
    }
}

fn setup(mut commands: Commands, options: Res<Options>) {
    let Some(out) = options.crowd.bench.clone() else {
        return;
    };
    commands.insert_resource(Bench {
        out,
        started: Instant::now(),
        settle_start: None,
        phase: Phase::Loading,
        loaded_s: 0.0,
        yaw0: 0.0,
        tick: 0,
        shot_taken: false,
        frame_ms: Vec::new(),
        main_ms: Vec::new(),
        crowd_ms: Vec::new(),
        render: BTreeMap::new(),
        gpu_total_ms: Vec::new(),
        seen: None,
    });
}

fn main_start(mut t: ResMut<MainTime>) {
    t.start = Some(Instant::now());
}

fn main_end(mut t: ResMut<MainTime>) {
    if let Some(s) = t.start.take() {
        t.last_ms = s.elapsed().as_secs_f64() * 1000.0;
    }
}

fn pose_at(session: &Session, yaw0: f32, tick: u64, window: &Window) -> CamPose {
    let yaw = f64::from(yaw0) + f64::from(TAU) * tick as f64 / TURN_TICKS as f64;
    let aspect = f64::from(window.width()) / f64::from(window.height().max(1.0));
    CamPose::level(eye_of(session), yaw, aspect)
}

/// The turn's tick, and the camera at it, for the crowd.
fn advance(
    bench: Option<ResMut<Bench>>,
    mut ticks: ResMut<CrowdTick>,
    session: Res<Session>,
    window: Single<&Window, With<PrimaryWindow>>,
) {
    let Some(mut b) = bench else { return };
    match b.phase {
        Phase::Settling { .. } | Phase::Shot { .. } | Phase::Paused { .. } => {}
        Phase::Turn if b.tick < TURN_TICKS => b.tick += 1,
        _ => return,
    }
    ticks.tick = Some(b.tick);
    ticks.cam = Some(pose_at(&session, b.yaw0, b.tick, &window));
}

/// Nearest-rank percentile of sorted values.
fn pct(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let k = ((q * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted[k - 1]
}

fn summary(values: &[f64]) -> Value {
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let mean = v.iter().sum::<f64>() / v.len().max(1) as f64;
    let r = |x: f64| (x * 1000.0).round() / 1000.0;
    json!({
        "n": v.len(),
        "min": r(pct(&v, 0.0)),
        "median": r(pct(&v, 0.5)),
        "mean": r(mean),
        "p95": r(pct(&v, 0.95)),
        "p99": r(pct(&v, 0.99)),
        "max": r(pct(&v, 1.0)),
    })
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn control(
    mut commands: Commands,
    bench: Option<ResMut<Bench>>,
    time: (Res<Time<Real>>, Res<Time<Fixed>>),
    store: Res<DiagnosticsStore>,
    travel: Res<Travel>,
    session: Res<Session>,
    streamed: Res<StreamStats>,
    options: Res<Options>,
    mut look: ResMut<Look>,
    world: (
        Res<Clock>,
        Res<SunState>,
        Res<Quality>,
        Option<Res<RenderAdapterInfo>>,
        Single<&Window, With<PrimaryWindow>>,
    ),
    crowd: (
        Option<Res<CrowdState>>,
        ResMut<CrowdSpent>,
        Res<MainTime>,
        Option<Res<Checks>>,
    ),
    mut exit: MessageWriter<AppExit>,
) {
    let Some(mut bench) = bench else {
        return;
    };
    let b = &mut *bench;
    let (real, fixed) = time;
    let (clock, sun, quality, adapter, window) = world;
    let (state, mut spent, main_time, checks) = crowd;
    let crowd_wanted = options.crowd.lanes.is_some();
    match b.phase {
        Phase::Loading => {
            let loaded = travel.pending.is_none()
                && streamed.wanted == 0
                && streamed.shown > 0
                && !session.stalled
                && (!crowd_wanted || state.is_some());
            if loaded {
                b.loaded_s = b.started.elapsed().as_secs_f64();
                b.yaw0 = look.yaw;
                look.pitch = 0.0;
                info!(
                    "crowd bench: loaded after {:.1} s, settling at tick 0",
                    b.loaded_s
                );
                b.settle_start = Some(Instant::now());
                b.phase = Phase::Settling { frames: 0 };
            }
        }
        Phase::Settling { frames } => {
            look.yaw = b.yaw0;
            let settled = b
                .settle_start
                .is_some_and(|s| s.elapsed().as_secs_f64() >= SETTLE_S)
                && frames >= SETTLE_FRAMES;
            b.phase = if settled {
                if options.crowd.shot_tick == 0
                    && let Some(path) = &options.screenshot
                {
                    commands
                        .spawn(Screenshot::primary_window())
                        .observe(save_to_disk(path.clone()));
                    b.shot_taken = true;
                }
                Phase::Shot { frames: 0 }
            } else {
                Phase::Settling { frames: frames + 1 }
            };
        }
        Phase::Shot { frames } => {
            look.yaw = b.yaw0;
            if frames >= SHOT_FRAMES {
                b.phase = Phase::Turn;
                // Count only render diagnostics that arrive from now on.
                b.seen = store
                    .iter()
                    .filter(|d| d.path().as_str().starts_with("render/"))
                    .filter_map(|d| d.measurement().map(|m| m.time))
                    .max();
                spent.0 = 0.0;
            } else {
                b.phase = Phase::Shot { frames: frames + 1 };
            }
        }
        Phase::Paused { frames } => {
            look.yaw = b.yaw0 + TAU * b.tick as f32 / TURN_TICKS as f32;
            if frames == SHOT_FRAMES
                && let Some(path) = &options.screenshot
            {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(path.clone()));
                b.shot_taken = true;
            }
            b.phase = if frames >= 2 * SHOT_FRAMES {
                spent.0 = 0.0;
                Phase::Turn
            } else {
                Phase::Paused { frames: frames + 1 }
            };
        }
        Phase::Turn => {
            let frac = fixed.overstep_fraction().clamp(0.0, 1.0);
            look.yaw = b.yaw0 + TAU * (b.tick as f32 + frac) / TURN_TICKS as f32;
            b.frame_ms.push(real.delta_secs_f64() * 1000.0);
            b.main_ms.push(main_time.last_ms);
            b.crowd_ms.push(spent.0);
            spent.0 = 0.0;
            record_render(b, &store);
            if !b.shot_taken && options.crowd.shot_tick > 0 && b.tick >= options.crowd.shot_tick {
                b.phase = Phase::Paused { frames: 0 };
            } else if b.tick >= TURN_TICKS {
                let report = report(
                    b,
                    &options,
                    &clock,
                    &sun,
                    &quality,
                    adapter.as_deref(),
                    &window,
                    state.as_deref(),
                    checks.as_deref(),
                    &streamed,
                );
                match std::fs::write(
                    &b.out,
                    serde_json::to_string_pretty(&report).unwrap_or_default() + "\n",
                ) {
                    Ok(()) => info!("crowd bench: wrote {}", b.out.display()),
                    Err(e) => error!("crowd bench: {}: {e}", b.out.display()),
                }
                info!(
                    "crowd bench: frame median {} ms, main update median {} ms, GPU median {} ms, crowd CPU median {} ms",
                    report["frame_ms"]["median"],
                    report["main_update_ms"]["median"],
                    report["gpu_total_ms"]["median"],
                    report["crowd_cpu_ms"]["median"]
                );
                b.phase = Phase::Done;
                exit.write(AppExit::Success);
            }
        }
        Phase::Done => {}
    }
}

/// One batch of render diagnostics per frame, all stamped alike; a span
/// name can come twice in a batch: sum them.
fn record_render(b: &mut Bench, store: &DiagnosticsStore) {
    let mut batch: Option<bevy::platform::time::Instant> = None;
    let mut total = 0.0;
    for d in store.iter() {
        let path = d.path().as_str();
        if !path.starts_with("render/") {
            continue;
        }
        let (mut sum, mut newest) = (0.0, None);
        for m in d.measurements() {
            if b.seen.is_none_or(|s| m.time > s) {
                sum += m.value;
                newest =
                    Some(newest.map_or(m.time, |t: bevy::platform::time::Instant| t.max(m.time)));
            }
        }
        let Some(time) = newest else {
            continue;
        };
        batch = Some(batch.map_or(time, |t| t.max(time)));
        if path.ends_with("/elapsed_gpu") && path.matches('/').count() == 2 {
            total += sum;
        }
        b.render.entry(path.to_string()).or_default().push(sum);
    }
    if let Some(t) = batch {
        b.seen = Some(t);
        if total > 0.0 {
            b.gpu_total_ms.push(total);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn report(
    b: &Bench,
    options: &Options,
    clock: &Clock,
    sun: &SunState,
    quality: &Quality,
    adapter: Option<&RenderAdapterInfo>,
    window: &Window,
    state: Option<&CrowdState>,
    checks: Option<&Checks>,
    streamed: &StreamStats,
) -> Value {
    let mut gpu = serde_json::Map::new();
    for (path, values) in &b.render {
        if path.ends_with("/elapsed_gpu") {
            gpu.insert(
                path.trim_start_matches("render/").to_string(),
                summary(values),
            );
        }
    }
    let d = clock.local;
    let o = &options.crowd;
    let crowd = state.map(|s| {
        let st = &s.bake;
        let trips = s
            .lanes_variant
            .as_ref()
            .or(s.none_variant.as_ref())
            .map(|v| v.crowd.stats());
        json!({
            "load_ms": s.load_ms.round(),
            "shells": st.shells,
            "lanes_in": st.edges_in,
            "km_in": (st.km_in * 100.0).round() / 100.0,
            "lanes_near_walls_before": st.edges_near_walls,
            "lanes_inside_shells_before": st.edges_inside_shells,
            "nodes_moved": st.nodes_moved,
            "nodes_blocked": st.nodes_blocked,
            "lanes_dropped_why": {
                "end_not_clear": st.dropped_why[0],
                "wall_across": st.dropped_why[1],
                "walls_both_sides": st.dropped_why[2],
                "shift_over_1_m": st.dropped_why[3],
                "shift_at_an_end": st.dropped_why[4],
            },
            "lanes_shifted": st.edges_shifted,
            "max_shift_m": (st.max_shift_m * 100.0).round() / 100.0,
            "lanes_dropped": st.edges_dropped,
            "km_dropped": (st.km_dropped * 100.0).round() / 100.0,
            "lane_points_inside_after": st.points_inside_after,
            "lanes_with_entrance_ends": st.edges_entrance,
            "lanes_in_passages": st.edges_passage,
            "places": s.world.lanes.places.len(),
            "spots": s.world.lanes.spots.len(),
            "places_dropped": st.places_dropped,
            "spots_dropped": st.spots_dropped,
            "trip_window_min": (s.world.window as f64 / 3_600.0).round(),
            "trips_worked_out": trips.map(|t| t.trips),
            "places_in_reach": trips.map(|t| t.places_in_reach),
            "spots_in_reach": trips.map(|t| t.spots_in_reach),
            "trips_ending_nowhere": trips.map(|t| t.ending_nowhere),
            "held_until_out_of_view": s.held_total,
            "trip_ends_held_in_view": s.ghosts_total,
        })
    });
    json!({
        "format": "streetwide.crowd-proto-bench",
        "population_version": population::VERSION,
        "place": options.place,
        "date": format!("{:04}-{:02}-{:02}", d.year, d.month, d.day),
        "time": clock.hhmm(),
        "sun_elevation_deg": (sun.elevation * 10.0).round() / 10.0,
        "preset": if quality.high { "high" } else { "fast" },
        "window": [window.physical_width(), window.physical_height()],
        "present_mode": format!("{:?}", window.present_mode),
        "adapter": adapter.map(|a| json!({
            "name": a.name,
            "backend": format!("{:?}", a.backend),
            "driver": a.driver,
            "driver_info": a.driver_info,
        })),
        "options": {
            "lanes": o.lanes.as_ref().map(|p| p.display().to_string()),
            "radius_m": o.radius_m,
            "avoid": o.avoid_name(),
            "checks": o.checks,
            "seed": options.seed,
        },
        "loaded_after_s": (b.loaded_s * 100.0).round() / 100.0,
        "meshes_shown": streamed.shown,
        "frames": b.frame_ms.len(),
        "frame_ms": summary(&b.frame_ms),
        "main_update_ms": summary(&b.main_ms),
        "crowd_cpu_ms": summary(&b.crowd_ms),
        "gpu_total_ms": summary(&b.gpu_total_ms),
        "gpu_total_note": "sum of the top-level render spans' GPU time per frame",
        "gpu_ms": gpu,
        "crowd": crowd,
        "checks": checks.map(Checks::report),
    })
}

// ---------------------------------------------------------------------------
// The checks.

/// A person for the checks: who, where (region frame, m), whether at an
/// entrance or passage, and what they are doing.
type Pt = (Key, [f64; 3], bool, &'static str);

/// What a person is doing, for the checks' examples.
fn what(p: &super::population::Person) -> &'static str {
    match (p.pose, p.lane_kind) {
        (super::population::Pose::Stand, _) => "standing at a spot",
        (super::population::Pose::Sit, _) => "sitting",
        (_, 1) => "walking on a sidewalk",
        (_, 2) => "walking on a footway",
        (_, 3) => "walking on a crossing",
        (_, 4) => "walking on steps",
        (_, 5) => "walking on a pedestrian street",
        (_, 6) => "walking on a living street",
        (_, 7) => "walking on a lane without sidewalks",
        (_, 9) => "walking across a square",
        (_, 10) => "walking to a door or spot",
        (_, 11) => "walking on a platform",
        _ => "walking",
    }
}

/// Overlaps and people inside shells for one way of avoiding each other.
#[derive(Default)]
struct Way {
    pairs: BTreeSet<(Key, Key)>,
    onsets: u64,
    pair_ticks: u64,
    inside_ticks: u64,
    inside_people: BTreeSet<Key>,
    worst_inside_m: f64,
    examples: Vec<Value>,
    overlap_examples: Vec<Value>,
}

/// The checks counted at every tick of the turn.
#[derive(Resource, Default)]
pub struct Checks {
    ticks: u64,
    drawn_prev: BTreeMap<Key, Vec3>,
    appear: u64,
    vanish: u64,
    jumps: u64,
    events: Vec<Value>,
    /// none, lanes (in the function), offsets (client), both.
    ways: [Way; 4],
    counts: Vec<Value>,
    hashes: Vec<Value>,
    in_view_sum: u64,
    in_near_sum: u64,
    drawn_sum: u64,
    group_people_sum: u64,
    people_sum: u64,
    ghosts_sum: u64,
}

const WAYS: [&str; 4] = ["none", "lanes", "offsets", "both"];

impl Checks {
    pub fn record(
        &mut self,
        state: &CrowdState,
        options: &Options,
        session: &Session,
        tick: u64,
        t: i64,
    ) {
        let Some(cam) = state.cam else { return };
        let fade_start = f64::from(options.crowd.radius_m - super::FADE_M);
        let to_render = session.offset_m([0, 0, 0]);
        let region = |v: Vec3| {
            let p = v - to_render;
            [f64::from(p.x), f64::from(p.y), f64::from(p.z)]
        };
        let visible = |p: [f64; 3]| cam.sees(p) && cam.distance(p) < fade_start;
        let radius = f64::from(options.crowd.radius_m);
        let drawn_people = if options.crowd.avoid_lanes {
            state.lanes_variant.as_ref()
        } else {
            state.none_variant.as_ref()
        };
        // Counts and hashes of the function's people.
        if (tick.is_multiple_of(60) || tick == TURN_TICKS)
            && let Some(v) = drawn_people
        {
            let centre = [
                (cam.eye[0] * 1000.0).round() as i64,
                (cam.eye[2] * 1000.0).round() as i64,
            ];
            let target = state.world.target(t, centre, (radius * 1000.0) as i64);
            self.counts.push(json!({
                "tick": tick,
                "people": v.people.len(),
                "target": (target as f64 / 1000.0).round(),
            }));
        }
        if tick == 0 || tick == 600 || tick == TURN_TICKS {
            for (name, v) in [
                ("lanes", &state.lanes_variant),
                ("none", &state.none_variant),
            ] {
                if let Some(v) = v {
                    self.hashes.push(json!({
                        "tick": tick,
                        "function": name,
                        "people": v.people.len(),
                        "hash": people_hash(&v.people),
                    }));
                }
            }
        }
        // Appearing, vanishing and jumping in view (the drawn way).
        let drawn: BTreeMap<Key, Vec3> = state.curr.iter().map(|(k, d)| (*k, d.pos)).collect();
        if tick > 0 && !state.cut {
            for (k, p) in &drawn {
                let at = region(*p);
                match self.drawn_prev.get(k) {
                    None if visible(at) => {
                        self.appear += 1;
                        self.event("appear", tick, *k, at);
                    }
                    Some(q) if visible(at) && (*p - *q).length() > JUMP_M => {
                        self.jumps += 1;
                        self.event("jump", tick, *k, at);
                    }
                    _ => {}
                }
            }
            for (k, q) in &self.drawn_prev.clone() {
                let at = region(*q);
                if !drawn.contains_key(k) && visible(at) {
                    self.vanish += 1;
                    self.event("vanish", tick, *k, at);
                }
            }
        }
        // Overlaps and feet inside shells, for every way.
        let ghosts: Vec<Pt> = state
            .curr
            .iter()
            .filter(|(_, d)| d.ghost)
            .map(|(k, d)| (*k, region(d.pos), d.at_entrance, "ghost"))
            .collect();
        for (w, name) in WAYS.iter().enumerate() {
            let (variant, offsets) = match *name {
                "none" => (&state.none_variant, false),
                "lanes" => (&state.lanes_variant, false),
                "offsets" => (&state.none_variant, true),
                _ => (&state.lanes_variant, true),
            };
            let Some(v) = variant else { continue };
            let positions = if offsets { &v.offset } else { &v.plain };
            let mut pts: Vec<Pt> = v
                .people
                .iter()
                .zip(positions)
                .filter(|(p, _)| drawn.contains_key(&p.key()))
                .map(|(p, xz)| {
                    (
                        p.key(),
                        [xz[0], p.pos[1] as f64 / 1000.0, xz[1]],
                        p.at_entrance,
                        what(p),
                    )
                })
                .collect();
            pts.extend(ghosts.iter().copied());
            self.ways[w].count(&pts, &cam, &state.walls, tick);
        }
        // What was seen.
        if let Some(v) = drawn_people {
            self.people_sum += v.people.len() as u64;
            let mut per_trip: BTreeMap<u64, u64> = BTreeMap::new();
            for p in &v.people {
                *per_trip.entry(p.trip).or_default() += 1;
            }
            self.group_people_sum += per_trip.values().filter(|&&n| n > 1).sum::<u64>();
        }
        for p in drawn.values() {
            let at = region(*p);
            if visible(at) {
                self.in_view_sum += 1;
                if cam.distance(at) < NEAR_TIER_M {
                    self.in_near_sum += 1;
                }
            }
        }
        self.drawn_sum += drawn.len() as u64;
        self.ghosts_sum += ghosts.len() as u64;
        self.ticks += 1;
        self.drawn_prev = drawn;
    }

    fn event(&mut self, what: &str, tick: u64, k: Key, at: [f64; 3]) {
        if self.events.len() < 40 {
            self.events.push(json!({
                "what": what,
                "tick": tick,
                "person": format!("{:016x}/{}", k.0, k.1),
                "at": [at[0].round(), at[2].round()],
            }));
        }
    }

    fn report(&self) -> Value {
        let turn_minutes = TURN_TICKS as f64 / 3_600.0;
        let n = self.ticks.max(1) as f64;
        let ways: serde_json::Map<String, Value> = WAYS
            .iter()
            .zip(&self.ways)
            .map(|(name, w)| {
                (
                    name.to_string(),
                    json!({
                        "overlap_onsets": w.onsets,
                        "overlaps_per_minute": ((w.onsets as f64 / turn_minutes) * 10.0).round() / 10.0,
                        "overlapping_pair_ticks": w.pair_ticks,
                        "people_inside_shells": w.inside_people.len(),
                        "person_ticks_inside_shells": w.inside_ticks,
                        "deepest_inside_m": (w.worst_inside_m * 100.0).round() / 100.0,
                        "inside_examples": w.examples,
                        "overlap_examples": w.overlap_examples,
                    }),
                )
            })
            .collect();
        json!({
            "ticks": self.ticks,
            "appear_in_view": self.appear,
            "vanish_in_view": self.vanish,
            "jumps_in_view": self.jumps,
            "events": self.events,
            "ways": ways,
            "counts": self.counts,
            "hashes": self.hashes,
            "mean_drawn": (self.drawn_sum as f64 / n).round(),
            "mean_in_view": (self.in_view_sum as f64 / n).round(),
            "mean_in_view_within_40_m": ((self.in_near_sum as f64 / n) * 10.0).round() / 10.0,
            "mean_trip_ends_held_in_view": ((self.ghosts_sum as f64 / n) * 10.0).round() / 10.0,
            "share_in_groups": if self.people_sum > 0 {
                ((self.group_people_sum as f64 / self.people_sum as f64) * 1000.0).round() / 1000.0
            } else {
                0.0
            },
        })
    }
}

impl Way {
    fn count(&mut self, pts: &[Pt], cam: &CamPose, walls: &super::walls::Walls, tick: u64) {
        // Overlaps in view within NEAR_TIER_M.
        let near: Vec<&Pt> = pts
            .iter()
            .filter(|(_, p, _, _)| cam.distance(*p) < NEAR_TIER_M && cam.sees(*p))
            .collect();
        let mut cells: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
        for (i, (_, p, _, _)) in near.iter().enumerate() {
            cells
                .entry((
                    (p[0] / OVERLAP_M).floor() as i64,
                    (p[2] / OVERLAP_M).floor() as i64,
                ))
                .or_default()
                .push(i);
        }
        let mut pairs = BTreeSet::new();
        for (ki, p, _, _) in near.iter() {
            let c = (
                (p[0] / OVERLAP_M).floor() as i64,
                (p[2] / OVERLAP_M).floor() as i64,
            );
            for dx in -1..=1 {
                for dz in -1..=1 {
                    let Some(list) = cells.get(&(c.0 + dx, c.1 + dz)) else {
                        continue;
                    };
                    for &j in list {
                        let (kj, q, _, _) = near[j];
                        if *kj <= *ki {
                            continue;
                        }
                        let d = ((p[0] - q[0]).powi(2) + (p[2] - q[2]).powi(2)).sqrt();
                        if d < OVERLAP_M {
                            pairs.insert((*ki, *kj));
                        }
                    }
                }
            }
        }
        for (a, b) in pairs.difference(&self.pairs) {
            self.onsets += 1;
            if self.overlap_examples.len() < 12 {
                let find = |k: &Key| near.iter().find(|x| x.0 == *k).map(|x| (x.1, x.3));
                if let (Some((pa, wa)), Some((pb, wb))) = (find(a), find(b)) {
                    let d = ((pa[0] - pb[0]).powi(2) + (pa[2] - pb[2]).powi(2)).sqrt();
                    self.overlap_examples.push(json!({
                        "tick": tick,
                        "same_group": a.0 == b.0,
                        "a": wa,
                        "b": wb,
                        "distance_m": (d * 100.0).round() / 100.0,
                        "from_camera_m": cam.distance(pa).round(),
                    }));
                }
            }
        }
        self.pair_ticks += pairs.len() as u64;
        self.pairs = pairs;
        // Feet inside a shell, outside entrances and passages, for everyone
        // drawn.
        let mut scratch = Vec::new();
        for (k, p, entrance, what) in pts {
            if *entrance {
                continue;
            }
            if let Some((d, _)) = walls.signed_distance(
                p[0],
                p[2],
                p[1] + super::lanes::WALL_CUT_M,
                1.0,
                &mut scratch,
            ) && d < -INSIDE_M
            {
                self.inside_ticks += 1;
                self.worst_inside_m = self.worst_inside_m.max(-d);
                if self.inside_people.insert(*k) && self.examples.len() < 12 {
                    self.examples.push(json!({
                        "tick": tick,
                        "person": format!("{:016x}/{}", k.0, k.1),
                        "at": [(p[0] * 10.0).round() / 10.0, (p[2] * 10.0).round() / 10.0],
                        "depth_m": (-d * 100.0).round() / 100.0,
                        "what": what,
                    }));
                }
            }
        }
    }
}
