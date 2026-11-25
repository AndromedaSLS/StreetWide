//! Crowd prototype (ADR-0021, option C2; branch `crowd-prototype`, throwaway,
//! not for merging as is): people drawn by the client from a population
//! that is a pure function of the lanes, places and profiles, the session
//! seed, the crowd's clock, the population's version and the character
//! catalogue (`population`). They stay outside the simulation's state,
//! schedule and hash, and do not collide.
//!
//! Everything here is off unless `--proto-crowd <lanes>` or
//! `--proto-crowd-bench <out.json>` turns it on; without them the plugin
//! adds no system, so the client behaves exactly as before:
//! - `--proto-crowd <file>`: the lanes, places and spots that the
//!   conversion script writes from the ingest's OSM cache (a scratch file,
//!   not world data). They are cleared of building shells at load
//!   (`lanes`), as the ingest would;
//! - `--proto-crowd-radius <m>`: the drawing radius (160 m); people fade
//!   out over its last [`FADE_M`];
//! - `--proto-crowd-avoid none|lanes|offsets|both`: avoidance between
//!   people built into the function (keep right and step aside, `lanes`),
//!   client offsets near the camera that fade back to the path
//!   (`offsets`), both (the default) or none;
//! - `--proto-crowd-tick <n>`: draw the crowd at a pinned tick;
//! - `--proto-crowd-bench <out.json>`: a benchmark of one stop (`bench`),
//!   with `--proto-crowd-checks` to count the prototype's checks;
//! - `--proto-crowd-census`: people against the profiles' target at the
//!   benchmark's stops and times, by lane kind, then exit (for tuning).
//!
//! **The crowd's clock** is the client's date and time (`sky::Clock`) plus
//! the simulation's ticks (the benchmark counts from the end of loading, and
//! a pinned tick freezes it). People are worked out once per tick, at a
//! whole tick, and drawn between the last two ticks.
//!
//! **The camera may delay drawing, never move anyone:** someone who first
//! appears in view nearer than the fade (a trip starting at a door) is
//! drawn once they have been out of view or beyond the fade; someone whose
//! trip ends in view stays drawn, standing where it ended, until out of view
//! or beyond the fade. Positions never depend on the camera.
//!
//! **Hidden in F2's plain look**, as other decoration is (ADR-0012).

mod bench;
mod figures;
mod lanes;
pub mod population;
mod walls;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

use bevy::prelude::*;

use crate::Options;
use crate::materials::MaterialSet;
use crate::scene::{PlayerCamera, SceneSystems};
use crate::session::{EYE_HEIGHT, Session};
use crate::sky::Clock;
use figures::Figures;
use lanes::BakeStats;
use population::{Avoid, Crowd, Inputs, Person, Pose, World};
use walls::{Ground, Walls};

pub const USAGE: &str = "crowd prototype: [--proto-crowd <lanes.txt>] [--proto-crowd-radius <m>] [--proto-crowd-avoid none|lanes|offsets|both] [--proto-crowd-tick <n>] [--proto-crowd-census] [--proto-crowd-bench <out.json> [--proto-crowd-checks] [--proto-crowd-shot-tick <n>]]";

/// People fade out over this much of the radius (m).
pub const FADE_M: f32 = 25.0;
/// Client offsets work within this distance of the camera (m).
pub const NEAR_TIER_M: f64 = 40.0;
/// Near meshes (knees, hands, feet) within this distance (m).
const LOD_NEAR_M: f32 = 30.0;
/// Within this distance people move every frame; beyond it every tick.
const PER_FRAME_M: f32 = 60.0;
/// The camera's vertical field of view (scene::projection).
const FOV_Y_DEG: f64 = 70.0;

/// The prototype's options; all off by default.
#[derive(Clone, Debug)]
pub struct CrowdOptions {
    pub lanes: Option<PathBuf>,
    pub radius_m: f32,
    pub avoid_lanes: bool,
    pub offsets: bool,
    pub tick: Option<u64>,
    pub bench: Option<PathBuf>,
    pub checks: bool,
    pub shot_tick: u64,
    /// Print people against the target at the stops, then exit.
    pub census: bool,
}

impl Default for CrowdOptions {
    fn default() -> Self {
        CrowdOptions {
            lanes: None,
            radius_m: 160.0,
            avoid_lanes: true,
            offsets: true,
            tick: None,
            bench: None,
            checks: false,
            shot_tick: 0,
            census: false,
        }
    }
}

impl CrowdOptions {
    pub fn any(&self) -> bool {
        self.lanes.is_some() || self.bench.is_some()
    }

    fn avoid_name(&self) -> &'static str {
        match (self.avoid_lanes, self.offsets) {
            (true, true) => "both",
            (true, false) => "lanes",
            (false, true) => "offsets",
            (false, false) => "none",
        }
    }
}

