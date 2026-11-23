//! `op_replay`: check, run, inspect and record StreetWide replays.
//!
//! ```text
//! op_replay check <file> [--world <dir>]   replay and compare every checkpoint
//! op_replay run   <file> [--world <dir>]   replay and print the final state hash
//! op_replay info  <file>                   print the header and sizes
//! op_replay trace <file> [--world <dir>] [--every <ticks>]
//!                                          print the player's path
//! op_replay synth [--world <dir>] [--out <file>]
//!                                          record the scripted sample walk
//! ```
//!
//! Without `--world`, the world is looked up by id in `fixtures/world` and
//! `data/world/<world name>`. Exit codes: 0 ok, 1 the replay diverged,
//! 2 usage or input error.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use op_replay::synth::{self, Meter};
use op_replay::{Recorder, Replay, Verdict, check, find_world, open_world, play};

const USAGE: &str = "usage:
  op_replay check <file> [--world <dir>]
  op_replay run   <file> [--world <dir>]
  op_replay info  <file>
  op_replay trace <file> [--world <dir>] [--every <ticks>]
  op_replay synth [--world <dir>] [--out <file>]";

struct Args {
    positional: Vec<String>,
    world: Option<PathBuf>,
    out: Option<PathBuf>,
    every: u64,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut a = Args {
        positional: Vec::new(),
        world: None,
        out: None,
        every: 60,
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--world" => a.world = Some(PathBuf::from(value()?)),
            "--out" => a.out = Some(PathBuf::from(value()?)),
            "--every" => {
                a.every = value()?
                    .parse()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or("--every needs a positive number")?
            }
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ => a.positional.push(arg.clone()),
        }
    }
    Ok(a)
}

fn load(path: &str) -> Result<Replay, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    Replay::from_bytes(&bytes).map_err(|e| format!("{path}: {e}"))
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some((command, rest)) = argv.split_first() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let result =
        parse(rest).and_then(
            |args| match (command.as_str(), args.positional.as_slice()) {
                ("check", [file]) => cmd_check(file, args.world.as_deref()),
                ("run", [file]) => cmd_run(file, args.world.as_deref()),
                ("info", [file]) => cmd_info(file),
                ("trace", [file]) => cmd_trace(file, args.world.as_deref(), args.every),
                ("synth", []) => cmd_synth(args.world.as_deref(), args.out.as_deref()),
                _ => Err(USAGE.to_string()),
            },
        );
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

fn cmd_check(file: &str, world: Option<&Path>) -> Result<ExitCode, String> {
    let replay = load(file)?;
    let dir = find_world(&replay.header, world).map_err(|e| e.to_string())?;
    let mut source = open_world(&replay.header, &dir).map_err(|e| e.to_string())?;
    match check(&replay, &mut source).map_err(|e| e.to_string())? {
        Verdict::Match(hash) => {
            println!(
                "ok: {} ticks, {} checkpoints match, final state {hash}",
                replay.header.ticks,
                replay.checkpoints.len()
            );
            Ok(ExitCode::SUCCESS)
        }
        Verdict::Diverged {
            tick,
            expected,
            actual,
        } => {
            println!("DIVERGED at checkpoint tick {tick}: recorded {expected}, got {actual}");
            Ok(ExitCode::from(1))
        }
    }
}

fn cmd_run(file: &str, world: Option<&Path>) -> Result<ExitCode, String> {
    let replay = load(file)?;
    let dir = find_world(&replay.header, world).map_err(|e| e.to_string())?;
    let mut source = open_world(&replay.header, &dir).map_err(|e| e.to_string())?;
    let trail =
        play(&replay.header, &replay.inputs, &mut source, |_| {}).map_err(|e| e.to_string())?;
    let last = trail.last().ok_or("empty replay")?;
    println!("{}", last.hash);
    Ok(ExitCode::SUCCESS)
}

fn cmd_info(file: &str) -> Result<ExitCode, String> {
    let replay = load(file)?;
    let h = &replay.header;
    println!("world      {} ({})", h.world_name, h.world_id);
    println!("recorder   {} {}", h.recorder.name, h.recorder.version);
    println!(
        "contracts  replay {}, sim {}",
        h.contract_version, h.sim_contract_version
    );
    println!("seed       {}", h.seed);
    println!(
        "spawn      {:?} mm, yaw {}",
        h.spawn.position_mm, h.spawn.yaw
    );
    println!(
        "ticks      {} ({:.1} s at {} Hz), {} checkpoints",
        h.ticks,
        h.ticks as f64 / f64::from(h.tick_hz),
        h.tick_hz,
        replay.checkpoints.len()
    );
    if let Some(last) = replay.checkpoints.last() {
        println!("final      {}", last.hash);
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_trace(file: &str, world: Option<&Path>, every: u64) -> Result<ExitCode, String> {
    let replay = load(file)?;
    let dir = find_world(&replay.header, world).map_err(|e| e.to_string())?;
    let mut source = open_world(&replay.header, &dir).map_err(|e| e.to_string())?;
    println!("tick  feet x/y/z in the sim frame (m)      grounded  yaw    colliders");
    play(&replay.header, &replay.inputs, &mut source, |sim| {
        if sim.ticks() % every == 0 {
            let p = sim.player();
            let [x, y, z] = p.feet();
            println!(
                "{:5} {x:9.3} {y:9.3} {z:9.3}   {:8}  {:5}  {}",
                sim.ticks(),
                p.grounded,
                p.yaw,
                sim.active_colliders()
            );
        }
    })
    .map_err(|e| e.to_string())?;
    Ok(ExitCode::SUCCESS)
}

fn cmd_synth(world: Option<&Path>, out: Option<&Path>) -> Result<ExitCode, String> {
    let dir = world.unwrap_or(Path::new("fixtures/world"));
    let out = out.unwrap_or(Path::new("fixtures/replays/sample.oprep"));
    let mut meter = Meter::default();
    let replay = op_replay::record(
        dir,
        synth::SEED,
        None,
        synth::sample_inputs(),
        Recorder {
            name: "op_replay synth".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
        |sim| meter.observe(sim),
    )
    .map_err(|e| e.to_string())?;
    let bytes = replay.to_bytes().map_err(|e| e.to_string())?;
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(out, &bytes).map_err(|e| format!("{}: {e}", out.display()))?;
    let r = meter.report();
    println!("wrote {} ({} bytes)", out.display(), bytes.len());
    println!(
        "walked {:.1} m, blocked by a wall for {} ticks, airborne for {} ticks, climbed {:.2} m, {} headings",
        r.walked_m, r.blocked_ticks, r.airborne_ticks, r.climbed_m, r.headings
    );
    println!(
        "final state {}",
        replay
            .checkpoints
            .last()
            .map(|c| c.hash.hex())
            .unwrap_or_default()
    );
    Ok(ExitCode::SUCCESS)
}
