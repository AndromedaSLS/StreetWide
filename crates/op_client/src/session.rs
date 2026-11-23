//! The simulation session: runs `op_sim` in `FixedUpdate`, keeps the input
//! log and hash trail for replays, and plays replays back.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use bevy::prelude::*;
use op_kernel::geometry::{mm_to_m, sim_origin_mm};
use op_kernel::{PlayerInput, StateHash, TickOutcome};
use op_replay::format::{CHECKPOINT_INTERVAL, CONTRACT_VERSION, FORMAT, checkpoint_ticks};
use op_replay::{Checkpoint, Recorder, Replay, ReplayHeader, find_world, new_sim};
use op_sim::{SIM_CONTRACT_VERSION, Sim};
use op_stream::{StreamSource, Streamer};
use op_world::world::Spawn;

use crate::Options;

/// Loader threads and decoded-mesh cache budget for the client.
const LOADERS: usize = 4;
const CACHE_BYTES: usize = 768 << 20;
/// Eye height above the feet (m).
pub const EYE_HEIGHT: f32 = 1.65;

/// Where the playback of a `--replay` stands.
pub enum Playback {
    Running,
    Matched(StateHash),
    Diverged {
        tick: u64,
        expected: StateHash,
        actual: StateHash,
    },
}

/// Input for the next tick, written every frame by `controls`.
#[derive(Resource, Default)]
pub struct LatchedInput {
    pub input: PlayerInput,
    /// A jump pressed since the last tick, kept until a tick uses it.
    pub jump_pending: bool,
}

#[derive(Resource)]
pub struct Session {
    sim: Sim,
    source: StreamSource,
    streamer: Arc<Streamer>,
    spawn: Spawn,
    seed: u64,
    /// Region-frame origin of the sim frame (mm); rendering uses it too.
    pub sim_origin_mm: [i64; 3],
    inputs: Vec<PlayerInput>,
    checkpoints: Vec<Checkpoint>,
    last_hash: Option<StateHash>,
    pub prev_feet: Vec3,
    pub curr_feet: Vec3,
    pub stalled: bool,
    pub error: Option<String>,
    pub message: Option<(String, f32)>,
    /// `Some` when playing a replay back.
    replay: Option<Replay>,
    pub playback: Option<Playback>,
}

fn feet(sim: &Sim) -> Vec3 {
    Vec3::from_array(sim.player().feet())
}

impl Session {
    pub fn start(options: &Options) -> Result<Session, String> {
        let replay = match &options.replay {
            Some(path) => {
                let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
                Some(Replay::from_bytes(&bytes).map_err(|e| format!("{}: {e}", path.display()))?)
            }
            None => None,
        };
        let world_dir = match (&replay, &options.world) {
            (Some(r), w) => find_world(&r.header, w.as_deref()).map_err(|e| e.to_string())?,
            (None, Some(w)) => w.clone(),
            (None, None) => default_world()?,
        };
        let streamer = Arc::new(
            Streamer::open(&world_dir, LOADERS, CACHE_BYTES)
                .map_err(|e| format!("{}: {e}", world_dir.display()))?,
        );
        if let Some(r) = &replay
            && r.header.sim_contract_version != SIM_CONTRACT_VERSION
        {
            return Err(format!(
                "the replay needs simulation contract {}, this client implements {SIM_CONTRACT_VERSION}",
                r.header.sim_contract_version
            ));
        }
        let (spawn, seed) = match &replay {
            Some(r) => (r.header.spawn, r.header.seed),
            None => (streamer.index().world().spawn, options.seed),
        };
        let origin = sim_origin_mm(spawn.position_mm);
        let source = StreamSource::new(Arc::clone(&streamer), origin);
        let sim = new_sim(&header(&streamer, spawn, seed, 0));
        let f = feet(&sim);
        Ok(Session {
            sim,
            source,
            playback: replay.as_ref().map(|_| Playback::Running),
            replay,
            streamer,
            spawn,
            seed,
            sim_origin_mm: origin,
            inputs: Vec::new(),
            checkpoints: Vec::new(),
            last_hash: None,
            prev_feet: f,
            curr_feet: f,
            stalled: false,
            error: None,
            message: None,
        })
    }

    pub fn world_name(&self) -> &str {
        &self.streamer.index().world().name
    }

    pub fn streamer(&self) -> &Arc<Streamer> {
        &self.streamer
    }

    pub fn sim(&self) -> &Sim {
        &self.sim
    }

    pub fn spawn_yaw(&self) -> u16 {
        self.spawn.yaw
    }

    pub fn is_playback(&self) -> bool {
        self.replay.is_some()
    }

    /// Back to the spawn point with a fresh simulation and input log.
    pub fn restart(&mut self) {
        self.sim = new_sim(&header(&self.streamer, self.spawn, self.seed, 0));
        self.inputs.clear();
        self.checkpoints.clear();
        self.last_hash = None;
        self.prev_feet = feet(&self.sim);
        self.curr_feet = self.prev_feet;
    }

    /// Writes the session so far as a replay into `dir`.
    pub fn save_replay(&self, dir: &Path) -> Result<PathBuf, String> {
        let ticks = self.inputs.len() as u64;
        let last = self.last_hash.ok_or("nothing to save yet")?;
        let mut checkpoints = self.checkpoints.clone();
        if checkpoints.last().map(|c| c.tick) != Some(ticks) {
            checkpoints.push(Checkpoint {
                tick: ticks,
                hash: last,
            });
        }
        debug_assert_eq!(
            checkpoints.iter().map(|c| c.tick).collect::<Vec<_>>(),
            checkpoint_ticks(ticks, CHECKPOINT_INTERVAL)
        );
        let replay = Replay {
            header: header(&self.streamer, self.spawn, self.seed, ticks),
            inputs: self.inputs.clone(),
            checkpoints,
        };
        let bytes = replay.to_bytes().map_err(|e| e.to_string())?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = dir.join(format!("{}-{stamp}.oprep", self.world_name()));
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(path)
    }

