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
//! op_replay check-change <world-dir> <bundle-dir> [--replay <file>]... [--report <file>]
//!                                          the full contribution gate, G1-G10
//!                                          (RFC 0002, ADR-0009)
//! op_replay merge <world-dir> <bundle-dir> (--previous <record> --record <file>
//!     --report <file> | --log <dir>) --reason <text>
//!     --review <handle>=<verdict>[:<notes>]... [--takedown] [--require-base]
//!     [--replay <file>]...
//!                                          op_world merge with the full gate,
//!                                          so change sets that touch colliders
//!                                          can be merged
//! op_replay rerecord <file> --world <dir> --out <file>
//!                                          play a replay's inputs again in a
//!                                          new version of its world
//! op_replay compare-worlds <before-dir> <after-dir> [--replay <file>]... [--report <file>]
//!                                          G9's and G10's measurements between
//!                                          two versions of a world, for review
//! ```
//!
//! Without `--world`, the world is looked up by id in `fixtures/world` and
//! `data/world/<world name>`. Exit codes: 0 ok, 1 the replay diverged or
//! the change set does not pass the gate, 2 usage or input error. Like
//! `op_world merge`, `merge` exits 2 when it refuses a change set, such as
//! a baseline refresh set (author `streetwide:ingest`) planned on another
//! world than the store's, with or without `--require-base` (RFC 0002
//! section 7). It also refuses a `--replay` recorded in another world than
//! the store's, which G10 would ignore.
//! `compare-worlds` exits 0 when it finds something: its findings are for
//! review.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use op_replay::synth::{self, Meter};
use op_replay::{Recorder, Replay, Verdict, check, find_world, open_world, play};
use op_world::merge::MergeArgs;

fn usage() -> String {
    format!(
        "usage:
  op_replay check <file> [--world <dir>]
  op_replay run   <file> [--world <dir>]
  op_replay info  <file>
  op_replay trace <file> [--world <dir>] [--every <ticks>]
  op_replay synth [--world <dir>] [--out <file>]
  op_replay check-change <world-dir> <bundle-dir> [--replay <file>]... [--report <file>]
  op_replay merge {} [--replay <file>]...
  op_replay rerecord <file> --world <dir> --out <file>
  op_replay compare-worlds <before-dir> <after-dir> [--replay <file>]... [--report <file>]",
        MergeArgs::USAGE
    )
}

