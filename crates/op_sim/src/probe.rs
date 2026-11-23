//! Static collision probes for the contribution gate's walkability check
//! (RFC 0002 G9, ADR-0009): could the player stand at a point?
//!
//! A point is **blocked** if the player capsule (the same shape the
//! simulation moves, [`crate::player`]) standing there overlaps a collider,
//! or if it is **enclosed**: rays from the capsule's centre towards the four
//! horizontal directions and straight up all hit something within
//! [`ENCLOSED_RANGE_M`]. The second test catches a capsule that sits
//! entirely inside a closed volume without touching any of its surfaces.
//! The gate compares the base and the candidate world, so streets that are
//! already enclosed (an arcade, a covered passage) never count against a
//! change.

use std::sync::Arc;

use op_kernel::geometry::mm_to_m;
use op_kernel::{ColliderMesh, GeometryError};
use rapier3d::prelude::*;

use crate::player::{CENTRE_ABOVE_FEET, HALF_HEIGHT, RADIUS};

/// Range of the enclosure rays (m).
pub const ENCLOSED_RANGE_M: f32 = 30.0;

/// Static colliders to probe against.
pub struct ProbeWorld {
    world: PhysicsWorld,
    shape: SharedShape,
}

impl ProbeWorld {
    /// Colliders from meshes in a sim frame (integer millimetres).
    pub fn new(meshes: &[Arc<ColliderMesh>]) -> Result<Self, GeometryError> {
        let mut world = PhysicsWorld::new();
        for m in meshes {
            let vertices = m
                .vertices_mm
                .iter()
                .map(|v| Vector::new(mm_to_m(v[0]), mm_to_m(v[1]), mm_to_m(v[2])))
                .collect();
            let collider = ColliderBuilder::trimesh(vertices, m.triangles.clone())
                .map_err(|e| GeometryError(format!("probe collider: {e:?}")))?
                .build();
            world.insert_collider(collider, None);
        }
        world.detect_collisions(&(), &());
        Ok(ProbeWorld {
            world,
            shape: SharedShape::capsule_y(HALF_HEIGHT, RADIUS),
        })
    }

    /// Whether the player could not stand with its feet at `feet_mm`
    /// (sim frame).
    pub fn blocked(&self, feet_mm: [i64; 3]) -> bool {
        let [x, y, z] = feet_mm.map(mm_to_m);
        let centre = Vector::new(x, y + CENTRE_ABOVE_FEET, z);
        let queries = self.world.query_pipeline();
        if queries
            .intersect_shape(Pose::from_translation(centre), self.shape.as_ref())
            .next()
            .is_some()
        {
            return true;
        }
        [Vector::X, -Vector::X, Vector::Z, -Vector::Z, Vector::Y]
            .into_iter()
            .all(|dir| {
                queries
                    .cast_ray(&Ray::new(centre, dir), ENCLOSED_RANGE_M, true)
                    .is_some()
            })
    }
}
