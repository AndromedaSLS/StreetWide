//! World manifests (RFC 0001, section 4).

use serde::{Deserialize, Serialize};

use crate::id::ContentId;
use crate::keys::{RegionKey, TileKey};
use crate::limits::Document;
use crate::manifest::BoundsMm;
use crate::rules::{is_slug, is_sorted_unique, is_text};
use crate::{CONTRACT_VERSION, Error, Result, cjson};

/// Latitude and longitude in integer nanodegrees.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnchorE9 {
    pub lat: i64,
    pub lon: i64,
}

/// The region whose frame the world uses.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub key: RegionKey,
    pub anchor_e9: AnchorE9,
}

/// Default spawn point.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spawn {
    /// Position in the region frame, integer millimetres.
    pub position_mm: [i64; 3],
    /// Heading in 1/65536 turns, 0 = north, clockwise seen from above.
    pub yaw: u16,
}

/// One tile as listed in a world manifest.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldTile {
    pub tile: TileKey,
    pub tile_id: ContentId,
    pub origin_mm: [i64; 3],
    /// Union of the tile's asset bounds, relative to `origin_mm`.
    pub bounds_mm: BoundsMm,
}

/// A playable world. Its hash is the world id.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldManifest {
    pub format: String,
    pub contract_version: String,
    pub name: String,
    pub region: Region,
    pub attribution: Vec<String>,
    pub spawn: Spawn,
    pub tiles: Vec<WorldTile>,
}

/// Web Mercator's latitude limit, in nanodegrees.
pub const MAX_LAT_E9: i64 = 85_051_128_780;
pub const MAX_LON_E9: i64 = 180_000_000_000;

impl WorldManifest {
    pub const FORMAT: &'static str = "streetwide.world";

    /// Parses and checks a canonical world manifest. World manifests have
    /// no size limit of their own: bytes over the limit of every file
    /// ([`Document::WorldManifest`]) are refused before they are parsed.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Document::WorldManifest.check(bytes)?;
        let world: Self = cjson::from_canonical(bytes)?;
        world.ensure_valid()?;
        Ok(world)
    }

    /// Canonical bytes; fails if the manifest breaks a rule or its bytes
    /// would be over the limit of every file.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        self.ensure_valid()?;
        let bytes = cjson::to_canonical(self)?;
        if !Document::WorldManifest.fits(bytes.len()) {
            return Err(Error::Invalid(format!(
                "world manifest {:?}: {}",
                self.name,
                Document::WorldManifest.too_large()
            )));
        }
        Ok(bytes)
    }

    /// The world id: the hash of the canonical world manifest.
    pub fn id(&self) -> Result<ContentId> {
        Ok(ContentId::of(&self.to_bytes()?))
    }

    pub fn ensure_valid(&self) -> Result<()> {
        let problems = self.problems();
        if problems.is_empty() {
            Ok(())
        } else {
            Err(Error::Invalid(format!(
                "world manifest {:?}: {}",
                self.name,
                problems.join("; ")
            )))
        }
    }

    pub fn problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        if self.format != Self::FORMAT {
            p.push(format!("format must be {:?}", Self::FORMAT));
        }
        if self.contract_version != CONTRACT_VERSION {
            p.push(format!(
                "unsupported contract_version {:?}",
                self.contract_version
            ));
        }
        if !is_slug(&self.name) {
            p.push(format!("name {:?} must be a lowercase slug", self.name));
        }
        let a = self.region.anchor_e9;
        if !(-MAX_LAT_E9..=MAX_LAT_E9).contains(&a.lat)
            || !(-MAX_LON_E9..=MAX_LON_E9).contains(&a.lon)
        {
            p.push("region.anchor_e9 is out of range".into());
        }
        if self.attribution.is_empty()
            || !self.attribution.iter().all(|s| is_text(s))
            || !is_sorted_unique(&self.attribution)
        {
            p.push("attribution must be non-empty, sorted and unique".into());
        }
        if self.tiles.is_empty() {
            p.push("a world must have at least one tile".into());
        }
        let keys: Vec<TileKey> = self.tiles.iter().map(|t| t.tile).collect();
        if !is_sorted_unique(&keys) {
            p.push("tiles must be sorted by x, then y, without duplicates".into());
        }
        for t in &self.tiles {
            if t.tile.region() != self.region.key {
                p.push(format!(
                    "tile {} is outside region {}",
                    t.tile, self.region.key
                ));
            }
            if !t.bounds_mm.is_valid() {
                p.push(format!(
                    "bounds_mm of tile {}: min must not exceed max",
                    t.tile
                ));
            }
        }
        p
    }
}
