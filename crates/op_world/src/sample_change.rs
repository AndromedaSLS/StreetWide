//! A sample contribution for the gate's golden fixture
//! (`fixtures/changes/facade-sample`) and tests: a facade "skin" 3 cm
//! outside the walls of one building of the ingest fixture
//! (`fixtures/world`). It is a test object, not a real contribution.
//!
//! Its geometry is derived from the swisstopo shell, so it carries the
//! combined licence `CC-BY-SA-4.0 AND LicenseRef-swisstopo-OGD` and both
//! credits: exactly what RFC 0002 G4 and G5 ask of such a contribution.
//!
//! [`sample_points_draft`] makes a baseline refresh with a points asset
//! (RFC 0004), for the gate's tests of the point layers.

use std::collections::BTreeMap;
use std::path::Path;

use crate::contrib::{CONTRIBUTION_CONTRACT_VERSION, Change, ChangeSet, INGEST_AUTHOR, Op};
use crate::gate::{COLLECTIVE_CREDIT, CONTRIBUTION_LICENSE, CONTRIBUTIONS_DATASET};
use crate::glb::{self, GlbMesh, Material, Node, Primitive, f32_to_mm, mm_to_f32};
use crate::id::ContentId;
use crate::keys::TileKey;
use crate::limits::Document;
use crate::manifest::{AssetManifest, Collider, Generator, Layer, Physics, Provenance, Source};
use crate::points::{
    Attributes, LightAttributes, Point, PointKind, PointsDocument, TreeAttributes,
};
use crate::store::{ObjectKind, Store};
use crate::tile::TileIndex;
use crate::{CONTRACT_VERSION, Error, Result};

pub const SAMPLE_AUTHOR: &str = "streetwide:fixtures";
/// How far the skin sits outside the walls (mm).
const OFFSET_MM: f32 = 30.0;
const MIN_WALL_TRIANGLES: usize = 20;

/// A change set with its one new asset, before it is written.
pub struct Draft {
    pub change: ChangeSet,
    pub manifest: AssetManifest,
    pub mesh: GlbMesh,
}

/// The sample facade for the first swissBUILDINGS3D shell (in tile and
/// feature order) that has at least 20 wall triangles.
pub fn sample_draft(world_dir: &Path) -> Result<Draft> {
    let world = Store::new(world_dir);
    let (world_id, manifest) = world.read_world()?;
    for wt in &manifest.tiles {
        let index = TileIndex::from_bytes(&world.read_document(&wt.tile_id, Document::TileIndex)?)?;
        for a in index
            .assets
            .iter()
            .filter(|a| a.layer == Layer::Shell && a.feature_id.starts_with("swisstopo:"))
        {
            let shell = glb::read(&world.read_object(&a.content_id, ObjectKind::Glb)?)?;
            let Some(walls) = shell.nodes.iter().flat_map(|n| &n.primitives).find(|p| {
                p.material == Material::Wall && p.indices.len() / 3 >= MIN_WALL_TRIANGLES
            }) else {
                continue;
            };
            let shell_manifest = AssetManifest::from_bytes(
                &world.read_document(&a.manifest_id, Document::AssetManifest)?,
            )?;
            return draft(world_id, wt.tile, &a.feature_id, walls, &shell_manifest);
        }
    }
    Err(Error::Invalid(
        "no building with enough walls in the world".into(),
    ))
}

