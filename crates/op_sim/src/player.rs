//! The player: a capsule moved by Rapier's kinematic character controller.
//!
//! Each tick:
//!
//! 1. **prepare**: ask the geometry source which collider assets lie within
//!    [`ACTIVE_RADIUS_MM`] of the player and build the colliders that are
//!    missing. Nothing in the physics world changes yet.
//! 2. **step**: remove colliders that left the area and add the new ones,
//!    both in [`AssetKey`] order; then turn the input into a desired
//!    movement and let the character controller resolve it against the
//!    world (sliding along walls, stepping up kerbs, climbing slopes up to
//!    45 degrees, snapping to the ground when walking downhill).
//!
//! All parameters are constants: changing one changes state hashes and
//! needs a [`crate::SIM_CONTRACT_VERSION`] bump (ADR-0004).

use std::collections::BTreeMap;
use std::f32::consts::FRAC_PI_4;

use op_kernel::geometry::{m_to_mm, mm_to_m};
use op_kernel::input::buttons;
use op_kernel::{
    AssetKey, BoundsMm, ColliderAsset, ColliderMesh, Fetch, GeometryError, GeometrySource, Module,
    PlayerInput, Readiness, StateHasher, StepContext, TICK_DT, dmath,
};
use rapier3d::control::{CharacterAutostep, CharacterLength, KinematicCharacterController};
use rapier3d::prelude::*;

/// Capsule radius (m).
pub const RADIUS: f32 = 0.3;
/// Half the length of the capsule's straight part (m): total height 1.8 m.
pub const HALF_HEIGHT: f32 = 0.6;
/// From the feet to the capsule centre (m).
pub const CENTRE_ABOVE_FEET: f32 = HALF_HEIGHT + RADIUS;
/// Walking and sprinting speed (m/s).
pub const WALK_SPEED: f32 = 1.4;
pub const SPRINT_SPEED: f32 = 4.0;
/// Upward speed at take-off (m/s): about 0.6 m high.
pub const JUMP_SPEED: f32 = 3.5;
pub const GRAVITY: f32 = 9.81;
pub const TERMINAL_SPEED: f32 = 50.0;
/// Kerbs and stairs up to this height are stepped over (m).
pub const STEP_HEIGHT: f32 = 0.3;
pub const STEP_MIN_WIDTH: f32 = 0.2;
/// Steepest walkable slope.
pub const MAX_SLOPE: f32 = FRAC_PI_4;
/// Walking downhill keeps contact with ground up to this far below (m).
pub const SNAP_DISTANCE: f32 = 0.3;
/// Gap the controller keeps to surfaces (m).
pub const SKIN: f32 = 0.02;
/// Colliders whose bounds come within this distance (per axis) of the
/// capsule centre are in the physics world (mm).
pub const ACTIVE_RADIUS_MM: i64 = 160_000;

/// The player's simulated state, in the sim frame.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PlayerState {
    /// Capsule centre (m).
    pub position: [f32; 3],
    /// Vertical speed (m/s), positive up.
    pub vertical_speed: f32,
    pub grounded: bool,
    /// Heading in 1/65536 turns, 0 = north (-Z), clockwise from above.
    pub yaw: u16,
    pub pitch: i16,
}

impl PlayerState {
    pub fn feet(&self) -> [f32; 3] {
        let [x, y, z] = self.position;
        [x, y - CENTRE_ABOVE_FEET, z]
    }
}

/// What the last tick asked for and what it got (m).
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct LastMove {
    pub desired: [f32; 3],
    pub actual: [f32; 3],
}

struct Active {
    content_id: [u8; 32],
    handle: ColliderHandle,
}

/// The player physics module.
pub struct PlayerPhysics {
    world: PhysicsWorld,
    controller: KinematicCharacterController,
    shape: SharedShape,
    active: BTreeMap<AssetKey, Active>,
    /// Colliders built by `prepare`, not yet in the world (a cache).
    built: BTreeMap<AssetKey, ([u8; 32], Collider)>,
    remove: Vec<AssetKey>,
    add: Vec<AssetKey>,
    state: PlayerState,
    last_move: LastMove,
}

impl PlayerPhysics {
    pub fn new(spawn_feet_mm: [i64; 3], yaw: u16) -> Self {
        let [x, y, z] = spawn_feet_mm.map(mm_to_m);
        PlayerPhysics {
            world: PhysicsWorld::new(),
            controller: KinematicCharacterController {
                up: Vector::Y,
                offset: CharacterLength::Absolute(SKIN),
                slide: true,
                autostep: Some(CharacterAutostep {
                    max_height: CharacterLength::Absolute(STEP_HEIGHT),
                    min_width: CharacterLength::Absolute(STEP_MIN_WIDTH),
                    include_dynamic_bodies: false,
                }),
                max_slope_climb_angle: MAX_SLOPE,
                min_slope_slide_angle: MAX_SLOPE,
                snap_to_ground: Some(CharacterLength::Absolute(SNAP_DISTANCE)),
                ..KinematicCharacterController::default()
            },
            shape: SharedShape::capsule_y(HALF_HEIGHT, RADIUS),
            active: BTreeMap::new(),
            built: BTreeMap::new(),
            remove: Vec::new(),
            add: Vec::new(),
            state: PlayerState {
                position: [x, y + CENTRE_ABOVE_FEET, z],
                vertical_speed: 0.0,
                grounded: false,
                yaw,
                pitch: 0,
            },
            last_move: LastMove::default(),
        }
    }

    pub fn state(&self) -> &PlayerState {
        &self.state
    }

