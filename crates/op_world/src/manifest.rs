//! Asset manifests (RFC 0001, section 3).

use serde::{Deserialize, Serialize};

use crate::id::ContentId;
use crate::keys::TileKey;
use crate::limits::Document;
use crate::rules::{is_feature_id, is_https_url, is_license, is_sorted_unique, is_text};
use crate::{CONTRACT_VERSION, Error, Result, cjson};

/// World layers. Variants are declared in byte order of their names, so the
/// derived `Ord` matches the sort order used in tile indexes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    Facade,
    Furniture,
    Interactivity,
    Interior,
    Shell,
    Terrain,
    Transport,
    /// Water surfaces (RFC 0003).
    Water,
}

impl Layer {
    pub const ALL: [Layer; 8] = [
        Layer::Facade,
        Layer::Furniture,
        Layer::Interactivity,
        Layer::Interior,
        Layer::Shell,
        Layer::Terrain,
        Layer::Transport,
        Layer::Water,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Layer::Facade => "facade",
            Layer::Furniture => "furniture",
            Layer::Interactivity => "interactivity",
            Layer::Interior => "interior",
            Layer::Shell => "shell",
            Layer::Terrain => "terrain",
            Layer::Transport => "transport",
            Layer::Water => "water",
        }
    }

    /// Private by design: these layers must always be fictional.
    pub fn must_be_fictional(self) -> bool {
        matches!(self, Layer::Interior | Layer::Furniture)
    }

    /// Visual only (RFC 0003): these layers must have `collider: none`, so
    /// the simulation never sees them and the terrain stays the surface
    /// players stand on.
    pub fn must_not_collide(self) -> bool {
        matches!(self, Layer::Water)
    }
}

/// Whether the simulation collides with an asset's mesh.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Collider {
    Trimesh,
    None,
}

/// Axis-aligned bounds in integer millimetres.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundsMm {
    pub min: [i64; 3],
    pub max: [i64; 3],
}

impl BoundsMm {
    pub fn is_valid(&self) -> bool {
        (0..3).all(|i| self.min[i] <= self.max[i])
    }

    pub fn union(&self, other: &BoundsMm) -> BoundsMm {
        BoundsMm {
            min: [0, 1, 2].map(|i| self.min[i].min(other.min[i])),
            max: [0, 1, 2].map(|i| self.max[i].max(other.max[i])),
        }
    }

    /// Bounds moved by `offset` (e.g. from tile-local to region frame).
    pub fn translated(&self, offset: [i64; 3]) -> BoundsMm {
        BoundsMm {
            min: [0, 1, 2].map(|i| self.min[i] + offset[i]),
            max: [0, 1, 2].map(|i| self.max[i] + offset[i]),
        }
    }

    pub fn intersects(&self, other: &BoundsMm) -> bool {
        (0..3).all(|i| self.min[i] <= other.max[i] && other.min[i] <= self.max[i])
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Physics {
    pub collider: Collider,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub dataset: String,
    pub version: String,
    pub license: String,
    pub attribution: String,
    pub url: String,
    pub source_ids: Vec<String>,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Generator {
    pub name: String,
    pub version: String,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub license: String,
    pub attribution: Vec<String>,
    pub sources: Vec<Source>,
    pub generator: Generator,
}

/// One asset: a glb mesh in one layer, with lineage and provenance.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetManifest {
    pub format: String,
    pub contract_version: String,
    pub content_id: ContentId,
    pub size_bytes: u64,
    pub layer: Layer,
    pub feature_id: String,
    pub tile: TileKey,
    pub bounds_mm: BoundsMm,
    pub physics: Physics,
    pub fictional: bool,
    pub parent_version_id: Option<ContentId>,
    pub provenance: Provenance,
}

/// Largest allowed glb, in bytes.
pub const MAX_ASSET_BYTES: u64 = 64 * 1024 * 1024;

impl AssetManifest {
    pub const FORMAT: &'static str = "streetwide.asset";

    /// Parses and checks a canonical manifest. Bytes over the limit for
    /// asset manifests ([`Document::AssetManifest`]) are refused before
    /// they are parsed.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Document::AssetManifest.check(bytes)?;
        let manifest: Self = cjson::from_canonical(bytes)?;
        manifest.ensure_valid()?;
        Ok(manifest)
    }

    /// Canonical bytes; fails if the manifest breaks a rule or its bytes
    /// would be over the limit for asset manifests.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        self.ensure_valid()?;
        let bytes = cjson::to_canonical(self)?;
        if !Document::AssetManifest.fits(bytes.len()) {
            return Err(Error::Invalid(format!(
                "asset manifest for {:?}: {}",
                self.feature_id,
                Document::AssetManifest.too_large()
            )));
        }
        Ok(bytes)
    }

