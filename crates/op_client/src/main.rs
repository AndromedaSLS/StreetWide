//! StreetWide desktop client.
//!
//! ```text
//! cargo run -p op_client [-- --world <dir>] [--seed <n>] [--preset high|fast]
//!     [--place <name or number> | --at <lon>,<lat>[,<heading>]] [--pitch <degrees>]
//!     [--time <HH:MM>] [--date <YYYY-MM-DD>] [--view walk|fly|map] [--details]
//!     [--inspect]
//! cargo run -p op_client -- --replay <file.oprep> [--world <dir>] [--exit-when-done]
//!     [--screenshot <file.png> [--screenshot-tick <n>]]
//! cargo run --release -p op_client -- --benchmark <route.json> [--world <dir>]
//!     [--preset high|fast] [--out <report.json>] [--shots <dir>]
//! ```
//!
//! `--preset high|fast` chooses the graphics at start (F4 switches). The
//! benchmark (`benchmark`, ADR-0018) runs a route of stops at 1920 x 1080
//! without vsync and writes a report of its frame times.
//!
//! The deterministic simulation (`op_sim`) runs in Bevy's `FixedUpdate` at
//! 60 Hz. Input is sampled every frame and latched for the next tick;
//! rendering interpolates between the last two simulated states. Meshes
//! stream in around the camera from the same cache the simulation loads its
//! colliders from (`op_stream`).
//!
//! Press F1 in the client for the controls: walking, flying, the map, named
//! places, the time of day, graphics settings and inspect mode (I: who made
//! what is under the crosshair, and how to improve it).
//!
//! The detailed look draws what the data does not hold: textures and
//! windows (ADR-0012), and at night lit windows, moonlight and the city's
//! glow (ADR-0022). Which windows are lit is invented, seeded from the
//! building, wall, window and clock, never from data about real homes. F2
//! shows the data alone.

mod benchmark;
mod controls;
mod geo;
mod grid;
mod heights;
mod hud;
mod inspect;
mod materials;
mod pick;
mod scene;
mod session;
mod sky;
mod texgen;
mod travel;
mod water;
mod windows;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bevy::prelude::*;

/// Command-line options.
#[derive(Resource, Clone, Debug, Default)]
pub struct Options {
    pub world: Option<PathBuf>,
    pub replay: Option<PathBuf>,
    pub seed: u64,
    pub exit_when_done: bool,
    pub screenshot: Option<PathBuf>,
    /// Take the screenshot after this many ticks instead of at the end.
    pub screenshot_tick: Option<u64>,
    /// Start at a named place of the world (or its number).
    pub place: Option<String>,
    /// Start at a longitude, latitude and heading (degrees).
    pub at: Option<(f64, f64, f64)>,
    /// Start looking up (positive) or down (degrees).
    pub pitch: Option<f64>,
    /// Time of day (minutes after midnight, Swiss time) and date.
    pub time: Option<u32>,
    pub date: Option<(i32, u32, u32)>,
    /// Start flying or on the map.
    pub view: Option<String>,
    /// Open the details panel (F3) at start.
    pub details: bool,
    /// Start in inspect mode (I).
    pub inspect: bool,
    /// The graphics preset at start (`--preset`): high if not given.
    pub preset: Option<scene::Quality>,
    /// Run the benchmark route in this file (ADR-0018) ...
    pub benchmark: Option<PathBuf>,
    /// ... writing its report here ...
    pub out: Option<PathBuf>,
    /// ... and a screenshot of each stop into this directory.
    pub shots: Option<PathBuf>,
}

const USAGE: &str = "usage: op_client [--world <dir>] [--seed <n>] [--preset high|fast] [--place <name|number> | --at <lon>,<lat>[,<heading>]] [--pitch <degrees>] [--time <HH:MM>] [--date <YYYY-MM-DD>] [--view walk|fly|map] [--details] [--inspect] [--replay <file.oprep>] [--exit-when-done] [--screenshot <file.png> [--screenshot-tick <n>]]\n       op_client --benchmark <route.json> [--world <dir>] [--seed <n>] [--preset high|fast] [--out <report.json>] [--shots <dir>]";

