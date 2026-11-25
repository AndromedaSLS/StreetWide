//! A benchmark of one stop: `--bench <out.json>` with `--place`, `--date`
//! and `--time`. The window is 1920 x 1080 with vsync off. The run waits
//! until the place is reached, every mesh in range is shown and every
//! prototype point is placed, lets frames settle, takes the `--screenshot`
//! if one is asked for, records `--bench-frames` frames (600 by default),
//! writes their statistics as JSON and exits.
//!
//! Frame time is the wall-clock time between frames (CPU and GPU together,
//! as a player sees it). GPU time per pass and pipeline statistics come
//! from Bevy's `RenderDiagnosticsPlugin` (Vulkan and DX12 only), which the
//! benchmark adds. Bevy 0.20.1 does not count draw calls.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use bevy::diagnostic::DiagnosticsStore;
use bevy::prelude::*;
use bevy::render::renderer::RenderAdapterInfo;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::window::PrimaryWindow;
use serde_json::{Value, json};

use super::ProtoStats;
use super::trees::TreePart;
use crate::Options;
use crate::controls::Look;
use crate::scene::{Quality, StreamStats};
use crate::session::Session;
use crate::sky::{Clock, SunState};
use crate::travel::Travel;

/// Settling after loading: at least this long and this many frames.
const SETTLE_S: f64 = 5.0;
const SETTLE_FRAMES: u32 = 300;
/// Frames between the screenshot and the first recorded frame.
const AFTER_SHOT_FRAMES: u32 = 20;

enum Phase {
    Loading,
    Settling { since: Instant, frames: u32 },
    AfterShot { frames: u32 },
    Recording,
    Done,
}

#[derive(Resource)]
struct Bench {
    out: PathBuf,
    frames: usize,
    turn: bool,
    started: Instant,
    phase: Phase,
    loaded_s: f64,
    frame_ms: Vec<f64>,
    /// Every `render/...` diagnostic, one value per frame it reported.
    render: BTreeMap<String, Vec<f64>>,
    gpu_total_ms: Vec<f64>,
    seen: Option<bevy::platform::time::Instant>,
    yaw0: f32,
}

pub fn build(app: &mut App) {
    app.add_plugins(bevy::render::diagnostic::RenderDiagnosticsPlugin)
        .add_systems(Startup, setup)
        .add_systems(Update, step.before(crate::travel::TravelSystems));
}

