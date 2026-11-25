//! The benchmark mode (ADR-0018):
//!
//! ```text
//! op_client --benchmark <route.json> [--world <dir>] [--preset high|fast]
//!     [--out <report.json>] [--shots <dir>]
//! ```
//!
//! The window is 1920 x 1080 physical pixels with vsync off. For each stop
//! of the route file (`route`), in order, the client:
//! 1. sets the clock to the stop's date and time, and goes there as
//!    `--place` does: a new session, facing the place's heading
//!    (ADR-0013);
//! 2. waits until every mesh in range is shown and the simulation runs;
//! 3. lets frames settle, facing the heading at eye height;
//! 4. with `--shots`, saves the stop's screenshot there, with the credit
//!    line naming the ODbL (ADR-0019);
//! 5. turns the camera once round at eye height in 20 seconds, camera
//!    only as in flight, and records every frame.
//!
//! A stop that cannot be reached or loaded within 60 seconds, or whose
//! window is not 1920 x 1080 from the screenshot to the end of the turn
//! (minimised or resized), is reported as not measured.
//!
//! Then it writes the report (`report`; by default to
//! `benchmarks/<world>-<preset>-<UTC time>.json`), prints a summary and
//! exits, with code 0 if every stop was measured and 1 otherwise.
//!
//! The turn's tick counts 60 Hz steps from the end of loading: 0 while
//! frames settle and the screenshot is taken, then one a step to 1,200,
//! and the heading follows it. A crowd drawn at that tick would show the
//! same people at a stop on every run (ADR-0021).
//!
//! Frame time is the time from one frame to the next, as the player sees
//! it, and is what the budget measures. The main update's CPU time runs
//! from Bevy's `First` to its `Last`. GPU time per render pass comes from
//! Bevy's `RenderDiagnosticsPlugin`, which only the benchmark adds: Vulkan
//! and DX12 give it, Metal does not.

pub mod report;
pub mod route;

use std::f32::consts::TAU;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bevy::diagnostic::{DiagnosticsStore, SystemInfo};
use bevy::prelude::*;
use bevy::render::renderer::RenderAdapterInfo;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::window::{PresentMode, PrimaryWindow, WindowResolution};

use crate::controls::Look;
use crate::geo;
use crate::scene::{PlayerCamera, Quality, StreamStats};
use crate::session::Session;
use crate::sky::{Clock, SunState};
use crate::travel::{Landing, Travel, TravelSystems, Trip, TripEnd, View, load_places};
use report::{Build, Gpu, GpuTimes, Machine, Run, StopResult};
use route::{Route, Stop};

/// ADR-0018's window: 1080p.
pub const WIDTH: u32 = 1920;
pub const HEIGHT: u32 = 1080;
/// The turn: once round in 20 seconds of 60 Hz steps.
pub const TURN_TICKS: u64 = 1_200;
/// Settling after loading: at least this long, and this many frames.
const SETTLE_S: f64 = 3.0;
const SETTLE_FRAMES: u32 = 120;
/// Frames between the screenshot and the turn.
const SHOT_FRAMES: u32 = 30;
/// Give up on a stop that has not loaded after this long (s).
const STOP_TIMEOUT_S: f64 = 60.0;

/// The benchmark's window: 1920 x 1080 physical pixels, whatever the
/// display's scale, and vsync off.
pub fn window(title: String) -> Window {
    Window {
        title,
        resolution: WindowResolution::new(WIDTH, HEIGHT).with_scale_factor_override(1.0),
        present_mode: PresentMode::AutoNoVsync,
        ..default()
    }
}

