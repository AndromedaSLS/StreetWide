# ADR-0004: Use Rapier's kinematic character controller directly in op_sim

- **Status:** Accepted (2026-11-23, stage 4 checkpoint)
- **Date:** 2026-11-23
- **Deciders:** @victorpicaud
- **Supersedes / superseded by:** none

## Context

The first playable slice needs first-person walking with collision:
- streets and squares;
- bridges over the Limmat;
- stairs and kerbs;
- slopes such as the lane up to the Zwingliplatz;
- walls that stop you.

The simulation must be deterministic (ADR-0003) and must not depend on
rendering (ADR-0002). Rapier 0.36 is pinned (ADR-0005).

## Options considered

### Integration

- **A. `rapier3d` directly in `op_sim`** (chosen). `op_sim` owns the
  physics world; the kernel feeds it geometry; Bevy never sees it.
- **B. `bevy_rapier`.** Convenient, but it puts the simulation inside
  Bevy's ECS and schedule, against ADR-0002, and ties physics upgrades to
  Bevy upgrades.

### Player body

- **A. Kinematic capsule moved by `KinematicCharacterController`**
  (chosen). The controller does shape casts with sliding, auto-stepping,
  slope limits and snapping to the ground. Movement is fully decided by
  inputs and geometry, with no solver tuning.
- **B. Dynamic rigid body pushed by forces.** Feels floaty, slides down
  slopes, and needs friction and damping tuning; its behaviour depends on
  the solver.

### Which geometry collides

- **A. One trimesh collider per asset with `collider: trimesh`, in the
  active set around the player** (chosen). These are terrain, building
  shells and bridge decks; road ribbons are visual only.
- **B. All assets of all loaded tiles.** Simpler, but the physics world
  grows with streaming distance.

## Decision

All options A. Parameters (all constants in `op_sim::player`):

| Parameter | Value |
|---|---|
| Capsule | radius 0.3 m, height 1.8 m |
| Walk / sprint speed | 1.4 / 4.0 m/s |
| Jump | 3.5 m/s take-off (about 0.6 m) |
| Gravity | 9.81 m/s², terminal speed 50 m/s |
| Autostep | up to 0.3 m, needs 0.2 m of free space |
| Steepest walkable slope | 45° |
| Snap to ground | 0.3 m |
| Skin (controller offset) | 0.02 m |
| Active set | assets whose bounds come within 160 m (per axis) of the capsule centre |

**Movement.** Heading 0 faces north (−Z) and the right side is east (+X).
Diagonal input is normalised. Vertical speed integrates gravity while
airborne, and is zeroed on landing and when hitting a ceiling.

**Active set.** It is re-evaluated before every tick. Leaving colliders
are removed and new ones added, both in `AssetKey` order. The broad phase
is then refreshed with `detect_collisions`. Colliders are built during the
prepare phase, so a stall never half-applies a change.

## Consequences

- **Tests:** physics behaviour is tested on a synthetic world (landing,
  walking speed, walls, a 20° ramp, a 0.2 m kerb, jump height). The real
  fixture replay walks, hits a wall, jumps and climbs 2.4 m.
- **Version bumps:** changing any parameter changes state hashes, so it
  needs a `SIM_CONTRACT_VERSION` bump and re-recorded replays.
- **Known limitation:** pushing into an unclimbable concave corner can make
  the capsule oscillate by about 2 cm between two surfaces from tick to
  tick. It is deterministic. Smoothing it is a good first issue.
  (Addressed by [ADR-0011](0011-stop-corner-oscillation.md).)
- **Known limitation:** trimeshes have no internal-edge fixing, which costs
  sub-millimetre sideways drift along mesh edges.
- **Later:** the client may want a camera collider, swimming once water
  exists, and moving platforms. Each is a new module or parameter set
  behind a contract version bump.