fn draft(
    world_id: ContentId,
    tile: TileKey,
    feature_id: &str,
    walls: &Primitive,
    shell: &AssetManifest,
) -> Result<Draft> {
    let mut positions = Vec::with_capacity(walls.positions.len());
    for (p, n) in walls.positions.iter().zip(&walls.normals) {
        let mut v = [0.0f32; 3];
        for k in 0..3 {
            let mm = f32_to_mm(p[k])
                .ok_or_else(|| Error::Invalid("shell position not mm-snapped".into()))?;
            v[k] = mm_to_f32(mm + (n[k] * OFFSET_MM).round() as i64);
        }
        positions.push(v);
    }
    let mesh = GlbMesh {
        nodes: vec![Node {
            name: feature_id.to_string(),
            primitives: vec![Primitive {
                material: Material::Wall,
                positions,
                normals: walls.normals.clone(),
                indices: walls.indices.clone(),
            }],
        }],
    };
    let mut sources = vec![Source {
        dataset: CONTRIBUTIONS_DATASET.into(),
        version: CONTRIBUTION_CONTRACT_VERSION.into(),
        license: CONTRIBUTION_LICENSE.into(),
        attribution: COLLECTIVE_CREDIT.into(),
        url: "https://github.com/AndromedaSLS/StreetWide".into(),
        source_ids: vec![SAMPLE_AUTHOR.into()],
    }];
    sources.extend(shell.provenance.sources.iter().cloned());
    let mut attribution: Vec<String> = sources.iter().map(|s| s.attribution.clone()).collect();
    attribution.sort();
    attribution.dedup();
    let mut licenses: Vec<&str> = sources.iter().map(|s| s.license.as_str()).collect();
    licenses.sort();
    licenses.dedup();
    let manifest = AssetManifest {
        format: AssetManifest::FORMAT.into(),
        contract_version: CONTRACT_VERSION.into(),
        content_id: ContentId::of(&[]),
        size_bytes: 1,
        layer: Layer::Facade,
        feature_id: feature_id.to_string(),
        tile,
        bounds_mm: shell.bounds_mm,
        physics: Physics {
            collider: Collider::None,
        },
        fictional: false,
        parent_version_id: None,
        provenance: Provenance {
            license: licenses.join(" AND "),
            attribution,
            sources,
            generator: Generator {
                name: "op_world sample change".into(),
                version: CONTRIBUTION_CONTRACT_VERSION.into(),
            },
        },
    };
    let change = ChangeSet {
        format: ChangeSet::FORMAT.into(),
        contract_version: CONTRIBUTION_CONTRACT_VERSION.into(),
        base_world_id: world_id,
        title: "Sample facade (test fixture)".into(),
        author: SAMPLE_AUTHOR.into(),
        rationale: "A facade skin 3 cm outside the walls of one building, used to test the contribution gate. Not a real contribution.".into(),
        changes: vec![Change {
            op: Op::Add,
            tile,
            layer: Layer::Facade,
            feature_id: feature_id.to_string(),
            parent_version_id: None,
            manifest_id: None,
        }],
    };
    Ok(Draft {
        change,
        manifest,
        mesh,
    })
}

/// Writes the draft as a bundle (`change.json` and `objects/`) into
/// `bundle_dir`: the mesh, then the manifest (with its content id, size and
/// bounds taken from the mesh), then the change set, whose changes that
/// carry a manifest point to it. Returns the change id.
pub fn write_bundle(draft: &mut Draft, bundle_dir: &Path) -> Result<ContentId> {
    let bundle = Store::new(bundle_dir);
    let bytes = glb::write(&draft.mesh)?;
    draft.manifest.content_id = bundle.write_object(&bytes, ObjectKind::Glb)?;
    draft.manifest.size_bytes = bytes.len() as u64;
    draft.manifest.bounds_mm = draft
        .mesh
        .bounds_mm()
        .ok_or_else(|| Error::Invalid("empty mesh".into()))?;
    let manifest_id = bundle.write_object(&draft.manifest.to_bytes()?, ObjectKind::Json)?;
    for c in &mut draft.change.changes {
        if c.op != Op::Remove {
            c.manifest_id = Some(manifest_id);
        }
    }
    let change = draft.change.to_bytes()?;
    std::fs::write(bundle_dir.join("change.json"), &change)?;
    Ok(ContentId::of(&change))
}

/// A baseline refresh that adds one points asset (RFC 0004) to the first
/// tile of a world, before it is written: for tests of the gate's rules
/// for the point layers. Not committed as a fixture.
pub struct PointsDraft {
    pub change: ChangeSet,
    pub manifest: AssetManifest,
    pub doc: PointsDocument,
}

/// How many points [`sample_points_draft`] makes.
pub const SAMPLE_POINTS: usize = 3;

