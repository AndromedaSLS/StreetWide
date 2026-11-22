//! A tiny hand-made world used as the world-format golden fixture
//! (`fixtures/format/`). It exercises every document type and is small
//! enough to read by eye. All of its content is fictional.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::glb::{self, GlbMesh, Material, Node, Primitive, mm_to_f32};
use crate::id::ContentId;
use crate::keys::TileKey;
use crate::manifest::{AssetManifest, Collider, Generator, Layer, Physics, Provenance, Source};
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
/// `meshes` must contain `house` (a shell) and `ground` (terrain).
pub fn build_sample_world(store: &Store, meshes: &BTreeMap<String, MeshSpec>) -> Result<ContentId> {
    let tile = TileKey::parse(SAMPLE_TILE)?;
    // Tile centre relative to the region anchor (rounded to the millimetre).
    let origin_mm = [-1_967_230, 0, 931_920];

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
