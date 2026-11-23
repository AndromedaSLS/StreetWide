//! StreetWide kernel: the contracts every simulation module builds on.
//!
//! - [`schedule`]: the [`Module`] trait and the fixed-order [`Schedule`]
//!   that advances the simulation one 60 Hz tick at a time.
//! - [`input`]: the per-tick [`PlayerInput`], quantised to integers so it
//!   can be recorded and replayed exactly.
//! - [`hash`]: the BLAKE3 [`StateHasher`] behind per-tick state hashes.
//! - [`rng`]: the only source of randomness, seeded from the replay.
//! - [`dmath`]: deterministic trigonometry (libm), the same bits on every
//!   platform.
//! - [`geometry`]: how world geometry reaches the simulation
//!   ([`GeometrySource`]), in integer millimetres.
//!
//! The kernel is engine-free: no Bevy, no physics engine, no I/O
//! (ADR-0002). Determinism rules are in ADR-0003.

pub mod dmath;
pub mod geometry;
pub mod hash;
pub mod input;
pub mod rng;
pub mod schedule;

pub use geometry::{
    AssetKey, BoundsMm, ColliderAsset, ColliderMesh, Fetch, GeometryError, GeometrySource,
};
pub use hash::{StateHash, StateHasher};
pub use input::{InputError, PlayerInput};
pub use rng::Rng;
pub use schedule::{Module, Readiness, Schedule, SimError, StepContext, TickOutcome};

/// Simulation ticks per second.
pub const TICK_HZ: u32 = 60;

/// Length of one tick in seconds.
pub const TICK_DT: f32 = 1.0 / 60.0;
