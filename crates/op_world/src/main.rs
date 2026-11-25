//! `op_world` command-line tool.
//!
//! ```text
//! op_world validate <world-dir>   validate a whole world (exit 1 if invalid)
//! op_world id <file>...           print the content id of files
//! op_world check-change <world-dir> <bundle-dir> [--report <file>] [--candidate <dir>]
//!                                 run the contribution gate (RFC 0002) on a
//!                                 change set; exit 1 if it does not pass
//! op_world baseline <world-dir> (--record <file> | --log <dir>) --reason <text>
//!                                 write the first record of a world log
//! op_world merge <world-dir> <bundle-dir> (--previous <record> --record <file>
//!     --report <file> | --log <dir>) --reason <text>
//!     --review <handle>=<verdict>[:<notes>]... [--takedown] [--require-base]
//!                                 merge a change set into the store and write
//!                                 its merge record (RFC 0002 section 6)
//! ```
//!
//! `--log <dir>` keeps a world log in a directory (`op_world::merge::WorldLog`):
//! `baseline` starts it, and `merge` takes the previous record from it and
//! appends the new record, the change set, the report and the new world.
//! `--require-base` refuses a change set that was planned on another world.
//! A change set authored by `streetwide:ingest`, a baseline refresh, is
//! refused on another world with or without it (RFC 0002 section 7).
//! A merge takes `merge.lock` in the store, and with `--log` `log.lock` in
//! the log, before it reads them, and keeps them until it has written the
//! store and the record; a second merge that finds one stops before it
//! writes anything. `op_replay merge` takes the same arguments and runs the
//! complete gate.
//!
//! `id` reads each file as a store read does: a regular file itself (not a
//! link, a directory or a FIFO) of at most 64 MiB.

use std::path::Path;
use std::process::ExitCode;

use op_world::contrib::Outcome;
use op_world::merge::{MergeArgs, WorldLog};
use op_world::store::{MAX_FILE_BYTES, read_file};
use op_world::validate::validate_store;
use op_world::{ContentId, Store, gate};

fn usage() -> String {
    format!(
        "usage:\n  op_world validate <world-dir>\n  op_world id <file>...\n  op_world check-change <world-dir> <bundle-dir> [--report <file>] [--candidate <dir>]\n  op_world baseline <world-dir> (--record <file> | --log <dir>) --reason <text>\n  op_world merge {}",
        MergeArgs::USAGE
    )
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("validate") if args.len() == 2 => validate(Path::new(&args[1])),
        Some("id") if args.len() >= 2 => print_ids(&args[1..]),
        Some("check-change") if args.len() >= 3 => check_change(&args[1..]),
        Some("baseline") if args.len() >= 2 => baseline(&args[1..]),
        Some("merge") if args.len() >= 3 => merge(&args[1..]),
        _ => {
            eprintln!("{}", usage());
            ExitCode::from(2)
        }
    }
}

