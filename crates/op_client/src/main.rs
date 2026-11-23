//! StreetWide desktop client.
//!
//! ```text
//! cargo run -p op_client [-- --world <dir>] [--seed <n>]
//! cargo run -p op_client -- --replay <file.oprep> [--world <dir>] [--exit-when-done]
//!     [--screenshot <file.png> [--screenshot-tick <n>]]
//! ```
//!
//! The deterministic simulation (`op_sim`) runs in Bevy's `FixedUpdate` at
//! 60 Hz. Input is sampled every frame and latched for the next tick;
//! rendering interpolates between the last two simulated states. Meshes
//! stream in around the camera from the same cache the simulation loads its
//! colliders from (`op_stream`).
//!
//! Controls: click to capture the mouse, mouse to look, WASD to move, Shift
//! to sprint, Space to jump, F9 to save a replay of the session, R to
//! restart at the spawn point, Esc to release the mouse.

mod controls;
mod hud;
mod scene;
mod session;

use std::path::PathBuf;
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
}

const USAGE: &str = "usage: op_client [--world <dir>] [--seed <n>] [--replay <file.oprep>] [--exit-when-done] [--screenshot <file.png> [--screenshot-tick <n>]]";

fn parse_args() -> Result<Options, String> {
    let mut o = Options::default();
    let mut args = std::env::args().skip(1);
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
            "-h" | "--help" => return Err(USAGE.into()),
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
    }
    Ok(o)
}

fn main() -> ExitCode {
    let options = match parse_args() {
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
    let title = format!("StreetWide - {}", session.world_name());
    let exit = App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window { title, ..default() }),
            ..default()
        }))
        .insert_resource(options)
        .insert_resource(session)
        .add_plugins((
            session::SessionPlugin,
            controls::ControlsPlugin,
            scene::ScenePlugin,
            hud::HudPlugin,
        ))
        .run();
    match exit {
        AppExit::Success => ExitCode::SUCCESS,
        AppExit::Error(code) => ExitCode::from(code.get()),
    }
}
