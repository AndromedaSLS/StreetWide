//! Size limits of JSON documents (RFC 0001, section 7).
//!
//! Parsing JSON builds a tree many times the size of its text: serde_json
//! takes 16 to 105 bytes of memory per byte of text, so a crafted document
//! of 64 MiB, the largest file a store read takes ([`MAX_FILE_BYTES`]),
//! would take about 7 GB and seconds to parse. Every kind of document
//! therefore has a limit, set from its largest valid document with a wide
//! margin, and every reader checks it before parsing:
//!
//! - `from_bytes` of each document type refuses longer bytes first;
//! - reads of a file or a store object whose kind is known
//!   ([`store::read_document`](crate::store::read_document),
//!   [`Store::read_document`](crate::Store::read_document)) refuse a longer
//!   file before reading it.
//!
//! `to_bytes` checks the limit too, so nothing is written that a reader
//! would refuse. A document at 1 MiB takes up to about 110 MB to parse, one
//! at 16 MiB about 1.7 GB. `tools/worldfmt/limits.py` sets the same limits,
//! with the same messages.
//!
//! | Kind | Limit | Largest valid document |
//! |---|---|---|
//! | asset manifest | 1 MiB | 4,108 bytes in the Zurich district |
//! | merge record | 1 MiB | a few hundred bytes, plus each review's notes (at most 4,096 characters) |
//! | change set | 8 MiB | under 64 KiB (50 changes, texts of at most 4,096 characters) |
//! | check report | 16 MiB | one detail of at most 4,096 characters per finding |
//! | tile index | 16 MiB | 59,148 bytes in the Zurich district |
//! | world manifest | 64 MiB, the limit of every file | about 230 KB for the 1,024 tiles of a region |
//! | points document | 64 MiB, the limit of every file | about 22 MB for the most points a document holds, 65,536 trees of about 330 bytes (RFC 0004 section 3) |
//!
//! World manifests have no limit of their own: they grow with the world.
//! Points documents (RFC 0004) keep the 64 MiB limit of every file until a
//! limit of their own is set the same way, and navigation documents (RFC
//! 0005) too.

use crate::contrib::{ChangeSet, CheckReport, MergeRecord};
use crate::manifest::AssetManifest;
use crate::points::PointsDocument;
use crate::store::MAX_FILE_BYTES;
use crate::tile::TileIndex;
use crate::world::WorldManifest;
use crate::{Error, Result};

const MIB: u64 = 1 << 20;

/// Largest asset manifest, in bytes (1 MiB).
pub const MAX_ASSET_MANIFEST_BYTES: u64 = MIB;
/// Largest merge record, in bytes (1 MiB).
pub const MAX_MERGE_RECORD_BYTES: u64 = MIB;
/// Largest change set, in bytes (8 MiB).
pub const MAX_CHANGE_SET_BYTES: u64 = 8 * MIB;
/// Largest check report, in bytes (16 MiB).
pub const MAX_CHECK_REPORT_BYTES: u64 = 16 * MIB;
/// Largest tile index, in bytes (16 MiB).
pub const MAX_TILE_INDEX_BYTES: u64 = 16 * MIB;
/// Largest world manifest, in bytes: the limit of every file (64 MiB), as
/// world manifests have no limit of their own.
pub const MAX_WORLD_MANIFEST_BYTES: u64 = MAX_FILE_BYTES;
/// Largest points document (RFC 0004), in bytes: the limit of every file
/// (64 MiB), until points documents get a limit of their own (RFC 0001
/// section 7).
pub const MAX_POINTS_BYTES: u64 = MAX_FILE_BYTES;

/// A kind of JSON document, with its size limit.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Document {
    AssetManifest,
    TileIndex,
    WorldManifest,
    ChangeSet,
    CheckReport,
    MergeRecord,
    /// A points document (RFC 0004), the content of a point asset.
    Points,
}

impl Document {
    pub const ALL: [Document; 7] = [
        Document::AssetManifest,
        Document::TileIndex,
        Document::WorldManifest,
        Document::ChangeSet,
        Document::CheckReport,
        Document::MergeRecord,
        Document::Points,
    ];

