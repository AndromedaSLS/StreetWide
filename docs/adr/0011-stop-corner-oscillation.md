# ADR-0011: Stop the player oscillating in concave corners

- **Status:** Accepted (2026-11-25)
- **Date:** 2026-11-23
- **Deciders:** @victorpicaud
- **Supersedes / superseded by:** none (addresses a known limitation of
  ADR-0004)

## Context

ADR-0004 moves the player with Rapier's `KinematicCharacterController`
and records a known limitation: pushing into an unclimbable concave
corner can make the capsule oscillate by about 2 cm from tick to tick.

The fixture world has such a corner at the top of the lane east of the
Grossmünster, where a building's wall meets terrain of about 57 degrees,
steeper than the walkable 45. Sprinting into it, the controller slides
along one surface in one tick and along the other in the next. The
player's horizontal move then reverses direction 174 times in 200 ticks:
- the camera shakes;
- the player stands at a different spot every tick;
- recorded walks that end in a corner carry this noise in their state.

The oscillation is deterministic, so it does not break replays, but a
fix changes state hashes. It therefore needs a new
`SIM_CONTRACT_VERSION` (ADR-0003) and re-recorded replays.

What we know about the cause:
- A single wall or slope never produces it. Sliding projects the input
  onto the surface, which never points against the input.
- Two vertical walls don't produce it either: Rapier resolves them
  steadily.
- It takes a wall plus ground too steep to climb. Rapier's slope handling
  then fights the wall. On a flat synthetic slope it happens a few times
  per 200 ticks; on real, triangulated terrain, almost every tick.

## Options considered

### A. Never move against the input (recommended)

After the controller resolves a tick, check its horizontal move. If the
move points more than about 91 degrees away from the input's horizontal
direction, resolve the tick again with only the vertical part of the
input. The player then stays put horizontally, but still falls, lands and
collides. An exception keeps real slides: on ground too steep to stand
on, a move that descends is left alone.

- Pros:
  - small: one check after the controller, and a second shape cast only
    in the ticks it fires;
  - every applied move is still resolved by the controller, so nothing
    can end up inside geometry;
  - removes the oscillation in the fixture corner: 174 reversals → 0;
  - leaves single-surface sliding exactly as it was, by construction (see
    Context), and tested.
- Cons:
  - a rule on top of Rapier, which future movement must respect. Moving
    platforms or pushers would need to apply their push separately.
  - It suppresses the symptom, not Rapier's cause, so other kinds of
    corner may need another look.

### B. Tune the controller's parameters

Change the skin, the normal nudge, the slope limits or the slide setting.

- Pros: no code of our own.
- Cons:
  - each parameter changes every walk, not only corners;
  - no setting is known to remove the oscillation; finding one is
    trial and error against one corner.

### C. Smooth it in the client only

Low-pass filter the rendered camera position.

- Pros: no simulation change, no contract bump, replays stay valid.
- Cons:
  - the simulated state still oscillates, which also affects the gate's
    replay check (G10) and anything built on the player's position later;
  - every client must repeat the filter;
  - the camera lags.

### D. Write our own move-and-slide

Replace Rapier's controller with a controller that clips the move against
all contact planes at once, which resolves corners exactly.

- Pros: full control, and it fixes the cause.
- Cons:
  - weeks of work, with step-up, slopes and ground snapping to redo and
    re-test;
  - it gives up the reason for choosing Rapier's controller in ADR-0004.

## Decision

*Accepted on 2026-11-25 by @victorpicaud:* option A, the recommended
option, as `SIM_CONTRACT_VERSION` 0.2.0. It removes the oscillation
with the smallest change to the simulation, keeps every move
collision-checked, and does not change how the player walks anywhere
else. Option D stays open if more corner cases turn up.

The rule, in `op_sim::player`:
- the move counts as "against the input" when the dot product of the two
  horizontal vectors is below −0.02 × their lengths' product (about 91
  degrees). The margin avoids firing on rounding noise at right angles.
- A slide down ground too steep to stand on is exempt only when it
  actually descends: Rapier also reports such a slide while the capsule
  pushes up and back in the corner.

## Consequences

- **Version:** the simulation contract is now 0.2.0. Replays recorded with
  0.1.0, including any walks recorded with the client before this change,
  are refused with a message naming both versions (ADR-0007).
- **Sample replay:** `fixtures/replays/sample.oprep` is re-recorded. The
  walk still lands, runs into the wall (86 ticks blocked, was 88), jumps
  (42 ticks airborne) and climbs the lane (2.47 m, was 2.45). Its final
  state is now `212bd353…`.
- **Tests:**
  - `op_replay`'s `corners` test pushes into the fixture corner for 10
    seconds. It asserts no reversal in the last 200 ticks, then that the
    player walks more than 2 m out of the corner.
  - `op_sim`'s synthetic tests add a wall beside a 60° slope (6 reversals
    without the rule, 0 with it), a wedge of two walls, and sliding along
    a single wall at 45°, which must keep its speed.
- **ADR-0004:** its known limitation about corners is addressed by this
  ADR. The limitation about internal mesh edges remains.
- **Revisit** when moving platforms or other pushes arrive, or if a corner
  is found where the player sticks or still shakes.
