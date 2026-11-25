//! Whole-world validation: canonical form, hashes, references, bounds and
//! licence rules across the world manifest, tile indexes, manifests and
//! their content, meshes and points documents (RFC 0001, RFC 0004).

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::glb;
use crate::id::ContentId;
use crate::limits::Document;
use crate::manifest::{AssetManifest, BoundsMm, ContentKind, Layer};
use crate::points::{PointsDocument, Summary};
use crate::store::{ObjectKind, Store};
use crate::tile::TileIndex;

/// Result of validating a world.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub world_id: Option<ContentId>,
    pub tiles: usize,
    pub assets: usize,
    pub triangles: usize,
    /// Points in the world's points documents (RFC 0004), counted once per
    /// listing.
    pub points: usize,
    pub errors: Vec<String>,
}

impl Report {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty() && self.world_id.is_some()
    }
}

/// What reading and checking one object gave: the parsed object or the
/// error's message.
type Outcome<T> = Rc<std::result::Result<T, String>>;

/// A mesh's length, and its triangle count and bounds (or why it is not a
/// valid glb).
type MeshFacts = (
    usize,
    std::result::Result<(usize, Option<BoundsMm>), String>,
);

/// A points document's length, and its summary and point ids (or why it is
/// not a valid points document).
type PointsFacts = (usize, std::result::Result<(Summary, Vec<String>), String>);

/// Outcomes of objects listed more than once. A world may list one object
/// many times (validly a shared mesh, or a crafted world listing a 64 MiB
/// mesh thousands of times), and each listing gets its own errors, but the
/// object is read and checked at most twice. Objects listed once, as every
/// manifest in a valid world is, are not kept.
struct Seen<T> {
    once: BTreeSet<ContentId>,
    kept: BTreeMap<ContentId, Outcome<T>>,
}

impl<T> Seen<T> {
    fn new() -> Self {
        Seen {
            once: BTreeSet::new(),
            kept: BTreeMap::new(),
        }
    }

