//! The `.oprep` replay file (ADR-0007).
//!
//! ```text
//! offset        size        field
//! 0             8           magic "SWREPLAY"
//! 8             4           file version, u32 LE (1)
//! 12            4           header length H, u32 LE
//! 16            H           header: canonical JSON (schemas/replay-header.schema.json)
//! 16+H          8 * ticks   one input record per tick (op_kernel::input)
//! ...           4           checkpoint count C, u32 LE
//! ...           40 * C      checkpoints: tick u64 LE, state hash (32 bytes)
//! ```
//!
//! Checkpoints are at every multiple of `checkpoint_interval` and at the
//! last tick. Nothing may follow the last checkpoint. Readers reject any
//! file that does not re-encode to exactly the same bytes.

use std::fmt;

use op_kernel::input::RECORD_BYTES;
use op_kernel::{InputError, PlayerInput, StateHash};
use op_world::cjson::{self, MAX_SAFE_INTEGER};
use op_world::world::Spawn;
use op_world::{ContentId, rules};
use serde::{Deserialize, Serialize};

pub const MAGIC: [u8; 8] = *b"SWREPLAY";
pub const FILE_VERSION: u32 = 1;
pub const FORMAT: &str = "streetwide.replay";
/// Version of the replay contract (header fields and binary layout).
pub const CONTRACT_VERSION: &str = "0.1.0";
pub const CHECKPOINT_INTERVAL: u64 = 60;
const MAX_HEADER_BYTES: usize = 64 * 1024;
/// One day at 60 Hz.
pub const MAX_TICKS: u64 = 60 * 60 * 60 * 24;

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recorder {
    pub name: String,
    pub version: String,
}

/// What a replay was recorded with.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayHeader {
    pub format: String,
    pub contract_version: String,
    /// `op_sim::SIM_CONTRACT_VERSION` of the recording simulation.
    pub sim_contract_version: String,
    /// The world the replay was recorded in (RFC 0001 world id).
    pub world_id: ContentId,
    /// The world's name, a hint for finding it on disk.
    pub world_name: String,
    /// Seed of the simulation's random number generator.
    pub seed: u64,
    /// Where the player starts, in the region frame.
    pub spawn: Spawn,
    pub tick_hz: u32,
    pub ticks: u64,
    pub checkpoint_interval: u64,
    pub recorder: Recorder,
}

impl ReplayHeader {
    pub fn problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        if self.format != FORMAT {
            p.push(format!("format must be {FORMAT:?}"));
        }
        if self.contract_version != CONTRACT_VERSION {
            p.push(format!(
                "unknown replay contract version {:?}",
                self.contract_version
            ));
        }
        for (name, text) in [
            ("sim_contract_version", &self.sim_contract_version),
            ("recorder.name", &self.recorder.name),
            ("recorder.version", &self.recorder.version),
        ] {
            if !rules::is_text(text) {
                p.push(format!("{name} must be non-empty text"));
            }
        }
        if !rules::is_slug(&self.world_name) {
            p.push("world_name must be lowercase words joined by '-'".into());
        }
        if self.seed > MAX_SAFE_INTEGER as u64 {
            p.push("seed must be at most 2^53 - 1".into());
        }
        if self.tick_hz != op_kernel::TICK_HZ {
            p.push(format!("tick_hz must be {}", op_kernel::TICK_HZ));
        }
        if !(1..=MAX_TICKS).contains(&self.ticks) {
            p.push(format!("ticks must be 1..={MAX_TICKS}"));
        }
        if !(1..=3600).contains(&self.checkpoint_interval) {
            p.push("checkpoint_interval must be 1..=3600".into());
        }
        p
    }
}

/// The state hash after `tick` ticks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Checkpoint {
    pub tick: u64,
    pub hash: StateHash,
}

/// Ticks that carry a checkpoint.
pub fn checkpoint_ticks(ticks: u64, interval: u64) -> Vec<u64> {
    let mut out: Vec<u64> = (1..=ticks / interval).map(|k| k * interval).collect();
    if out.last() != Some(&ticks) {
        out.push(ticks);
    }
    out
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Replay {
    pub header: ReplayHeader,
    pub inputs: Vec<PlayerInput>,
    pub checkpoints: Vec<Checkpoint>,
}

#[derive(Debug)]
pub enum ReplayError {
    Malformed(String),
    Header(String),
    Input { tick: u64, error: InputError },
}

impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReplayError::Malformed(m) => write!(f, "malformed replay: {m}"),
            ReplayError::Header(m) => write!(f, "invalid replay header: {m}"),
            ReplayError::Input { tick, error } => {
                write!(f, "invalid input at tick {tick}: {error}")
            }
        }
    }
}

