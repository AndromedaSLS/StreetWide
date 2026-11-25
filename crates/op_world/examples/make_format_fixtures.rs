//! Regenerates the world-format golden fixture.
//!
//! ```text
//! cargo run -p op_world --example make_format_fixtures -- fixtures/format
//! ```
//!
//! Reads `<dir>/meshes.json` and `<dir>/points.json`, rebuilds
//! `<dir>/world/` from scratch and prints the world id. The `format_fixtures` test fails if the committed
//! fixture differs from what this produces, so only regenerate when the
//! format or the sample changes on purpose, and explain why in the commit.

use std::path::PathBuf;
use std::process::ExitCode;

use op_world::Store;
use op_world::sample::{build_sample_world, parse_mesh_specs, parse_points_specs};

fn main() -> ExitCode {
    let Some(dir) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: make_format_fixtures <fixtures/format dir>");
        return ExitCode::from(2);
    };
    let run = || -> Result<String, Box<dyn std::error::Error>> {
        let specs = parse_mesh_specs(&std::fs::read(dir.join("meshes.json"))?)?;
        let points = parse_points_specs(&std::fs::read(dir.join("points.json"))?)?;
        let world_dir = dir.join("world");
        if world_dir.exists() {
            std::fs::remove_dir_all(&world_dir)?;
        }
        Ok(build_sample_world(&Store::new(&world_dir), &specs, &points)?.to_string())
    };
    match run() {
        Ok(id) => {
            println!("{id}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
