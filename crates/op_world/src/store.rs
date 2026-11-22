//! World store on disk (RFC 0001, section 7).
//!
//! ```text
//! world.json                  the world manifest
//! objects/<aa>/<hex>.json     tile indexes and asset manifests
//! objects/<aa>/<hex>.glb      meshes
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use crate::id::ContentId;
use crate::world::WorldManifest;
use crate::{Error, Result};

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
#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn world_path(&self) -> PathBuf {
        self.root.join("world.json")
    }

    pub fn object_path(&self, id: &ContentId, kind: ObjectKind) -> PathBuf {
        let hex = id.hex();
        self.root
            .join("objects")
            .join(&hex[..2])
            .join(format!("{hex}.{}", kind.extension()))
    }

    /// Reads an object and verifies that its bytes hash to `id`.
    pub fn read_object(&self, id: &ContentId, kind: ObjectKind) -> Result<Vec<u8>> {
        let path = self.object_path(id, kind);
        let bytes = fs::read(&path).map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
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
        if fs::read(&path).is_ok_and(|existing| existing == bytes) {
            return Ok(id);
        }
        write_atomically(&path, bytes)?;
        Ok(id)
    }

    /// Reads and parses `world.json`, returning the world id and manifest.
    pub fn read_world(&self) -> Result<(ContentId, WorldManifest)> {
        let path = self.world_path();
        let bytes = fs::read(&path).map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
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

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}
