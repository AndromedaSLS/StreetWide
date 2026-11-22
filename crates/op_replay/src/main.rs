//! `op_replay` command-line tool.
//!
//! Stage 4 adds `check`, `run`, `info` and `synth`. Until then this binary
//! only reports that it is not implemented.

use std::process::ExitCode;

fn main() -> ExitCode {
    eprintln!("op_replay: not implemented yet (see docs/progress.md, stage 4)");
    ExitCode::from(2)
}
