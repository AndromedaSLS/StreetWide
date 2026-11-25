//! Build information for the benchmark's reports (ADR-0018): the Git
//! commit the client is built from, whether its code differed from that
//! commit, and Cargo's profile.
//!
//! `STREETWIDE_COMMIT` overrides the commit, for builds outside a Git
//! checkout. Without it, and without Git, a report has no commit. The code
//! is the workspace's crates (their `src`, `build.rs` and `Cargo.toml`)
//! and the root `Cargo.toml` and `Cargo.lock`; the script runs again when
//! any of them, or the commit, changes.

use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(["--no-optional-locks"])
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn watch(path: &Path) {
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

/// HEAD's commit, and whether the code differs from it.
fn from_git(here: &Path) -> Option<(String, bool)> {
    let commit = git(here, &["rev-parse", "HEAD"])?;
    let top = PathBuf::from(git(here, &["rev-parse", "--show-toplevel"])?);
    // A new commit or checkout moves HEAD or the branch it names.
    if let Some(dir) = git(here, &["rev-parse", "--absolute-git-dir"]) {
        watch(&Path::new(&dir).join("HEAD"));
    }
    if let Some(common) = git(
        here,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ) {
        let common = PathBuf::from(common);
        watch(&common.join("packed-refs"));
        if let Some(branch) = git(here, &["symbolic-ref", "-q", "HEAD"]) {
            watch(&common.join(branch));
        }
    }
    // The code, relative to the top of the checkout, with `/`.
    let mut code = vec!["Cargo.toml".to_string(), "Cargo.lock".to_string()];
    if let Ok(crates) = std::fs::read_dir(top.join("crates")) {
        let mut names: Vec<String> = crates
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for n in names {
            code.extend(["src", "build.rs", "Cargo.toml"].map(|f| format!("crates/{n}/{f}")));
        }
    }
    code.retain(|p| top.join(p).exists());
    for p in &code {
        watch(&top.join(p));
    }
    let mut args = vec!["status", "--porcelain", "--"];
    args.extend(code.iter().map(String::as_str));
    let dirty = !git(&top, &args)?.is_empty();
    Some((commit, dirty))
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=STREETWIDE_COMMIT");
    for (name, var) in [("PROFILE", "PROFILE"), ("OPT_LEVEL", "OPT_LEVEL")] {
        let value = std::env::var(var).unwrap_or_default();
        println!("cargo:rustc-env=STREETWIDE_{name}={value}");
    }
    let given = std::env::var("STREETWIDE_COMMIT").unwrap_or_default();
    let (commit, dirty) = if !given.trim().is_empty() {
        (given.trim().to_string(), String::new())
    } else {
        let here = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
        match from_git(&here) {
            Some((commit, dirty)) => (commit, dirty.to_string()),
            None => (String::new(), String::new()),
        }
    };
    println!("cargo:rustc-env=STREETWIDE_COMMIT={commit}");
    println!("cargo:rustc-env=STREETWIDE_DIRTY={dirty}");
}