/// Parses one prototype option; `Ok(false)` if `arg` is not one.
pub fn parse_arg(
    o: &mut CrowdOptions,
    arg: &str,
    value: &mut dyn FnMut() -> Result<String, String>,
) -> Result<bool, String> {
    let number = |text: String, what: &str| -> Result<f64, String> {
        text.parse().map_err(|_| format!("{what} needs a number"))
    };
    match arg {
        "--proto-crowd" => o.lanes = Some(PathBuf::from(value()?)),
        "--proto-crowd-radius" => {
            let r = number(value()?, arg)?;
            if !(30.0..=600.0).contains(&r) {
                return Err("--proto-crowd-radius needs metres from 30 to 600".into());
            }
            o.radius_m = r as f32;
        }
        "--proto-crowd-avoid" => {
            (o.avoid_lanes, o.offsets) = match value()?.as_str() {
                "none" => (false, false),
                "lanes" => (true, false),
                "offsets" => (false, true),
                "both" => (true, true),
                _ => return Err("--proto-crowd-avoid needs none, lanes, offsets or both".into()),
            }
        }
        "--proto-crowd-tick" => o.tick = Some(number(value()?, arg)? as u64),
        "--proto-crowd-bench" => o.bench = Some(PathBuf::from(value()?)),
        "--proto-crowd-checks" => o.checks = true,
        "--proto-crowd-shot-tick" => o.shot_tick = number(value()?, arg)? as u64,
        "--proto-crowd-census" => o.census = true,
        _ => return Ok(false),
    }
    Ok(true)
}

type Key = (u64, u8);

/// What the background load hands over.
struct Baked {
    world: World,
    walls: Walls,
    stats: BakeStats,
    load_ms: f64,
}

#[derive(Resource)]
struct Loading(Mutex<Receiver<Result<Baked, String>>>);

/// One person as drawn at a tick (render frame).
#[derive(Clone, Copy, Debug)]
pub struct Drawn {
    pub pos: Vec3,
    pub dir: Vec2,
    pub pose: Pose,
    pub look: u32,
    pub height: f32,
    pub ghost: bool,
    pub at_entrance: bool,
}

/// The camera at a tick: where it is and what it sees.
#[derive(Clone, Copy, Debug)]
pub struct CamPose {
    /// Eye (region frame, m).
    pub eye: [f64; 3],
    pub fwd: [f64; 3],
    pub right: [f64; 3],
    pub up: [f64; 3],
    pub tan_h: f64,
    pub tan_v: f64,
}

impl CamPose {
    /// Level, looking along `yaw` (clockwise from north).
    pub fn level(eye: [f64; 3], yaw: f64, aspect: f64) -> CamPose {
        let (s, c) = yaw.sin_cos();
        let tan_v = (FOV_Y_DEG.to_radians() / 2.0).tan();
        CamPose {
            eye,
            fwd: [s, 0.0, -c],
            right: [c, 0.0, s],
            up: [0.0, 1.0, 0.0],
            tan_h: tan_v * aspect,
            tan_v,
        }
    }

    /// Whether a person standing at `p` (region frame, m) is in the view
    /// frustum (their body, 1.8 m tall and 0.5 m wide).
    pub fn sees(&self, p: [f64; 3]) -> bool {
        let r = [
            p[0] - self.eye[0],
            p[1] + 0.9 - self.eye[1],
            p[2] - self.eye[2],
        ];
        let dot = |a: [f64; 3]| a[0] * r[0] + a[1] * r[1] + a[2] * r[2];
        let z = dot(self.fwd);
        if z < 0.05 {
            return false;
        }
        dot(self.right).abs() <= z * self.tan_h + 0.3 && dot(self.up).abs() <= z * self.tan_v + 0.9
    }

    pub fn distance(&self, p: [f64; 3]) -> f64 {
        ((p[0] - self.eye[0]).powi(2) + (p[2] - self.eye[2]).powi(2)).sqrt()
    }
}

/// Offsets near the camera, per person (m, region frame x and z).
#[derive(Default)]
pub struct Offsets {
    map: BTreeMap<Key, [f64; 2]>,
}

