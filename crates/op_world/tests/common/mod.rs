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

/// A fresh, empty temporary directory unique to this test, deleted again
/// when this is dropped, normally at the end of the test. Keep it in a
/// variable for as long as the test uses the directory: a guard that is
/// only a temporary is dropped at the end of its statement.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl std::ops::Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for TempDir {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl From<&TempDir> for PathBuf {
    fn from(dir: &TempDir) -> PathBuf {
        dir.0.clone()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A fresh, empty temporary directory unique to this test (see [`TempDir`]).
pub fn temp_dir(name: &str) -> TempDir {
    let dir = std::env::temp_dir().join(format!("op_world-{name}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean temp dir");
    }
    std::fs::create_dir_all(&dir).expect("create temp dir");
    TempDir(dir)
}

/// A path that does not exist yet, inside a fresh temporary directory that
/// is deleted when this is dropped: for outputs that must be made by the
/// code under test, such as a candidate world.
pub struct TempPath {
    path: PathBuf,
    _dir: TempDir,
}

impl std::ops::Deref for TempPath {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for TempPath {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl From<&TempPath> for PathBuf {
    fn from(path: &TempPath) -> PathBuf {
        path.path.clone()
    }
}

/// `rel` inside a fresh temporary directory named after `name`.
pub fn temp_path(name: &str, rel: &str) -> TempPath {
    let dir = temp_dir(name);
    TempPath {
        path: dir.join(rel),
        _dir: dir,
    }
}
