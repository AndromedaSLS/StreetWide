//! World store on disk (RFC 0001, section 7).
//!
//! ```text
//! world.json                  the world manifest
//! objects/<aa>/<hex>.json     tile indexes, asset manifests and points
//!                             documents (RFC 0004)
//! objects/<aa>/<hex>.glb      meshes
//! ```
//!
//! A store may come from elsewhere (a downloaded world, a contributor's
//! bundle), so reads check what they read: an object's bytes must hash to
//! its id, and every file is read with [`read_file`], which takes only a
//! regular file of at most [`MAX_FILE_BYTES`]. A JSON document whose kind
//! is known is read with [`read_document`] or [`Store::read_document`],
//! which take at most the limit for its kind ([`crate::limits`]).

use std::fs;
use std::io::{self, Read, Write as _};
use std::path::{Path, PathBuf};

use crate::id::ContentId;
use crate::limits::Document;
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
///
/// A file larger than `limit` is refused with [`io::ErrorKind::FileTooLarge`],
/// anything else that is not a regular file with
/// [`io::ErrorKind::InvalidData`].
///
/// The path is checked, then opened, and the open file is checked again, so
/// a device or a larger file put in its place in between is refused too.
/// Two such swaps get through: a link to a regular file put there in
/// between is followed, and a FIFO makes the open itself block. Only
/// someone who writes the store while it is read can do that, and they
/// control its content anyway.
pub fn read_file(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    read_file_opened_with(path, limit, |p| fs::File::open(p))
}

/// [`read_file`], opening the checked path with `open` (tests swap the
/// file in between).
fn read_file_opened_with(
    path: &Path,
    limit: u64,
    open: impl FnOnce(&Path) -> io::Result<fs::File>,
) -> io::Result<Vec<u8>> {
    check_file(&fs::symlink_metadata(path)?, limit)?;
    let file = open(path)?;
    check_file(&file.metadata()?, limit)?;
    let mut bytes = Vec::new();
    // `take` stops a file that grew after the checks above.
    file.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(too_large(limit));
    }
    Ok(bytes)
}

/// What [`read_file`] takes: a regular file of at most `limit` bytes.
fn check_file(meta: &fs::Metadata, limit: u64) -> io::Result<()> {
    if !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a regular file",
        ));
    }
    if meta.len() > limit {
        return Err(too_large(limit));
    }
    Ok(())
}

fn too_large(limit: u64) -> io::Error {
    io::Error::new(
        io::ErrorKind::FileTooLarge,
        format!("larger than {limit} bytes"),
    )
}

/// The bytes of the JSON document of kind `kind` at `path`: [`read_file`]
/// with the limit for its kind ([`Document::max_bytes`], RFC 0001 section
/// 7), so a longer file is refused before it is read, and the error says
/// the kind and its limit ([`Document::too_large`]).
pub fn read_document(path: &Path, kind: Document) -> io::Result<Vec<u8>> {
    read_file(path, kind.max_bytes()).map_err(|e| {
        if e.kind() == io::ErrorKind::FileTooLarge {
            io::Error::new(io::ErrorKind::FileTooLarge, kind.too_large())
        } else {
            e
        }
    })
}

