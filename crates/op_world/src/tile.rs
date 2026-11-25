//! Tile indexes (RFC 0001, section 4).

use serde::{Deserialize, Serialize};

use crate::id::ContentId;
use crate::keys::{RegionKey, TileKey};
use crate::limits::Document;
use crate::manifest::{BoundsMm, Collider, Layer, point_feature_id_problem};
use crate::rules::{is_feature_id, is_license};
use crate::{CONTRACT_VERSION, Error, Result, cjson};

/// One asset as listed in a tile index. The copied fields must match the
/// asset's manifest.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TileAsset {
    pub layer: Layer,
    pub feature_id: String,
    pub manifest_id: ContentId,
    pub content_id: ContentId,
    pub license: String,
    pub collider: Collider,
    pub bounds_mm: BoundsMm,
}

/// The assets of one z17 tile.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TileIndex {
    pub format: String,
    pub contract_version: String,
    pub tile: TileKey,
    pub region: RegionKey,
    pub origin_mm: [i64; 3],
    pub assets: Vec<TileAsset>,
}

impl TileIndex {
    pub const FORMAT: &'static str = "streetwide.tile";

    /// Parses and checks a canonical tile index. Bytes over the limit for
    /// tile indexes ([`Document::TileIndex`]) are refused before they are
    /// parsed.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Document::TileIndex.check(bytes)?;
        let index: Self = cjson::from_canonical(bytes)?;
        index.ensure_valid()?;
        Ok(index)
    }

    /// Canonical bytes; fails if the index breaks a rule or its bytes would
    /// be over the limit for tile indexes.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        self.ensure_valid()?;
        let bytes = cjson::to_canonical(self)?;
        if !Document::TileIndex.fits(bytes.len()) {
            return Err(Error::Invalid(format!(
                "tile index {}: {}",
                self.tile,
                Document::TileIndex.too_large()
            )));
        }
        Ok(bytes)
    }

    /// The tile id: the hash of the canonical tile index.
    pub fn id(&self) -> Result<ContentId> {
        Ok(ContentId::of(&self.to_bytes()?))
    }

    /// Union of all asset bounds, relative to the tile origin.
    pub fn bounds(&self) -> Option<BoundsMm> {
        self.assets
            .iter()
            .map(|a| a.bounds_mm)
            .reduce(|a, b| a.union(&b))
    }

    pub fn ensure_valid(&self) -> Result<()> {
        let problems = self.problems();
        if problems.is_empty() {
            Ok(())
        } else {
            Err(Error::Invalid(format!(
                "tile index {}: {}",
                self.tile,
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
        if self.region != self.tile.region() {
            p.push(format!(
                "region {} does not contain tile {} (expected {})",
                self.region,
                self.tile,
                self.tile.region()
            ));
        }
        if self.assets.is_empty() {
            p.push("a tile must list at least one asset".into());
        }
        let keys: Vec<(Layer, &str)> = self
            .assets
            .iter()
            .map(|a| (a.layer, a.feature_id.as_str()))
            .collect();
        if !keys.windows(2).all(|w| w[0] < w[1]) {
            p.push("assets must be sorted by (layer, feature_id) without duplicates".into());
        }
        for a in &self.assets {
            if !is_feature_id(&a.feature_id) {
                p.push(format!("feature_id {:?} is not valid", a.feature_id));
            } else if let Some(problem) =
                point_feature_id_problem(a.layer, &a.feature_id, self.tile)
            {
                p.push(problem);
            }
            if !is_license(&a.license) {
                p.push(format!(
                    "license {:?} of {:?} is not valid",
                    a.license, a.feature_id
                ));
            }
            if !a.bounds_mm.is_valid() {
                p.push(format!(
                    "bounds_mm of {:?}: min must not exceed max",
                    a.feature_id
                ));
            }
            // Checked here too: the simulation reads colliders from tile
            // indexes and never opens manifests.
            if a.layer.must_not_collide() && a.collider != Collider::None {
                p.push(format!(
                    "{} asset {:?} must have collider none (visual only)",
                    a.layer.as_str(),
                    a.feature_id
                ));
            }
        }
        p
    }
}
