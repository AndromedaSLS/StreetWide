//! World store on disk (RFC 0001, section 7).
//!
//! ```text
//! world.json                  the world manifest
//! objects/<aa>/<hex>.json     tile indexes and asset manifests
//! objects/<aa>/<hex>.glb      meshes
//! ```
//!
//! A store may come from elsewhere (a downloaded world, a contributor's
//! bundle), so reads check what they read: an object's bytes must hash to
//! its id, and every file is read with [`read_file`], which takes only a
//! regular file of at most [`MAX_FILE_BYTES`].

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::id::ContentId;
use crate::manifest::MAX_ASSET_BYTES;
use crate::world::WorldManifest;
use crate::{Error, Result};

/// Largest file a store read accepts: the largest asset a manifest may
/// describe ([`MAX_ASSET_BYTES`]), far more than any JSON document needs.
pub const MAX_FILE_BYTES: u64 = MAX_ASSET_BYTES;

/// The bytes of the regular file at `path`, which may be at most `limit`
/// bytes long.
///
/// Stores, contribution bundles and replays may come from anyone. A crafted
/// one can put a FIFO, a device such as `/dev/zero`, or a link to a file
/// outside its directory where a file should be (git keeps symbolic links),
/// or a file far larger than any valid one; reading those would block,
/// never end, exhaust the memory or quote another file in a report. They
/// are refused before anything is read: the path must name a regular file
/// itself, not a link to one. The error does not name the path; callers
/// say what was read.
pub fn read_file(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let refuse = |why: String| Err(io::Error::new(io::ErrorKind::InvalidData, why));
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() {
        return refuse("not a regular file".into());
    }
    if meta.len() > limit {
        return refuse(format!("larger than {limit} bytes"));
    }
    let mut bytes = Vec::new();
    // `take` stops a file that grew after the check above.
    fs::File::open(path)?
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return refuse(format!("larger than {limit} bytes"));
    }
    Ok(bytes)
}

/// Kind of object, which decides its file extension.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ObjectKind {
    Json,
    Glb,
}

impl ObjectKind {
    pub fn extension(self) -> &'static str {
        match self {
            ObjectKind::Json => "json",
            ObjectKind::Glb => "glb",
        }
    }
}

/// A world directory.
///
/// A store may have read-only *fallback* directories: objects missing from
/// the root are looked up in them, in order. Writes always go to the root.
/// The contribution gate (RFC 0002) uses this to see a candidate world as
/// its new tile indexes over the contributor's bundle over the canonical
/// world, without copying anything.
#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
    fallbacks: Vec<PathBuf>,
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            fallbacks: Vec::new(),
        }
    }

    /// A store at `root` that also reads objects from `fallbacks`.
    pub fn layered(root: impl Into<PathBuf>, fallbacks: Vec<PathBuf>) -> Self {
        Self {
            root: root.into(),
            fallbacks,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn world_path(&self) -> PathBuf {
        self.root.join("world.json")
    }

    pub fn object_path(&self, id: &ContentId, kind: ObjectKind) -> PathBuf {
        object_path_in(&self.root, id, kind)
    }

    /// Whether the object exists in the root or a fallback.
    pub fn has_object(&self, id: &ContentId, kind: ObjectKind) -> bool {
        std::iter::once(&self.root)
            .chain(&self.fallbacks)
            .any(|dir| object_path_in(dir, id, kind).is_file())
    }

    /// Reads an object (from the root, else the first fallback that has it)
    /// and verifies that its bytes hash to `id`.
    pub fn read_object(&self, id: &ContentId, kind: ObjectKind) -> Result<Vec<u8>> {
        let path = std::iter::once(&self.root)
            .chain(&self.fallbacks)
            .map(|dir| object_path_in(dir, id, kind))
            .find(|p| p.is_file())
            .unwrap_or_else(|| self.object_path(id, kind));
        let bytes =
            read_file(&path, MAX_FILE_BYTES).map_err(|e| Error::Io(format!("object {id}: {e}")))?;
        let actual = ContentId::of(&bytes);
        if actual != *id {
            return Err(Error::HashMismatch {
                expected: id.to_string(),
                actual: actual.to_string(),
            });
        }
        Ok(bytes)
    }

    /// Stores bytes under their content id and returns the id.
    pub fn write_object(&self, bytes: &[u8], kind: ObjectKind) -> Result<ContentId> {
        let id = ContentId::of(bytes);
        let path = self.object_path(&id, kind);
        if read_file(&path, bytes.len() as u64).is_ok_and(|existing| existing == bytes) {
            return Ok(id);
        }
        write_atomically(&path, bytes)?;
        Ok(id)
    }

    /// Reads and parses `world.json`, returning the world id and manifest.
    pub fn read_world(&self) -> Result<(ContentId, WorldManifest)> {
        let path = self.world_path();
        let bytes = read_file(&path, MAX_FILE_BYTES)
            .map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
        let world = WorldManifest::from_bytes(&bytes)?;
        Ok((ContentId::of(&bytes), world))
    }

    /// Writes `world.json` and returns the world id.
    pub fn write_world(&self, world: &WorldManifest) -> Result<ContentId> {
        let bytes = world.to_bytes()?;
        write_atomically(&self.world_path(), &bytes)?;
        Ok(ContentId::of(&bytes))
    }
}

fn object_path_in(root: &Path, id: &ContentId, kind: ObjectKind) -> PathBuf {
    let hex = id.hex();
    root.join("objects")
        .join(&hex[..2])
        .join(format!("{hex}.{}", kind.extension()))
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}