    pub fn last_move(&self) -> &LastMove {
        &self.last_move
    }

    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    fn build_collider(
        asset: &ColliderAsset,
        mesh: &ColliderMesh,
    ) -> Result<Collider, GeometryError> {
        let vertices = mesh
            .vertices_mm
            .iter()
            .map(|v| Vector::new(mm_to_m(v[0]), mm_to_m(v[1]), mm_to_m(v[2])))
            .collect();
        ColliderBuilder::trimesh(vertices, mesh.triangles.clone())
            .map(|b| b.build())
            .map_err(|e| GeometryError(format!("{:?}: {e:?}", asset.key)))
    }

    /// Turns the input into this tick's desired translation and updates
    /// the vertical speed.
    fn desired_translation(&mut self, input: &PlayerInput) -> Vector {
        let s = &mut self.state;
        s.yaw = input.yaw;
        s.pitch = input.pitch;

        let mut fwd = f32::from(input.forward) / 127.0;
        let mut side = f32::from(input.strafe) / 127.0;
        let len2 = fwd * fwd + side * side;
        if len2 > 1.0 {
            let len = len2.sqrt();
            fwd /= len;
            side /= len;
        }
        let speed = if input.pressed(buttons::SPRINT) {
            SPRINT_SPEED
        } else {
            WALK_SPEED
        };
        // Heading 0 faces north (-Z); right of it is east (+X).
        let (sin, cos) = dmath::heading(input.yaw);
        let vx = (sin * fwd + cos * side) * speed;
        let vz = (-cos * fwd + sin * side) * speed;

        if s.grounded && input.pressed(buttons::JUMP) {
            s.vertical_speed = JUMP_SPEED;
            s.grounded = false;
        } else if !s.grounded {
            s.vertical_speed = (s.vertical_speed - GRAVITY * TICK_DT).max(-TERMINAL_SPEED);
        }
        Vector::new(vx * TICK_DT, s.vertical_speed * TICK_DT, vz * TICK_DT)
    }
}

impl Module for PlayerPhysics {
    fn name(&self) -> &'static str {
        "player_physics"
    }

    fn prepare(
        &mut self,
        _tick: u64,
        _input: &PlayerInput,
        geometry: &mut dyn GeometrySource,
    ) -> Result<Readiness, GeometryError> {
        let centre = self.state.position.map(m_to_mm);
        let area = BoundsMm::around(centre, [ACTIVE_RADIUS_MM; 3]);
        let wanted = match geometry.colliders_in(&area)? {
            Fetch::Ready(list) => list,
            Fetch::Pending => return Ok(Readiness::Pending),
        };
        let wanted: BTreeMap<AssetKey, ColliderAsset> =
            wanted.into_iter().map(|a| (a.key.clone(), a)).collect();

        self.remove = self
            .active
            .iter()
            .filter(|(k, a)| wanted.get(*k).is_none_or(|w| w.content_id != a.content_id))
            .map(|(k, _)| k.clone())
            .collect();
        self.built
            .retain(|k, (content, _)| wanted.get(k).is_some_and(|w| w.content_id == *content));
        self.add.clear();
        let mut ready = true;
        for (key, asset) in &wanted {
            if self
                .active
                .get(key)
                .is_some_and(|a| a.content_id == asset.content_id)
            {
                continue;
            }
            if !self.built.contains_key(key) {
                match geometry.mesh(asset)? {
                    Fetch::Ready(mesh) => {
                        let collider = Self::build_collider(asset, &mesh)?;
                        self.built.insert(key.clone(), (asset.content_id, collider));
                    }
                    Fetch::Pending => {
                        ready = false;
                        continue;
                    }
                }
            }
            self.add.push(key.clone());
        }
        Ok(if ready {
            Readiness::Ready
        } else {
            Readiness::Pending
        })
    }

    fn step(&mut self, ctx: &mut StepContext<'_>) {
        let changed = !self.remove.is_empty() || !self.add.is_empty();
        for key in std::mem::take(&mut self.remove) {
            if let Some(a) = self.active.remove(&key) {
                self.world.remove_collider(a.handle);
            }
        }
        for key in std::mem::take(&mut self.add) {
            if let Some((content_id, collider)) = self.built.remove(&key) {
                let handle = self.world.insert_collider(collider, None);
                self.active.insert(key, Active { content_id, handle });
            }
        }
        if changed {
            // Refreshes the broad phase that scene queries use.
            self.world.detect_collisions(&(), &());
        }

        let desired = self.desired_translation(ctx.input);
        let [x, y, z] = self.state.position;
        let movement = self.controller.move_shape(
            TICK_DT,
            &self.world.query_pipeline(),
            self.shape.as_ref(),
            &Pose::from_translation(Vector::new(x, y, z)),
            desired,
            |_| {},
        );
        let t = movement.translation;
        let s = &mut self.state;
        s.position = [x + t.x, y + t.y, z + t.z];
        s.grounded = movement.grounded;
        if s.grounded && s.vertical_speed < 0.0 {
            s.vertical_speed = 0.0;
        }
        if s.vertical_speed > 0.0 && t.y < desired.y * 0.5 {
            // Hit a ceiling.
            s.vertical_speed = 0.0;
        }
        self.last_move = LastMove {
            desired: [desired.x, desired.y, desired.z],
            actual: [t.x, t.y, t.z],
        };
    }

    fn hash_state(&self, h: &mut StateHasher) {
        let s = &self.state;
        for v in s.position {
            h.f32(v);
        }
        h.f32(s.vertical_speed)
            .bool(s.grounded)
            .u16(s.yaw)
            .i16(s.pitch);
        h.u64(self.active.len() as u64);
        for (key, a) in &self.active {
            h.u32(key.tile[0]).u32(key.tile[1]);
            h.str(&key.layer).str(&key.feature_id).bytes(&a.content_id);
        }
    }
}
