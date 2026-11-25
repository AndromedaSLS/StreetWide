//! Points documents (RFC 0004): the content of assets in the `vegetation`
//! and `street_objects` layers.
//!
//! A points document is canonical JSON (RFC 0001 section 5) with `format:
//! "streetwide.points"`, stored as `objects/<aa>/<hex>.json` and named by
//! its hash like a mesh. It holds 1 to 65,536 points of one source, sorted
//! by id. Each point keeps the source's own id, an anchor on the ground in
//! integer millimetres relative to the tile origin, every attribute of its
//! kind (`null` when unknown), and the attributes a rule set rather than
//! the source (`estimated`). `rules` names the rule behind each estimated
//! attribute.
//!
//! [`PointsDocument::problems`] checks the document alone (section 3 and
//! 4); [`Summary::asset_problems`] checks it against the manifest of its
//! asset (sections 1, 2 and 6): the kinds belong to the layer, the ids
//! carry the source of the feature id, and `bounds_mm` are the exact bounds
//! of the anchors. `schemas/points.schema.json` states the same rules as
//! far as JSON Schema can, and `tools/worldfmt/points.py` implements them
//! in Python.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::id::ContentId;
use crate::keys::TileKey;
use crate::limits::Document;
use crate::manifest::{BoundsMm, Layer};
use crate::rules::{
    MAX_POINT_ID_CHARS, is_rule_id, is_sorted_unique, is_text, per_tile_source, point_source,
};
use crate::{CONTRACT_VERSION, Error, Result, cjson};

/// Most points in one document (RFC 0004 section 3).
pub const MAX_POINTS: usize = 65_536;

/// The kind of a point (RFC 0004 section 1). Each kind belongs to one
/// layer; a new kind needs an amendment of RFC 0004. Variants are declared
/// in byte order of their names.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PointKind {
    /// A light point as lighting data records it, not necessarily a street
    /// lamp. In `street_objects`.
    Light,
    /// A single tree, anchored at the base of its trunk. In `vegetation`.
    Tree,
}

impl PointKind {
    pub const ALL: [PointKind; 2] = [PointKind::Light, PointKind::Tree];

    pub fn as_str(self) -> &'static str {
        match self {
            PointKind::Light => "light",
            PointKind::Tree => "tree",
        }
    }

    /// The kind named `name`, if there is one.
    pub fn from_name(name: &str) -> Option<PointKind> {
        Self::ALL.into_iter().find(|k| k.as_str() == name)
    }

    /// The layer points of this kind belong to.
    pub fn layer(self) -> Layer {
        match self {
            PointKind::Light => Layer::StreetObjects,
            PointKind::Tree => Layer::Vegetation,
        }
    }

    /// The names of this kind's attributes, in byte order (RFC 0004 section
    /// 4). Every point has all of them.
    pub fn attributes(self) -> &'static [&'static str] {
        match self {
            PointKind::Light => &[
                "count",
                "light_height_mm",
                "method",
                "mount",
                "purpose",
                "yaw",
            ],
            PointKind::Tree => &[
                "crown_diameter_mm",
                "genus",
                "height_mm",
                "leaf_cycle",
                "leaf_type",
                "planted_year",
                "species",
                "taxon",
                "trunk_circumference_mm",
            ],
        }
    }

    /// The attributes a rule may set for this kind in format 0.1: only a
    /// light's `yaw` (RFC 0004 section 4 and its Decision: ADR-0016 keeps
    /// `height_mm` measured or unknown, ADR-0015 leaves `mount` unknown).
    /// Any other needs an amendment of RFC 0004.
    pub fn estimable(self) -> &'static [&'static str] {
        match self {
            PointKind::Light => &["yaw"],
            PointKind::Tree => &[],
        }
    }
}

/// A tree's leaf cycle.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeafCycle {
    Deciduous,
    Evergreen,
    SemiDeciduous,
    SemiEvergreen,
}

