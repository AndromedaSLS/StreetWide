# ADR-0009: Run the simulation-based gate checks in op_replay

- **Status:** Accepted (2026-11-23)
- **Date:** 2026-11-23
- **Deciders:** @victorpicaud
- **Supersedes / superseded by:** none

## Context

RFC 0002's gate has two checks that need the simulation:

- **G9 walkability:** the player capsule, placed on public ways, must not
  newly collide with anything;
- **G10 replays:** recorded walks must still reach their waypoints in the
  candidate world.

`op_world` implements G1–G8, but it must not depend on `op_sim`
(ADR-0002: `op_world` has no workspace dependencies). Until G9 and G10
exist, no change set that touches colliders can pass the gate.

## Options considered

### A. A gate module and command in `op_replay` (recommended)

`op_replay` already depends on `op_sim`, `op_stream` and `op_world`
(ADR-0002). An `op_replay check-change` command would:
1. run `op_world`'s gate for G1–G8;
2. run G9 and G10 on the candidate world with the real simulation code;
3. write the same report format, with all ten checks.

- Pros:
  - no new crate and no change to the dependency policy;
  - replays (G10) already live in `op_replay`.
- Cons:
  - the crate grows beyond "replays";
  - two commands exist (`op_world check-change` for checks without the
    simulation, `op_replay check-change` for all of them).

### B. A new crate `op_gate`

It would depend on `op_world`, `op_sim`, `op_stream` and `op_replay`.

- Pros: one clearly named home for the whole gate.
- Cons:
  - changes the crate list fixed in CLAUDE.md, ADR-0002 and
    `tools/ci/deps-policy.toml`;
  - one more crate to build and maintain for about 300 lines of code.

### C. Let `op_world` depend on `op_sim`

- Cons: it breaks the dependency direction. The format crate would pull
  in the physics engine, and every tool that reads worlds would compile
  Rapier.

### D. Implement G9 in Python next to the ingest

- Cons: a second physics implementation would not behave bit-for-bit like
  the simulation, which is the point of the check.

## Decision

Option A.

**G9 walkability:**
1. Take the transport ribbon meshes (`collider: none`) within 2 m of the
   changed assets. Use each ribbon triangle's centroid as a probe point,
   which gives points roughly 1 to 2 m apart.
2. At each point, place the player capsule (ADR-0004) with its feet 2 cm
   above the ribbon.
3. The point is **blocked** if the capsule overlaps any collider, terrain
   included, or if rays from its centre in the four horizontal directions
   and straight up all hit something within 30 m. The second test catches
   a capsule standing entirely inside a new closed volume.
4. A probe point free in the base world must still be free in the
   candidate world. Comparing the two worlds means steep streets or
   arcades that already count as blocked never count against a change.

**G10 replays:** the replays registered for the canonical world (their
location is ADR-0010; until then they are passed as `--replay` arguments)
that come within 160 m of the change:
1. Re-run them in the candidate world.
2. Record the player's path every second.
3. The candidate passes if every recorded waypoint is reached within 2 m.
   Hash equality is not expected, since the world changed.

## Implementation

- `op_sim::probe::ProbeWorld` (the player capsule against static
  colliders).
- `op_replay::gate::check_change` and
  `op_replay check-change <world> <bundle> --replay <file>...`.

Tested on the fixture tile:
- removing the house the sample walk runs into fails G10 (5.6 m off the
  path at tick 300);
- removing a building far from the path keeps every waypoint;
- a 3 m block placed on a street fails G9.

## Consequences

- Shell and terrain contributions can now pass the gate.
- G9's probes give contributors a precise answer: the tile and position
  (region frame) where a public way is newly blocked.
- If the gate later needs a home of its own, option B can supersede this
  ADR with no format change.
