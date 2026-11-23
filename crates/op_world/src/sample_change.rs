//! A sample contribution for the gate's golden fixture
//! (`fixtures/changes/facade-sample`) and tests: a facade "skin" 3 cm
//! outside the walls of one building of the ingest fixture
//! (`fixtures/world`). It is a test object, not a real contribution.
//!
//! Its geometry is derived from the swisstopo shell, so it carries the
//! combined licence `CC-BY-SA-4.0 AND LicenseRef-swisstopo-OGD` and both
//! credits: exactly what RFC 0002 G4 and G5 ask of such a contribution.

use std::path::Path;

use crate::contrib::{CONTRIBUTION_CONTRACT_VERSION, Change, ChangeSet, Op};
use crate::gate::{COLLECTIVE_CREDIT, CONTRIBUTION_LICENSE, CONTRIBUTIONS_DATASET};
use crate::glb::{self, GlbMesh, Material, Node, Primitive, f32_to_mm, mm_to_f32};
use crate::id::ContentId;
use crate::keys::TileKey;
use crate::manifest::{AssetManifest, Collider, Generator, Layer, Physics, Provenance, Source};
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
        let index = TileIndex::from_bytes(&world.read_object(&wt.tile_id, ObjectKind::Json)?)?;
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
            let shell_manifest =
                AssetManifest::from_bytes(&world.read_object(&a.manifest_id, ObjectKind::Json)?)?;
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