impl Offsets {
    /// People step aside from anyone closer than 0.8 m within
    /// [`NEAR_TIER_M`] of the camera, as offsets from the function's path
    /// that fade back to it (time constant about a third of a second).
    /// Returns everyone's positions (region frame, m).
    pub fn apply(&mut self, people: &[Person], eye: [f64; 2]) -> Vec<[f64; 2]> {
        const RANGE: f64 = 0.8;
        let base: Vec<[f64; 2]> = people
            .iter()
            .map(|p| [p.pos[0] as f64 / 1000.0, p.pos[2] as f64 / 1000.0])
            .collect();
        let near: Vec<bool> = base
            .iter()
            .map(|b| ((b[0] - eye[0]).powi(2) + (b[1] - eye[1]).powi(2)).sqrt() < NEAR_TIER_M)
            .collect();
        let cur: Vec<[f64; 2]> = people
            .iter()
            .zip(&base)
            .map(|(p, b)| {
                let o = self.map.get(&p.key()).copied().unwrap_or([0.0; 2]);
                [b[0] + o[0], b[1] + o[1]]
            })
            .collect();
        // A grid of 0.8 m cells over those near the camera (sorted cells).
        let reach = NEAR_TIER_M + 2.0;
        let mut cells: Vec<((i64, i64), usize)> = cur
            .iter()
            .enumerate()
            .filter(|(_, c)| ((c[0] - eye[0]).powi(2) + (c[1] - eye[1]).powi(2)).sqrt() < reach)
            .map(|(i, c)| {
                (
                    ((c[0] / RANGE).floor() as i64, (c[1] / RANGE).floor() as i64),
                    i,
                )
            })
            .collect();
        cells.sort_unstable();
        let in_cell = |c: (i64, i64)| {
            let lo = cells.partition_point(|x| x.0 < c);
            let hi = cells.partition_point(|x| x.0 <= c);
            cells[lo..hi].iter().map(|x| x.1)
        };
        let mut next = BTreeMap::new();
        let mut out = Vec::with_capacity(people.len());
        for (i, p) in people.iter().enumerate() {
            let key = p.key();
            let mut o = self.map.get(&key).copied().unwrap_or([0.0; 2]);
            let walking = matches!(p.pose, Pose::Walk(_));
            if near[i] && walking {
                let c = cur[i];
                let (cx, cz) = ((c[0] / RANGE).floor() as i64, (c[1] / RANGE).floor() as i64);
                let mut push = [0.0; 2];
                for dx in -1..=1 {
                    for dz in -1..=1 {
                        for j in in_cell((cx + dx, cz + dz)) {
                            if j == i {
                                continue;
                            }
                            let d = [c[0] - cur[j][0], c[1] - cur[j][1]];
                            let dist = (d[0] * d[0] + d[1] * d[1]).sqrt();
                            if dist >= RANGE {
                                continue;
                            }
                            let share = if matches!(people[j].pose, Pose::Walk(_)) {
                                0.5
                            } else {
                                1.0
                            };
                            let g = (RANGE - dist) * share;
                            let u = if dist > 1e-3 {
                                [d[0] / dist, d[1] / dist]
                            } else {
                                let r = p.right;
                                let s = if key < people[j].key() { 1.0 } else { -1.0 };
                                [s * r[0] as f64 / 1024.0, s * r[1] as f64 / 1024.0]
                            };
                            push = [push[0] + u[0] * g, push[1] + u[1] * g];
                        }
                    }
                }
                o = [o[0] * 0.95 + push[0] * 0.2, o[1] * 0.95 + push[1] * 0.2];
            } else {
                o = [o[0] * 0.9, o[1] * 0.9];
            }
            // At most 5 cm a tick (3 m/s): stepping aside, never jumping.
            let was = self.map.get(&key).copied().unwrap_or([0.0; 2]);
            let change = [o[0] - was[0], o[1] - was[1]];
            let c = (change[0] * change[0] + change[1] * change[1]).sqrt();
            if c > 0.05 {
                o = [was[0] + change[0] * 0.05 / c, was[1] + change[1] * 0.05 / c];
            }
            // At most 0.6 m, and sideways only within the room beside the
            // lane.
            let l = (o[0] * o[0] + o[1] * o[1]).sqrt();
            if l > 0.6 {
                o = [o[0] * 0.6 / l, o[1] * 0.6 / l];
            }
            let r = [p.right[0] as f64 / 1024.0, p.right[1] as f64 / 1024.0];
            let side = o[0] * r[0] + o[1] * r[1];
            let clamped = side.clamp(p.lateral.0 as f64 / 1000.0, p.lateral.1 as f64 / 1000.0);
            o = [
                o[0] + r[0] * (clamped - side),
                o[1] + r[1] * (clamped - side),
            ];
            if !walking {
                o = [0.0; 2];
            }
            if o[0].abs() + o[1].abs() > 1e-4 {
                next.insert(key, o);
            }
            out.push([base[i][0] + o[0], base[i][1] + o[1]]);
        }
        self.map = next;
        out
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }
}

/// One variant of the crowd: the function with or without built-in
/// avoidance, and its client offsets.
pub struct Variant {
    pub crowd: Crowd,
    pub people: Vec<Person>,
    pub offsets: Offsets,
    /// Where everyone is drawn, without and with offsets (region m, x z).
    pub plain: Vec<[f64; 2]>,
    pub offset: Vec<[f64; 2]>,
    /// Time spent on the last tick (ms): the function, and the offsets.
    pub function_ms: f64,
    pub offsets_ms: f64,
}

