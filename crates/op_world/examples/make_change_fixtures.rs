//! Regenerates the contribution golden fixture.
//!
//! ```text
//! cargo run -p op_world --example make_change_fixtures -- fixtures
//! ```
//!
//! Builds `<dir>/changes/facade-sample/` (a change set bundle and the gate's
//! report on it) from the ingest fixture `<dir>/world`. The `gate` test
//! fails if the committed fixture differs from what this produces.

use std::path::PathBuf;
use std::process::ExitCode;

use op_world::gate;
use op_world::sample_change::{sample_draft, write_bundle};

fn main() -> ExitCode {
    let Some(dir) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: make_change_fixtures <fixtures dir>");
        return ExitCode::from(2);
    };
    let run = || -> Result<String, Box<dyn std::error::Error>> {
        let world = dir.join("world");
        let bundle = dir.join("changes").join("facade-sample");
        if bundle.exists() {
            std::fs::remove_dir_all(&bundle)?;
        }
        std::fs::create_dir_all(&bundle)?;
        let mut draft = sample_draft(&world)?;
        let change_id = write_bundle(&mut draft, &bundle)?;
        let candidate =
            std::env::temp_dir().join(format!("streetwide-fixture-gate-{}", std::process::id()));
        let report = gate::run(&world, &bundle, &candidate);
        let _ = std::fs::remove_dir_all(&candidate);
        let report = report?;
        std::fs::write(bundle.join("report.json"), report.to_bytes()?)?;
        Ok(format!("change {change_id}, passed {}", report.passed))
    };
    match run() {
        Ok(s) => {
            println!("{s}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