/// A tree's leaf type.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeafType {
    Broadleaved,
    Needleleaved,
}

/// How a light point makes light.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LightMethod {
    Fluorescent,
    Gas,
    Incandescent,
    Led,
    Mercury,
    MetalHalide,
    Sodium,
}

/// How a light point is mounted.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mount {
    Ceiling,
    Ground,
    Pole,
    Wall,
    Wire,
}

/// What a light point lights.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    /// Street and path lighting (OSM's `highway=street_lamp`).
    Street,
    /// Lighting for shipping.
    Navigation,
    /// Object, art and decorative lighting.
    Decorative,
}

/// A tree's attributes (RFC 0004 section 4). Lengths are integer
/// millimetres; `None` (`null`) means unknown.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeAttributes {
    /// 1 to 100,000.
    pub crown_diameter_mm: Option<i64>,
    /// The Latin genus, `^[A-Z][a-z]+$`, at most 64 characters.
    pub genus: Option<String>,
    /// 1 to 150,000, from the anchor to the top.
    pub height_mm: Option<i64>,
    pub leaf_cycle: Option<LeafCycle>,
    pub leaf_type: Option<LeafType>,
    /// 1000 to 2999.
    pub planted_year: Option<i64>,
    /// A Latin binomial, at most 128 characters.
    pub species: Option<String>,
    /// The source's full scientific name, with any cultivar, at most 256
    /// characters.
    pub taxon: Option<String>,
    /// 1 to 50,000, at 1.3 m above the ground.
    pub trunk_circumference_mm: Option<i64>,
}

/// A light point's attributes (RFC 0004 section 4); `None` (`null`) means
/// unknown.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LightAttributes {
    /// 1 to 64: the luminaires the source records at this point.
    pub count: Option<i64>,
    /// 1 to 100,000: the light above the anchor.
    pub light_height_mm: Option<i64>,
    pub method: Option<LightMethod>,
    pub mount: Option<Mount>,
    pub purpose: Option<Purpose>,
    /// 0 to 65,535: where the head or arm points, in 1/65536 of a turn
    /// clockwise from north, like the world manifest's spawn yaw.
    pub yaw: Option<i64>,
}

/// A point's attributes: every key of its kind. Written as a plain object;
/// read as the kind whose keys it has, which [`PointsDocument::problems`]
/// checks against the point's `kind`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Attributes {
    Tree(TreeAttributes),
    Light(LightAttributes),
}

impl Attributes {
    /// The kind these attributes are of.
    pub fn kind(&self) -> PointKind {
        match self {
            Attributes::Tree(_) => PointKind::Tree,
            Attributes::Light(_) => PointKind::Light,
        }
    }

    /// Whether the attribute `name` is known (not `null`); `None` if the
    /// kind has no such attribute.
    pub fn is_known(&self, name: &str) -> Option<bool> {
        let known = match self {
            Attributes::Tree(t) => match name {
                "crown_diameter_mm" => t.crown_diameter_mm.is_some(),
                "genus" => t.genus.is_some(),
                "height_mm" => t.height_mm.is_some(),
                "leaf_cycle" => t.leaf_cycle.is_some(),
                "leaf_type" => t.leaf_type.is_some(),
                "planted_year" => t.planted_year.is_some(),
                "species" => t.species.is_some(),
                "taxon" => t.taxon.is_some(),
                "trunk_circumference_mm" => t.trunk_circumference_mm.is_some(),
                _ => return None,
            },
            Attributes::Light(l) => match name {
                "count" => l.count.is_some(),
                "light_height_mm" => l.light_height_mm.is_some(),
                "method" => l.method.is_some(),
                "mount" => l.mount.is_some(),
                "purpose" => l.purpose.is_some(),
                "yaw" => l.yaw.is_some(),
                _ => return None,
            },
        };
        Some(known)
    }

