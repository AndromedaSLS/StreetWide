//! Point assets (RFC 0004) on the world-format golden fixture
//! (`fixtures/format`), whose tile holds a vegetation and a street_objects
//! asset: opening, the simulation's colliders and the streamer take them
//! without drawing anything.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use op_kernel::{BoundsMm, Fetch, GeometrySource};
use op_stream::{StoreSource, StreamSource, Streamer, WorldIndex};
use op_world::points::PointKind;

fn format_world() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/format/world")
}

#[test]
fn point_assets_are_never_colliders() {
    let index = WorldIndex::open(&format_world()).expect("the fixture opens");
    let layers: Vec<&str> = index.tiles()[0]
        .assets
        .iter()
        .map(|a| a.layer.as_str())
        .collect();
    assert_eq!(layers, ["shell", "street_objects", "terrain", "vegetation"]);
    let colliders: Vec<String> = index
        .colliders([0, 0, 0])
        .into_iter()
        .map(|c| c.asset.key.layer)
        .collect();
    assert_eq!(colliders, ["shell", "terrain"]);

    // The simulation's source serves the same colliders, and its meshes.
    let mut source = StoreSource::open(&format_world(), [0, 0, 0]).unwrap();
    let all = BoundsMm::around([0, 0, 0], [1 << 40; 3]);
    let Ok(Fetch::Ready(list)) = source.colliders_in(&all) else {
        panic!("ready")
    };
    assert_eq!(list.len(), 2);
    for asset in &list {
        assert!(matches!(source.mesh(asset), Ok(Fetch::Ready(_))));
    }
}

#[test]
fn points_documents_load_and_are_not_meshes() {
    let index = WorldIndex::open(&format_world()).unwrap();
    let assets = &index.tiles()[0].assets;
    for a in assets.iter().filter(|a| a.layer.has_points()) {
        let doc = index.load_points(&a.content_id).expect("a points document");
        assert_eq!(doc.points.len(), 2);
        let kind = if a.layer.as_str() == "vegetation" {
            PointKind::Tree
        } else {
            PointKind::Light
        };
        assert!(doc.points.iter().all(|p| p.kind == kind));
        assert_eq!(doc.bounds(), Some(a.bounds_mm));
        let err = index.load_mesh(&a.content_id).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "content {} is a points document (RFC 0004), not a mesh",
                a.content_id
            )
        );
    }
    // And a mesh is not a points document.
    let shell = assets.iter().find(|a| a.layer.as_str() == "shell").unwrap();
    assert!(index.load_mesh(&shell.content_id).is_ok());
    assert!(index.load_points(&shell.content_id).is_err());
}

/// The client asks the streamer for every asset near the camera: a points
/// asset fails to load as a mesh, once, and the streamer keeps serving the
/// rest.
#[test]
fn the_streamer_carries_point_assets_without_drawing_them() {
    let streamer = Arc::new(Streamer::open(&format_world(), 2, 1 << 20).unwrap());
    let assets = streamer.index().tiles()[0].assets.clone();
    for a in &assets {
        streamer.request(&a.content_id);
    }
    // No wall clock (clippy.toml): at most 12,000 waits of 5 ms.
    for _ in 0..12_000 {
        if streamer.in_flight() == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(streamer.in_flight(), 0, "loads finish");
    for a in &assets {
        if a.layer.has_points() {
            assert!(streamer.get(&a.content_id).is_none());
            let error = streamer.error(&a.content_id).expect("a failed load");
            assert!(error.contains("is a points document"), "{error}");
        } else {
            assert!(
                streamer.get(&a.content_id).is_some(),
                "{}",
                a.layer.as_str()
            );
        }
    }
    // The simulation's view of the streamer has no point assets at all.
    let mut sim = StreamSource::new(Arc::clone(&streamer), [0, 0, 0]);
    let all = BoundsMm::around([0, 0, 0], [1 << 40; 3]);
    let Ok(Fetch::Ready(list)) = sim.colliders_in(&all) else {
        panic!("ready")
    };
    assert!(
        list.iter()
            .all(|c| c.key.layer != "street_objects" && c.key.layer != "vegetation")
    );
}
