//! A tiny hand-made world used as the world-format golden fixture
//! (`fixtures/format/`). It exercises every document type and is small
//! enough to read by eye: a house, a ground plate, and two trees and two
//! light points as points documents (RFC 0004). All of its content is
//! fictional.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::glb::{self, GlbMesh, Material, Node, Primitive, mm_to_f32};
use crate::id::ContentId;
use crate::keys::TileKey;
use crate::manifest::{AssetManifest, Collider, Generator, Layer, Physics, Provenance, Source};
use crate::points::PointsDocument;
use crate::store::{ObjectKind, Store};
use crate::tile::{TileAsset, TileIndex};
use crate::world::{AnchorE9, Region, Spawn, WorldManifest, WorldTile};
use crate::{CONTRACT_VERSION, Error, Result};

/// A mesh described with integer millimetre positions and integer normals,
/// so Rust and Python build bit-identical meshes from it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeshSpec {
    pub nodes: Vec<NodeSpec>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeSpec {
    pub name: String,
    pub primitives: Vec<PrimitiveSpec>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrimitiveSpec {
    pub material: String,
    pub positions_mm: Vec<[i64; 3]>,
    pub normals: Vec<[i32; 3]>,
    pub indices: Vec<u32>,
}

impl MeshSpec {
    pub fn to_mesh(&self) -> Result<GlbMesh> {
        let nodes = self
            .nodes
            .iter()
            .map(|n| {
                let primitives = n
                    .primitives
                    .iter()
                    .map(|p| {
                        Ok(Primitive {
                            material: Material::from_name(&p.material).ok_or_else(|| {
                                Error::Invalid(format!("unknown material {:?}", p.material))
                            })?,
                            positions: p.positions_mm.iter().map(|v| v.map(mm_to_f32)).collect(),
                            normals: p.normals.iter().map(|v| v.map(|c| c as f32)).collect(),
                            indices: p.indices.clone(),
                        })
                    })
                    .collect::<Result<_>>()?;
                Ok(Node {
                    name: n.name.clone(),
                    primitives,
                })
            })
            .collect::<Result<_>>()?;
        Ok(GlbMesh { nodes })
    }
}

/// Parses `fixtures/format/meshes.json`.
pub fn parse_mesh_specs(bytes: &[u8]) -> Result<BTreeMap<String, MeshSpec>> {
    Ok(serde_json::from_slice(bytes)?)
}

/// Parses `fixtures/format/points.json`: a points document per point layer,
/// written as plain JSON (any key order, any whitespace), and a `comment`.
/// Each document must be valid; its canonical bytes are what the fixture
/// stores.
pub fn parse_points_specs(bytes: &[u8]) -> Result<BTreeMap<Layer, PointsDocument>> {
    let specs: BTreeMap<String, serde_json::Value> = serde_json::from_slice(bytes)?;
    let mut out = BTreeMap::new();
    for (key, value) in specs {
        if key == "comment" {
            continue;
        }
        let layer: Layer = serde_json::from_value(serde_json::Value::String(key.clone()))
            .map_err(|_| Error::Invalid(format!("points.json: {key:?} is not a layer")))?;
        if !layer.has_points() {
            return Err(Error::Invalid(format!(
                "points.json: the {key} layer holds no points"
            )));
        }
        let doc: PointsDocument = serde_json::from_value(value)?;
        doc.ensure_valid()?;
        out.insert(layer, doc);
    }
    Ok(out)
}

pub const SAMPLE_TILE: &str = "17/68646/45908";
const LICENSE: &str = "CC-BY-SA-4.0";
const CREDIT: &str = "The StreetWide contributors";

fn provenance() -> Provenance {
    Provenance {
        license: LICENSE.into(),
        attribution: vec![CREDIT.into()],
        sources: vec![Source {
            dataset: "StreetWide world-format fixtures".into(),
            version: CONTRACT_VERSION.into(),
            license: LICENSE.into(),
            attribution: CREDIT.into(),
            url: "https://github.com/AndromedaSLS/StreetWide".into(),
            source_ids: vec![],
        }],
        generator: Generator {
            name: "op_world sample builder".into(),
            version: CONTRACT_VERSION.into(),
        },
    }
}

/// Writes the sample world into `store` and returns its world id.
///
/// `meshes` must contain `house` (a shell) and `ground` (terrain); `points`
/// holds a points document for each point layer that has an asset
/// (`fixtures/format/points.json`).
pub fn build_sample_world(
    store: &Store,
    meshes: &BTreeMap<String, MeshSpec>,
    points: &BTreeMap<Layer, PointsDocument>,
) -> Result<ContentId> {
    let tile = TileKey::parse(SAMPLE_TILE)?;
    // Tile centre in the region frame at height 0 (RFC 0001 section 1,
    // geodetic -> ECEF -> ENU), rounded to the millimetre, with y = 0.
    // `tools/ingest/tests/test_geo.py` recomputes it.
    let origin_mm = [-1_971_116, 0, 930_405];

    let specs = [
        ("house", Layer::Shell, "streetwide:sample-house".to_string()),
        ("ground", Layer::Terrain, format!("tile:{tile}")),
    ];
    let mut assets = Vec::new();
    for (key, layer, feature_id) in specs {
        let spec = meshes
            .get(key)
            .ok_or_else(|| Error::Invalid(format!("meshes.json has no {key:?}")))?;
        let mesh = spec.to_mesh()?;
        let bytes = glb::write(&mesh)?;
        let content_id = store.write_object(&bytes, ObjectKind::Glb)?;
        let bounds_mm = mesh
            .bounds_mm()
            .ok_or_else(|| Error::Invalid(format!("{key} has no geometry")))?;
        let manifest = AssetManifest {
            format: AssetManifest::FORMAT.into(),
            contract_version: CONTRACT_VERSION.into(),
            content_id,
            size_bytes: bytes.len() as u64,
            layer,
            feature_id: feature_id.clone(),
            tile,
            bounds_mm,
            physics: Physics {
                collider: Collider::Trimesh,
            },
            fictional: true,
            parent_version_id: None,
            provenance: provenance(),
        };
        let manifest_id = store.write_object(&manifest.to_bytes()?, ObjectKind::Json)?;
        assets.push(TileAsset {
            layer,
            feature_id,
            manifest_id,
            content_id,
            license: LICENSE.into(),
            collider: Collider::Trimesh,
            bounds_mm,
        });
    }
    // Points (RFC 0004): one asset per point layer, visual only.
    for (&layer, doc) in points {
        let bytes = doc.to_bytes()?;
        let content_id = store.write_object(&bytes, ObjectKind::Json)?;
        let source = doc
            .source()
            .ok_or_else(|| Error::Invalid(format!("{} points have no source", layer.as_str())))?;
        let feature_id = format!("tile:{tile}/{source}");
        let bounds_mm = doc
            .bounds()
            .ok_or_else(|| Error::Invalid(format!("{} has no points", layer.as_str())))?;
        let manifest = AssetManifest {
            format: AssetManifest::FORMAT.into(),
            contract_version: CONTRACT_VERSION.into(),
            content_id,
            size_bytes: bytes.len() as u64,
            layer,
            feature_id: feature_id.clone(),
            tile,
            bounds_mm,
            physics: Physics {
                collider: Collider::None,
            },
            fictional: true,
            parent_version_id: None,
            provenance: provenance(),
        };
        let problems = doc.asset_problems(layer, &feature_id, tile, &bounds_mm);
        if !problems.is_empty() {
            return Err(Error::Invalid(problems.join("; ")));
        }
        let manifest_id = store.write_object(&manifest.to_bytes()?, ObjectKind::Json)?;
        assets.push(TileAsset {
            layer,
            feature_id,
            manifest_id,
            content_id,
            license: LICENSE.into(),
            collider: Collider::None,
            bounds_mm,
        });
    }
    assets.sort_by(|a, b| (a.layer, &a.feature_id).cmp(&(b.layer, &b.feature_id)));

    let index = TileIndex {
        format: TileIndex::FORMAT.into(),
        contract_version: CONTRACT_VERSION.into(),
        tile,
        region: tile.region(),
        origin_mm,
        assets,
    };
    let bounds_mm = index
        .bounds()
        .ok_or_else(|| Error::Invalid("empty tile".into()))?;
    let tile_id = store.write_object(&index.to_bytes()?, ObjectKind::Json)?;

    let world = WorldManifest {
        format: WorldManifest::FORMAT.into(),
        contract_version: CONTRACT_VERSION.into(),
        name: "format-sample".into(),
        region: Region {
            key: tile.region(),
            // Centre of z12 cell 2145/1434 (Web Mercator inverse).
            anchor_e9: AnchorE9 {
                lat: 47_368_594_345,
                lon: 8_569_335_938,
            },
        },
        attribution: vec![CREDIT.into()],
        spawn: Spawn {
            position_mm: [origin_mm[0], 410_000, origin_mm[2] + 8_000],
            yaw: 0,
        },
        tiles: vec![WorldTile {
            tile,
            tile_id,
            origin_mm,
            bounds_mm,
        }],
    };
    store.write_world(&world)
}
