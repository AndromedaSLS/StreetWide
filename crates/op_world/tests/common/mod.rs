#![allow(dead_code)] // each test binary uses a different subset

use std::path::{Path, PathBuf};

use op_world::{ContentId, Store};

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn format_dir() -> PathBuf {
    repo_root().join("fixtures/format")
}

pub fn fixture_store() -> Store {
    Store::new(format_dir().join("world"))
}

/// The world id pinned in `fixtures/format/expected.json`.
pub fn expected_world_id() -> ContentId {
    let bytes = std::fs::read(format_dir().join("expected.json")).expect("expected.json");
    let v: serde_json::Value = serde_json::from_slice(&bytes).expect("expected.json is JSON");
    ContentId::parse(v["world_id"].as_str().expect("world_id")).expect("valid id")
}

/// All files under `dir`, as sorted relative paths with `/` separators.
pub fn files_under(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("readable dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                let rel = path.strip_prefix(base).expect("under base");
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// A fresh, empty temporary directory unique to this test.
pub fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("op_world-{name}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean temp dir");
    }
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}
