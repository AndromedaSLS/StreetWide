//! Whole-world validation: canonical form, hashes, references, bounds and
//! licence rules across the world manifest, tile indexes, manifests and
//! meshes (RFC 0001).

use std::collections::BTreeSet;

use crate::glb;
use crate::id::ContentId;
use crate::manifest::{AssetManifest, Layer};
use crate::store::{ObjectKind, Store};
use crate::tile::TileIndex;

/// Result of validating a world.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub world_id: Option<ContentId>,
    pub tiles: usize,
    pub assets: usize,
    pub triangles: usize,
    pub errors: Vec<String>,
}

impl Report {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty() && self.world_id.is_some()
    }
}

/// Validates every object reachable from `world.json`. Collects all errors
/// rather than stopping at the first.
pub fn validate_store(store: &Store) -> Report {
    let mut report = Report::default();
    let (world_id, world) = match store.read_world() {
        Ok(w) => w,
        Err(e) => {
            report.errors.push(format!("world.json: {e}"));
            return report;
        }
    };
    report.world_id = Some(world_id);

    let mut attribution: BTreeSet<String> = BTreeSet::new();
    let mut features: BTreeSet<(Layer, String)> = BTreeSet::new();

    for wt in &world.tiles {
        let ctx = format!("tile {}", wt.tile);
        let index = match store
            .read_object(&wt.tile_id, ObjectKind::Json)
            .and_then(|b| TileIndex::from_bytes(&b))
        {
            Ok(i) => i,
            Err(e) => {
                report.errors.push(format!("{ctx}: {e}"));
                continue;
            }
        };
        report.tiles += 1;
        if index.tile != wt.tile {
            report
                .errors
                .push(format!("{ctx}: index describes tile {}", index.tile));
        }
        if index.origin_mm != wt.origin_mm {
            report
                .errors
                .push(format!("{ctx}: origin_mm differs from the world manifest"));
        }
        if index.bounds() != Some(wt.bounds_mm) {
            report.errors.push(format!(
                "{ctx}: bounds_mm is not the union of its asset bounds"
            ));
        }

        for asset in &index.assets {
            let actx = format!("{ctx}, {} {}", asset.layer.as_str(), asset.feature_id);
            if !features.insert((asset.layer, asset.feature_id.clone())) {
                report
                    .errors
                    .push(format!("{actx}: listed more than once in the world"));
            }
            let manifest = match store
                .read_object(&asset.manifest_id, ObjectKind::Json)
                .and_then(|b| AssetManifest::from_bytes(&b))
            {
                Ok(m) => m,
                Err(e) => {
                    report.errors.push(format!("{actx}: manifest: {e}"));
                    continue;
                }
            };
            report.assets += 1;
            attribution.extend(manifest.provenance.attribution.iter().cloned());

            let mismatch = [
                ("tile", manifest.tile != index.tile),
                ("layer", manifest.layer != asset.layer),
                ("feature_id", manifest.feature_id != asset.feature_id),
                ("content_id", manifest.content_id != asset.content_id),
                ("license", manifest.provenance.license != asset.license),
                ("collider", manifest.physics.collider != asset.collider),
                ("bounds_mm", manifest.bounds_mm != asset.bounds_mm),
            ];
            for (field, differs) in mismatch {
                if differs {
                    report.errors.push(format!(
                        "{actx}: {field} in the tile index differs from the manifest"
                    ));
                }
            }

            let bytes = match store.read_object(&manifest.content_id, ObjectKind::Glb) {
                Ok(b) => b,
                Err(e) => {
                    report.errors.push(format!("{actx}: mesh: {e}"));
                    continue;
                }
            };
            if bytes.len() as u64 != manifest.size_bytes {
                report
                    .errors
                    .push(format!("{actx}: size_bytes does not match the mesh"));
            }
            match glb::read(&bytes) {
                Ok(mesh) => {
                    report.triangles += mesh.triangle_count();
                    if mesh.bounds_mm() != Some(manifest.bounds_mm) {
                        report
                            .errors
                            .push(format!("{actx}: bounds_mm does not match the mesh"));
                    }
                }
                Err(e) => report.errors.push(format!("{actx}: {e}")),
            }
        }
    }

    let expected: Vec<String> = attribution.into_iter().collect();
    if world.attribution != expected {
        report.errors.push(format!(
            "world attribution {:?} must be the sorted union of asset attributions {:?}",
            world.attribution, expected
        ));
    }
    report
}