fn parse_time(text: &str) -> Option<u32> {
    let (h, m) = text.split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

fn parse_date(text: &str) -> Option<(i32, u32, u32)> {
    let mut parts = text.split('-');
    let y = parts.next()?.parse().ok()?;
    let m = parts.next()?.parse().ok()?;
    let d = parts.next()?.parse().ok()?;
    (parts.next().is_none() && (1..=12).contains(&m) && (1..=31).contains(&d)).then_some((y, m, d))
}

fn parse_at(text: &str) -> Option<(f64, f64, f64)> {
    let v: Vec<f64> = text
        .split(',')
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    match v[..] {
        [lon, lat] => Some((lon, lat, 0.0)),
        [lon, lat, heading] => Some((lon, lat, heading)),
        _ => None,
    }
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut o = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--world" => o.world = Some(PathBuf::from(value()?)),
            "--replay" => o.replay = Some(PathBuf::from(value()?)),
            "--seed" => o.seed = value()?.parse().map_err(|_| "--seed needs a number")?,
            "--screenshot" => o.screenshot = Some(PathBuf::from(value()?)),
            "--screenshot-tick" => {
                o.screenshot_tick = Some(
                    value()?
                        .parse()
                        .map_err(|_| "--screenshot-tick needs a number")?,
                )
            }
            "--exit-when-done" => o.exit_when_done = true,
            "--details" => o.details = true,
            "--inspect" => o.inspect = true,
            "--pitch" => {
                o.pitch = Some(
                    value()?
                        .parse()
                        .ok()
                        .filter(|p: &f64| (-90.0..=90.0).contains(p))
                        .ok_or("--pitch needs degrees from -90 to 90")?,
                )
            }
            "--place" => o.place = Some(value()?),
            "--at" => o.at = Some(parse_at(&value()?).ok_or("--at needs <lon>,<lat>[,<heading>]")?),
            "--time" => o.time = Some(parse_time(&value()?).ok_or("--time needs HH:MM")?),
            "--date" => o.date = Some(parse_date(&value()?).ok_or("--date needs YYYY-MM-DD")?),
            "--view" => {
                let v = value()?;
                if !["walk", "fly", "map"].contains(&v.as_str()) {
                    return Err("--view needs walk, fly or map".into());
                }
                o.view = Some(v)
            }
            "--preset" => {
                o.preset =
                    Some(scene::Quality::parse(&value()?).ok_or("--preset needs high or fast")?)
            }
            "--benchmark" => o.benchmark = Some(PathBuf::from(value()?)),
            "--out" => o.out = Some(PathBuf::from(value()?)),
            "--shots" => o.shots = Some(PathBuf::from(value()?)),
            "-h" | "--help" => return Err(USAGE.into()),
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
    }
    if o.benchmark.is_some() {
        // The route sets where, when and how to look; nothing else may.
        let other = [
            ("--replay", o.replay.is_some()),
            ("--place", o.place.is_some()),
            ("--at", o.at.is_some()),
            ("--pitch", o.pitch.is_some()),
            ("--time", o.time.is_some()),
            ("--date", o.date.is_some()),
            ("--view", o.view.is_some()),
            ("--details", o.details),
            ("--inspect", o.inspect),
            ("--exit-when-done", o.exit_when_done),
            ("--screenshot", o.screenshot.is_some()),
            ("--screenshot-tick", o.screenshot_tick.is_some()),
        ];
        if let Some((name, _)) = other.iter().find(|(_, given)| *given) {
            return Err(format!(
                "--benchmark takes its places, dates and times from the route; it does not combine with {name}"
            ));
        }
    } else if o.out.is_some() || o.shots.is_some() {
        return Err("--out and --shots go with --benchmark".into());
    }
    Ok(o)
}

