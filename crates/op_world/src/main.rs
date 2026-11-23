//! `op_world` command-line tool.
//!
//! ```text
//! op_world validate <world-dir>   validate a whole world (exit 1 if invalid)
//! op_world id <file>...           print the content id of files
//! op_world check-change <world-dir> <bundle-dir> [--report <file>] [--candidate <dir>]
//!                                 run the contribution gate (RFC 0002) on a
//!                                 change set; exit 1 if it does not pass
//! op_world baseline <world-dir> --record <file> --reason <text>
//!                                 write the first record of a world log
//! op_world merge <world-dir> <bundle-dir> --previous <record> --record <file>
//!     --report <file> --reason <text> --review <handle>=<verdict>[:<notes>]... [--takedown]
//!                                 merge a change set into the store and write
//!                                 its merge record (RFC 0002 section 6)
//! ```

use std::path::Path;
use std::process::ExitCode;

use op_world::contrib::{Decision, Outcome, Review, Verdict};
use op_world::validate::validate_store;
use op_world::{ContentId, Store, gate};

const USAGE: &str = "usage:\n  op_world validate <world-dir>\n  op_world id <file>...\n  op_world check-change <world-dir> <bundle-dir> [--report <file>] [--candidate <dir>]\n  op_world baseline <world-dir> --record <file> --reason <text>\n  op_world merge <world-dir> <bundle-dir> --previous <record> --record <file> --report <file> --reason <text> --review <handle>=<verdict>[:<notes>]... [--takedown]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("validate") if args.len() == 2 => validate(Path::new(&args[1])),
        Some("id") if args.len() >= 2 => print_ids(&args[1..]),
        Some("check-change") if args.len() >= 3 => check_change(&args[1..]),
        Some("baseline") if args.len() >= 2 => baseline(&args[1..]),
        Some("merge") if args.len() >= 3 => merge(&args[1..]),
        _ => {
            eprintln!("{USAGE}");
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
        "world {world}: {} tile(s), {} asset(s), {} triangle(s), {} error(s)",
        report.tiles,
        report.assets,
        report.triangles,
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
        match std::fs::read(f) {
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
                eprintln!("{USAGE}");
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
        let record = op_world::merge::baseline(Path::new(&args[0]), &one(&f, "--reason")?)
            .map_err(|e| e.to_string())?;
        let bytes = record.to_bytes().map_err(|e| e.to_string())?;
        std::fs::write(one(&f, "--record")?, &bytes).map_err(|e| e.to_string())?;
        Ok(format!(
            "baseline record {} for world {}",
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

fn parse_review(text: &str) -> Result<Review, String> {
    let (reviewer, rest) = text
        .split_once('=')
        .ok_or("--review is <handle>=<verdict>[:<notes>]")?;
    let (verdict, notes) = rest.split_once(':').unwrap_or((rest, rest));
    let verdict = match verdict {
        "better" => Verdict::Better,
        "not_better" => Verdict::NotBetter,
        "blocked" => Verdict::Blocked,
        other => {
            return Err(format!(
                "unknown verdict {other} (better, not_better, blocked)"
            ));
        }
    };
    Ok(Review {
        reviewer: reviewer.into(),
        verdict,
        notes: notes.into(),
    })
}

fn merge(args: &[String]) -> ExitCode {
    let run = || -> Result<String, String> {
        let f = flags(&args[2..])?;
        let reviews = f
            .iter()
            .filter(|(n, _)| n == "--review")
            .map(|(_, v)| parse_review(v.as_deref().unwrap_or("")))
            .collect::<Result<Vec<_>, _>>()?;
        let decision = if f.iter().any(|(n, _)| n == "--takedown") {
            Decision::Takedown
        } else {
            Decision::Merged
        };
        let previous = std::fs::read(one(&f, "--previous")?).map_err(|e| e.to_string())?;
        let scratch = std::env::temp_dir().join(format!("streetwide-merge-{}", std::process::id()));
        let result = op_world::merge::merge(
            Path::new(&args[0]),
            Path::new(&args[1]),
            &previous,
            decision,
            reviews,
            &one(&f, "--reason")?,
            &scratch,
        );
        let _ = std::fs::remove_dir_all(&scratch);
        let merged = result.map_err(|e| e.to_string())?;
        std::fs::write(one(&f, "--report")?, &merged.report_bytes).map_err(|e| e.to_string())?;
        std::fs::write(one(&f, "--record")?, &merged.record_bytes).map_err(|e| e.to_string())?;
        Ok(format!(
            "record {}: world {} -> {}",
            ContentId::of(&merged.record_bytes),
            merged
                .record
                .world_before
                .map(|w| w.to_string())
                .unwrap_or_default(),
            merged.record.world_after
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
