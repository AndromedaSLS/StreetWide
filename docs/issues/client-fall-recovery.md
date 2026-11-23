# Bring the player back after falling off the world

Labels: `good first issue`, `area: client`

## Why

The built area ends at its bounding box. Walking past the edge, the player
falls forever until they press R, which is easy to miss.

## What to do

In the client only (`crates/op_client`), not the simulation:

- When the player's feet are more than 50 m below the lowest point of the
  world's terrain, restart the session at the spawn point. Do it the same
  way the R key does (`Session::restart`), and show a message such as
  "You left the world; back at the spawn point".
- The lowest terrain point can come from the world's tile bounds:
  `WorldManifest::tiles[*].bounds_mm` plus the tile origins.

This stays outside the deterministic simulation. Restarting begins a new
session and a new replay, so saved replays remain valid.

## Done when

- Walking off the edge brings you back to the spawn point within a few
  seconds, with a message.
- A unit test covers the "below the world" computation.
- `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo test --workspace` pass.

## Pointers

- `crates/op_client/src/session.rs` (`restart`),
  `crates/op_client/src/controls.rs` (the R key), `crates/op_client/src/hud.rs`
- ADR-0008