fn validate(dir: &Path) -> ExitCode {
    let report = validate_store(&Store::new(dir));
    for e in &report.errors {
        eprintln!("error: {e}");
    }
    let world = report
        .world_id
        .map_or_else(|| "-".to_string(), |id| id.to_string());
    println!(
        "world {world}: {} tile(s), {} asset(s), {} triangle(s), {} point(s), {} error(s)",
        report.tiles,
        report.assets,
        report.triangles,
        report.points,
        report.errors.len()
    );
    if report.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn print_ids(files: &[String]) -> ExitCode {
    let mut status = ExitCode::SUCCESS;
    for f in files {
        match read_file(Path::new(f), MAX_FILE_BYTES) {
            Ok(bytes) => println!("{}  {f}", ContentId::of(&bytes)),
            Err(e) => {
                eprintln!("{f}: {e}");
                status = ExitCode::FAILURE;
            }
        }
    }
    status
}

fn check_change(args: &[String]) -> ExitCode {
    let (world, bundle) = (Path::new(&args[0]), Path::new(&args[1]));
    let mut report_path = None;
    let mut candidate = None;
    let mut rest = args[2..].iter();
    while let Some(flag) = rest.next() {
        match (flag.as_str(), rest.next()) {
            ("--report", Some(v)) => report_path = Some(v.clone()),
            ("--candidate", Some(v)) => candidate = Some(std::path::PathBuf::from(v)),
            _ => {
                eprintln!("{}", usage());
                return ExitCode::from(2);
            }
        }
    }
    let keep = candidate.is_some();
    let candidate = candidate.unwrap_or_else(|| {
        std::env::temp_dir().join(format!("streetwide-gate-{}", std::process::id()))
    });
    let result = gate::run(world, bundle, &candidate);
    if !keep {
        let _ = std::fs::remove_dir_all(&candidate);
    }
    let report = match result {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    for c in &report.checks {
        let outcome = match c.outcome {
            Outcome::Pass => "pass",
            Outcome::Fail => "FAIL",
            Outcome::NotApplicable => "not applicable",
            Outcome::NotRun => "NOT RUN",
            Outcome::Review => "review",
        };
        println!("{:>3} {:<13} {outcome}", c.id, c.name);
        for d in &c.details {
            println!("      {d}");
        }
    }
    let bytes = match report.to_bytes() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    if let Some(path) = report_path
        && let Err(e) = std::fs::write(&path, &bytes)
    {
        eprintln!("{path}: {e}");
        return ExitCode::from(2);
    }
    println!(
        "change {}: {} (report {})",
        report.change_id,
        if report.passed {
            "passes the gate"
        } else {
            "does NOT pass the gate"
        },
        ContentId::of(&bytes)
    );
    if report.passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// `--flag value` pairs and bare `--flags` after the positional arguments.
fn flags(args: &[String]) -> Result<Vec<(String, Option<String>)>, String> {
    let mut out = Vec::new();
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        if !a.starts_with("--") {
            return Err(format!("unexpected argument {a}"));
        }
        let value = match it.peek() {
            Some(v) if !v.starts_with("--") => it.next().cloned(),
            _ => None,
        };
        out.push((a.clone(), value));
    }
    Ok(out)
}

fn one(flags: &[(String, Option<String>)], name: &str) -> Result<String, String> {
    flags
        .iter()
        .find(|(f, _)| f == name)
        .and_then(|(_, v)| v.clone())
        .ok_or(format!("{name} <value> is required"))
}

fn fail(msg: impl std::fmt::Display) -> ExitCode {
    eprintln!("error: {msg}");
    ExitCode::from(2)
}

fn baseline(args: &[String]) -> ExitCode {
    let run = || -> Result<String, String> {
        let f = flags(&args[1..])?;
        if let Some((name, _)) = f
            .iter()
            .find(|(n, _)| !["--record", "--log", "--reason"].contains(&n.as_str()))
        {
            return Err(format!("unknown option {name}"));
        }
        let has = |name: &str| f.iter().any(|(n, _)| n == name);
        let log = match (has("--record"), has("--log")) {
            (true, true) => {
                return Err(
                    "--record cannot be combined with --log, which writes the record into the log"
                        .into(),
                );
            }
            (false, false) => return Err("--record <file> or --log <dir> is required".into()),
            (_, log) => log,
        };
        let world = Path::new(&args[0]);
        let record =
            op_world::merge::baseline(world, &one(&f, "--reason")?).map_err(|e| e.to_string())?;
        let bytes = record.to_bytes().map_err(|e| e.to_string())?;
        let place = if log {
            // The bytes the record names: `WorldLog::start` refuses others.
            let path = Store::new(world).world_path();
            let world_bytes =
                read_file(&path, MAX_FILE_BYTES).map_err(|e| format!("{}: {e}", path.display()))?;
            WorldLog::start(Path::new(&one(&f, "--log")?), &record, &world_bytes)
                .map_err(|e| e.to_string())?;
            " (log/00000000.json)"
        } else {
            std::fs::write(one(&f, "--record")?, &bytes).map_err(|e| e.to_string())?;
            ""
        };
        Ok(format!(
            "baseline record {}{place} for world {}",
            ContentId::of(&bytes),
            record.world_after
        ))
    };
    match run() {
        Ok(s) => {
            println!("{s}");
            ExitCode::SUCCESS
        }
        Err(e) => fail(e),
    }
}

fn merge(args: &[String]) -> ExitCode {
    let merge = match MergeArgs::parse(args) {
        Ok(m) => m,
        Err(e) => return fail(e),
    };
    let scratch = std::env::temp_dir().join(format!("streetwide-merge-{}", std::process::id()));
    match merge.run(&scratch, gate::run) {
        Ok(s) => {
            println!("{s}");
            ExitCode::SUCCESS
        }
        Err(e) => fail(e),
    }
}
