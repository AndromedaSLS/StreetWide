//! `op_world` command-line tool.
//!
//! ```text
//! op_world validate <world-dir>   validate a whole world (exit 1 if invalid)
//! op_world id <file>...           print the content id of files
//! op_world check-change <world-dir> <bundle-dir> [--report <file>] [--candidate <dir>]
//!                                 run the contribution gate (RFC 0002) on a
//!                                 change set; exit 1 if it does not pass
//! ```

use std::path::Path;
use std::process::ExitCode;

use op_world::contrib::Outcome;
use op_world::validate::validate_store;
use op_world::{ContentId, Store, gate};

const USAGE: &str = "usage:\n  op_world validate <world-dir>\n  op_world id <file>...\n  op_world check-change <world-dir> <bundle-dir> [--report <file>] [--candidate <dir>]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("validate") if args.len() == 2 => validate(Path::new(&args[1])),
        Some("id") if args.len() >= 2 => print_ids(&args[1..]),
        Some("check-change") if args.len() >= 3 => check_change(&args[1..]),
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