impl Variant {
    fn new(world: &World, avoid: Avoid, seed: u64, radius_mm: i64) -> Variant {
        Variant {
            crowd: Crowd::new(
                world,
                Inputs {
                    seed,
                    avoid,
                    looks: figures::LOOKS,
                },
                radius_mm,
            ),
            people: Vec::new(),
            offsets: Offsets::default(),
            plain: Vec::new(),
            offset: Vec::new(),
            function_ms: 0.0,
            offsets_ms: 0.0,
        }
    }

    fn step(
        &mut self,
        world: &World,
        t: i64,
        centre: [i64; 2],
        radius_mm: i64,
        eye: [f64; 2],
        offsets: bool,
    ) {
        let t0 = Instant::now();
        self.crowd
            .people(world, t, 0, centre, radius_mm, &mut self.people);
        self.function_ms = t0.elapsed().as_secs_f64() * 1000.0;
        self.plain = self
            .people
            .iter()
            .map(|p| [p.pos[0] as f64 / 1000.0, p.pos[2] as f64 / 1000.0])
            .collect();
        let t1 = Instant::now();
        self.offset = if offsets {
            self.offsets.apply(&self.people, eye)
        } else {
            self.plain.clone()
        };
        self.offsets_ms = t1.elapsed().as_secs_f64() * 1000.0;
    }
}

/// The crowd once loaded.
#[derive(Resource)]
pub struct CrowdState {
    pub world: World,
    pub walls: Walls,
    pub bake: BakeStats,
    pub load_ms: f64,
    /// The function with built-in avoidance, and without.
    pub lanes_variant: Option<Variant>,
    pub none_variant: Option<Variant>,
    clock: Option<crate::geo::DateTime>,
    last_tick: Option<u64>,
    /// The people of the last tick (sorted), and those of them held until
    /// out of view.
    known: Vec<Key>,
    held: BTreeSet<Key>,
    /// People whose trip ended in view, drawn where it ended.
    ghosts: BTreeMap<Key, Drawn>,
    /// Drawn at the last two ticks (render frame), sorted by person.
    pub prev: Vec<(Key, Drawn)>,
    pub curr: Vec<(Key, Drawn)>,
    /// The tick the figures were last fully updated for.
    drawn_tick: Option<u64>,
    entities: BTreeMap<Key, Entity>,
    free: Vec<Entity>,
    was_plain: bool,
    /// The camera at the last tick, and whether that tick was a cut.
    pub cam: Option<CamPose>,
    pub cut: bool,
    pub held_total: u64,
    pub ghosts_total: u64,
    /// Time spent on the last tick's work (ms): the function and offsets,
    /// and the frame's drawing.
    pub tick_ms: f64,
    pub draw_ms: f64,
}

/// The tick being worked out, as the benchmark or the session sets it.
#[derive(Resource, Default)]
pub struct CrowdTick {
    /// The tick to draw, or None while nothing should be drawn yet.
    pub tick: Option<u64>,
    /// The camera at that tick, if the benchmark sets it.
    pub cam: Option<CamPose>,
}

#[derive(Component)]
struct Figure;

pub struct CrowdPlugin;

impl Plugin for CrowdPlugin {
    fn build(&self, app: &mut App) {
        let Some(options) = app.world().get_resource::<Options>().cloned() else {
            return;
        };
        if !options.crowd.any() {
            return;
        }
        app.init_resource::<CrowdTick>();
        if options.crowd.lanes.is_some() {
            app.add_systems(Startup, start_loading)
                .add_systems(Update, (finish_loading, draw.after(SceneSystems)).chain())
                .add_systems(FixedUpdate, step.after(bench::BenchTick));
        }
        if options.crowd.bench.is_some() {
            bench::build(app);
        }
    }
}

fn start_loading(mut commands: Commands, options: Res<Options>, session: Res<Session>) {
    let Some(path) = options.crowd.lanes.clone() else {
        return;
    };
    let streamer = session.streamer().clone();
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let start = Instant::now();
        let result = (|| {
            let text =
                std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let raw = lanes::read(&text)?;
            let walls = Walls::load(streamer.index())?;
            let ground = Ground::load(streamer.index())?;
            let (lanes, stats) = lanes::bake(&raw, &walls, &ground);
            let world = World::new(lanes, population::Profiles::zurich());
            Ok(Baked {
                world,
                walls,
                stats,
                load_ms: start.elapsed().as_secs_f64() * 1000.0,
            })
        })();
        tx.send(result).ok();
    });
    commands.insert_resource(Loading(Mutex::new(rx)));
}

