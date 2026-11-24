# RFC 0003: A water layer

- **Status:** Draft
- **Authors:** @victorpicaud
- **Created:** 2026-11-24 (comment period until at least 2026-12-01)
- **Contract version affected:** world format 0.1, amended in place
  (`contract_version` stays `0.1.0`, see "Compatibility and migration");
  contribution contract 0.1, amended in place; gate 0.1.0 → 0.1.1

## Summary

The world format gets an eighth layer, `water`, for the surfaces of
rivers, lakes, canals and ponds:
- **One asset per tile.** Its meshes use the palette material `water`.
- **Visual only.** A water asset must have `collider: none`, so the
  simulation never sees it, and the terrain stays the surface players
  stand on.
- **Not open to contributions.** Errors are fixed in the source data.

This amends [RFC 0001](0001-world-format.md) sections 2 and 3 and
[RFC 0002](0002-contributions-and-merge-rules.md) section 3. Where the
data comes from and how surfaces are placed is an ingest decision,
recorded in [ADR-0014](../docs/adr/0014-water-areas-from-openstreetmap.md).

## Motivation

- **The world has no water.** The Limmat runs through the middle of the
  first district, but the format has nowhere to put water. The terrain
  model carries the river's surface as ground, so the river is drawn as
  ground, and the client's detailed look paves it like a square
  ([ADR-0012](../docs/adr/0012-client-realism-from-procedural-detail.md)).
- **It will come back everywhere.** Water appears in almost every city,
  so a planet-scale world needs one generic way to represent it.
- **The simulation must not change.** Players cannot swim yet. Making
  water solid, or taking the river out of the ground, would change where
  players can walk and every replay near water. Both need their own
  simulation decision (ADR-0004 lists swimming for later).

## Design

### 1. The layer (amends RFC 0001 section 2)

A new row in the layer table:

| Layer | What | Granularity |
|---|---|---|
| `water` | Water surfaces: rivers, lakes, canals, ponds | one asset per tile |

- **Feature id:** `tile:17/{x}/{y}`, the convention for per-tile layers.
  A tile's water therefore comes from one source; see the unresolved
  questions.
- **Sort order:** `water` sorts after `transport` in byte order, so the
  order of existing tile-index entries, and their ids, do not change.

### 2. Collider (amends RFC 0001 section 3)

A water asset must have `"physics": {"collider": "none"}`. The rule is
enforced in four places:
- the asset-manifest schema;
- the tile-index schema, because the simulation reads colliders from tile
  indexes and never opens manifests;
- `op_world`'s manifest and tile-index checks;
- the Python tools.

The terrain under the water stays a trimesh collider and stays the
collision surface. In format 0.1, players walk on the river at water
level, as they do today.

### 3. Mesh

- **Profile:** the StreetWide glb profile 0.1 (RFC 0001 section 6),
  unchanged.
- **Material:** every primitive uses the palette material `water`.
- **What the mesh describes:** the water surface, in the region frame like
  every asset. It may extend under the banks, where the terrain hides it.
- **The terrain:** the water asset never changes the terrain asset of its
  tile. Clients may draw the terrain under the water lower, for display
  only.

### 4. Contributions (amends RFC 0002 section 3)

A new row in the table of layers that accept contributions:

| Layer | Contributions in 0.1 | Collider | Notes |
|---|---|---|---|
| `water` | **no: fix it in the source** | `none` | Water comes from open data (OpenStreetMap today, ADR-0014). The next baseline refresh brings fixes in. |

The gate's layer check G3:
- fails a contributed water asset, with that advice;
- gives a baseline refresh authored by `streetwide:ingest` the result
  `review`, as for `transport`.

The gate's version becomes 0.1.1, because its output changes for change
sets that touch water.

## Options considered

### A. A `water` layer, independent of the source (recommended)

The design above.

