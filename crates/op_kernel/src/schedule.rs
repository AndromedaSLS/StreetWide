//! Modules and the fixed-order schedule.
//!
//! Every tick runs in two phases:
//!
//! 1. **prepare**: each module, in order, gathers what it needs (for
//!    example collision meshes from a [`GeometrySource`]). Preparing must
//!    not change simulation state; it may only fill caches. If any module
//!    is not ready, the tick **stalls**: nothing changes and the tick
//!    counter does not advance, so a stall can never make two runs differ.
//! 2. **step**: each module, in order, advances its state by one tick.
//!
//! After the step the schedule hashes the whole state: the tick number, the
//! RNG and every module's [`Module::hash_state`], in order.

use std::any::Any;
use std::fmt;

use crate::geometry::{GeometryError, GeometrySource};
use crate::hash::{StateHash, StateHasher};
use crate::input::{InputError, PlayerInput};
use crate::rng::Rng;

/// Domain of the per-tick state hash. Bump the version when the encoding
/// changes.
pub const STATE_HASH_DOMAIN: &str = "streetwide.state.v1";

/// Whether a module can step this tick.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Readiness {
    Ready,
    Pending,
}

/// What a module sees while stepping.
pub struct StepContext<'a> {
    /// Index of the tick being computed (0 for the first tick).
    pub tick: u64,
    pub input: &'a PlayerInput,
    /// The simulation's only randomness. Modules draw from it in schedule
    /// order.
    pub rng: &'a mut Rng,
}

/// A part of the simulation.
pub trait Module: Any {
    /// Stable name, hashed into the state (so renaming changes hashes).
    fn name(&self) -> &'static str;

    /// Gathers what the next step needs. Must not change state that
    /// [`Module::hash_state`] covers or that later steps depend on.
    fn prepare(
        &mut self,
        tick: u64,
        input: &PlayerInput,
        geometry: &mut dyn GeometrySource,
    ) -> Result<Readiness, GeometryError>;

    /// Advances the module by one tick.
    fn step(&mut self, ctx: &mut StepContext<'_>);

    /// Feeds the module's complete state into the hasher.
    fn hash_state(&self, h: &mut StateHasher);
}

/// The result of [`Schedule::tick`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TickOutcome {
    /// The tick ran; `tick` ticks have now run, and `hash` is the state
    /// hash after them.
    Advanced { tick: u64, hash: StateHash },
    /// Geometry is still loading; nothing changed. Call again later with
    /// the same input.
    Stalled,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SimError {
    Input(InputError),
    Geometry(GeometryError),
}

impl fmt::Display for SimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SimError::Input(e) => write!(f, "invalid input: {e}"),
            SimError::Geometry(e) => write!(f, "geometry: {e}"),
        }
    }
}

impl std::error::Error for SimError {}

/// Runs modules in a fixed order, one tick at a time.
pub struct Schedule {
    tick: u64,
    rng: Rng,
    modules: Vec<Box<dyn Module>>,
}

impl Schedule {
    pub fn new(seed: u64, modules: Vec<Box<dyn Module>>) -> Self {
        Schedule {
            tick: 0,
            rng: Rng::new(seed),
            modules,
        }
    }

    /// Number of ticks run so far.
    pub fn ticks(&self) -> u64 {
        self.tick
    }

    pub fn tick(
        &mut self,
        input: &PlayerInput,
        geometry: &mut dyn GeometrySource,
    ) -> Result<TickOutcome, SimError> {
        input.check().map_err(SimError::Input)?;
        for m in &mut self.modules {
            let readiness = m
                .prepare(self.tick, input, geometry)
                .map_err(SimError::Geometry)?;
            if readiness == Readiness::Pending {
                return Ok(TickOutcome::Stalled);
            }
        }
        let mut ctx = StepContext {
            tick: self.tick,
            input,
            rng: &mut self.rng,
        };
        for m in &mut self.modules {
            m.step(&mut ctx);
        }
        self.tick += 1;
        Ok(TickOutcome::Advanced {
            tick: self.tick,
            hash: self.state_hash(),
        })
    }

