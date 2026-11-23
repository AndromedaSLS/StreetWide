# ADR-0008: Host the fixed-step simulation in Bevy and stream one shared mesh cache

- **Status:** Accepted (2026-11-23, stage 6 checkpoint)
- **Date:** 2026-11-23
- **Deciders:** @victorpicaud
- **Supersedes / superseded by:** none

## Context

The desktop client (`op_client`, Bevy 0.20) must:
- let a player walk the world in first person;
- record sessions that `op_replay check` reproduces bit for bit
  (ADR-0003, ADR-0007);
- stream geometry so it scales beyond one district.

Rendering may be nondeterministic. The simulation may not, and it must not
depend on Bevy (ADR-0002).

## Decisions and options considered

### 1. Running the simulation

- **A. `op_sim` inside Bevy's `FixedUpdate` at 60 Hz, with interpolated
  rendering** (chosen):
  - `Time<Fixed>` is set to exactly 60 Hz (Bevy's default is 64).
  - Each fixed step runs one simulation tick.
  - The camera interpolates between the last two simulated positions,
    using the fixed clock's overstep fraction.
- **B. The simulation on its own thread with a channel to Bevy.** It
  isolates frame hitches, but adds synchronisation for little gain at this
  scale.
- **C. One tick per rendered frame.** The simulation speed would depend on
  the frame rate.

### 2. Input

- Devices are sampled every frame and the latest state is latched for the
  next tick.
- A jump pressed between two ticks is kept until a tick consumes it, so
  quick taps are never lost at high frame rates.
- Mouse look turns the camera every frame at full precision. The
  simulation gets the heading quantised to 1/65536 turn, which is what the
  replay records.

### 3. Geometry streaming

- **A. One mesh cache for rendering and physics** (chosen). `op_stream`'s
  `Streamer` has a pool of loader threads that decode meshes into an LRU
  cache.
  - The renderer requests everything within 600 m of the camera, nearest
    first, and builds Bevy meshes from the cache. It unloads assets beyond
    700 m.
  - The simulation's `StreamSource` reads colliders from the same cache.
    If a mesh it needs is still loading, the tick stalls without changing
    state (ADR-0003). The HUD then shows "Loading…".
- **B. Separate loaders for rendering (Bevy assets) and physics.** Two code
  paths could disagree about geometry, and Bevy's asset system would sit in
  the physics path.
- **C. Load the whole world at start.** Fine for one district, impossible
  for a planet.

### 4. Recording

- **F9 saves the session so far.** The client keeps the complete input log
  and the hash every 60 ticks from the session start; F9 writes them as a
  replay. A replay must start from a fresh simulation at a spawn point, so
  "start recording now" would need a new session. R does exactly that.
- **Playback.** `--replay <file>` plays a replay back in real time and
  checks every checkpoint. With `--exit-when-done` and `--screenshot`, the
  client can be scripted (exit code 0 if everything matched).

### 5. Credits on screen

The world manifest's `attribution` is always shown. Bevy's built-in font
covers ASCII only. The client therefore loads a common system font (Segoe
UI, Arial, DejaVu Sans or Noto Sans) and otherwise writes "(c)" for "©".

Alternatives:
- **Ship a font:** adds a binary asset and its licence to the repository.
- **Bevy's `system_font_discovery`:** adds platform font back-ends to every
  build.

## Consequences

- **Tested:**
  - streaming reproduces the sample replay's hash trail, with a tiny cache
    that forces evictions and stalls;
  - a live session saved the F9 way passes `op_replay check`;
  - the client plays the sample replay in a window and matches every
    checkpoint.
- **Known gaps:**
  - no levels of detail beyond 600 m;
  - one tile list for the whole world (fine for the 0.1 single-region
    format);
  - the simulation shares the main thread with rendering.
- **Revisit when:** worlds span several regions (origin rebasing,
  RFC 0001), or the simulation's cost per tick competes with rendering.
