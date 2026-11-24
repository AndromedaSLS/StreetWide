# ADR-0013: Travel in the client by starting a new session at the destination

- **Status:** Proposed
- **Date:** 2026-11-24
- **Deciders:** @victorpicaud
- **Supersedes / superseded by:** none (extends ADR-0008)

## Context

Getting from one end of the district to the other on foot takes minutes,
which slows down testing, checking contributions and simply looking
around. Testers need to:
- fly freely;
- see the whole area;
- jump to a named place or any point.

The simulation is deterministic (ADR-0003). A replay is a spawn point,
a seed and the inputs (ADR-0007); walking speed and every other physics
parameter are part of the simulation contract (ADR-0004). How travel
interacts with the simulation is therefore a real choice.

## Options considered

### A. Camera-only travel, and arrival as a new session (recommended)

- Flying and the map move only the camera; the player stands still.
- Arriving somewhere (a named place, a click on the map, landing from
  flight) starts a fresh simulation session at the destination, exactly
  as if the client had been started there.

- Pros:
  - no change to the simulation, its contract or the replay format;
  - every session, and every replay saved from one, starts at its own
    spawn point and checks with `op_replay` as before.
- Cons:
  - a replay covers only the walk since the last arrival;
  - flying cannot be recorded.

### B. Teleport and noclip as simulation inputs

New input buttons or records that move the player.

- Pros: one replay can cover a whole session, jumps included.
- Cons:
  - changes the input encoding, the replay format and the simulation
    contract;
  - replays could then contain moves no walker could make. The
    contribution gate replays walks (G10, ADR-0009), so these would need
    to be ruled out there.

### C. A faster walking speed for testing

- Pros: simple.
- Cons:
  - a physics parameter, so it changes the simulation contract;
  - walks recorded with it say nothing about real walking;
  - still slow across a kilometre.

## Decision

Option A.

**The two frames:**
- The client keeps a render frame that stays fixed for the run, so the
  scene doesn't move when the simulation's origin does.
- `Session::teleport` builds the new session: simulation, input log and
  hash trail.

**Where an arrival lands:**
- **Named places and the map:** on open ground (terrain, streets,
  squares, bridges). If the point is inside a building, the nearest open
  ground within 40 m is used, then up to 4 m further from the building.
- **Landing from flight:** on the highest surface below the camera, roof
  or not.

**Where named places come from:** the world definition,
`worlds/<name>.json` under `places`, with positions taken from
OpenStreetMap and credited in ATTRIBUTION.md.

**During replay playback:** flying and the map work for watching, and
arrivals are off.

## Consequences

- **Replays:**
  - F9 saves the session since the last arrival;
  - the replay header holds the arrival point as its spawn;
  - a test checks that such a replay passes `op_replay check`.
- **No determinism impact:** the sample replay's final state is
  unchanged.
- **Command line:** `--place`, `--at`, `--time`, `--date`, `--view` and
  `--details` make it possible to start anywhere, including for
  scripted screenshots.
- **Revisit** if long recordings across several areas become important,
  e.g. for regression walks through a whole city (option B).