    /// The region-frame offset of a tile origin in the sim frame (m).
    pub fn offset_m(&self, origin_mm: [i64; 3]) -> Vec3 {
        Vec3::new(
            mm_to_m(origin_mm[0] - self.sim_origin_mm[0]),
            mm_to_m(origin_mm[1] - self.sim_origin_mm[1]),
            mm_to_m(origin_mm[2] - self.sim_origin_mm[2]),
        )
    }

    fn tick(&mut self, live: &mut LatchedInput) {
        if self.error.is_some() {
            return;
        }
        let input = match &self.replay {
            Some(r) => match r.inputs.get(self.sim.ticks() as usize) {
                Some(i) => *i,
                None => return,
            },
            None => {
                let mut i = live.input;
                if live.jump_pending {
                    i.buttons |= op_kernel::input::buttons::JUMP;
                }
                i
            }
        };
        match self.sim.tick(&input, &mut self.source) {
            Ok(TickOutcome::Advanced { tick, hash }) => {
                self.stalled = false;
                live.jump_pending = false;
                self.prev_feet = self.curr_feet;
                self.curr_feet = feet(&self.sim);
                self.inputs.push(input);
                self.last_hash = Some(hash);
                if tick % CHECKPOINT_INTERVAL == 0 {
                    self.checkpoints.push(Checkpoint { tick, hash });
                }
                self.check_playback(tick, hash);
            }
            Ok(TickOutcome::Stalled) => {
                self.stalled = true;
                self.prev_feet = self.curr_feet;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    fn check_playback(&mut self, tick: u64, hash: StateHash) {
        let (Some(replay), Some(Playback::Running)) = (&self.replay, &self.playback) else {
            return;
        };
        if let Some(want) = replay.checkpoints.iter().find(|c| c.tick == tick)
            && want.hash != hash
        {
            self.playback = Some(Playback::Diverged {
                tick,
                expected: want.hash,
                actual: hash,
            });
        } else if tick == replay.header.ticks {
            self.playback = Some(Playback::Matched(hash));
        }
    }
}

fn header(streamer: &Streamer, spawn: Spawn, seed: u64, ticks: u64) -> ReplayHeader {
    let index = streamer.index();
    ReplayHeader {
        format: FORMAT.into(),
        contract_version: CONTRACT_VERSION.into(),
        sim_contract_version: SIM_CONTRACT_VERSION.into(),
        world_id: index.world_id(),
        world_name: index.world().name.clone(),
        seed,
        spawn,
        tick_hz: op_kernel::TICK_HZ,
        ticks,
        checkpoint_interval: CHECKPOINT_INTERVAL,
        recorder: Recorder {
            name: "op_client".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
    }
}

/// The Zurich district if it has been built, else the committed fixture.
fn default_world() -> Result<PathBuf, String> {
    for dir in ["data/world/zurich-altstadt", "fixtures/world"] {
        if Path::new(dir).join("world.json").exists() {
            if dir == "fixtures/world" {
                info!(
                    "data/world/zurich-altstadt not found; using the one-tile fixture (build the district with `python -m tools.ingest --world zurich-altstadt`)"
                );
            }
            return Ok(PathBuf::from(dir));
        }
    }
    Err("no world found; run from the repository root or pass --world <dir>".into())
}

pub struct SessionPlugin;

impl Plugin for SessionPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Time::<Fixed>::from_hz(f64::from(op_kernel::TICK_HZ)))
            .init_resource::<LatchedInput>()
            .add_systems(FixedUpdate, tick_session);
    }
}

fn tick_session(mut session: ResMut<Session>, mut live: ResMut<LatchedInput>) {
    session.tick(&mut live);
}

#[cfg(test)]
mod tests {
    use op_kernel::input::buttons;
    use op_replay::{Verdict, check, open_world};

    use super::*;

    /// A live session (streaming geometry, stalls, latched input, jump
    /// presses) saved the way F9 does must pass `op_replay check`.
    #[test]
    fn a_saved_live_session_checks_with_op_replay() {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let world = repo.join("fixtures/world");
        let options = Options {
            world: Some(world.clone()),
            seed: 99,
            ..Options::default()
        };
        let mut session = Session::start(&options).unwrap();
        let mut live = LatchedInput::default();
        for t in 0..430u32 {
            live.input = PlayerInput {
                forward: if t > 60 { 127 } else { 0 },
                strafe: if t % 200 > 150 { -60 } else { 0 },
                yaw: (49_152 + t * 40) as u16,
                pitch: -300,
                buttons: if t > 300 { buttons::SPRINT } else { 0 },
            };
            if t == 200 {
                live.jump_pending = true;
            }
            let before = session.sim().ticks();
            while session.sim().ticks() == before {
                session.tick(&mut live);
                assert!(session.error.is_none(), "{:?}", session.error);
                std::thread::yield_now();
            }
        }
        let dir =
            std::env::temp_dir().join(format!("streetwide-client-test-{}", std::process::id()));
        let path = session.save_replay(&dir).unwrap();
        let replay = Replay::from_bytes(&std::fs::read(&path).unwrap()).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!((replay.header.ticks, replay.header.seed), (430, 99));
        assert_eq!(replay.checkpoints.last().unwrap().tick, 430);
        let mut source = open_world(&replay.header, &world).unwrap();
        assert!(matches!(
            check(&replay, &mut source).unwrap(),
            Verdict::Match(_)
        ));
    }
}