/// The build the client runs, from `build.rs`.
pub fn build_info() -> Build {
    let commit = env!("STREETWIDE_COMMIT");
    Build {
        commit: (!commit.is_empty()).then(|| commit.to_string()),
        dirty: match env!("STREETWIDE_DIRTY") {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        profile: env!("STREETWIDE_PROFILE").to_string(),
        opt_level: env!("STREETWIDE_OPT_LEVEL").to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// The time now in UTC: ISO 8601, and compact for file names.
fn utc_now() -> (String, String) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d) = geo::civil_from_days((secs / 86_400) as i64);
    let s = secs % 86_400;
    let (hh, mm, ss) = (s / 3600, s / 60 % 60, s % 60);
    (
        format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z"),
        format!("{y:04}{m:02}{d:02}-{hh:02}{mm:02}{ss:02}"),
    )
}

/// A stop's name for a file name: lower case, words joined by `-`.
fn slug(name: &str) -> String {
    let mut s = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            s.push(c);
        } else if !s.is_empty() && !s.ends_with('-') {
            s.push('-');
        }
    }
    s.trim_end_matches('-').to_string()
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// Going to the next stop, or finishing.
    Next,
    /// On the way: `travel` lands it.
    Travelling,
    /// Arrived; meshes and colliders loading.
    Loading,
    Settling {
        since: Instant,
        frames: u32,
    },
    Shot {
        frames: u32,
    },
    Turn,
    Done,
}

/// A route being run.
#[derive(Resource)]
pub struct Benchmark {
    route_file: String,
    route_world: String,
    stops: Vec<Stop>,
    out: PathBuf,
    shots: Option<PathBuf>,
    preset: String,
    started_utc: String,
    next: usize,
    phase: Phase,
    /// When the current stop began.
    stop_start: Instant,
    /// The turn's tick (see the module's description).
    tick: u64,
    yaw0: f32,
    current: Option<StopResult>,
    results: Vec<StopResult>,
    /// The newest render diagnostics already counted.
    seen: Option<bevy::platform::time::Instant>,
}

impl Benchmark {
    /// Reads a route for the world the session opened, with the places of
    /// its definition in `worlds_dir`.
    pub fn load(
        route_file: &Path,
        world_name: &str,
        worlds_dir: &Path,
        preset: Quality,
        out: Option<PathBuf>,
        shots: Option<PathBuf>,
    ) -> Result<Benchmark, String> {
        let at = |e: String| format!("{}: {e}", route_file.display());
        let text = std::fs::read_to_string(route_file).map_err(|e| at(e.to_string()))?;
        let route = Route::parse(&text).map_err(at)?;
        if route.world != world_name {
            return Err(at(format!(
                "the route is for world {}, but the client opened {world_name} (see --world)",
                route.world
            )));
        }
        let stops = route
            .resolve(&load_places(worlds_dir, world_name))
            .map_err(at)?;
        if let Some(dir) = &shots {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let (started_utc, stamp) = utc_now();
        let preset = preset.name().to_string();
        let out = out.unwrap_or_else(|| {
            PathBuf::from("benchmarks").join(format!("{world_name}-{preset}-{stamp}.json"))
        });
        Ok(Benchmark {
            route_file: route_file.display().to_string(),
            route_world: route.world,
            stops,
            out,
            shots,
            preset,
            started_utc,
            next: 0,
            phase: Phase::Next,
            stop_start: Instant::now(),
            tick: 0,
            yaw0: 0.0,
            current: None,
            results: Vec::new(),
            seen: None,
        })
    }

    /// Ends the current stop, measured or not, and goes on to the next.
    fn end_stop(&mut self, error: Option<String>) {
        if let Some(mut r) = self.current.take() {
            match &error {
                Some(e) => warn!("benchmark: {}: not measured: {e}", r.stop.name),
                None => {
                    let s = report::Stats::of(&r.frame_ms);
                    info!(
                        "benchmark: {}: {} frames, median {:.2} ms, p99 {:.2} ms, worst {:.2} ms",
                        r.stop.name,
                        r.frame_ms.len(),
                        s.map_or(f64::NAN, |s| s.median),
                        s.map_or(f64::NAN, |s| s.p99),
                        s.map_or(f64::NAN, |s| s.max),
                    );
                }
            }
            if error.is_some() {
                r.error = error;
            }
            self.results.push(r);
        }
        self.phase = Phase::Next;
    }

    /// Starts recording: render diagnostics count from the next batch.
    fn start_turn(&mut self, store: &DiagnosticsStore) {
        self.seen = store
            .iter()
            .filter(|d| d.path().as_str().starts_with("render/"))
            .filter_map(|d| d.measurement().map(|m| m.time))
            .max();
        self.phase = Phase::Turn;
    }
}

/// The main update's CPU time, from `First` to `Last`.
#[derive(Resource, Default)]
struct MainTime {
    start: Option<Instant>,
    last_ms: f64,
}

pub struct BenchmarkPlugin;

impl Plugin for BenchmarkPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(bevy::render::diagnostic::RenderDiagnosticsPlugin)
            .init_resource::<MainTime>()
            .add_systems(First, main_start)
            .add_systems(Last, main_end)
            .add_systems(FixedUpdate, advance_turn)
            .add_systems(Update, run.before(TravelSystems));
    }
}