- Pros:
  - The simulation is untouched by construction: no collider, no
    state-hash input, no change to any replay's checkpoints.
  - Existing assets keep their bytes.
  - It works with any source: OpenStreetMap today, a national source
    later.
- Cons:
  - A format change.
  - Readers built before it reject worlds that contain water.

### B. A `water` primitive inside each tile's terrain asset

- Pros: no new layer.
- Cons:
  - **Replays:** the terrain content id is hashed into the simulation
    state every tick, so every replay's checkpoints change.
  - **Collision:** water triangles become part of the collider.
  - **Licence:** OpenStreetMap outlines would make the swisstopo terrain
    ODbL, so only swisstopo outlines could be used (see ADR-0014).
  - **Gate:** check G8 treats shorelines as open tile edges.

### C. A `water` primitive inside each tile's transport asset

- Pros: no new layer; the transport assets are already OSM-derived.
- Cons:
  - Water is not transport.
  - Check G9 turns every triangle of a transport asset without a collider
    into a public-way probe point, so the river would count as a street
    unless the gate filtered by material.
  - It ties water to OpenStreetMap for good.

### D. A general land-cover layer (water, squares, green areas)

- Pros: one layer for several kinds of ground.
- Cons: water has its own geometry rules and a future physics role
  (swimming). Paved squares and green areas can get their own layer or
  layers later.

## Compatibility and migration

- **Contract version:** RFC 0001 is a Draft and no world has been
  published, so this RFC amends world format 0.1 in place and keeps
  `contract_version` at `0.1.0`.
  - **Cost:** a reader built before this change fails on a world that
    contains water with an "unknown layer" error, instead of the clean
    "unknown contract version" error RFC 0001 promises.
  - **Alternative, for the maintainer to choose:** bump to `0.2.0`. That
    changes the id of every manifest, tile index and world, including the
    golden ids in `fixtures/format`. It changes no mesh and no replay
    checkpoint.
  - **One decision with RFC 0004:** [RFC 0004](0004-point-features.md)
    amends the same format, and proposes choosing the version once for
    both RFCs.
- **Existing worlds:** a re-ingest adds water assets and new tile indexes,
  so world ids change. Existing assets stay byte-identical. Replays
  recorded in the old world no longer match its id and must be recorded
  again; their checkpoints do not change, because water is not in the
  simulation.

## Determinism, provenance and privacy

- **Determinism:** water assets are never colliders, so they never enter
  the simulation's active set or state hash.
- **Provenance:** every water asset carries its sources and licence like
  any asset. OSM-derived water is licensed
  `ODbL-1.0 AND LicenseRef-swisstopo-OGD` when swisstopo terrain heights
  place it, and stays in its own assets (RFC 0001 section 2).
- **One source per area:** within any area, water stays single-source.
  Mixing OSM water with other water data in one area would bring the other
  data under the ODbL (ADR-0014).
- **Privacy:** no concern; water surfaces show no people or property.

## Tests and checks

- **Schemas and `op_world`:** a water asset with a trimesh collider is
  rejected, in manifests and in tile indexes; one with `none` is accepted.
  The Python tools agree.
- **Gate:** a contributed water change fails G3; a baseline refresh with
  water gets `review`.
- **Fixture world:** it gets a water asset and validates. The Khronos glTF
  validator passes on its mesh.
- **Replays:** the sample replay's checkpoints are unchanged; only its
  header's world id changes.

## Unresolved questions

- **Entering water:** swimming, or keeping players out of rivers. Both
  need a collider kind for water and a simulation contract change.
- **Several sources in one tile:** for example at a national border, or
  if a national water source replaces OpenStreetMap in one country. That
  needs feature ids by source rather than by tile.
- **Gradients and weirs:** one flat level per water body and terrain
  source tile (ADR-0014) cannot model a weir inside a tile.
- **Riverbeds:** no open data covers riverbeds (swissBATHY3D covers lakes
  only).
- **Flow:** the direction of flow, for rendering and later for physics.
