//! Fuzzing of the replay reader, `Replay::from_bytes` (ADR-0007; roadmap
//! M1, "Before announcing"). Replays are untrusted: players share them, the
//! client opens them and the full gate takes them as `--replay` arguments.
//! A malformed file must be an error, never a panic, a hang or an
//! allocation sized by a length field; an accepted replay must re-encode to
//! exactly its bytes.
//!
//! The engine is shared with `op_world`'s fuzz tests through `#[path]` (no
//! dependency between the crates' tests); its environment variables
//! (`STREETWIDE_FUZZ_ITERS`, `STREETWIDE_FUZZ_SEED`) and failure reports are
//! described in `op_world/tests/fuzzing/mod.rs`. Inputs that once broke the
//! reader are pinned at the end of this file.

#[path = "../../op_world/tests/fuzzing/mod.rs"]
mod fuzzing;

use std::sync::OnceLock;

use fuzzing::{Rng, Target};
use op_kernel::{PlayerInput, StateHash};
use op_replay::format::{self, checkpoint_ticks};
use op_replay::{Checkpoint, Recorder, Replay, ReplayHeader};
use op_world::ContentId;
use op_world::cjson::MAX_SAFE_INTEGER;
use op_world::world::Spawn;
use serde_json::Value;

// ----------------------------------------------------------------- corpus --

/// A valid replay with the given header values and arbitrary valid inputs.
fn synthetic(ticks: u64, interval: u64, seed: u64, spawn: Spawn) -> Vec<u8> {
    let header = ReplayHeader {
        format: format::FORMAT.into(),
        contract_version: format::CONTRACT_VERSION.into(),
        sim_contract_version: op_sim::SIM_CONTRACT_VERSION.into(),
        world_id: ContentId::of(b"a world"),
        world_name: "fuzz-world".into(),
        seed,
        spawn,
        tick_hz: op_kernel::TICK_HZ,
        ticks,
        checkpoint_interval: interval,
        recorder: Recorder {
            name: "fuzz_replay".into(),
            version: "0".into(),
        },
    };
    let inputs = (0..ticks)
        .map(|t| PlayerInput {
            forward: ((t % 255) as i16 - 127) as i8,
            strafe: -((t % 127) as i8),
            yaw: (t * 977) as u16,
            pitch: (t % 16_385) as i16,
            buttons: (t % 4) as u8,
        })
        .collect();
    let checkpoints = checkpoint_ticks(ticks, interval)
        .into_iter()
        .map(|tick| Checkpoint {
            tick,
            hash: StateHash(*ContentId::of(&tick.to_le_bytes()).as_bytes()),
        })
        .collect();
    Replay {
        header,
        inputs,
        checkpoints,
    }
    .to_bytes()
    .expect("valid synthetic replay")
}

fn corpus() -> &'static Vec<Vec<u8>> {
    static CORPUS: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    CORPUS.get_or_init(|| {
        let mut out: Vec<Vec<u8>> = fuzzing::fixture_files(&["fixtures/replays"])
            .into_iter()
            .map(|(_, bytes)| bytes)
            .collect();
        assert!(!out.is_empty(), "fixtures/replays is missing");
        let spawn = Spawn {
            position_mm: [1, 2, 3],
            yaw: 0,
        };
        let far = Spawn {
            position_mm: [MAX_SAFE_INTEGER, -MAX_SAFE_INTEGER, 0],
            yaw: u16::MAX,
        };
        let max_seed = MAX_SAFE_INTEGER as u64;
        out.push(synthetic(1, 1, 0, spawn));
        out.push(synthetic(1, 3600, max_seed, far));
        out.push(synthetic(3, 3600, 7, spawn));
        out.push(synthetic(61, 60, 1, far));
        out.push(synthetic(120, 7, max_seed, spawn));
        out
    })
}

// -------------------------------------------------------------- mutations --

/// The header and everything after it, if the header length fits.
fn split(replay: &[u8]) -> Option<(&[u8], &[u8])> {
    let len = u32::from_le_bytes(replay.get(12..16)?.try_into().ok()?) as usize;
    Some((replay.get(16..16 + len)?, replay.get(16 + len..)?))
}