    /// Every value that breaks its rule (RFC 0004 section 4).
    fn problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        let mut range = |name: &str, value: Option<i64>, lo: i64, hi: i64| {
            if value.is_some_and(|v| !(lo..=hi).contains(&v)) {
                p.push(format!("{name} must be {lo}..={hi}"));
            }
        };
        match self {
            Attributes::Tree(t) => {
                range("crown_diameter_mm", t.crown_diameter_mm, 1, 100_000);
                range("height_mm", t.height_mm, 1, 150_000);
                range("planted_year", t.planted_year, 1000, 2999);
                range(
                    "trunk_circumference_mm",
                    t.trunk_circumference_mm,
                    1,
                    50_000,
                );
                if t.genus.as_deref().is_some_and(|g| !is_genus(g)) {
                    p.push("genus must match ^[A-Z][a-z]+$ (at most 64 characters)".into());
                }
                if t.species.as_deref().is_some_and(|s| !is_species(s)) {
                    p.push("species must be a Latin binomial of at most 128 characters".into());
                }
                if t.taxon
                    .as_deref()
                    .is_some_and(|s| !is_text(s) || s.chars().count() > MAX_TAXON_CHARS)
                {
                    p.push("taxon must be text of at most 256 characters".into());
                }
            }
            Attributes::Light(l) => {
                range("count", l.count, 1, 64);
                range("light_height_mm", l.light_height_mm, 1, 100_000);
                range("yaw", l.yaw, 0, 65_535);
            }
        }
        p
    }
}

/// Longest genus, in characters.
pub const MAX_GENUS_CHARS: usize = 64;
/// Longest species, in characters.
pub const MAX_SPECIES_CHARS: usize = 128;
/// Longest taxon, in characters.
pub const MAX_TAXON_CHARS: usize = 256;

/// A Latin genus: `^[A-Z][a-z]+$`, at most 64 characters.
fn is_genus(s: &str) -> bool {
    let mut chars = s.chars();
    s.len() <= MAX_GENUS_CHARS
        && chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && !chars.as_str().is_empty()
        && chars.all(|c| c.is_ascii_lowercase())
}

/// A Latin binomial, at most 128 characters: a genus, then one or more
/// words, each after a single space, of any characters but spaces and
/// control characters: `^[A-Z][a-z]+( [^ \x00-\x1F\x7F]+)+$`. So
/// `Tilia cordata` and `Platanus × hispanica`.
fn is_species(s: &str) -> bool {
    let mut words = s.split(' ');
    let genus = words.next().unwrap_or_default();
    let mut rest = words.peekable();
    s.chars().count() <= MAX_SPECIES_CHARS
        && is_genus_word(genus)
        && rest.peek().is_some()
        && rest.all(|w| !w.is_empty() && !w.chars().any(crate::cjson::is_control))
}

/// `[A-Z][a-z]+`, without a length limit (a species' first word).
fn is_genus_word(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && !chars.as_str().is_empty()
        && chars.all(|c| c.is_ascii_lowercase())
}

/// One point (RFC 0004 section 3). Every key is written; `null` means
/// unknown.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Point {
    /// The source's own id, `{source}:{local id}` ([`point_source`]).
    pub id: String,
    pub kind: PointKind,
    /// The ground at the object's position in the source, `[x, y, z]` in
    /// integer millimetres relative to the tile origin (section 5). Never
    /// estimated.
    pub anchor_mm: [i64; 3],
    pub attributes: Attributes,
    /// The attributes a rule set, rather than the source record or a
    /// measurement: sorted, unique, each known and allowed to be estimated
    /// for the kind ([`PointKind::estimable`]).
    pub estimated: Vec<String>,
}

