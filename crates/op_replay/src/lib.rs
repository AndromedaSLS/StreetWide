//! StreetWide replays: record inputs, play them back, and check that the
//! simulation reproduces the recorded state hashes (ADR-0007).
//!
//! A replay holds the seed, the spawn point and one [`PlayerInput`] per
//! tick, plus a hash trail: the state hash every 60 ticks and at the end.
//! Playing it back in the same world must reproduce every hash bit for
//! bit, on every platform. `op_replay check` does exactly that, and CI runs
//! it on Linux, Windows and macOS.

pub mod format;
pub mod synth;

use std::fmt;
use std::path::{Path, PathBuf};

use op_kernel::geometry::sim_origin_mm;
use op_kernel::{PlayerInput, StateHash, TickOutcome};
use op_sim::{SIM_CONTRACT_VERSION, Sim};
use op_stream::StoreSource;
use op_world::{ContentId, Store};

pub use format::{Checkpoint, Recorder, Replay, ReplayError, ReplayHeader};

#[derive(Debug)]
pub enum Error {
    Replay(ReplayError),
    /// The replay needs a different simulation contract than this build.
    SimVersion {
        recorded: String,
    },
    World(String),
    Sim(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Replay(e) => write!(f, "{e}"),
            Error::SimVersion { recorded } => write!(
                f,
                "recorded with simulation contract {recorded}, this build implements {SIM_CONTRACT_VERSION}"
            ),
            Error::World(m) => write!(f, "world: {m}"),
            Error::Sim(m) => write!(f, "simulation: {m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<ReplayError> for Error {
    fn from(e: ReplayError) -> Self {
        Error::Replay(e)
    }
}

/// Finds the world a replay was recorded in: `explicit` if given,
/// otherwise `fixtures/world` or `data/world/<world_name>` under the current
/// directory, whichever has the recorded world id.
pub fn find_world(header: &ReplayHeader, explicit: Option<&Path>) -> Result<PathBuf, Error> {
    let candidates: Vec<PathBuf> = match explicit {
        Some(p) => vec![p.to_path_buf()],
        None => vec![
            PathBuf::from("fixtures/world"),
            Path::new("data/world").join(&header.world_name),
        ],
    };
    for dir in &candidates {
        if let Ok((id, _)) = Store::new(dir).read_world() {
            if id == header.world_id {
                return Ok(dir.clone());
            }
            if explicit.is_some() {
                return Err(Error::World(format!(
                    "{} is world {id}, the replay needs {}",
                    dir.display(),
                    header.world_id
                )));
            }
        }
    }
    Err(Error::World(format!(
        "world {} ({}) not found in {}; pass --world <dir>",
        header.world_name,
        header.world_id,
        candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

/// Opens the world store for a replay and checks its id.
pub fn open_world(header: &ReplayHeader, dir: &Path) -> Result<StoreSource, Error> {
    let source = StoreSource::open(dir, sim_origin_mm(header.spawn.position_mm))
        .map_err(|e| Error::World(e.to_string()))?;
    if source.world_id() != header.world_id {
        return Err(Error::World(format!(
            "{} is world {}, the replay needs {}",
            dir.display(),
            source.world_id(),
            header.world_id
        )));
    }
    Ok(source)
}

/// A fresh simulation at the replay's spawn point.
pub fn new_sim(header: &ReplayHeader) -> Sim {
    let spawn = header.spawn.position_mm;
    let origin = sim_origin_mm(spawn);
    let feet = [0, 1, 2].map(|i| spawn[i] - origin[i]);
    Sim::new(header.seed, feet, header.spawn.yaw)
}

/// Runs `inputs` from the start and returns the hash trail. `on_tick` sees
/// the simulation after every tick.
pub fn play(
    header: &ReplayHeader,
    inputs: &[PlayerInput],
    source: &mut StoreSource,
    mut on_tick: impl FnMut(&Sim),
) -> Result<Vec<Checkpoint>, Error> {
    if header.sim_contract_version != SIM_CONTRACT_VERSION {
        return Err(Error::SimVersion {
            recorded: header.sim_contract_version.clone(),
        });
    }
    let wanted = format::checkpoint_ticks(inputs.len() as u64, header.checkpoint_interval);
    let mut sim = new_sim(header);
    let mut trail = Vec::with_capacity(wanted.len());
    for input in inputs {
        match sim
            .tick(input, source)
            .map_err(|e| Error::Sim(e.to_string()))?
        {
            TickOutcome::Advanced { tick, hash } => {
                if wanted.binary_search(&tick).is_ok() {
                    trail.push(Checkpoint { tick, hash });
                }
            }
            TickOutcome::Stalled => {
                return Err(Error::Sim(
                    "the world store stalled (it never should)".into(),
                ));
            }
        }
        on_tick(&sim);
    }
    Ok(trail)
}

/// The outcome of checking a replay.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    /// Every checkpoint matched; the final state hash.
    Match(StateHash),
    /// The first checkpoint that differs.
    Diverged {
        tick: u64,
        expected: StateHash,
        actual: StateHash,
    },
}

/// Plays a replay back and compares its hash trail.
pub fn check(replay: &Replay, source: &mut StoreSource) -> Result<Verdict, Error> {
    let trail = play(&replay.header, &replay.inputs, source, |_| {})?;
    for (want, got) in replay.checkpoints.iter().zip(&trail) {
        if want != got {
            return Ok(Verdict::Diverged {
                tick: want.tick,
                expected: want.hash,
                actual: got.hash,
            });
        }
    }
    let last = trail
        .last()
        .ok_or_else(|| Error::Sim("empty replay".into()))?;
    Ok(Verdict::Match(last.hash))
}

/// Records `inputs` played in the world at `dir` into a replay.
pub fn record(
    dir: &Path,
    seed: u64,
    spawn: Option<op_world::world::Spawn>,
    inputs: Vec<PlayerInput>,
    recorder: Recorder,
    on_tick: impl FnMut(&Sim),
) -> Result<Replay, Error> {
    let (world_id, world): (ContentId, _) = Store::new(dir)
        .read_world()
        .map_err(|e| Error::World(format!("{}: {e}", dir.display())))?;
    let header = ReplayHeader {
        format: format::FORMAT.into(),
        contract_version: format::CONTRACT_VERSION.into(),
        sim_contract_version: SIM_CONTRACT_VERSION.into(),
        world_id,
        world_name: world.name.clone(),
        seed,
        spawn: spawn.unwrap_or(world.spawn),
        tick_hz: op_kernel::TICK_HZ,
        ticks: inputs.len() as u64,
        checkpoint_interval: format::CHECKPOINT_INTERVAL,
        recorder,
    };
    let mut source = open_world(&header, dir)?;
    let checkpoints = play(&header, &inputs, &mut source, on_tick)?;
    let replay = Replay {
        header,
        inputs,
        checkpoints,
    };
    replay.check()?;
    Ok(replay)
}
