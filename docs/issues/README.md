# Issue drafts

Ready-to-file descriptions of work that newcomers can pick up. Each is
self-contained: why it matters, what to do, when it is done, and where to
look. None of them changes the deterministic simulation or the world
format, so none needs an RFC.

When one is filed on GitHub, add the link here and delete the draft.

| Draft | Area | Size |
|---|---|---|
| [Show progress while the ingest runs](ingest-progress.md) | ingest (Python) | small |
| [Draw pedestrian squares mapped as areas](ingest-pedestrian-areas.md) | ingest (Python) | medium |
| [Support OSM `building:part` in the fallback](ingest-building-parts.md) | ingest (Python) | small |
| [Bring the player back after falling off the world](client-fall-recovery.md) | client (Rust, Bevy) | small |
| [Replay playback controls: pause and speed](client-replay-controls.md) | client (Rust, Bevy) | small |
| [A minimap](client-minimap.md) | client (Rust, Bevy) | medium |
| [Licence and advisory checks with cargo-deny](ci-cargo-deny.md) | CI | small |
| [Generate a reference page from the JSON Schemas](docs-schema-reference.md) | docs, CI | small |

## Larger known gaps

These need a design discussion first: an RFC, or a simulation contract
bump with re-recorded replays.

- **Water.** The Limmat is walkable terrain at water level. Water needs a
  layer or asset kind that RFC 0001 does not have yet.
- **Corner jitter.** Pushing into some unclimbable concave corners makes
  the capsule oscillate by about 2 cm (ADR-0004). A fix changes the
  physics, so it needs `SIM_CONTRACT_VERSION` 0.2.0 and a re-recorded
  `fixtures/replays/sample.oprep`.
- **Levels of detail** beyond the 600 m streaming radius (RFC 0001
  unresolved questions).
- **Bridges that end at building doors** are anchored to the ground
  (ADR-0006).