#[allow(clippy::too_many_arguments)]
fn finish_loading(
    mut commands: Commands,
    loading: Option<Res<Loading>>,
    options: Res<Options>,
    clock: Res<Clock>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut session: ResMut<Session>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(loading) = loading else {
        return;
    };
    let Ok(rx) = loading.0.lock() else {
        return;
    };
    let Ok(result) = rx.try_recv() else {
        return;
    };
    drop(rx);
    commands.remove_resource::<Loading>();
    let baked = match result {
        Ok(b) => b,
        Err(e) => {
            error!("crowd prototype: {e}");
            session.message = Some((format!("Crowd prototype: {e}"), 10.0));
            return;
        }
    };
    let s = &baked.stats;
    info!(
        "crowd prototype: {} shells; {} lanes ({:.2} km): {} near walls, {} inside shells; {} nodes moved, {} lanes shifted (at most {:.2} m), {} dropped ({:.2} km; why {:?}); {} lane points still inside; {} places and {} spots dropped; loaded in {:.0} ms",
        s.shells,
        s.edges_in,
        s.km_in,
        s.edges_near_walls,
        s.edges_inside_shells,
        s.nodes_moved,
        s.edges_shifted,
        s.max_shift_m,
        s.edges_dropped,
        s.km_dropped,
        s.dropped_why,
        s.points_inside_after,
        s.places_dropped,
        s.spots_dropped,
        baked.load_ms
    );
    let world = baked.world;
    let radius_mm = (f64::from(options.crowd.radius_m) * 1000.0) as i64;
    let seed = options.seed;
    let o = &options.crowd;
    let lanes_variant =
        (o.avoid_lanes || o.checks).then(|| Variant::new(&world, Avoid::Lanes, seed, radius_mm));
    let none_variant =
        (!o.avoid_lanes || o.checks).then(|| Variant::new(&world, Avoid::None, seed, radius_mm));
    let season = population::season(clock.local.month);
    let start = Instant::now();
    let figures = Figures::build(season, &mut meshes, &mut materials);
    info!(
        "crowd prototype: {} looks for season {season}, {} meshes, {} vertices, built in {:.0} ms; {} places, {} spots, trips up to {:.0} min",
        figures::LOOKS,
        figures.meshes.len() * figures::SLOTS,
        figures.vertices,
        start.elapsed().as_secs_f64() * 1000.0,
        world.lanes.places.len(),
        world.lanes.spots.len(),
        world.window as f64 / 3_600.0
    );
    if o.census {
        census(&world, &session, seed);
        exit.write(AppExit::Success);
    }
    commands.insert_resource(figures);
    commands.insert_resource(CrowdState {
        world,
        walls: baked.walls,
        bake: baked.stats,
        load_ms: baked.load_ms,
        lanes_variant,
        none_variant,
        clock: None,
        last_tick: None,
        known: Vec::new(),
        held: BTreeSet::new(),
        ghosts: BTreeMap::new(),
        prev: Vec::new(),
        curr: Vec::new(),
        drawn_tick: None,
        entities: BTreeMap::new(),
        free: Vec::new(),
        was_plain: false,
        cam: None,
        cut: true,
        held_total: 0,
        ghosts_total: 0,
        tick_ms: 0.0,
        draw_ms: 0.0,
    });
}

/// The camera now, from its transform (outside the benchmark).
fn camera_pose(session: &Session, t: &Transform, window: Option<&Window>) -> CamPose {
    let eye = session.render_to_region(t.translation);
    let f = t.forward();
    let r = t.right();
    let u = t.up();
    let tan_v = (FOV_Y_DEG.to_radians() / 2.0).tan();
    let aspect = window.map_or(16.0 / 9.0, |w| {
        f64::from(w.width()) / f64::from(w.height().max(1.0))
    });
    let v = |a: Dir3| [f64::from(a.x), f64::from(a.y), f64::from(a.z)];
    CamPose {
        eye,
        fwd: v(f),
        right: v(r),
        up: v(u),
        tan_h: tan_v * aspect,
        tan_v,
    }
}

