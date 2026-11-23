# ADR-0003: Make the simulation bit-identical on every platform

- **Status:** Accepted (2026-11-23, stage 4 checkpoint)
- **Date:** 2026-11-23
- **Deciders:** @victorpicaud
- **Supersedes / superseded by:** none

## Context

CLAUDE.md: "The simulation is deterministic. Rendering may not be." The
same world, seed and inputs must produce the same state, bit for bit, on
every platform and every build, so that:

- replays can be checked (`op_replay check`);
- a contribution's effect on the world can be reproduced by anyone;
- networking can later exchange inputs instead of state.

The usual sources of divergence are:

- wall-clock time and variable frame rates;
- unseeded randomness;
- iteration over hash maps;
- platform math libraries (`sin`, `exp`, … differ in the last bits
  between operating systems);
- fused multiply-add and SIMD paths chosen per CPU;
- compiler or engine upgrades.

## Options considered

- **A. IEEE floats with discipline** (chosen). f32 arithmetic is exactly
  rounded by IEEE 754 (`+ - * /` and `sqrt`). Everything else is
  controlled: no platform transcendental functions, no FMA contraction
  (Rust never contracts on its own), physics with scalar math, pinned
  toolchain and engine.
- **B. Fixed-point simulation.** Deterministic by construction, but Rapier
  and every physics library would have to be replaced or rewritten.
- **C. Lockstep checks only at runtime.** Detects divergence but does not
  prevent it; useless for single-player replays.

## Decision

Option A, with these rules (enforced where a tool can enforce them):

1. **Fixed 60 Hz tick** (`op_kernel::TICK_HZ`, `TICK_DT = 1/60`). No
   wall-clock time in simulation crates (`Instant` and `SystemTime` are
   banned by `clippy.toml`). The client runs the simulation in fixed steps
   and interpolates for rendering.
2. **Integer inputs.** `PlayerInput` is quantised: movement axes `i8`,
   heading `u16` in 1/65536 turns, pitch `i16`, button bits. That is
   exactly what a replay records.
3. **One seeded RNG** (`op_kernel::Rng`, xoshiro256** seeded by
   SplitMix64). The seed is stored in the replay, and the RNG state is
   part of the state hash. Unseeded `rand` functions are banned.
4. **Ordered containers.** `HashMap`, `HashSet` and `RandomState` are
   banned; use `BTreeMap` or sorted `Vec`s. Colliders are added and
   removed in `AssetKey` order. A geometry source may return assets in any
   order; the simulation sorts them.
5. **Deterministic math.** `f32`/`f64` `sin`, `cos`, `tan`, `atan2`,
   `exp`, `ln`, `powf`, `powi`, `hypot` and the like are banned. Simulation
   code uses `op_kernel::dmath` (pure-Rust `libm`), and quarter turns are
   exact constants.
6. **Physics with `enhanced-determinism`.** Rapier (and its math stack)
   use scalar code and libm (ADR-0004, ADR-0005).
7. **Integer geometry hand-off.** World geometry reaches the simulation
   as integer millimetres in a sim frame. The sim frame is the region
   frame shifted by an integer origin at the spawn point, which keeps f32
   coordinates small. Each value is converted to f32 metres in exactly one
   way: the f32 nearest to `mm / 1000` in f64.
8. **Stalls change nothing.** Each tick first prepares (geometry may
   still be streaming) and only then steps. A tick whose geometry is
   pending stalls without touching state or advancing the tick counter.
9. **State hash every tick.** BLAKE3 over a versioned, explicit encoding
   (`streetwide.state.v1`):
   - the tick count;
   - the RNG state;
   - each module's state, in schedule order, with floats as bit patterns;
   - the active collider set.
10. **Pinned toolchain and engine** (ADR-0005). Upgrades re-run the
    determinism suite.

**Checks.**

- **Clippy bans** for rules 1, 3, 4 and 5, in every crate except
  `op_client`.
- **Pinned bits:** the Rapier canary digest, libm outputs and the RNG
  sequence.
- **In-process checks:** two simulations in one process give identical
  hash trails, and a run with stalls equals a run without.
- **Cross-platform replay check:** the fixture replay
  (`fixtures/replays/sample.oprep`) is checked on Linux x86_64, Windows
  x86_64 and macOS arm64, in debug and release, in separate processes.

## Consequences

- Simulation code is written with a small vocabulary: `+ - * /`, `sqrt`,
  `min`, `max`, `abs`, and `dmath` for anything else. New code that needs
  a banned function gets a `dmath` wrapper.
- Any change that alters a state hash bumps `op_sim::SIM_CONTRACT_VERSION`
  and re-records the replay fixtures. Old replays then fail with a clear
  version error instead of a silent divergence.
- **Not covered yet:** origin rebasing for long journeys (f32 precision is
  0.06 mm at 1 km from the sim origin). Multithreaded simulation would
  need its own rules.
- **Revisit if** a platform diverges despite these rules. The cause would
  be found and fixed, never the test weakened.
