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

- **Water.** Now proposed: [RFC 0003](../../rfcs/0003-water-layer.md)
  (Draft) adds a visual-only `water` layer, and
  [ADR-0014](../adr/0014-water-areas-from-openstreetmap.md) (Proposed)
  takes water areas from OpenStreetMap and places them as level surfaces
  that the client draws as water. Still open: players walk on the river at
  water level, because entering water (swimming, or keeping players out)
  needs a collider kind for water and a simulation contract change (RFC
  0003 unresolved questions).
- **Building types.** Churches get the client's house windows: the shells
  carry no building type. A type from OpenStreetMap kept by the ingest
  could choose styles (ADR-0012).
- **Levels of detail** beyond the 600 m streaming radius (RFC 0001
  unresolved questions).
- **Bridges that end at building doors** are anchored to the ground
  (ADR-0006).