/// A replay as parts for shrinking: its header and everything after it.
fn replay_split(replay: &[u8]) -> Option<Vec<Vec<u8>>> {
    let (header, body) = split(replay)?;
    Some(vec![header.to_vec(), body.to_vec()])
}

fn replay_join(parts: &[Vec<u8>]) -> Vec<u8> {
    let part = |k: usize| parts.get(k).map_or(&[][..], Vec::as_slice);
    pack(part(0), part(1))
}

fn pack(header: &[u8], body: &[u8]) -> Vec<u8> {
    let mut out = format::MAGIC.to_vec();
    out.extend_from_slice(&format::FILE_VERSION.to_le_bytes());
    out.extend_from_slice(&(header.len() as u32).to_le_bytes());
    out.extend_from_slice(header);
    out.extend_from_slice(body);
    out
}

/// Valid inputs and checkpoints for what `header` says, when it says
/// something small enough to build.
fn body_for(header: &[u8]) -> Option<Vec<u8>> {
    let v: Value = serde_json::from_slice(header).ok()?;
    let ticks = v.get("ticks")?.as_u64().filter(|t| *t <= 20_000)?;
    let interval = v.get("checkpoint_interval")?.as_u64().filter(|i| *i >= 1)?;
    let mut body = Vec::new();
    for t in 0..ticks {
        body.extend_from_slice(
            &PlayerInput {
                forward: (t % 100) as i8,
                ..PlayerInput::default()
            }
            .to_bytes(),
        );
    }
    let at = checkpoint_ticks(ticks, interval);
    body.extend_from_slice(&(at.len() as u32).to_le_bytes());
    for tick in at {
        body.extend_from_slice(&tick.to_le_bytes());
        body.extend_from_slice(&[0xab; 32]);
    }
    Some(body)
}

fn ticks_of(header: &[u8]) -> Option<usize> {
    let v: Value = serde_json::from_slice(header).ok()?;
    usize::try_from(v.get("ticks")?.as_u64()?).ok()
}