/// The work of one tick: the people of each variant, client offsets, and
/// which of them are drawn.
#[allow(clippy::too_many_arguments)]
fn step(
    state: Option<ResMut<CrowdState>>,
    mut ticks: ResMut<CrowdTick>,
    options: Res<Options>,
    clock: Res<Clock>,
    session: Res<Session>,
    camera: Single<&Transform, With<PlayerCamera>>,
    window: Option<Single<&Window>>,
    (mut checks, mut spent): (
        Option<ResMut<bench::Checks>>,
        Option<ResMut<bench::CrowdSpent>>,
    ),
) {
    let Some(mut state) = state else {
        return;
    };
    let o = &options.crowd;
    let tick = if o.bench.is_some() {
        match ticks.tick {
            Some(t) => t,
            None => return,
        }
    } else if let Some(t) = o.tick {
        t
    } else {
        session.sim().ticks()
    };
    let crowd_clock = population::Clock {
        year: clock.local.year,
        month: clock.local.month,
        day: clock.local.day,
        minute: clock.local.minute,
    };
    let clock_changed = state.clock != Some(clock.local);
    if !clock_changed && state.last_tick == Some(tick) {
        return;
    }
    let cut = clock_changed || state.last_tick.is_none_or(|l| tick < l);
    state.clock = Some(clock.local);
    state.last_tick = Some(tick);
    let start = Instant::now();
    let cam = ticks
        .cam
        .unwrap_or_else(|| camera_pose(&session, *camera, window.as_deref().copied()));
    ticks.cam = None;
    let t = population::crowd_time(&crowd_clock, tick);
    let centre = [
        (cam.eye[0] * 1000.0).round() as i64,
        (cam.eye[2] * 1000.0).round() as i64,
    ];
    let radius_mm = (f64::from(o.radius_m) * 1000.0) as i64;
    let eye = [cam.eye[0], cam.eye[2]];
    let state = &mut *state;
    if cut {
        for v in [&mut state.lanes_variant, &mut state.none_variant]
            .into_iter()
            .flatten()
        {
            v.offsets.clear();
        }
    }
    let world = &state.world;
    let drawn_lanes = o.avoid_lanes;
    for (v, is_lanes) in [
        (&mut state.lanes_variant, true),
        (&mut state.none_variant, false),
    ] {
        let Some(v) = v else { continue };
        let drawn = is_lanes == drawn_lanes;
        // Offsets are worked out for the drawn variant if asked for, and for
        // every variant when counting the checks.
        v.step(
            world,
            t,
            centre,
            radius_mm,
            eye,
            (drawn && o.offsets) || o.checks,
        );
    }
    let drawn = if drawn_lanes {
        &state.lanes_variant
    } else {
        &state.none_variant
    };
    let Some(drawn) = drawn else { return };
    let positions = if o.offsets {
        &drawn.offset
    } else {
        &drawn.plain
    };
    // Who is drawn: the camera may delay drawing, never move anyone.
    let fade_start = f64::from(o.radius_m - FADE_M);
    let to_render = session.offset_m([0, 0, 0]);
    let mut next: Vec<(Key, Drawn)> = Vec::with_capacity(drawn.people.len() + state.ghosts.len());
    let mut known = Vec::with_capacity(drawn.people.len());
    let mut kj = 0;
    for (p, xz) in drawn.people.iter().zip(positions) {
        let key = p.key();
        known.push(key);
        while kj < state.known.len() && state.known[kj] < key {
            kj += 1;
        }
        let was_known = kj < state.known.len() && state.known[kj] == key;
        let at = [xz[0], p.pos[1] as f64 / 1000.0, xz[1]];
        let in_view = || cam.sees(at) && cam.distance(at) < fade_start;
        let shown = if !was_known {
            // New: a trip starting, or someone coming into the radius.
            if cut || !in_view() {
                true
            } else {
                state.held.insert(key);
                state.held_total += 1;
                false
            }
        } else if state.held.contains(&key) {
            if in_view() {
                false
            } else {
                state.held.remove(&key);
                true
            }
        } else {
            true
        };
        if !shown {
            continue;
        }
        if !state.ghosts.is_empty() {
            state.ghosts.remove(&key);
        }
        let d = Vec2::new(p.dir[0] as f32, p.dir[1] as f32);
        next.push((
            key,
            Drawn {
                pos: Vec3::new(at[0] as f32, at[1] as f32, at[2] as f32) + to_render,
                dir: d.normalize_or_zero(),
                pose: p.pose,
                look: p.look,
                height: p.height as f32 / 1000.0,
                ghost: false,
                at_entrance: p.at_entrance,
            },
        ));
    }
    state.held.retain(|k| known.binary_search(k).is_ok());
    // Trips that ended in view: stand where they ended until out of view.
    if !cut {
        for (key, d) in &state.curr {
            if d.ghost || known.binary_search(key).is_ok() || state.ghosts.contains_key(key) {
                continue;
            }
            let p = d.pos - to_render;
            let at = [f64::from(p.x), f64::from(p.y), f64::from(p.z)];
            if cam.sees(at) && cam.distance(at) < fade_start {
                state.ghosts.insert(
                    *key,
                    Drawn {
                        pose: if d.pose == Pose::Sit {
                            Pose::Sit
                        } else {
                            Pose::Stand
                        },
                        ghost: true,
                        ..*d
                    },
                );
                state.ghosts_total += 1;
            }
        }
    } else {
        state.ghosts.clear();
    }
    state.ghosts.retain(|_, d| {
        let p = d.pos - to_render;
        let at = [f64::from(p.x), f64::from(p.y), f64::from(p.z)];
        cam.sees(at) && cam.distance(at) < fade_start
    });
    if !state.ghosts.is_empty() {
        for (k, d) in &state.ghosts {
            next.push((*k, *d));
        }
        next.sort_by_key(|(k, _)| *k);
    }
    state.known = known;
    state.prev = std::mem::replace(&mut state.curr, next);
    state.cam = Some(cam);
    state.cut = cut;
    state.tick_ms = start.elapsed().as_secs_f64() * 1000.0;
    if tick % 300 == 0 {
        let v = drawn_variant(state, o.avoid_lanes);
        info!(
            "crowd tick {tick}: {:.2} ms in all, function {:.2} ms, offsets {:.2} ms, {} people, {} trips",
            state.tick_ms,
            v.map_or(0.0, |v| v.function_ms),
            v.map_or(0.0, |v| v.offsets_ms),
            v.map_or(0, |v| v.people.len()),
            v.map_or(0, |v| v.crowd.stats().trips)
        );
    }
    if let Some(spent) = spent.as_deref_mut() {
        spent.0 += state.tick_ms;
    }
    if let Some(checks) = checks.as_deref_mut() {
        checks.record(state, &options, &session, tick, t);
    }
}