    /// Hash of the complete current state.
    pub fn state_hash(&self) -> StateHash {
        let mut h = StateHasher::new(STATE_HASH_DOMAIN);
        h.u64(self.tick);
        self.rng.hash_state(&mut h);
        for m in &self.modules {
            h.str(m.name());
            m.hash_state(&mut h);
        }
        h.finish()
    }

    /// The first module of type `T`.
    pub fn module<T: Module>(&self) -> Option<&T> {
        self.modules
            .iter()
            .find_map(|m| (m.as_ref() as &dyn Any).downcast_ref::<T>())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::geometry::{BoundsMm, ColliderAsset, ColliderMesh, Fetch};

    /// A source that is pending for the first `pending` questions.
    struct Slow {
        pending: u32,
    }

    impl GeometrySource for Slow {
        fn colliders_in(
            &mut self,
            _: &BoundsMm,
        ) -> Result<Fetch<Vec<ColliderAsset>>, GeometryError> {
            if self.pending > 0 {
                self.pending -= 1;
                return Ok(Fetch::Pending);
            }
            Ok(Fetch::Ready(Vec::new()))
        }

        fn mesh(&mut self, _: &ColliderAsset) -> Result<Fetch<Arc<ColliderMesh>>, GeometryError> {
            Err(GeometryError("no meshes".into()))
        }
    }

    /// Counts forward input and draws one random number per tick.
    struct Counter {
        total: i64,
        last_random: u64,
    }

    impl Module for Counter {
        fn name(&self) -> &'static str {
            "counter"
        }

        fn prepare(
            &mut self,
            _: u64,
            _: &PlayerInput,
            g: &mut dyn GeometrySource,
        ) -> Result<Readiness, GeometryError> {
            Ok(match g.colliders_in(&BoundsMm::default())? {
                Fetch::Ready(_) => Readiness::Ready,
                Fetch::Pending => Readiness::Pending,
            })
        }

        fn step(&mut self, ctx: &mut StepContext<'_>) {
            self.total += i64::from(ctx.input.forward);
            self.last_random = ctx.rng.next_u64();
        }

        fn hash_state(&self, h: &mut StateHasher) {
            h.i64(self.total).u64(self.last_random);
        }
    }

    fn schedule(seed: u64) -> Schedule {
        Schedule::new(
            seed,
            vec![Box::new(Counter {
                total: 0,
                last_random: 0,
            })],
        )
    }

    #[test]
    fn a_stall_changes_nothing() {
        let input = PlayerInput {
            forward: 3,
            ..PlayerInput::default()
        };
        let mut s = schedule(1);
        let before = s.state_hash();
        let mut slow = Slow { pending: 2 };
        assert_eq!(s.tick(&input, &mut slow), Ok(TickOutcome::Stalled));
        assert_eq!(s.tick(&input, &mut slow), Ok(TickOutcome::Stalled));
        assert_eq!((s.ticks(), s.state_hash()), (0, before));

        let mut fast = Slow { pending: 0 };
        let mut reference = schedule(1);
        let a = s.tick(&input, &mut slow).unwrap();
        let b = reference.tick(&input, &mut fast).unwrap();
        assert_eq!(
            a, b,
            "a stalled run must continue exactly like an unstalled one"
        );
        assert_eq!(s.module::<Counter>().unwrap().total, 3);
    }

    #[test]
    fn seed_and_input_reach_the_hash() {
        let mut g = Slow { pending: 0 };
        let input = PlayerInput::default();
        let mut a = schedule(1);
        let mut b = schedule(2);
        assert_ne!(a.tick(&input, &mut g), b.tick(&input, &mut g));
        let bad = PlayerInput {
            buttons: 0x80,
            ..PlayerInput::default()
        };
        assert!(matches!(a.tick(&bad, &mut g), Err(SimError::Input(_))));
    }
}