/// Whether `path` names a regular file itself, as [`read_file`] requires. A
/// link to one does not count, like a directory or a FIFO.
fn is_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
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

    /// Whether the object exists, as a regular file, in the root or a
    /// fallback.
    pub fn has_object(&self, id: &ContentId, kind: ObjectKind) -> bool {
        std::iter::once(&self.root)
            .chain(&self.fallbacks)
            .any(|dir| is_regular_file(&object_path_in(dir, id, kind)))
    }

    /// Reads an object (from the root, else the first fallback that has it
    /// as a regular file) and verifies that its bytes hash to `id`. A link,
    /// a directory or a FIFO in one layer does not hide the object in the
    /// next: objects are named by their hash, so any copy will do.
    pub fn read_object(&self, id: &ContentId, kind: ObjectKind) -> Result<Vec<u8>> {
        self.read_object_with(id, kind, |path| read_file(path, MAX_FILE_BYTES))
    }

    /// [`Store::read_object`] for a JSON document of kind `kind`, such as a
    /// tile index or an asset manifest: a file over the limit for its kind
    /// is refused before it is read ([`read_document`]).
    pub fn read_document(&self, id: &ContentId, kind: Document) -> Result<Vec<u8>> {
        self.read_object_with(id, ObjectKind::Json, |path| read_document(path, kind))
    }

    /// [`Store::read_object`], reading the file it finds with `read`.
    fn read_object_with(
        &self,
        id: &ContentId,
        kind: ObjectKind,
        read: impl FnOnce(&Path) -> io::Result<Vec<u8>>,
    ) -> Result<Vec<u8>> {
        let path = std::iter::once(&self.root)
            .chain(&self.fallbacks)
            .map(|dir| object_path_in(dir, id, kind))
            .find(|p| is_regular_file(p))
            .unwrap_or_else(|| self.object_path(id, kind));
        let bytes = read(&path).map_err(|e| Error::Io(format!("object {id}: {e}")))?;
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

/// Writes `path` through a temporary file next to it, then renames it, so a
/// reader never sees half a file. Whatever is at the temporary path (a file
/// left by a crash, a link, a FIFO) is removed first and the temporary file
/// is created anew: a store may come from elsewhere, and the write must not
/// follow a link out of it or wait on a FIFO. If the write or the rename
/// fails, the temporary file is removed again.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    match fs::remove_file(&tmp) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e.into()),
        _ => {}
    }
    let written = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .and_then(|mut file| file.write_all(bytes))
        .and_then(|()| fs::rename(&tmp, path));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    Ok(written?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Found by review: the path was checked, then opened again, so a
    /// device put in its place in between was read as if it were the file.
    /// The open file is now checked too. Here the open finds the null
    /// device, which reads as an empty file.
    #[test]
    fn a_device_put_in_place_after_the_check_is_refused() {
        let dir = std::env::temp_dir().join(format!("op_world-store-swap-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("file");
        fs::write(&path, b"12345").unwrap();
        let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
        let err = read_file_opened_with(&path, 10, |_| fs::File::open(null))
            .expect_err("the null device is not a regular file");
        if cfg!(unix) {
            // Windows gives no metadata for it at all.
            assert_eq!(err.to_string(), "not a regular file");
        }
        assert_eq!(read_file(&path, 10).unwrap(), b"12345");
        fs::remove_dir_all(&dir).ok();
    }

    /// Found by review: the temporary file was written with `fs::write`,
    /// which follows a link at its path (a store may come from elsewhere)
    /// and waits forever on a FIFO, and it was left behind when the rename
    /// failed. Whatever is at the temporary path is now removed first, and
    /// the temporary file is removed again after a failure.
    #[test]
    fn a_write_follows_no_link_and_leaves_no_temporary_file() {
        let dir = std::env::temp_dir().join(format!("op_world-store-tmp-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // The rename fails: a directory is where the file goes.
        let blocked = dir.join("blocked.json");
        fs::create_dir_all(blocked.join("inside")).unwrap();
        assert!(write_atomically(&blocked, b"new").is_err());
        assert!(
            fs::symlink_metadata(dir.join("blocked.tmp")).is_err(),
            "the temporary file is left behind"
        );

        // A file left by a crash at the temporary path is replaced.
        let path = dir.join("world.json");
        fs::write(dir.join("world.tmp"), b"left by a crash").unwrap();
        write_atomically(&path, b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert!(fs::symlink_metadata(dir.join("world.tmp")).is_err());

        // A link at the temporary path is removed, not written through.
        let outside = dir.join("outside.txt");
        fs::write(&outside, b"not the store's").unwrap();
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&outside, dir.join("world.tmp"));
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_file(&outside, dir.join("world.tmp"));
        #[cfg(not(any(unix, windows)))]
        let linked: io::Result<()> = Err(io::ErrorKind::Unsupported.into());
        match linked {
            Ok(()) => {
                write_atomically(&path, b"newer").unwrap();
                assert_eq!(fs::read(&path).unwrap(), b"newer");
                assert_eq!(fs::read(&outside).unwrap(), b"not the store's");
            }
            Err(e) => println!("links not checked: cannot make a link here ({e})"),
        }
        fs::remove_dir_all(&dir).ok();
    }
}
