# ADR-0005: Toolchain and engine pinning

- **Status:** Proposed
- **Date:** 2026-11-22
- **Deciders:** @victorpicaud

## Context

Replays must reproduce bit-identical state hashes. Any change in the
compiler, physics engine or math libraries can change floating-point
results and invalidate recorded replays. CLAUDE.md requires Bevy and Rapier
to be pinned in the workspace `Cargo.toml`. Contributors and CI must build
with the same versions.

## Options considered

- **A. Exact pins + committed `Cargo.lock` + pinned toolchain.** Fully
  reproducible; upgrades are deliberate, reviewed PRs.
- **B. Caret requirements (`0.20`) with `Cargo.lock`.** `cargo update` can
  silently move patch versions of the engine.
- **C. Floating `stable` toolchain.** Each new Rust release could change
  codegen without anyone deciding to.

## Decision

Option A:

- `rust-toolchain.toml` pins **Rust 1.100.0** (Bevy 0.20 requires ≥ 1.97.1),
  with `rustfmt` and `clippy`.
- `[workspace.dependencies]` pins **`bevy = "=0.20.1"`** and
  **`rapier3d = "=0.36.1"`** (with `enhanced-determinism`, see ADR-0003).
- `Cargo.lock` is committed. CI builds with `--locked` where it matters.
- Rust edition 2024.
- GitHub Actions are pinned to commit SHAs.

## Consequences

- Upgrading Rust, Bevy or Rapier is a dedicated PR that updates this ADR,
  re-runs the determinism suite and, if hashes change, re-records fixtures
  with an explanation.
- `op_sim`'s Rapier canary test (`crates/op_sim/tests/rapier_canary.rs`)
  holds a golden digest and fails if the pinned engine's results change.
- Contributors get the right toolchain automatically through rustup.
