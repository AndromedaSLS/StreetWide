//! StreetWide's deterministic simulation.
//!
//! A [`Sim`] is an [`op_kernel::Schedule`] with one module so far:
//! [`player::PlayerPhysics`], a Rapier kinematic character (a capsule)
//! walking on the world's collision meshes. Given the same seed, spawn
//! point, world and inputs, it produces the same state hash after every
//! tick on every platform (ADR-0003, ADR-0004).
//!
//! The simulation never depends on rendering, and it sees the world only
//! through [`op_kernel::GeometrySource`].

pub mod player;

use op_kernel::{GeometrySource, PlayerInput, Schedule, SimError, StateHash, TickOutcome};

pub use player::{LastMove, PlayerPhysics, PlayerState};

/// Version of the simulation contract: the state, its hash encoding and the
/// player physics. Replays record it; a change that alters any state hash
/// must bump it.
pub const SIM_CONTRACT_VERSION: &str = "0.1.0";

/// The simulation of one session.
pub struct Sim {
    schedule: Schedule,
}

impl Sim {
    /// A new session. `spawn_feet_mm` is where the player's feet start, in
    /// the sim frame (see [`op_kernel::geometry`]); `yaw` is the initial
    /// heading in 1/65536 turns.
    pub fn new(seed: u64, spawn_feet_mm: [i64; 3], yaw: u16) -> Sim {
        Sim {
            schedule: Schedule::new(seed, vec![Box::new(PlayerPhysics::new(spawn_feet_mm, yaw))]),
        }
    }

    /// Runs one tick, or stalls without changing anything if geometry is
    /// still loading.
    pub fn tick(
        &mut self,
        input: &PlayerInput,
        geometry: &mut dyn GeometrySource,
    ) -> Result<TickOutcome, SimError> {
        self.schedule.tick(input, geometry)
    }

    /// Ticks run so far.
    pub fn ticks(&self) -> u64 {
        self.schedule.ticks()
    }

    pub fn state_hash(&self) -> StateHash {
        self.schedule.state_hash()
    }

    fn physics(&self) -> &PlayerPhysics {
        self.schedule
            .module::<PlayerPhysics>()
            .expect("the schedule always has the player module")
    }

    pub fn player(&self) -> &PlayerState {
        self.physics().state()
    }

    /// The movement asked for and achieved in the last tick (diagnostics;
    /// fully determined by the state, so not hashed separately).
    pub fn last_move(&self) -> &LastMove {
        self.physics().last_move()
    }

    /// Number of colliders currently in the physics world.
    pub fn active_colliders(&self) -> usize {
        self.physics().active_count()
    }
}