    /// The version id: the hash of the canonical manifest.
    pub fn version_id(&self) -> Result<ContentId> {
        Ok(ContentId::of(&self.to_bytes()?))
    }

    pub fn ensure_valid(&self) -> Result<()> {
        let problems = self.problems();
        if problems.is_empty() {
            Ok(())
        } else {
            Err(Error::Invalid(format!(
                "asset manifest for {:?}: {}",
                self.feature_id,
                problems.join("; ")
            )))
        }
    }

    /// Every rule this manifest breaks (empty if valid).
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
        if self.size_bytes == 0 || self.size_bytes > MAX_ASSET_BYTES {
            p.push(format!("size_bytes must be 1..={MAX_ASSET_BYTES}"));
        }
        if !is_feature_id(&self.feature_id) {
            p.push(format!("feature_id {:?} is not valid", self.feature_id));
        }
        if !self.bounds_mm.is_valid() {
            p.push("bounds_mm min must not exceed max".into());
        }
        if self.layer.must_be_fictional() && !self.fictional {
            p.push(format!(
                "{} assets must be fictional (private by design)",
                self.layer.as_str()
            ));
        }
        if self.layer.must_not_collide() && self.physics.collider != Collider::None {
            p.push(format!(
                "{} assets must have collider none (visual only)",
                self.layer.as_str()
            ));
        }
        p.extend(self.provenance.problems());
        p
    }
}

impl Provenance {
    fn problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        if !is_license(&self.license) {
            p.push(format!(
                "provenance.license {:?} is not valid",
                self.license
            ));
        }
        if self.attribution.is_empty()
            || !self.attribution.iter().all(|a| is_text(a))
            || !is_sorted_unique(&self.attribution)
        {
            p.push("provenance.attribution must be non-empty, sorted and unique".into());
        }
        if self.sources.is_empty() {
            p.push("provenance.sources must not be empty".into());
        }
        for (i, s) in self.sources.iter().enumerate() {
            if ![&s.dataset, &s.version, &s.attribution]
                .iter()
                .all(|t| is_text(t))
            {
                p.push(format!(
                    "provenance.sources[{i}] has an empty or invalid text field"
                ));
            }
            if !is_license(&s.license) {
                p.push(format!("provenance.sources[{i}].license is not valid"));
            }
            if !is_https_url(&s.url) {
                p.push(format!("provenance.sources[{i}].url must be an https URL"));
            }
            if !s.source_ids.iter().all(|t| is_text(t)) || !is_sorted_unique(&s.source_ids) {
                p.push(format!(
                    "provenance.sources[{i}].source_ids must be sorted and unique"
                ));
            }
        }
        if !is_text(&self.generator.name) || !is_text(&self.generator.version) {
            p.push("provenance.generator needs a name and a version".into());
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_are_declared_in_byte_order_of_their_names() {
        // Tile indexes sort by layer name; the derived order must agree, so
        // a new layer goes where its name sorts (water after transport).
        let mut by_name = Layer::ALL;
        by_name.sort_by_key(|l| l.as_str().as_bytes());
        assert_eq!(by_name, Layer::ALL);
        assert!(Layer::ALL.windows(2).all(|w| w[0] < w[1]));
        for l in Layer::ALL {
            assert_eq!(serde_json::to_value(l).unwrap(), l.as_str());
        }
    }
}
