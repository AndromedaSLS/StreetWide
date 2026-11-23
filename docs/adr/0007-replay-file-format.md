# ADR-0007: Record replays as inputs plus a hash trail (`.oprep`)

- **Status:** Accepted (2026-11-23, stage 4 checkpoint)
- **Date:** 2026-11-23
- **Deciders:** @victorpicaud
- **Supersedes / superseded by:** none

## Context

Replays are StreetWide's determinism test and, later, its bug reports and
contribution evidence. A replay must:

- reproduce a session exactly;
- tell *where* a re-run diverges;
- pin the world and simulation it was made with;
- stay small enough to commit as fixtures.

## Options considered

- **A. Inputs plus a hash trail** (chosen). Record one quantised input per
  tick, plus the state hash every 60 ticks and at the end. Re-running
  proves determinism and locates a divergence to within one second. It
  stays small: about 9 bytes per tick, or 10.8 KB for the 20 s fixture.
- **B. State snapshots.** Large, and they don't test determinism; they
  only replay it.
- **C. A hash every tick.** 32 bytes per tick for one-tick precision; the
  first divergent second can be bisected by re-running instead.

**Header encoding.** Options were canonical JSON (RFC 0001 §5) or a binary
struct. Canonical JSON is chosen: it is self-describing, has a JSON Schema
(`schemas/replay-header.schema.json`), and readers in other languages can
reuse the world-format code.

## Decision

Option A, with a canonical-JSON header. Layout (little-endian):

```text
"SWREPLAY" | u32 file version (1) | u32 header length | header (canonical JSON)
| ticks × 8-byte input | u32 checkpoint count | checkpoints × (u64 tick, 32-byte state hash)
```

- **Header fields:**
  - `format` is `"streetwide.replay"`, `contract_version` is `"0.1.0"`;
  - `sim_contract_version`;
  - `world_id` (RFC 0001), `world_name`;
  - `seed` (≤ 2^53−1);
  - `spawn` (region-frame millimetres and heading);
  - `tick_hz` (60), `ticks`;
  - `checkpoint_interval` (60);
  - `recorder` `{name, version}`.
- **Input record:** forward `i8`, strafe `i8`, yaw `u16`, pitch `i16`,
  buttons `u8`, and a reserved byte that must be 0.
- **Checkpoints:** at every multiple of the interval, and at the last
  tick. The hash after tick *t* is `Schedule::state_hash` after *t* ticks.
- **Strict reading:** files are read strictly. Magic, version, canonical
  header, valid inputs, exact checkpoint ticks and no trailing bytes are
  all checked, so every file has exactly one encoding.
- **World lookup:** a replay names its world by id. `op_replay` finds it
  with `--world <dir>`, or else in `fixtures/world` or
  `data/world/<world_name>`, and refuses a store with another id.
- **Version check:** replays recorded with another `sim_contract_version`
  are refused with a clear message, never re-simulated with different
  rules.

**CLI:** `op_replay check | run | info | trace | synth`. `synth` records
the scripted sample walk (`fixtures/replays/sample.oprep`). The walk lands,
runs into a wall, jumps and climbs a lane, and a test checks that each of
these happens.

## Consequences

- The client (stage 5) records live sessions with the same format.
  Pressing F9 writes `replays/*.oprep`, which `op_replay check` can verify.
- Changes to the layout or header bump the file version or
  `contract_version`, and go through an RFC.
- Re-recording fixtures is an explicit `op_replay synth` step, and the test
  that the sample regenerates byte for byte fails until it is done.