fn main_start(mut t: ResMut<MainTime>) {
    t.start = Some(Instant::now());
}

fn main_end(mut t: ResMut<MainTime>) {
    if let Some(s) = t.start.take() {
        t.last_ms = s.elapsed().as_secs_f64() * 1000.0;
    }
}

fn advance_turn(mut bench: ResMut<Benchmark>) {
    if bench.phase == Phase::Turn && bench.tick < TURN_TICKS {
        bench.tick += 1;
    }
}

/// Why a window of this size cannot be measured, if it cannot: a stop
/// counts only if the window stays 1920 x 1080 from the screenshot to the
/// end of the turn.
fn wrong_size(window: &Window) -> Option<String> {
    let (w, h) = (window.physical_width(), window.physical_height());
    ((w, h) != (WIDTH, HEIGHT)).then(|| {
        format!("the window was {w} x {h} pixels, not {WIDTH} x {HEIGHT} (minimised or resized)")
    })
}

/// Everything in range shown, and the simulation running at the stop.
fn loaded(travel: &Travel, session: &Session, stats: &StreamStats) -> bool {
    travel.pending.is_none()
        && stats.wanted == 0
        && stats.shown > 0
        && !session.stalled
        && session.sim().ticks() > 0
}

/// One batch of render diagnostics a frame, all stamped alike. A pass can
/// come twice in a batch (Bevy's early and late shadow passes share a
/// name): its times are added.
fn record_gpu(
    gpu: &mut GpuTimes,
    seen: &mut Option<bevy::platform::time::Instant>,
    store: &DiagnosticsStore,
) {
    let mut batch = None;
    let mut total = 0.0;
    for d in store.iter() {
        let path = d.path().as_str();
        let Some(name) = path
            .strip_prefix("render/")
            .and_then(|p| p.strip_suffix("/elapsed_gpu"))
        else {
            continue;
        };
        let (mut sum, mut newest) = (0.0, None);
        for m in d.measurements() {
            if seen.is_none_or(|s| m.time > s) {
                sum += m.value;
                newest =
                    Some(newest.map_or(m.time, |t: bevy::platform::time::Instant| t.max(m.time)));
            }
        }
        let Some(time) = newest else {
            continue;
        };
        batch = Some(batch.map_or(time, |t: bevy::platform::time::Instant| t.max(time)));
        if !name.contains('/') {
            total += sum;
        }
        gpu.passes.entry(name.to_string()).or_default().push(sum);
    }
    if let Some(t) = batch {
        *seen = Some(t);
        gpu.total.push(total);
    }
}