struct Args {
    positional: Vec<String>,
    world: Option<PathBuf>,
    out: Option<PathBuf>,
    every: u64,
    replays: Vec<PathBuf>,
    report: Option<PathBuf>,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut a = Args {
        positional: Vec::new(),
        world: None,
        out: None,
        every: 60,
        replays: Vec::new(),
        report: None,
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--world" => a.world = Some(PathBuf::from(value()?)),
            "--out" => a.out = Some(PathBuf::from(value()?)),
            "--replay" => a.replays.push(PathBuf::from(value()?)),
            "--report" => a.report = Some(PathBuf::from(value()?)),
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
    let bytes = op_world::store::read_file(Path::new(path), op_replay::format::MAX_REPLAY_BYTES)
        .map_err(|e| format!("{path}: {e}"))?;
    Replay::from_bytes(&bytes).map_err(|e| format!("{path}: {e}"))
}

fn load_all(paths: &[PathBuf]) -> Result<Vec<Replay>, String> {
    paths.iter().map(|p| load(&p.to_string_lossy())).collect()
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some((command, rest)) = argv.split_first() else {
        eprintln!("{}", usage());
        return ExitCode::from(2);
    };
    let result = if command == "merge" {
        // `op_world merge`'s options, plus `--replay`.
        cmd_merge(rest)
    } else {
        parse(rest).and_then(
            |args| match (command.as_str(), args.positional.as_slice()) {
                ("check", [file]) => cmd_check(file, args.world.as_deref()),
                ("run", [file]) => cmd_run(file, args.world.as_deref()),
                ("info", [file]) => cmd_info(file),
                ("trace", [file]) => cmd_trace(file, args.world.as_deref(), args.every),
                ("synth", []) => cmd_synth(args.world.as_deref(), args.out.as_deref()),
                ("check-change", [world, bundle]) => {
                    cmd_check_change(world, bundle, &args.replays, args.report.as_deref())
                }
                ("rerecord", [file]) => {
                    cmd_rerecord(file, args.world.as_deref(), args.out.as_deref())
                }
                ("compare-worlds", [before, after]) => {
                    cmd_compare_worlds(before, after, &args.replays, args.report.as_deref())
                }
                _ => Err(usage()),
            },
        )
    };
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

fn cmd_check_change(
    world: &str,
    bundle: &str,
    replays: &[PathBuf],
    report: Option<&Path>,
) -> Result<ExitCode, String> {
    let replays = load_all(replays)?;
    let candidate =
        std::env::temp_dir().join(format!("streetwide-full-gate-{}", std::process::id()));
    let result =
        op_replay::gate::check_change(Path::new(world), Path::new(bundle), &candidate, &replays);
    let _ = std::fs::remove_dir_all(&candidate);
    let r = result.map_err(|e| e.to_string())?;
    for c in &r.checks {
        let outcome = match c.outcome {
            op_world::contrib::Outcome::Pass => "pass",
            op_world::contrib::Outcome::Fail => "FAIL",
            op_world::contrib::Outcome::NotApplicable => "not applicable",
            op_world::contrib::Outcome::NotRun => "NOT RUN",
            op_world::contrib::Outcome::Review => "review",
        };
        println!("{:>3} {:<13} {outcome}", c.id, c.name);
        for d in &c.details {
            println!("      {d}");
        }
    }
    let bytes = r.to_bytes().map_err(|e| e.to_string())?;
    if let Some(path) = report {
        std::fs::write(path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    println!(
        "change {}: {} (report {})",
        r.change_id,
        if r.passed {
            "passes the gate"
        } else {
            "does NOT pass the gate"
        },
        op_world::ContentId::of(&bytes)
    );
    Ok(if r.passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

/// `op_world merge` with the complete gate. The options are `op_world
/// merge`'s, so they are parsed by the same code, after the `--replay`
/// files are taken out.
fn cmd_merge(args: &[String]) -> Result<ExitCode, String> {
    let (mut rest, mut replays) = (Vec::new(), Vec::new());
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if arg == "--replay" {
            replays.push(PathBuf::from(
                it.next().ok_or(format!("{arg} needs a value"))?,
            ));
        } else {
            rest.push(arg.clone());
        }
    }
    if rest.len() < 2 || rest[..2].iter().any(|a| a.starts_with("--")) {
        return Err(usage());
    }
    let merge = MergeArgs::parse(&rest)?;
    let paths = replays;
    let replays = load_all(&paths)?;
    // G10 ignores replays recorded in another world, so a merge with such a
    // replay would skip G10 without a word. After a merge the suite must be
    // re-recorded (op_replay rerecord) before the next one.
    if let Ok((world_id, _)) = op_world::Store::new(&merge.world_dir).read_world()
        && let Some((path, r)) = paths
            .iter()
            .zip(&replays)
            .find(|(_, r)| r.header.world_id != world_id)
    {
        return Err(format!(
            "--replay {}: {} was recorded in world {}, not in the store's world {world_id}, so G10 would ignore it; move it to this world with op_replay rerecord, or leave it out",
            path.display(),
            op_replay::gate::replay_name(r),
            r.header.world_id
        ));
    }
    let scratch =
        std::env::temp_dir().join(format!("streetwide-full-merge-{}", std::process::id()));
    let line = merge
        .run(&scratch, op_replay::gate::full_gate(&replays))
        .map_err(|e| e.to_string())?;
    println!("{line}");
    Ok(ExitCode::SUCCESS)
}

/// Writes through a temporary file next to `path`, so that rewriting a
/// replay in place never leaves half a file. Whatever is at the temporary
/// path (a file left by a crash, a link, a FIFO) is removed first and the
/// temporary file is created anew: a replay suite may come from elsewhere,
/// and the write must not follow a link out of it or wait on a FIFO. If the
/// write or the rename fails, the temporary file is removed again.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write as _;
    let failed = |e: std::io::Error| format!("{}: {e}", path.display());
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(failed)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    match std::fs::remove_file(&tmp) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(failed(e)),
        _ => {}
    }
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .and_then(|mut f| f.write_all(bytes))
        .and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written.map_err(failed)
}

fn cmd_rerecord(file: &str, world: Option<&Path>, out: Option<&Path>) -> Result<ExitCode, String> {
    let world = world.ok_or("--world <dir> is required")?;
    let out = out.ok_or("--out <file> is required")?;
    let replay = load(file)?;
    let moved = op_replay::rerecord(&replay, world).map_err(|e| e.to_string())?;
    let bytes = moved.to_bytes().map_err(|e| e.to_string())?;
    write_atomically(out, &bytes)?;
    let kept = replay
        .checkpoints
        .iter()
        .zip(&moved.checkpoints)
        .filter(|(a, b)| a == b)
        .count();
    println!(
        "rerecorded in world {} (recorded in {}): {kept} of {} checkpoints unchanged, final state {}",
        moved.header.world_id,
        replay.header.world_id,
        moved.checkpoints.len(),
        moved
            .checkpoints
            .last()
            .map(|c| c.hash.hex())
            .unwrap_or_default()
    );
    Ok(ExitCode::SUCCESS)
}

fn cmd_compare_worlds(
    before: &str,
    after: &str,
    replays: &[PathBuf],
    report: Option<&Path>,
) -> Result<ExitCode, String> {
    let replays = load_all(replays)?;
    let comparison =
        op_replay::compare::compare_worlds(Path::new(before), Path::new(after), &replays)
            .map_err(|e| e.to_string())?;
    let bytes = comparison.to_bytes().map_err(|e| e.to_string())?;
    if let Some(path) = report {
        std::fs::write(path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    let mut lines = comparison.lines();
    if let Some(last) = lines.last_mut() {
        last.push_str(&format!(" (report {})", op_world::ContentId::of(&bytes)));
    }
    for line in lines {
        println!("{line}");
    }
    Ok(ExitCode::SUCCESS)
}