    /// The largest document of this kind, in bytes.
    pub const fn max_bytes(self) -> u64 {
        match self {
            Document::AssetManifest => MAX_ASSET_MANIFEST_BYTES,
            Document::TileIndex => MAX_TILE_INDEX_BYTES,
            Document::WorldManifest => MAX_WORLD_MANIFEST_BYTES,
            Document::ChangeSet => MAX_CHANGE_SET_BYTES,
            Document::CheckReport => MAX_CHECK_REPORT_BYTES,
            Document::MergeRecord => MAX_MERGE_RECORD_BYTES,
            Document::Points => MAX_POINTS_BYTES,
        }
    }

    /// The `format` of documents of this kind, such as `streetwide.asset`.
    pub const fn format(self) -> &'static str {
        match self {
            Document::AssetManifest => AssetManifest::FORMAT,
            Document::TileIndex => TileIndex::FORMAT,
            Document::WorldManifest => WorldManifest::FORMAT,
            Document::ChangeSet => ChangeSet::FORMAT,
            Document::CheckReport => CheckReport::FORMAT,
            Document::MergeRecord => MergeRecord::FORMAT,
            Document::Points => PointsDocument::FORMAT,
        }
    }

    /// The kind in words, with its article: "an asset manifest".
    pub const fn name(self) -> &'static str {
        match self {
            Document::AssetManifest => "an asset manifest",
            Document::TileIndex => "a tile index",
            Document::WorldManifest => "a world manifest",
            Document::ChangeSet => "a change set",
            Document::CheckReport => "a check report",
            Document::MergeRecord => "a merge record",
            Document::Points => "a points document",
        }
    }

    /// Why a document of this kind is refused for its size: "larger than
    /// 1048576 bytes (1 MiB), the limit for an asset manifest".
    pub fn too_large(self) -> String {
        let limit = self.max_bytes();
        format!(
            "larger than {limit} bytes ({} MiB), the limit for {}",
            limit / MIB,
            self.name()
        )
    }

    /// Whether `len` bytes are within the limit.
    pub const fn fits(self, len: usize) -> bool {
        len as u64 <= self.max_bytes()
    }

    /// Refuses `bytes` longer than the limit, before anything parses them,
    /// with [`Document::too_large`] as an [`Error::Invalid`].
    pub fn check(self, bytes: &[u8]) -> Result<()> {
        if self.fits(bytes.len()) {
            Ok(())
        } else {
            Err(Error::Invalid(self.too_large()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limits_are_rfc_0001s() {
        let limits: Vec<(&str, u64)> = Document::ALL
            .iter()
            .map(|d| (d.format(), d.max_bytes()))
            .collect();
        assert_eq!(
            limits,
            [
                ("streetwide.asset", 1 << 20),
                ("streetwide.tile", 16 << 20),
                ("streetwide.world", 64 << 20),
                ("streetwide.change", 8 << 20),
                ("streetwide.check", 16 << 20),
                ("streetwide.merge", 1 << 20),
                ("streetwide.points", 64 << 20),
            ]
        );
        // Each fits in a file a store read takes.
        assert!(
            Document::ALL
                .iter()
                .all(|d| d.max_bytes() <= MAX_FILE_BYTES)
        );
    }

    #[test]
    fn the_message_names_the_kind_and_the_limit() {
        assert_eq!(
            Document::ChangeSet.too_large(),
            "larger than 8388608 bytes (8 MiB), the limit for a change set"
        );
        let at = vec![b' '; MAX_ASSET_MANIFEST_BYTES as usize];
        assert!(Document::AssetManifest.check(&at).is_ok());
        let over = vec![b' '; at.len() + 1];
        assert_eq!(
            Document::AssetManifest
                .check(&over)
                .unwrap_err()
                .to_string(),
            "invalid: larger than 1048576 bytes (1 MiB), the limit for an asset manifest"
        );
    }
}