    /// The outcome for `id`, and whether this is its first listing.
    fn get(
        &mut self,
        id: ContentId,
        check: impl FnOnce() -> std::result::Result<T, String>,
    ) -> (Outcome<T>, bool) {
        if let Some(kept) = self.kept.get(&id) {
            return (Rc::clone(kept), false);
        }
        let outcome = Rc::new(check());
        let first = self.once.insert(id);
        if !first {
            self.kept.insert(id, Rc::clone(&outcome));
        }
        (outcome, first)
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
    let mut tiles: Seen<TileIndex> = Seen::new();
    let mut manifests: Seen<AssetManifest> = Seen::new();
    let mut meshes: Seen<MeshFacts> = Seen::new();
    let mut documents: Seen<PointsFacts> = Seen::new();
    // Point ids are unique per layer across the world (RFC 0004 section 2).
    // A document listed twice in one layer repeats all of its ids: that is
    // reported once per listing, without going through its points again.
    let mut point_ids: BTreeSet<(Layer, String)> = BTreeSet::new();
    let mut listed_documents: BTreeSet<(Layer, ContentId)> = BTreeSet::new();

    for wt in &world.tiles {
        let ctx = format!("tile {}", wt.tile);
        let (index, _) = tiles.get(wt.tile_id, || {
            store
                .read_document(&wt.tile_id, Document::TileIndex)
                .and_then(|b| TileIndex::from_bytes(&b))
                .map_err(|e| e.to_string())
        });
        let index = match &*index {
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
            let (manifest, first) = manifests.get(asset.manifest_id, || {
                store
                    .read_document(&asset.manifest_id, Document::AssetManifest)
                    .and_then(|b| AssetManifest::from_bytes(&b))
                    .map_err(|e| e.to_string())
            });
            let manifest = match &*manifest {
                Ok(m) => m,
                Err(e) => {
                    report.errors.push(format!("{actx}: manifest: {e}"));
                    continue;
                }
            };
            report.assets += 1;
            if first {
                attribution.extend(manifest.provenance.attribution.iter().cloned());
                // Licences and credits (ADR-0017), once per manifest.
                for problem in manifest.provenance.licence_problems() {
                    report.errors.push(format!("{actx}: {problem}"));
                }
            }

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

            if manifest.layer.content() == ContentKind::Points {
                let (facts, _) = documents.get(manifest.content_id, || {
                    let bytes = store
                        .read_document(&manifest.content_id, Document::Points)
                        .map_err(|e| e.to_string())?;
                    let facts = PointsDocument::from_bytes(&bytes)
                        .map(|doc| {
                            let summary = doc.summary();
                            (summary, doc.points.into_iter().map(|p| p.id).collect())
                        })
                        .map_err(|e| e.to_string());
                    Ok((bytes.len(), facts))
                });
                let (len, facts) = match &*facts {
                    Ok(f) => f,
                    Err(e) => {
                        report.errors.push(format!("{actx}: points document: {e}"));
                        continue;
                    }
                };
                if *len as u64 != manifest.size_bytes {
                    report.errors.push(format!(
                        "{actx}: size_bytes does not match the points document"
                    ));
                }
                let (summary, ids) = match facts {
                    Ok(f) => f,
                    Err(e) => {
                        report.errors.push(format!("{actx}: {e}"));
                        continue;
                    }
                };
                report.points += summary.count;
                for problem in summary.asset_problems(
                    manifest.layer,
                    &manifest.feature_id,
                    manifest.tile,
                    &manifest.bounds_mm,
                ) {
                    report.errors.push(format!("{actx}: {problem}"));
                }
                let layer = manifest.layer;
                if !listed_documents.insert((layer, manifest.content_id)) {
                    report.errors.push(format!(
                        "{actx}: another {} asset lists the same points; point ids are unique per layer",
                        layer.as_str()
                    ));
                    continue;
                }
                let mut repeated = ids
                    .iter()
                    .filter(|id| !point_ids.insert((layer, (*id).clone())));
                if let Some(first) = repeated.next() {
                    let more = repeated.count();
                    let others = if more > 0 {
                        format!(" and {more} more")
                    } else {
                        String::new()
                    };
                    report.errors.push(format!(
                        "{actx}: point {first:?}{others} already listed in the {} layer; point ids are unique per layer",
                        layer.as_str()
                    ));
                }
                continue;
            }
            let (mesh, _) = meshes.get(manifest.content_id, || {
                let bytes = store
                    .read_object(&manifest.content_id, ObjectKind::Glb)
                    .map_err(|e| e.to_string())?;
                let facts = glb::read(&bytes)
                    .map(|mesh| (mesh.triangle_count(), mesh.bounds_mm()))
                    .map_err(|e| e.to_string());
                Ok((bytes.len(), facts))
            });
            let (len, facts) = match &*mesh {
                Ok(m) => m,
                Err(e) => {
                    report.errors.push(format!("{actx}: mesh: {e}"));
                    continue;
                }
            };
            if *len as u64 != manifest.size_bytes {
                report
                    .errors
                    .push(format!("{actx}: size_bytes does not match the mesh"));
            }
            match facts {
                Ok((triangles, bounds)) => {
                    report.triangles += triangles;
                    if *bounds != Some(manifest.bounds_mm) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn objects_listed_many_times_are_checked_at_most_twice() {
        let mut seen: Seen<u32> = Seen::new();
        let (a, b) = (ContentId::of(b"a"), ContentId::of(b"b"));
        let mut checks = 0;
        let mut firsts = Vec::new();
        for id in [a, b, a, a, a, b, a] {
            let (outcome, first) = seen.get(id, || {
                checks += 1;
                Ok(7)
            });
            assert_eq!(*outcome, Ok(7));
            firsts.push(first);
        }
        assert_eq!(checks, 4, "a and b, twice each");
        assert_eq!(firsts, [true, true, false, false, false, false, false]);
        // Only the objects listed more than once are kept.
        let mut once: Seen<u32> = Seen::new();
        for k in 0..100u8 {
            once.get(ContentId::of(&[k]), || Ok(1));
        }
        assert!(once.kept.is_empty());
    }
}