impl std::error::Error for ReplayError {}

fn malformed(m: &str) -> ReplayError {
    ReplayError::Malformed(m.into())
}

struct Reader<'a> {
    b: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ReplayError> {
        if self.b.len() < n {
            return Err(malformed("unexpected end of file"));
        }
        let (head, rest) = self.b.split_at(n);
        self.b = rest;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], ReplayError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    fn u32(&mut self) -> Result<u32, ReplayError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, ReplayError> {
        Ok(u64::from_le_bytes(self.array()?))
    }
}

impl Replay {
    pub fn check(&self) -> Result<(), ReplayError> {
        let problems = self.header.problems();
        if !problems.is_empty() {
            return Err(ReplayError::Header(problems.join("; ")));
        }
        if self.inputs.len() as u64 != self.header.ticks {
            return Err(malformed("input count differs from header.ticks"));
        }
        for (tick, i) in self.inputs.iter().enumerate() {
            i.check().map_err(|error| ReplayError::Input {
                tick: tick as u64,
                error,
            })?;
        }
        let expected = checkpoint_ticks(self.header.ticks, self.header.checkpoint_interval);
        if self
            .checkpoints
            .iter()
            .map(|c| c.tick)
            .ne(expected.iter().copied())
        {
            return Err(malformed("checkpoints are not at the expected ticks"));
        }
        Ok(())
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, ReplayError> {
        self.check()?;
        let header =
            cjson::to_canonical(&self.header).map_err(|e| ReplayError::Header(e.to_string()))?;
        if header.len() > MAX_HEADER_BYTES {
            return Err(ReplayError::Header("header too large".into()));
        }
        let mut out = Vec::with_capacity(
            16 + header.len() + self.inputs.len() * RECORD_BYTES + 4 + self.checkpoints.len() * 40,
        );
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&FILE_VERSION.to_le_bytes());
        out.extend_from_slice(&(header.len() as u32).to_le_bytes());
        out.extend_from_slice(&header);
        for i in &self.inputs {
            out.extend_from_slice(&i.to_bytes());
        }
        out.extend_from_slice(&(self.checkpoints.len() as u32).to_le_bytes());
        for c in &self.checkpoints {
            out.extend_from_slice(&c.tick.to_le_bytes());
            out.extend_from_slice(&c.hash.0);
        }
        Ok(out)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Replay, ReplayError> {
        let mut r = Reader { b: bytes };
        if r.array::<8>()? != MAGIC {
            return Err(malformed("not a StreetWide replay (bad magic)"));
        }
        let version = r.u32()?;
        if version != FILE_VERSION {
            return Err(malformed(&format!("unsupported file version {version}")));
        }
        let header_len = r.u32()? as usize;
        if header_len > MAX_HEADER_BYTES {
            return Err(malformed("header too large"));
        }
        let header: ReplayHeader = cjson::from_canonical(r.take(header_len)?)
            .map_err(|e| ReplayError::Header(e.to_string()))?;
        let problems = header.problems();
        if !problems.is_empty() {
            return Err(ReplayError::Header(problems.join("; ")));
        }
        let ticks = header.ticks as usize;
        let mut inputs = Vec::with_capacity(ticks);
        for tick in 0..header.ticks {
            let record: [u8; RECORD_BYTES] = r.array()?;
            inputs.push(
                PlayerInput::from_bytes(record)
                    .map_err(|error| ReplayError::Input { tick, error })?,
            );
        }
        let count = r.u32()? as usize;
        if count != checkpoint_ticks(header.ticks, header.checkpoint_interval).len() {
            return Err(malformed("wrong number of checkpoints"));
        }
        let mut checkpoints = Vec::with_capacity(count);
        for _ in 0..count {
            let tick = r.u64()?;
            checkpoints.push(Checkpoint {
                tick,
                hash: StateHash(r.array()?),
            });
        }
        if !r.b.is_empty() {
            return Err(malformed("trailing bytes after the last checkpoint"));
        }
        let replay = Replay {
            header,
            inputs,
            checkpoints,
        };
        replay.check()?;
        Ok(replay)
    }
}