fn set_u32(out: &mut [u8], at: usize, rng: &mut Rng) {
    if let Some(field) = out.get(at..at + 4) {
        let current = u32::from_le_bytes(field.try_into().expect("4 bytes"));
        let v = fuzzing::interesting_u32(rng, out.len(), current);
        out[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
}

fn mutate_replay(rng: &mut Rng, corpus: &[Vec<u8>]) -> Vec<u8> {
    let base = rng.pick(corpus);
    let donor = rng.pick(corpus);
    let Some((header, body)) = split(base) else {
        return base.clone();
    };
    match rng.below(10) {
        0..=3 => {
            // The header as JSON, written back canonically; the body often
            // rebuilt to match, so that the later checks run.
            let value = serde_json::from_slice::<Value>(header).ok();
            let donor_header = split(donor).map_or(header, |(h, _)| h);
            let donor_value = serde_json::from_slice::<Value>(donor_header).ok();
            let new = fuzzing::mutate_json(
                rng,
                header,
                value.as_ref(),
                donor_header,
                donor_value.as_ref(),
            );
            let body = match rng.chance(60).then(|| body_for(&new)).flatten() {
                Some(rebuilt) => rebuilt,
                None => body.to_vec(),
            };
            pack(&new, &body)
        }
        4 | 5 => {
            // Length and count fields: the header length, the checkpoint
            // count and the checkpoint ticks.
            let mut out = base.clone();
            let count_at = 16 + header.len() + ticks_of(header).unwrap_or(0) * 8;
            match rng.below(3) {
                0 => set_u32(&mut out, 12, rng),
                1 => set_u32(&mut out, count_at, rng),
                _ => {
                    let k = rng.below(8);
                    let at = count_at + 4 + 40 * k;
                    if let Some(field) = out.get(at..at + 8) {
                        let current = u64::from_le_bytes(field.try_into().expect("8 bytes"));
                        let v = fuzzing::interesting_u64(rng, current);
                        out[at..at + 8].copy_from_slice(&v.to_le_bytes());
                    }
                }
            }
            out
        }
        6 => {
            // One input record: reserved byte, axis -128, pitch beyond a
            // quarter turn, unknown buttons.
            let mut out = base.clone();
            let ticks = ticks_of(header).unwrap_or(0);
            if ticks > 0 {
                let at = 16 + header.len() + 8 * rng.below(ticks);
                let (offset, value) = *rng.pick(&[
                    (7, 1u8),
                    (0, 0x80),
                    (1, 0x80),
                    (5, 0x40),
                    (5, 0xbf),
                    (6, 4),
                    (6, 0xff),
                ]);
                if let Some(b) = out.get_mut(at + offset) {
                    *b = value;
                }
            }
            out
        }
        _ => {
            let mut out = base.clone();
            fuzzing::mutate_bytes(rng, &mut out, donor);
            out
        }
    }
}

fn check_replay(bytes: &[u8]) {
    if let Ok(r) = Replay::from_bytes(bytes) {
        r.check().expect("an accepted replay is valid");
        assert!(r.to_bytes().expect("an accepted replay re-encodes") == bytes);
        assert_eq!(r.inputs.len() as u64, r.header.ticks);
        let ticks: Vec<u64> = r.checkpoints.iter().map(|c| c.tick).collect();
        assert_eq!(
            ticks,
            checkpoint_ticks(r.header.ticks, r.header.checkpoint_interval)
        );
    }
}

#[test]
fn replay_reader() {
    let c = corpus();
    fuzzing::run(
        Target::new("replay", 30_000)
            .memory(64, 8 << 20)
            .parts(replay_split, replay_join),
        |rng| mutate_replay(rng, c),
        check_replay,
    );
}

// ------------------------------------------------------------- regressions --
//
// Inputs that once broke the reader, minimised. Each must stay an error.

/// A valid header claiming the most ticks a replay may have (one day at
/// 60 Hz), and nothing after it. The reader used to reserve memory for
/// every claimed input record (41 MB) before finding the file ends.
const MAX_TICKS_NO_RECORDS: &[u8] = b"SWREPLAY\x01\x00\x00\x00M\x01\x00\x00{\"checkpoint_interval\":3600,\
        \"contract_version\":\"0.1.0\",\"format\":\"streetwide.replay\",\
        \"recorder\":{\"name\":\"r\",\"version\":\"0\"},\"seed\":0,\
        \"sim_contract_version\":\"0.2.0\",\"spawn\":{\"position_mm\":[\
        0,0,0],\"yaw\":0},\"tick_hz\":60,\"ticks\":5184000,\"world_id\":\"blake3:af1349b9f5f9a1a6a0404dea36dcc9499bcb\
        25c9adc112b7cc9a93cae41f3262\",\"world_name\":\"w\"}";

#[test]
fn regression_replay_ticks_claimed_but_missing() {
    assert_eq!(MAX_TICKS_NO_RECORDS.len(), 349);
    // Where memory can be measured (Linux), the read runs alone in a child
    // process and may take at most 1 MiB beyond what the process held: the
    // reservation took 41 MB and gave the same error.
    fuzzing::run(
        Target::new("replay-max-ticks", 1)
            .exactly(1)
            .memory(0, 1 << 20),
        |_| MAX_TICKS_NO_RECORDS.to_vec(),
        |bytes| match Replay::from_bytes(bytes) {
            Err(op_replay::ReplayError::Malformed(m)) => assert_eq!(m, "unexpected end of file"),
            other => panic!("expected a malformed replay, got {other:?}"),
        },
    );
}

/// The largest valid replay file, which loaders refuse to go beyond.
#[test]
fn the_largest_replay_follows_from_the_limits() {
    let ticks = format::MAX_TICKS as usize;
    // Header, input records, checkpoint count, a checkpoint at every tick.
    assert_eq!(
        format::MAX_REPLAY_BYTES as usize,
        16 + 64 * 1024 + 8 * ticks + 4 + 40 * ticks
    );
    // A replay with a checkpoint at every tick is as long as that formula
    // says, for its own tick count and header.
    let bytes = synthetic(
        100,
        1,
        0,
        Spawn {
            position_mm: [0, 0, 0],
            yaw: 0,
        },
    );
    let (header, _) = split(&bytes).expect("a replay");
    assert_eq!(bytes.len(), 16 + header.len() + 8 * 100 + 4 + 40 * 100);
}