fn main() -> ExitCode {
    let options = match parse_args(std::env::args().skip(1)) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let session = match session::Session::start(&options) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    let bench = match &options.benchmark {
        Some(route) => match benchmark::Benchmark::load(
            route,
            session.world_name(),
            Path::new("worlds"),
            options.preset.unwrap_or_default(),
            options.out.clone(),
            options.shots.clone(),
        ) {
            Ok(b) => Some(b),
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::from(2);
            }
        },
        None => None,
    };
    if bench.is_some() && cfg!(debug_assertions) {
        eprintln!(
            "warning: a debug build; benchmark a release build (cargo run --release -p op_client)"
        );
    }
    let title = format!("StreetWide - {}", session.world_name());
    // Default: today at 14:00, Swiss time.
    let mut clock = sky::Clock::today_at(options.time.unwrap_or(14 * 60));
    if let Some((year, month, day)) = options.date {
        clock.local.year = year;
        clock.local.month = month;
        clock.local.day = day;
    }
    let panels = hud::Panels {
        help: options.screenshot.is_none() && bench.is_none(),
        debug: options.details,
    };
    let inspect = inspect::Inspect::new(options.inspect);
    let window = match bench {
        Some(_) => benchmark::window(title),
        None => Window { title, ..default() },
    };
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(window),
        ..default()
    }))
    .insert_resource(options.preset.unwrap_or_default())
    .insert_resource(options)
    .insert_resource(session)
    .insert_resource(clock)
    .insert_resource(panels)
    .insert_resource(inspect)
    .add_plugins((
        session::SessionPlugin,
        controls::ControlsPlugin,
        sky::SkyPlugin,
        windows::WindowsPlugin,
        scene::ScenePlugin,
        travel::TravelPlugin,
        inspect::InspectPlugin,
        hud::HudPlugin,
    ));
    if let Some(b) = bench {
        app.insert_resource(b)
            .add_plugins(benchmark::BenchmarkPlugin);
    }
    match app.run() {
        AppExit::Success => ExitCode::SUCCESS,
        AppExit::Error(code) => ExitCode::from(code.get()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Result<Options, String> {
        parse_args(a.iter().map(|s| s.to_string()))
    }

    #[test]
    fn presets_and_the_benchmark_options() {
        assert_eq!(args(&[]).unwrap().preset, None);
        assert_eq!(
            args(&["--preset", "fast"]).unwrap().preset,
            Some(scene::Quality { high: false })
        );
        assert_eq!(
            args(&["--place", "1", "--preset", "high"]).unwrap().preset,
            Some(scene::Quality { high: true })
        );
        let e = args(&["--preset", "ultra"]).unwrap_err();
        assert!(e.contains("--preset needs high or fast"), "{e}");
        let o = args(&[
            "--benchmark",
            "worlds/r.json",
            "--world",
            "w",
            "--preset",
            "fast",
            "--out",
            "r.json",
            "--shots",
            "s",
        ])
        .unwrap();
        assert_eq!(o.benchmark, Some(PathBuf::from("worlds/r.json")));
        assert_eq!(
            (o.out, o.shots, o.world),
            (
                Some(PathBuf::from("r.json")),
                Some(PathBuf::from("s")),
                Some(PathBuf::from("w"))
            )
        );
        for other in [
            &["--place", "1"][..],
            &["--time", "18:00"],
            &["--screenshot", "a.png"],
            &["--replay", "a.oprep"],
        ] {
            let mut a = vec!["--benchmark", "r.json"];
            a.extend(other);
            let e = args(&a).unwrap_err();
            assert!(
                e.contains(&format!("does not combine with {}", other[0])),
                "{e}"
            );
        }
        for alone in ["--out", "--shots"] {
            let e = args(&[alone, "x"]).unwrap_err();
            assert!(e.contains("go with --benchmark"), "{e}");
        }
    }
}