impl Point {
    /// Every rule of section 3 and 4 this point breaks on its own.
    fn problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        if point_source(&self.id).is_none() {
            p.push(format!(
                "id must be <source>:<local id>, at most {MAX_POINT_ID_CHARS} characters (RFC 0004 section 2)"
            ));
        }
        if self.attributes.kind() != self.kind {
            p.push(format!(
                "attributes must be those of a {}: {}",
                self.kind.as_str(),
                self.kind.attributes().join(", ")
            ));
        } else {
            p.extend(self.attributes.problems());
        }
        if !is_sorted_unique(&self.estimated) {
            p.push("estimated must be sorted and unique".into());
        }
        for name in &self.estimated {
            if !self.kind.estimable().contains(&name.as_str()) {
                p.push(format!(
                    "{name:?} may not be estimated for a {}",
                    self.kind.as_str()
                ));
            } else if self.attributes.is_known(name) == Some(false) {
                p.push(format!("estimated {name:?} is null"));
            }
        }
        p
    }
}

/// A points document (RFC 0004 section 3).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointsDocument {
    pub format: String,
    pub contract_version: String,
    /// 1 to 65,536 points, sorted by id in byte order, without duplicates.
    pub points: Vec<Point>,
    /// For each kind, its estimated attributes and the id of the rule that
    /// set each ([`is_rule_id`]). It names nothing that no point estimates,
    /// and is `{}` when nothing is estimated.
    pub rules: BTreeMap<String, BTreeMap<String, String>>,
}

impl PointsDocument {
    pub const FORMAT: &'static str = "streetwide.points";