/// A refresh by `streetwide:ingest` adding a points asset in `layer` (trees
/// in `vegetation`, light points in `street_objects`) to the first tile of
/// the world in `world_dir`, with [`SAMPLE_POINTS`] points anchored exactly
/// on vertices of that tile's terrain: so every anchor lies on the ground
/// (G8). The points are fictional: source `streetwide:sample-trees` or
/// `streetwide:sample-lights`. The first light has an estimated yaw.
pub fn sample_points_draft(world_dir: &Path, layer: Layer) -> Result<PointsDraft> {
    let world = Store::new(world_dir);
    let (world_id, manifest) = world.read_world()?;
    let wt = manifest
        .tiles
        .first()
        .ok_or_else(|| Error::Invalid("the world has no tile".into()))?;
    let index = TileIndex::from_bytes(&world.read_document(&wt.tile_id, Document::TileIndex)?)?;
    let terrain = index
        .assets
        .iter()
        .find(|a| a.layer == Layer::Terrain)
        .ok_or_else(|| Error::Invalid(format!("tile {} has no terrain", wt.tile)))?;
    let mesh = glb::read(&world.read_object(&terrain.content_id, ObjectKind::Glb)?)?;
    let mut vertices: Vec<[i64; 3]> = mesh
        .nodes
        .iter()
        .flat_map(|n| &n.primitives)
        .flat_map(|p| &p.positions)
        .map(|v| v.map(|c| f32_to_mm(c).unwrap_or_default()))
        .collect();
    vertices.sort_unstable();
    vertices.dedup();
    let anchors: Vec<[i64; 3]> = (1..=SAMPLE_POINTS)
        .map(|k| vertices[k * vertices.len() / (SAMPLE_POINTS + 1)])
        .collect();
    let (kind, source) = match layer {
        Layer::Vegetation => (PointKind::Tree, "streetwide:sample-trees"),
        Layer::StreetObjects => (PointKind::Light, "streetwide:sample-lights"),
        other => {
            return Err(Error::Invalid(format!(
                "the {} layer holds no points",
                other.as_str()
            )));
        }
    };
    let points: Vec<Point> = anchors
        .iter()
        .enumerate()
        .map(|(k, &anchor_mm)| {
            let (attributes, estimated) = match kind {
                PointKind::Tree => (
                    Attributes::Tree(TreeAttributes {
                        genus: Some("Tilia".into()),
                        species: Some("Tilia cordata".into()),
                        crown_diameter_mm: Some(6_000 + 1_000 * k as i64),
                        ..TreeAttributes::default()
                    }),
                    vec![],
                ),
                PointKind::Light => (
                    Attributes::Light(LightAttributes {
                        yaw: (k == 0).then_some(16_384),
                        ..LightAttributes::default()
                    }),
                    if k == 0 { vec!["yaw".into()] } else { vec![] },
                ),
            };
            Point {
                id: format!("{source}:P-{k}"),
                kind,
                anchor_mm,
                attributes,
                estimated,
            }
        })
        .collect();
    let mut rules = BTreeMap::new();
    if kind == PointKind::Light {
        rules.insert(
            "light".to_string(),
            BTreeMap::from([(
                "yaw".to_string(),
                "adr-0015:yaw-from-orientierung:1".to_string(),
            )]),
        );
    }
    let doc = PointsDocument {
        format: PointsDocument::FORMAT.into(),
        contract_version: CONTRACT_VERSION.into(),
        points,
        rules,
    };
    let feature_id = format!("tile:{}/{source}", wt.tile);
    let manifest = AssetManifest {
        format: AssetManifest::FORMAT.into(),
        contract_version: CONTRACT_VERSION.into(),
        content_id: ContentId::of(&[]),
        size_bytes: 1,
        layer,
        feature_id: feature_id.clone(),
        tile: wt.tile,
        bounds_mm: doc
            .bounds()
            .ok_or_else(|| Error::Invalid("no points".into()))?,
        physics: Physics {
            collider: Collider::None,
        },
        fictional: true,
        parent_version_id: None,
        provenance: Provenance {
            license: CONTRIBUTION_LICENSE.into(),
            attribution: vec![COLLECTIVE_CREDIT.into()],
            sources: vec![Source {
                dataset: "StreetWide sample points".into(),
                version: CONTRACT_VERSION.into(),
                license: CONTRIBUTION_LICENSE.into(),
                attribution: COLLECTIVE_CREDIT.into(),
                url: "https://github.com/AndromedaSLS/StreetWide".into(),
                source_ids: doc.points.iter().map(|p| p.id.clone()).collect(),
            }],
            generator: Generator {
                name: "op_world sample points".into(),
                version: CONTRACT_VERSION.into(),
            },
        },
    };
    let change = ChangeSet {
        format: ChangeSet::FORMAT.into(),
        contract_version: CONTRIBUTION_CONTRACT_VERSION.into(),
        base_world_id: world_id,
        title: "Sample points (test)".into(),
        author: INGEST_AUTHOR.into(),
        rationale: "Fictional points on the terrain's vertices, to test the gate's rules for the point layers. Not real data.".into(),
        changes: vec![Change {
            op: Op::Add,
            tile: wt.tile,
            layer,
            feature_id,
            parent_version_id: None,
            manifest_id: None,
        }],
    };
    Ok(PointsDraft {
        change,
        manifest,
        doc,
    })
}

/// Writes a points draft as a bundle into `bundle_dir`: the points document
/// (written canonically, whatever rule it breaks, so tests can break them),
/// then the manifest with the document's content id and size (its bounds
/// and feature id are left as the draft has them), then the change set,
/// whose changes that carry a manifest point to it. Returns the change id.
pub fn write_points_bundle(draft: &mut PointsDraft, bundle_dir: &Path) -> Result<ContentId> {
    let bundle = Store::new(bundle_dir);
    let bytes = crate::cjson::to_canonical(&draft.doc)?;
    draft.manifest.content_id = bundle.write_object(&bytes, ObjectKind::Json)?;
    draft.manifest.size_bytes = bytes.len() as u64;
    let manifest_id = bundle.write_object(&draft.manifest.to_bytes()?, ObjectKind::Json)?;
    for c in &mut draft.change.changes {
        if c.op != Op::Remove {
            c.manifest_id = Some(manifest_id);
        }
    }
    let change = draft.change.to_bytes()?;
    std::fs::write(bundle_dir.join("change.json"), &change)?;
    Ok(ContentId::of(&change))
}