fn setup(mut commands: Commands, options: Res<Options>) {
    let Some(out) = options.proto.bench.clone() else {
        return;
    };
    commands.insert_resource(Bench {
        out,
        frames: options.proto.bench_frames,
        turn: options.proto.bench_turn,
        started: Instant::now(),
        phase: Phase::Loading,
        loaded_s: 0.0,
        frame_ms: Vec::new(),
        render: BTreeMap::new(),
        gpu_total_ms: Vec::new(),
        seen: None,
        yaw0: 0.0,
    });
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
fn step(
    mut commands: Commands,
    bench: Option<ResMut<Bench>>,
    time: Res<Time<Real>>,
    store: Res<DiagnosticsStore>,
    travel: Res<Travel>,
    session: Res<Session>,
    streamed: Res<StreamStats>,
    stats: Res<ProtoStats>,
    options: Res<Options>,
    mut look: ResMut<Look>,
    world: (
        Res<Clock>,
        Res<SunState>,
        Res<Quality>,
        Option<Res<RenderAdapterInfo>>,
        Single<&Window, With<PrimaryWindow>>,
    ),
    parts: Query<(&TreePart, &ViewVisibility)>,
    entities: Query<Entity>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(mut bench) = bench else {
        return;
    };
    let b = &mut *bench;
    let (clock, sun, quality, adapter, window) = world;
    match &mut b.phase {
        Phase::Loading => {
            let loaded = travel.pending.is_none()
                && streamed.wanted == 0
                && streamed.shown > 0
                && !session.stalled
                && stats.placed();
            if loaded {
                b.loaded_s = b.started.elapsed().as_secs_f64();
                b.yaw0 = look.yaw;
                info!("bench: loaded after {:.1} s, settling", b.loaded_s);
                b.phase = Phase::Settling {
                    since: Instant::now(),
                    frames: 0,
                };
            }
        }
        Phase::Settling { since, frames } => {
            *frames += 1;
            if since.elapsed().as_secs_f64() >= SETTLE_S && *frames >= SETTLE_FRAMES {
                if let Some(path) = &options.screenshot {
                    commands
                        .spawn(Screenshot::primary_window())
                        .observe(save_to_disk(path.clone()));
                }
                b.phase = Phase::AfterShot { frames: 0 };
            }
        }
        Phase::AfterShot { frames } => {
            *frames += 1;
            if *frames >= AFTER_SHOT_FRAMES {
                b.phase = Phase::Recording;
                // Count only batches that arrive from now on.
                b.seen = store
                    .iter()
                    .filter(|d| d.path().as_str().starts_with("render/"))
                    .filter_map(|d| d.measurement().map(|m| m.time))
                    .max();
            }
        }
        Phase::Recording => {
            b.frame_ms.push(time.delta_secs_f64() * 1000.0);
            // One batch of render diagnostics per frame, all stamped alike.
            // A span name can come twice in a batch (the early and the late
            // shadow pass share "per_view_shadow_pass"): sum them.
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
                        newest = Some(
                            newest.map_or(m.time, |t: bevy::platform::time::Instant| t.max(m.time)),
                        );
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
            let k = b.frame_ms.len();
            if b.turn {
                look.yaw = b.yaw0 + std::f32::consts::TAU * k as f32 / b.frames as f32;
            }
            if k >= b.frames {
                let mut visible = [0usize; super::trees::LEVELS];
                for (p, v) in &parts {
                    if v.get() {
                        visible[p.level as usize] += 1;
                    }
                }
                let report = report(
                    b,
                    &stats,
                    &streamed,
                    &options,
                    &clock,
                    &sun,
                    &quality,
                    adapter.as_deref(),
                    &window,
                    visible,
                    entities.iter().count(),
                );
                match std::fs::write(
                    &b.out,
                    serde_json::to_string_pretty(&report).unwrap_or_default() + "\n",
                ) {
                    Ok(()) => info!("bench: wrote {}", b.out.display()),
                    Err(e) => error!("bench: {}: {e}", b.out.display()),
                }
                info!(
                    "bench: frame time median {} ms, p99 {} ms; GPU total median {} ms",
                    report["frame_ms"]["median"],
                    report["frame_ms"]["p99"],
                    report["gpu_total_ms"]["median"]
                );
                b.phase = Phase::Done;
                exit.write(AppExit::Success);
            }
        }
        Phase::Done => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn report(
    b: &Bench,
    stats: &ProtoStats,
    streamed: &StreamStats,
    options: &Options,
    clock: &Clock,
    sun: &SunState,
    quality: &Quality,
    adapter: Option<&RenderAdapterInfo>,
    window: &Window,
    visible: [usize; super::trees::LEVELS],
    entities: usize,
) -> Value {
    let mut gpu = serde_json::Map::new();
    let mut other = serde_json::Map::new();
    for (path, values) in &b.render {
        let name = path.trim_start_matches("render/").to_string();
        if path.ends_with("/elapsed_gpu") {
            gpu.insert(name, summary(values));
        } else {
            other.insert(name, summary(values));
        }
    }
    let p = &options.proto;
    let d = clock.local;
    json!({
        "format": "streetwide.proto-bench",
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
            "device_type": format!("{:?}", a.device_type),
        })),
        "options": {
            "points": p.points.as_ref().map(|x| x.display().to_string()),
            "trees": p.trees,
            "lamps_pool": p.lamps,
            "lamp_range_m": p.lamp_range_m,
            "lamp_lumens": p.lamp_lumens,
            "lamp_lights": if p.lamp_point { "point" } else { "spot" },
            "windows": p.windows,
            "lod_m": p.lod_m,
            "tree_shadow_levels": p.tree_shadow_levels,
            "night_ev100": p.night_ev100,
            "turn": p.bench_turn,
        },
        "loaded_after_s": (b.loaded_s * 100.0).round() / 100.0,
        "frames": b.frame_ms.len(),
        "frame_ms": summary(&b.frame_ms),
        "gpu_total_ms": summary(&b.gpu_total_ms),
        "gpu_total_note": "sum of the top-level render spans' GPU time per frame",
        "gpu_ms": gpu,
        "render_other": other,
        "draw_calls": Value::Null,
        "draw_calls_note": "Bevy 0.20.1 reports no draw-call count; see the passes' vertex and fragment invocations",
        "counts": {
            "entities": entities,
            "meshes_shown": streamed.shown,
            "trees": stats.trees_placed,
            "tree_entities": stats.tree_entities,
            "tree_parts_visible_by_level": visible,
            "crowns_narrowed": stats.crowns_narrowed,
            "lamps": stats.lamps_placed - stats.lamps_in_river_left_out,
            "lamp_records": stats.lamp_records,
            "lamps_on_bridges": stats.lamps_on_bridges,
            "lamps_in_river_left_out": stats.lamps_in_river_left_out,
            "lamps_inside_left_out": stats.lamps_inside_skipped,
            "lamp_entities": stats.lamp_entities,
            "lamps_lit": stats.lamps_lit,
            "real_lights_pool": stats.lights_pool,
            "real_lights_lit": stats.lights_lit,
            "leaves_on": stats.leaves_on,
            "windows_lit": stats.windows_lit,
            "windows_upper_share": stats.windows_upper_share,
            "windows_ground_share": stats.windows_ground_share,
        },
        "startup_ms": {
            "points_file": stats.points_ms,
            "tree_models_and_textures": stats.tree_assets_ms,
            "lamp_models": stats.lamp_assets_ms,
            "placement_spread_over_frames": stats.placement_ms,
            "window_masks": stats.window_masks_ms,
        },
        "ground_max_diff_m": stats.ground_max_diff_m,
    })
}
