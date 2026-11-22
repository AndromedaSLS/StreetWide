//! `op_world` command-line tool.
//!
//! ```text
//! op_world validate <world-dir>   validate a whole world (exit 1 if invalid)
//! op_world id <file>...           print the content id of files
//! ```

use std::path::Path;
use std::process::ExitCode;

use op_world::validate::validate_store;
use op_world::{ContentId, Store};

const USAGE: &str = "usage:\n  op_world validate <world-dir>\n  op_world id <file>...";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("validate") if args.len() == 2 => validate(Path::new(&args[1])),
        Some("id") if args.len() >= 2 => print_ids(&args[1..]),
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