/// Moves the figures: between the last two ticks near the camera, at the
/// last tick farther away; near or far meshes and fading by distance.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn draw(
    mut commands: Commands,
    state: Option<ResMut<CrowdState>>,
    figures: Option<Res<Figures>>,
    options: Res<Options>,
    (set, mut spent): (Res<MaterialSet>, Option<ResMut<bench::CrowdSpent>>),
    time: Res<Time<Fixed>>,
    camera: Single<&Transform, (With<PlayerCamera>, Without<Figure>)>,
    mut q: Query<
        (
            &mut Transform,
            &mut Mesh3d,
            &mut MeshMaterial3d<StandardMaterial>,
            &mut Visibility,
        ),
        With<Figure>,
    >,
) {
    let (Some(mut state), Some(figures)) = (state, figures) else {
        return;
    };
    let start = Instant::now();
    let state = &mut *state;
    // Hidden in the plain look.
    if !set.detailed_on {
        if !state.was_plain {
            for (_, _, _, mut v) in &mut q {
                *v = Visibility::Hidden;
            }
            state.was_plain = true;
        }
        return;
    }
    state.was_plain = false;
    let eye = camera.translation;
    let radius = options.crowd.radius_m;
    let fade_start = radius - FADE_M;
    let frac = if options.crowd.tick.is_some() {
        1.0
    } else {
        time.overstep_fraction().clamp(0.0, 1.0)
    };
    // A new tick: every figure; between ticks only those near the camera,
    // which move every frame.
    let full = state.drawn_tick != state.last_tick;
    state.drawn_tick = state.last_tick;
    let mut used = if full { Some(BTreeSet::new()) } else { None };
    for (key, d) in &state.curr {
        let dist = (d.pos - eye).xz().length();
        if dist >= radius {
            continue;
        }
        if let Some(used) = used.as_mut() {
            used.insert(*key);
        } else if dist >= PER_FRAME_M {
            continue;
        }
        let prev = state
            .prev
            .binary_search_by_key(key, |(k, _)| *k)
            .ok()
            .map(|i| state.prev[i].1);
        let mut pos = d.pos;
        if dist < PER_FRAME_M
            && let Some(p) = prev
            && !d.ghost
        {
            pos = p.pos.lerp(d.pos, frac);
        }
        let near = dist < LOD_NEAR_M;
        let look = (d.look % figures::LOOKS) as usize;
        let slot = match d.pose {
            Pose::Walk(phase) => {
                if near {
                    (usize::from(phase) * figures::WALK_NEAR) >> 16
                } else {
                    figures::WALK_NEAR + ((usize::from(phase) * figures::WALK_FAR) >> 16)
                }
            }
            Pose::Stand => {
                if near {
                    figures::STAND
                } else {
                    figures::STAND_FAR
                }
            }
            Pose::Sit => {
                if near {
                    figures::SIT
                } else {
                    figures::SIT_FAR
                }
            }
        };
        let mesh = &figures.meshes[look][slot];
        let material = if dist <= fade_start {
            &figures.material
        } else {
            let a = 1.0 - (dist - fade_start) / FADE_M;
            let level = ((a * figures::FADE_LEVELS.len() as f32) as usize)
                .min(figures::FADE_LEVELS.len() - 1);
            &figures.fades[level]
        };
        let mut transform =
            Transform::from_translation(pos).with_scale(Vec3::splat(d.height / figures::HEIGHT));
        if d.dir != Vec2::ZERO {
            transform.look_to(Vec3::new(d.dir.x, 0.0, d.dir.y), Vec3::Y);
        } else if let Some(p) = prev {
            transform.rotation = Transform::IDENTITY
                .looking_to(
                    Vec3::new(p.dir.x, 0.0, p.dir.y).normalize_or(Vec3::NEG_Z),
                    Vec3::Y,
                )
                .rotation;
        }
        match state.entities.get(key).copied() {
            Some(e) => {
                if let Ok((mut t, mut m, mut mat, mut v)) = q.get_mut(e) {
                    if *t != transform {
                        *t = transform;
                    }
                    if m.0 != *mesh {
                        m.0 = mesh.clone();
                    }
                    if mat.0 != *material {
                        mat.0 = material.clone();
                    }
                    if *v != Visibility::Inherited {
                        *v = Visibility::Inherited;
                    }
                }
            }
            None => {
                let e = match state.free.pop() {
                    Some(e) => {
                        commands.entity(e).insert((
                            transform,
                            Mesh3d(mesh.clone()),
                            MeshMaterial3d(material.clone()),
                            Visibility::Inherited,
                        ));
                        e
                    }
                    None => commands
                        .spawn((
                            Figure,
                            transform,
                            Mesh3d(mesh.clone()),
                            MeshMaterial3d(material.clone()),
                            Visibility::Inherited,
                        ))
                        .id(),
                };
                state.entities.insert(*key, e);
            }
        }
    }
    // Figures no longer drawn are hidden and kept for reuse.
    if let Some(used) = used {
        let gone: Vec<Key> = state
            .entities
            .keys()
            .filter(|k| !used.contains(k))
            .copied()
            .collect();
        for k in gone {
            if let Some(e) = state.entities.remove(&k) {
                if let Ok((_, _, _, mut v)) = q.get_mut(e) {
                    *v = Visibility::Hidden;
                }
                state.free.push(e);
            }
        }
    }
    state.draw_ms = start.elapsed().as_secs_f64() * 1000.0;
    if let Some(spent) = spent.as_deref_mut() {
        spent.0 += state.draw_ms;
    }
}

