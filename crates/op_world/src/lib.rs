//! StreetWide world format (RFC 0001).
//!
//! - [`cjson`]: canonical JSON, the only serialisation of every document;
//! - [`id`]: BLAKE3 content ids;
//! - [`keys`]: Web Mercator tile (z17) and region (z12) keys;
//! - [`manifest`], [`tile`], [`world`]: asset manifests, tile indexes and
//!   world manifests, mirroring the JSON Schemas in `schemas/`;
//! - [`limits`]: the size limit of each kind of JSON document;
//! - [`glb`]: the StreetWide glb profile (reference writer, strict reader);
//! - [`store`]: the content-addressed world directory;
//! - [`validate`]: whole-world validation;
//! - [`sample`]: the hand-made world used as the golden format fixture.
//!
//! This crate has no workspace dependencies (ADR-0002).

mod error;

pub mod cjson;
pub mod contrib;
pub mod gate;
pub mod glb;
pub mod id;
pub mod keys;
pub mod limits;
pub mod manifest;
pub mod merge;
pub mod rules;
pub mod sample;
pub mod sample_change;
pub mod store;
pub mod tile;
pub mod validate;
pub mod world;

pub use error::{Error, Result};
pub use id::ContentId;
pub use keys::{RegionKey, TileKey};
pub use manifest::{AssetManifest, BoundsMm, Collider, Layer};
pub use store::{ObjectKind, Store};
pub use tile::{TileAsset, TileIndex};
pub use world::WorldManifest;

/// The world format version this crate reads and writes.
pub const CONTRACT_VERSION: &str = "0.1.0";