    /// Parses and checks a canonical points document. Bytes over the limit
    /// for points documents ([`Document::Points`]) are refused before they
    /// are parsed.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Document::Points.check(bytes)?;
        let doc: Self = cjson::from_canonical(bytes)?;
        doc.ensure_valid()?;
        Ok(doc)
    }

    /// Canonical bytes; fails if the document breaks a rule or its bytes
    /// would be over the limit for points documents.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        self.ensure_valid()?;
        let bytes = cjson::to_canonical(self)?;
        if !Document::Points.fits(bytes.len()) {
            return Err(Error::Invalid(format!(
                "points document: {}",
                Document::Points.too_large()
            )));
        }
        Ok(bytes)
    }

    /// The content id of the document: the hash of its canonical bytes.
    pub fn id(&self) -> Result<ContentId> {
        Ok(ContentId::of(&self.to_bytes()?))
    }

    pub fn ensure_valid(&self) -> Result<()> {
        let problems = self.problems();
        if problems.is_empty() {
            Ok(())
        } else {
            Err(Error::Invalid(format!(
                "points document: {}",
                problems.join("; ")
            )))
        }
    }

    /// Every rule of RFC 0004 sections 3 and 4 the document breaks (empty
    /// if valid).
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
        if self.points.is_empty() || self.points.len() > MAX_POINTS {
            p.push(format!("a points document has 1..={MAX_POINTS} points"));
        }
        if !self
            .points
            .windows(2)
            .all(|w| w[0].id.as_bytes() < w[1].id.as_bytes())
        {
            p.push("points must be sorted by id without duplicates".into());
        }
        let mut sources: BTreeSet<&str> = BTreeSet::new();
        for point in &self.points {
            sources.extend(point_source(&point.id));
            for problem in point.problems() {
                p.push(format!("point {:?}: {problem}", point.id));
            }
        }
        if sources.len() > 1 {
            p.push(format!(
                "every point must have the same source (its id without the last part), not {}",
                sources
                    .iter()
                    .map(|s| format!("{s:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        p.extend(self.rule_problems());
        p
    }

    /// `rules` against the points' `estimated` (section 3).
    fn rule_problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        let mut used: BTreeSet<(&str, &str)> = BTreeSet::new();
        for point in &self.points {
            for name in &point.estimated {
                let kind = point.kind.as_str();
                used.insert((kind, name.as_str()));
                let ruled = self.rules.get(kind).is_some_and(|r| r.contains_key(name));
                if !ruled && point.kind.estimable().contains(&name.as_str()) {
                    p.push(format!(
                        "point {:?}: estimated {name:?} has no rule in rules.{kind}",
                        point.id
                    ));
                }
            }
        }
        for (kind, rules) in &self.rules {
            let Some(k) = PointKind::from_name(kind) else {
                p.push(format!("rules: {kind:?} is not a kind of point"));
                continue;
            };
            if rules.is_empty() {
                p.push(format!(
                    "rules.{kind} must name at least one estimated attribute"
                ));
            }
            for (name, rule) in rules {
                if !k.estimable().contains(&name.as_str()) {
                    p.push(format!(
                        "rules.{kind}: {name:?} may not be estimated for a {kind}"
                    ));
                    continue;
                }
                if !is_rule_id(rule) {
                    p.push(format!(
                        "rules.{kind}.{name}: {rule:?} is not a rule id (adr-<4 digits>:<name>:<version>)"
                    ));
                }
                if !used.contains(&(kind.as_str(), name.as_str())) {
                    p.push(format!(
                        "rules.{kind}.{name}: no {kind} point has an estimated {name}"
                    ));
                }
            }
        }
        p
    }

    /// The source of every point (the id without its last part), if they
    /// share one.
    pub fn source(&self) -> Option<&str> {
        let first = point_source(&self.points.first()?.id)?;
        self.points
            .iter()
            .all(|p| point_source(&p.id) == Some(first))
            .then_some(first)
    }

    /// The exact bounds of the anchors, relative to the tile origin: what an
    /// asset's `bounds_mm` must be. `None` without points.
    pub fn bounds(&self) -> Option<BoundsMm> {
        self.points
            .iter()
            .map(|p| BoundsMm {
                min: p.anchor_mm,
                max: p.anchor_mm,
            })
            .reduce(|a, b| a.union(&b))
    }

    /// What an asset's manifest is checked against, read once.
    pub fn summary(&self) -> Summary {
        Summary {
            source: self.source().map(str::to_string),
            bounds: self.bounds(),
            kinds: self.points.iter().map(|p| p.kind).collect(),
            count: self.points.len(),
        }
    }

    /// [`Summary::asset_problems`] of this document.
    pub fn asset_problems(
        &self,
        layer: Layer,
        feature_id: &str,
        tile: TileKey,
        bounds_mm: &BoundsMm,
    ) -> Vec<String> {
        self.summary()
            .asset_problems(layer, feature_id, tile, bounds_mm)
    }
}

/// What a manifest is checked against in its points document: the
/// points' source, the anchors' bounds, the kinds and the number of points.
/// Each takes one pass over the points, so a world that lists one document
/// many times checks each listing in constant time.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Summary {
    pub source: Option<String>,
    pub bounds: Option<BoundsMm>,
    pub kinds: BTreeSet<PointKind>,
    pub count: usize,
}

impl Summary {
    /// The rules an asset's manifest and its valid points document must
    /// agree on (RFC 0004 sections 1, 2 and 6), each as a message; empty if
    /// they agree:
    /// - the layer holds points documents, and every point's kind belongs
    ///   to it;
    /// - every point id carries the source of the feature id
    ///   `tile:17/{x}/{y}/{source}`;
    /// - `bounds_mm` are the exact bounds of the anchors.
    pub fn asset_problems(
        &self,
        layer: Layer,
        feature_id: &str,
        tile: TileKey,
        bounds_mm: &BoundsMm,
    ) -> Vec<String> {
        let mut p = Vec::new();
        if !layer.has_points() {
            p.push(format!(
                "{} assets are meshes, not points documents",
                layer.as_str()
            ));
            return p;
        }
        for kind in &self.kinds {
            if kind.layer() != layer {
                p.push(format!(
                    "points of kind {} belong in the {} layer, not {}",
                    kind.as_str(),
                    kind.layer().as_str(),
                    layer.as_str()
                ));
            }
        }
        if let Some(wanted) = per_tile_source(feature_id, tile)
            && self.source.as_deref() != Some(wanted)
        {
            p.push(format!(
                "the points' source must be {wanted:?}, the source of feature id {feature_id:?}"
            ));
        }
        if self.bounds.as_ref() != Some(bounds_mm) {
            p.push("bounds_mm are not the exact bounds of the anchors".into());
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(id: &str) -> Point {
        Point {
            id: id.into(),
            kind: PointKind::Tree,
            anchor_mm: [1, 2, 3],
            attributes: Attributes::Tree(TreeAttributes::default()),
            estimated: vec![],
        }
    }

    fn doc(points: Vec<Point>) -> PointsDocument {
        PointsDocument {
            format: PointsDocument::FORMAT.into(),
            contract_version: CONTRACT_VERSION.into(),
            points,
            rules: BTreeMap::new(),
        }
    }

    #[test]
    fn kinds_attributes_and_layers() {
        for kind in PointKind::ALL {
            assert_eq!(serde_json::to_value(kind).unwrap(), kind.as_str());
            assert_eq!(PointKind::from_name(kind.as_str()), Some(kind));
            assert!(kind.layer().has_points());
            assert!(kind.attributes().windows(2).all(|w| w[0] < w[1]));
            assert!(
                kind.estimable()
                    .iter()
                    .all(|a| kind.attributes().contains(a))
            );
            // The written attributes are exactly the kind's keys.
            let written = match kind {
                PointKind::Tree => serde_json::to_value(TreeAttributes::default()),
                PointKind::Light => serde_json::to_value(LightAttributes::default()),
            }
            .unwrap();
            let keys: Vec<&str> = written
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(keys, kind.attributes());
        }
        assert!(PointKind::ALL.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn species_and_genus() {
        for ok in [
            "Tilia cordata",
            "Platanus \u{d7} hispanica",
            "Acer x freemanii",
        ] {
            assert!(is_species(ok), "{ok}");
        }
        for bad in [
            "Tilia",
            "tilia cordata",
            "Tilia  cordata",
            "Tilia cordata ",
            " Tilia cordata",
            "T cordata",
            "Tilia\tcordata",
            "Tilia cor\u{7f}data",
        ] {
            assert!(!is_species(bad), "{bad}");
        }
        assert!(is_genus("Tilia") && !is_genus("T") && !is_genus("TIlia"));
        assert!(is_genus(&format!("A{}", "a".repeat(63))));
        assert!(!is_genus(&format!("A{}", "a".repeat(64))));
    }

    #[test]
    fn summary_and_asset_problems() {
        let mut a = tree("osm:node:1");
        a.anchor_mm = [-5, 7, 9];
        let d = doc(vec![a, tree("osm:node:2")]);
        assert!(d.problems().is_empty(), "{:?}", d.problems());
        let tile = TileKey::parse("17/1/2").unwrap();
        let bounds = BoundsMm {
            min: [-5, 2, 3],
            max: [1, 7, 9],
        };
        assert_eq!(d.bounds(), Some(bounds));
        assert_eq!(d.source(), Some("osm:node"));
        let fid = "tile:17/1/2/osm:node";
        assert!(
            d.asset_problems(Layer::Vegetation, fid, tile, &bounds)
                .is_empty()
        );
        assert_eq!(
            d.asset_problems(Layer::StreetObjects, fid, tile, &bounds),
            ["points of kind tree belong in the vegetation layer, not street_objects"]
        );
        assert_eq!(
            d.asset_problems(Layer::Vegetation, "tile:17/1/2/osm:way", tile, &bounds),
            [
                "the points' source must be \"osm:way\", the source of feature id \"tile:17/1/2/osm:way\""
            ]
        );
        let wider = BoundsMm {
            min: [-6, 2, 3],
            ..bounds
        };
        assert_eq!(
            d.asset_problems(Layer::Vegetation, fid, tile, &wider),
            ["bounds_mm are not the exact bounds of the anchors"]
        );
        assert_eq!(
            d.asset_problems(Layer::Shell, fid, tile, &bounds),
            ["shell assets are meshes, not points documents"]
        );
    }
}