/// People against the profiles' target at the benchmark's stops and times,
/// by lane kind: a tool for tuning, not one of the checks.
fn census(world: &World, session: &Session, seed: u64) {
    let places = crate::travel::load_places(std::path::Path::new("worlds"), session.world_name());
    let cases = [
        ("Lindenhof", (2026, 12, 21, 17 * 60)),
        ("Hirschenplatz", (2026, 12, 21, 17 * 60)),
        ("Rathaus", (2026, 12, 21, 17 * 60)),
        ("Lindenhof", (2026, 6, 21, 18 * 60)),
        ("Hirschenplatz", (2026, 6, 21, 18 * 60)),
        ("Rathaus", (2026, 6, 21, 18 * 60)),
    ];
    for (name, (y, m, d, minute)) in cases {
        let Some(p) = places.iter().find(|p| p.name.starts_with(name)) else {
            continue;
        };
        let at = session.frame().project(p.lon, p.lat, 0.0);
        let c = [(at[0] * 1000.0) as i64, (at[2] * 1000.0) as i64];
        let clock = population::Clock {
            year: y,
            month: m,
            day: d,
            minute,
        };
        let t = population::crowd_time(&clock, 0);
        for r in [160_000i64, 2_000_000] {
            let mut crowd = Crowd::new(
                world,
                Inputs {
                    seed,
                    avoid: Avoid::Lanes,
                    looks: figures::LOOKS,
                },
                r,
            );
            let mut out = Vec::new();
            let mut by_kind = [0i64; 12];
            let samples = 5;
            for k in 0..samples {
                crowd.people(world, t + k * 300, 0, c, r, &mut out);
                for p in &out {
                    by_kind[p.lane_kind as usize] += 1000 / samples;
                }
            }
            let want = world.target_by_kind(t, c, r);
            let got: i64 = by_kind.iter().sum();
            let total: i64 = want.iter().sum();
            info!(
                "census {name} {y}-{m:02}-{d:02} {:02}:00 r {} m: {} people, target {} ({:.2}); by kind got/want: {}",
                minute / 60,
                r / 1000,
                got / 1000,
                total / 1000,
                got as f64 / total.max(1) as f64,
                (0..12)
                    .filter(|&k| by_kind[k] > 0 || want[k] > 0)
                    .map(|k| format!("{k}:{}/{}", by_kind[k] / 1000, want[k] / 1000))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
    }
}

fn drawn_variant(state: &CrowdState, lanes: bool) -> Option<&Variant> {
    if lanes {
        state.lanes_variant.as_ref()
    } else {
        state.none_variant.as_ref()
    }
}

/// The eye at the player's feet (region frame, m).
pub fn eye_of(session: &Session) -> [f64; 3] {
    session.render_to_region(session.curr_feet + Vec3::Y * EYE_HEIGHT)
}