fn machine(adapter: Option<&RenderAdapterInfo>, system: Option<&SystemInfo>) -> Machine {
    let known = |s: &String| (s != "not available" && !s.is_empty()).then(|| s.clone());
    Machine {
        gpu: adapter.map(|a| Gpu {
            name: a.name.clone(),
            vendor_id: a.vendor,
            device_id: a.device,
            device_type: format!("{:?}", a.device_type),
            backend: format!("{:?}", a.backend),
            driver: a.driver.clone(),
            driver_info: a.driver_info.clone(),
        }),
        cpu: system.and_then(|s| known(&s.cpu)),
        cpu_cores: system.and_then(|s| known(&s.core_count)),
        memory: system.and_then(|s| known(&s.memory)),
        os: system.and_then(|s| known(&s.os)),
        kernel: system.and_then(|s| known(&s.kernel)),
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn run(
    mut commands: Commands,
    mut bench: ResMut<Benchmark>,
    (real, fixed, main_time): (Res<Time<Real>>, Res<Time<Fixed>>, Res<MainTime>),
    store: Res<DiagnosticsStore>,
    (mut travel, mut session, mut look, mut clock): (
        ResMut<Travel>,
        ResMut<Session>,
        ResMut<Look>,
        ResMut<Clock>,
    ),
    (stats, sun): (Res<StreamStats>, Res<SunState>),
    (adapter, system): (Option<Res<RenderAdapterInfo>>, Option<Res<SystemInfo>>),
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<&Transform, With<PlayerCamera>>,
    mut exit: MessageWriter<AppExit>,
) {
    let b = &mut *bench;
    let elapsed = b.stop_start.elapsed().as_secs_f64();
    match b.phase {
        Phase::Next => {
            let Some(stop) = b.stops.get(b.next).cloned() else {
                b.phase = Phase::Done;
                let run = Run {
                    route_file: b.route_file.clone(),
                    route_world: b.route_world.clone(),
                    world_name: session.world_name().to_string(),
                    world_id: session.streamer().index().world_id().hex(),
                    preset: b.preset.clone(),
                    resolution: [WIDTH, HEIGHT],
                    present_mode: format!("{:?}", window.present_mode),
                    turn_s: TURN_TICKS as f64 / f64::from(op_kernel::TICK_HZ),
                    settle_s: SETTLE_S,
                    started_utc: b.started_utc.clone(),
                    machine: machine(adapter.as_deref(), system.as_deref()),
                    build: build_info(),
                };
                exit.write(finish(&run, &b.results, &b.out));
                return;
            };
            b.next += 1;
            info!(
                "benchmark: stop {}/{}: {}",
                b.next,
                b.stops.len(),
                stop.name
            );
            clock.local = stop.clock;
            let p = session.frame().project(stop.lon, stop.lat, 0.0);
            travel.view = View::Walk;
            travel.last_end = None;
            travel.pending = Some(Trip {
                name: stop.name.clone(),
                x: p[0],
                z: p[2],
                landing: Landing::OpenGround,
                yaw: (stop.yaw_deg as f32).to_radians(),
                pitch: 0.0,
                waited: 0.0,
                keep_view: false,
            });
            b.yaw0 = (stop.yaw_deg as f32).to_radians();
            b.tick = 0;
            b.current = Some(StopResult::new(stop));
            b.stop_start = Instant::now();
            b.phase = Phase::Travelling;
        }
        Phase::Travelling => {
            if travel.pending.is_some() {
                return;
            }
            match travel.last_end.take() {
                Some(TripEnd::Arrived) => b.phase = Phase::Loading,
                Some(TripEnd::Failed(e)) => b.end_stop(Some(e)),
                None => b.end_stop(Some("the trip was cancelled".into())),
            }
        }
        Phase::Loading => {
            if let Some(e) = &session.error {
                let e = format!("simulation error: {e}");
                b.end_stop(Some(e));
            } else if loaded(&travel, &session, &stats) {
                if let Some(r) = &mut b.current {
                    r.loaded_after_s = Some(elapsed);
                    session.message = Some((
                        format!("Benchmark {}/{}: {}", b.next, b.stops.len(), r.stop.name),
                        1e9,
                    ));
                }
                look.yaw = b.yaw0;
                look.pitch = 0.0;
                b.phase = Phase::Settling {
                    since: Instant::now(),
                    frames: 0,
                };
            } else if elapsed > STOP_TIMEOUT_S {
                b.end_stop(Some(format!(
                    "{} meshes still loading after {STOP_TIMEOUT_S} s",
                    stats.wanted
                )));
            }
        }
        Phase::Settling { since, frames } => {
            look.yaw = b.yaw0;
            look.pitch = 0.0;
            if since.elapsed().as_secs_f64() < SETTLE_S || frames < SETTLE_FRAMES {
                b.phase = Phase::Settling {
                    since,
                    frames: frames + 1,
                };
                return;
            }
            if !loaded(&travel, &session, &stats) {
                b.phase = Phase::Loading;
                return;
            }
            // From here on, the camera only turns, as in flight.
            travel.view = View::Fly {
                pos: camera.translation,
            };
            let Some(r) = &mut b.current else {
                return;
            };
            r.sun_elevation_deg = Some(sun.elevation);
            r.meshes_shown = Some(stats.shown);
            r.resolution = Some([window.physical_width(), window.physical_height()]);
            if let Some(e) = wrong_size(&window) {
                b.end_stop(Some(e));
                return;
            }
            if let Some(dir) = &b.shots {
                let path = dir.join(format!("{:02}-{}.png", b.next, slug(&r.stop.name)));
                r.screenshot = Some(path.display().to_string());
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(path));
                b.phase = Phase::Shot { frames: 0 };
            } else {
                b.start_turn(&store);
            }
        }
        Phase::Shot { frames } => {
            look.yaw = b.yaw0;
            if frames >= SHOT_FRAMES {
                b.start_turn(&store);
            } else {
                b.phase = Phase::Shot { frames: frames + 1 };
            }
        }
        Phase::Turn => {
            let step = fixed.overstep_fraction().clamp(0.0, 1.0);
            let turned = ((b.tick as f32 + step) / TURN_TICKS as f32).min(1.0);
            look.yaw = (b.yaw0 + TAU * turned).rem_euclid(TAU);
            look.pitch = 0.0;
            // A minimised or resized window draws less, or nothing.
            if let Some(e) = wrong_size(&window) {
                b.end_stop(Some(e));
                return;
            }
            let Some(r) = &mut b.current else {
                return;
            };
            r.frame_ms.push(real.delta_secs_f64() * 1000.0);
            r.main_update_ms.push(main_time.last_ms);
            record_gpu(&mut r.gpu, &mut b.seen, &store);
            if b.tick >= TURN_TICKS {
                b.end_stop(None);
            }
        }
        Phase::Done => {}
    }
}

/// Writes the report and prints the summary; the exit code.
fn finish(run: &Run, results: &[StopResult], out: &Path) -> AppExit {
    let report = report::report(run, results);
    let text = serde_json::to_string_pretty(&report).unwrap_or_default() + "\n";
    let written = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(out, text));
    println!("{}", report::summary(&report));
    match &written {
        Ok(()) => println!("Report: {}", out.display()),
        Err(e) => error!("benchmark: could not write {}: {e}", out.display()),
    }
    if written.is_ok() && results.iter().all(StopResult::measured) {
        AppExit::Success
    } else {
        AppExit::from_code(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_names_make_file_names() {
        assert_eq!(slug("Lindenhof, 21 June, 18:00"), "lindenhof-21-june-18-00");
        assert_eq!(
            slug("Rathausbrücke, 21 December, 18:00"),
            "rathausbrücke-21-december-18-00"
        );
        assert_eq!(slug("  A -- b  "), "a-b");
    }

    #[test]
    fn utc_times_are_iso_8601() {
        let (iso, compact) = utc_now();
        assert_eq!(iso.len(), "2026-11-25T12:00:00Z".len(), "{iso}");
        assert!(iso.ends_with('Z') && iso.as_bytes()[10] == b'T', "{iso}");
        assert_eq!(compact.len(), "20261125-120000".len(), "{compact}");
    }

    /// The Zurich route loads for its world and is refused for another.
    #[test]
    fn routes_load_for_their_world_only() {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let file = repo.join("worlds/zurich-altstadt.route.json");
        let worlds = repo.join("worlds");
        let b = Benchmark::load(
            &file,
            "zurich-altstadt",
            &worlds,
            Quality { high: true },
            None,
            None,
        )
        .unwrap();
        assert_eq!(b.stops.len(), 9);
        assert_eq!(b.preset, "high");
        assert!(b.out.starts_with("benchmarks"), "{}", b.out.display());
        assert!(
            b.out
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("zurich-altstadt-high-"),
            "{}",
            b.out.display()
        );
        let e = Benchmark::load(
            &file,
            "fixture",
            &worlds,
            Quality { high: false },
            None,
            None,
        )
        .err()
        .unwrap();
        assert!(e.contains("the route is for world zurich-altstadt"), "{e}");
    }

    #[test]
    fn the_build_is_known() {
        let b = build_info();
        assert!(["debug", "release"].contains(&b.profile.as_str()), "{b:?}");
        assert_eq!(b.version, env!("CARGO_PKG_VERSION"));
        // A commit, when there is one, is a full hex id or what the
        // environment gave.
        if let Some(c) = &b.commit {
            assert!(!c.is_empty());
        }
    }
}
