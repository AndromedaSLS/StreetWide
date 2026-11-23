# Replay playback controls: pause and speed

Labels: `good first issue`, `area: client`

## Why

`op_client --replay <file>` plays a replay in real time from start to end.
Investigating a bug report means watching it all, with no way to pause or
skip ahead.

## What to do

In playback mode only:

- **P** pauses and resumes.
- **1 / 2 / 4** set the speed to 1×, 2× or 4×. Run more simulation ticks
  per fixed step, or change the fixed clock's rate (`Time<Fixed>`).
- **The HUD** shows the speed, the time position and the total length.

The simulation itself must not change. The same ticks run in the same
order, only faster or slower in wall-clock time, so every checkpoint must
still match.

## Done when

- Pausing, speeding up and slowing down work, and playback at 4× still
  ends with "Replay matches every checkpoint".
- `cargo run -p op_client -- --replay fixtures/replays/sample.oprep
  --exit-when-done` still exits with code 0.
- Clippy and tests pass.

## Pointers

- `crates/op_client/src/session.rs` (`Session::tick`, `SessionPlugin`),
  `crates/op_client/src/hud.rs`
- ADR-0007, ADR-0008
