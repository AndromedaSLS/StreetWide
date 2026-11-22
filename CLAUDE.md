# StreetWide

Open-source, planet-scale open world built and improved by its players.
An AI world compiler produces a baseline from open data; contributions merge
into the canonical world only if they beat what is there.

## Principles (do not trade these away)
- The canonical world only moves upward: every merge must pass checks and
  beat the current version.
- The simulation is deterministic. Rendering may not be.
- Contracts before features: world format, module interfaces and tests come
  first.
- Every asset carries provenance and licence in its manifest.
- Private by design: interiors are fictional, never replicas of real homes.

## Architecture rules
- Crates: op_kernel (module API, schedule), op_sim (deterministic simulation),
  op_replay, op_world (format and hashing), op_stream, op_client (Bevy app).
- Dependencies point inward. op_sim depends only on op_kernel. Nothing
  depends on op_client. The simulation never depends on rendering.
- Bevy and Rapier versions are pinned in the workspace Cargo.toml.

## Determinism rules (simulation code)
- Fixed 60 Hz timestep. No wall-clock time.
- Randomness only from a seeded RNG whose seed is stored in the replay.
- No iteration over HashMap/HashSet; use BTreeMap or IndexMap.
- No platform trig/transcendental functions; use the project's deterministic
  math helpers.
- Never weaken or skip a determinism test. Find the cause.

## Commands
- Build and run: cargo run -p op_client
- Tests: cargo test --workspace
- Lint: cargo fmt --check && cargo clippy --workspace -- -D warnings
- Determinism: cargo run -p op_replay -- check fixtures/replays/sample.oprep
- Ingest: python -m tools.ingest --bbox <minlon,minlat,maxlon,maxlat>

## Working rules
- Commit with sign-off: git commit -s
- One decision record per foundational choice in docs/adr/.
- Anything not covered by an ADR or RFC: draft options, don't decide silently.
- No large data in Git; download and cache via tools/ingest.
- Credit every data source in ATTRIBUTION.md.
- Update docs/progress.md at every checkpoint.