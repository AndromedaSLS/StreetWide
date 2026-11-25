# RFC 0004: Point features: vegetation and street objects

- **Status:** Draft
- **Authors:** @victorpicaud
- **Created:** 2026-11-24 (comment period until at least 2026-12-01)
- **Contract version affected:** world format 0.1, amended in place or
  moved to 0.2.0, in one decision with RFC 0003 (see "Compatibility and
  migration"); contribution contract 0.1, amended in place; gate
  0.1.2 → 0.1.3

## Summary

The world format gets two layers for real objects that open data records
as points: `vegetation` for single trees, and `street_objects` for light
points (benches, bollards or signs later, each by an amendment).
- **Points, not meshes.** An asset in these layers is a canonical-JSON
  `streetwide.points` document, one per tile and source, content-addressed
  like a mesh. Each point keeps the source's own id, an anchor in integer
  millimetres and what the source records. Values set by a rule are marked
  as estimated. The client draws the objects as decoration
  ([ADR-0012](../docs/adr/0012-client-realism-from-procedural-detail.md)).
- **Visual only.** Both layers must have `collider: none`.
- **Not open to contributions.** Errors are fixed in the source.

This amends [RFC 0001](0001-world-format.md) sections 2, 3, 5 and 7,
including what `content_id`, `size_bytes` and `bounds_mm` mean for content
that is not a mesh, and [RFC 0002](0002-contributions-and-merge-rules.md)
section 3, as [RFC 0003](0003-water-layer.md) did for water.

Related drafts: [ADR-0015](../docs/adr/0015-zurich-trees-and-lights.md)
and [ADR-0016](../docs/adr/0016-swisssurface3d-heights.md) for the
sources, [ADR-0017](../docs/adr/0017-licence-allowlist.md) and
[ADR-0019](../docs/adr/0019-odbl-and-the-published-world.md) for the
licences, [ADR-0018](../docs/adr/0018-performance-tier.md) for the
client's budget. Two follow-ups:
- **Navigation data,** which the [roadmap](../docs/roadmap.md) groups
  with this RFC, gets its own RFC, not drafted yet, after the pedestrian ADR
  ([ADR-0021](../docs/adr/0021-pedestrians.md)): if pedestrians
  enter `op_sim`, it is simulation input, not a visual layer.
- **How the client draws trees, lamps and lit windows** needs its own ADR
  extending ADR-0012, before that client work: drafted since as
  [ADR-0022](../docs/adr/0022-drawing-trees-lamps-and-windows.md).

**Nothing here is decided.** The maintainer, @victorpicaud, decides after
the comment period. Each open choice is drafted with a recommendation.
Two choices are tied to other documents:
- **The pedestrian ADR, [ADR-0021](../docs/adr/0021-pedestrians.md),
  first (recommended).** If it puts agents into
  `op_sim` in M2, trunks and poles become obstacles within M2, and
  section 7's colliders join this RFC before it is accepted. Deciding it
  first avoids declaring points visual only and reversing that in the
  same milestone.
- **The contract version** is decided once for RFC 0003 and this RFC
  ("Compatibility and migration").

## Motivation

- **M2 needs trees and lit lamps.** Its exit screenshots, at 18:00 on 21
  June and 21 December, must show trees, and street lamps lit in December.
- **The data exists, as points.** Zurich's tree register and lighting
  data (both CC0, ADR-0015) hold 1,005 trees and 1,959 light points in
  the district (the `zurich-altstadt` world), and 81,403 and 42,434 in
  the city. OpenStreetMap has 35.1 million `natural=tree` and 6.87
  million `highway=street_lamp` nodes (counted on 2026-11-24). Each
  record is a position and a few attributes, not a shape.
- **The format has nowhere to put them.** The glb profile has no palette
  material for bark, foliage or lamps, and no extensions, so no instancing.
  `furniture` is one asset per building and must be fictional.
- **Shapes are decoration.** A modelled tree freezes one look into
  canonical data, and ADR-0012 keeps invented detail in the client. The
  data should keep what is known and mark what is guessed.
- **The simulation must not change,** as for water.

## Design

### 1. Layers and kinds (amends RFC 0001 section 2)

| Layer | What | Granularity | Kinds in 0.1 |
|---|---|---|---|
| `street_objects` | Objects in public space recorded as points | one asset per tile and source | `light` |
| `vegetation` | Single trees | one asset per tile and source | `tree` |

- **Content:** the layer decides it: a points document (section 3) in these
  two layers, a glb mesh in all others. No manifest gains a key, so
  existing manifests and meshes keep their bytes.
- **Sort order:** in byte order, `street_objects` sorts between `shell` and
  `terrain`, and `vegetation` between `transport` and `water`. Existing
  tile-index entries keep their relative order; tiles without points keep
  their bytes.
- **Kinds:** a point whose kind belongs to another layer is rejected. A new
  kind (a shrub, a bench, a sign) needs an amendment of this RFC.

### 2. Ids (amends RFC 0001 section 2)

**Point ids** are the source's own ids, byte for byte:
`{namespace}:{dataset}:{local id}`.
- **Examples:** `stadt-zuerich:baumkataster:AA-1656` (a tree number),
  `stadt-zuerich:beleuchtung:LEU105227` (a light number), `osm:node:123`,
  and later `streetwide:{uuid}` for contributions. ADR-0015 chooses the
  Zurich source fields.
- **Rules:** pattern
  `^[a-z][a-z0-9_-]*(:[a-z0-9][a-z0-9_-]*)*:[!-9;-~]+$`, at most 256
  characters: the local id is printable ASCII without spaces or colons. It
  is never lowercased or trimmed; the ingest refuses, and reports, an id
  that does not fit. Unique per layer across the world.
- **Tile:** a point belongs to the tile that contains its anchor, as a
  building belongs to the tile with its footprint centroid.

**Asset ids** are `tile:17/{x}/{y}/{source}`, where `{source}` is the point
ids' prefix (the id without its last part), for example
`tile:17/68646/45903/stadt-zuerich:baumkataster`.
- **Pattern:**
  `^tile:17/(0|[1-9][0-9]{0,5})/(0|[1-9][0-9]{0,5})/[a-z][a-z0-9_-]*(:[a-z0-9][a-z0-9_-]*)*$`,
  with the tile part equal to the manifest's `tile`.
- **Single source by construction:** every point in the asset carries that
  prefix. Lineage runs per tile and source, and several sources can share a
  tile, for example at a national border.

**One grammar with RFC 0003.** Per-tile ids are `tile:17/{x}/{y}` with an
optional `/{source}`:
- point layers always carry it;
- water may take it when a second water source appears, which answers
  RFC 0003's open question. Water has no collider, so the rename (an `add`
  and a `remove`) costs nothing in the simulation;
- terrain and transport keep the plain form. Every tick, `op_sim` hashes
  the tile, layer, feature id and content id of each collider within 160 m
  of the player, so renaming them would change every replay checkpoint.

### 3. The points document (amends RFC 0001 sections 5 and 7)

Canonical JSON (RFC 0001 section 5) with `format: "streetwide.points"`,
validated by `schemas/points.schema.json`. It is stored as
`objects/<aa>/<hex>.json`, named by its hash, so the store layout does not
change.

**Top level** (all keys required): `contract_version`; `format`; `points`,
1 to 65,536 points sorted by `id` in byte order, without duplicates; and
`rules`.

**Each point** (all keys required; `null` means unknown):

| Key | Meaning |
|---|---|
| `id` | The point id. |
| `kind` | `tree` or `light`, belonging to the asset's layer. |
| `anchor_mm` | `[x, y, z]` in integer millimetres, relative to the tile origin: the ground (section 5) at the object's position in the source. Never estimated. |
| `attributes` | Every key of the kind (section 4). |
| `estimated` | The attributes a rule set, rather than the source record or a measurement from a source listed in the manifest. Sorted and unique; each names a non-null attribute that the kind allows to be estimated. |

- **Rules:** `rules` maps each kind to its estimated attributes and the id
  of the rule that set each, such as `adr-0015:yaw-from-orientierung:1`,
  defined in the source's ADR. It names nothing that no point estimates,
  and is `{}` when nothing is estimated.
- **Placeholders and defaults:** a publisher's placeholder, such as the
  register's 8 m crown default (183 trees in the district), is `null`, or
  an estimate where section 4 allows one, never a measurement. Client
  defaults stay in the client.
- **Nested attributes:** a later kind adds its keys without adding nulls to
  existing points.

**Example** (keys in reading order; the canonical form sorts them; values
illustrative):

```json
{
  "format": "streetwide.points",
  "contract_version": "0.1.0",
  "rules": { "light": { "yaw": "adr-0015:yaw-from-orientierung:1" } },
  "points": [
    {
      "id": "stadt-zuerich:beleuchtung:LEU105227",
      "kind": "light",
      "anchor_mm": [-41210, 409120, 87003],
      "attributes": {
        "count": null,
        "light_height_mm": null,
        "method": null,
        "mount": null,
        "purpose": null,
        "yaw": 16384
      },
      "estimated": ["yaw"]
    }
  ]
}
```

**Size:** about 330 bytes per tree and 230 per light point, measured with
the repository's writer on synthetic records (±20%). The district needs
about 0.9 MB, manifests included.

### 4. Attributes

Lengths are integer millimetres. Any value may be `null`.

**`tree`**, anchored at the base of its trunk:

| Attribute | Value | May be estimated |
|---|---|---|
| `crown_diameter_mm` | 1 to 100,000 | no |
| `genus` | Latin genus, `^[A-Z][a-z]+$`, at most 64 characters | no |
| `height_mm` | 1 to 150,000, from the anchor to the top | only under ADR-0016's policy 2 |
| `leaf_cycle` | `deciduous`, `evergreen`, `semi_deciduous` or `semi_evergreen` | no |
| `leaf_type` | `broadleaved` or `needleleaved` | no |
| `planted_year` | 1000 to 2999 | no |
| `species` | Latin binomial, at most 128 characters | no |
| `taxon` | The source's full scientific name, with any cultivar, at most 256 characters | no |
| `trunk_circumference_mm` | 1 to 50,000, at 1.3 m above the ground | no |

ADR-0016 drafts which heights are stored and how they are measured; it
recommends the measured height or `null`, measured from the anchor.

**`light`** is a light point as lighting data records it, not necessarily
a street lamp. Zurich's data mixes street and path lighting, lighting for
shipping, and object, art and decorative lighting, almost all under one
type code.
OSM's `highway=street_lamp` is the `street` purpose.

| Attribute | Value | May be estimated |
|---|---|---|
| `count` | 1 to 64, the luminaires the source records at this point | no |
| `light_height_mm` | 1 to 100,000, the light above the anchor | no |
| `method` | `fluorescent`, `gas`, `incandescent`, `led`, `mercury`, `metal_halide` or `sodium` | no |
| `mount` | `ceiling`, `ground`, `pole`, `wall` or `wire` | only under ADR-0015's option B for mounts |
| `purpose` | `street` (street and path lighting), `navigation` (lighting for shipping) or `decorative` (object, art and decorative lighting) | no |
| `yaw` | 0 to 65,535: where the head or arm points, in 1/65536 of a turn clockwise from north, like the world manifest's spawn yaw | yes |

**Estimated attributes** are those a source ADR proposes to set by rule:
`yaw` (ADR-0015, which maps Zurich's records to these attributes), and
`height_mm` or `mount` if the maintainer chooses the options named in the
tables. Any other estimated attribute amends this RFC ("Smaller
choices").

### 5. Ground and anchors

**Ground** is the surface of the tile's terrain asset at the anchor's
horizontal position: `x` and `z` are the source position in the region
frame, `y` the ground below the trunk or the light. Terrain never holds
OSM geometry (RFC 0001's licence separation), so no anchor depends on an
OSM-derived asset. An object standing on something else keeps its anchor
on the ground.

**The bridge problem:**
- about 190 of the district's 1,959 light points stand over water within
  5 m of a bridge deck (ADR-0015 section 6);
- the decks are OSM-derived transport assets. Placing CC0 points on them
  would derive their geometry from OSM, which RFC 0001's licence
  separation forbids;
- the terrain there is the river surface, so these anchors sit 2 to 4 m
  below the deck.

**Recommended:** anchors on the ground, as ADR-0015 also proposes; the
other options are under "Smaller choices". Until ADR-0019 is decided, the
client draws each object from its anchor, so a lamp head at a typical
5 m ends up 1 to 3 m above the deck. Lifting it onto the visible deck at
render time would use OSM data to adjust how a non-OSM layer is drawn. A
rendered frame is a produced work (ODbL 4.5 b), but the OSMF Horizontal
Map Layers guideline, written for maps, may count this as mixing, which
would bring the lamp layer under share-alike. ADR-0014's client already
does the same for terrain under water. ADR-0019 drafts a reading for
both, with a question for the OSMF.

### 6. Manifest fields (amends RFC 0001 sections 3, 5 and 7)

An asset is its **content** and its manifest. The content is a glb mesh
or, in the two point layers, a points document. For a points asset:

| Field | Meaning |
|---|---|
| `content_id` | `blake3:<hex>` of the points document's bytes. |
| `size_bytes` | The points document's size in bytes. |
| `feature_id` | `tile:17/{x}/{y}/{source}`. |
| `bounds_mm` | The exact bounds of the anchors, relative to the tile origin. |
| `physics` | `{"collider": "none"}`. |
| `fictional` | `false` for points from data; estimates do not make an asset fictional. |
| `parent_version_id` | Lineage per tile, layer and source. |
| `provenance` | The dataset, its version and the ids of its records, plus every source used to place or measure them. |

Tile-index entries keep their shape. In RFC 0001's text:
- **section 3:** "a mesh" becomes "its content: a mesh, or a points
  document in `vegetation` and `street_objects`", and `content_id`,
  `size_bytes` and `bounds_mm` refer to the content;
- **section 5:** points documents are canonical JSON, and an asset's
  content id is the hash of its content;
- **section 7:** `objects/<aa>/<hex>.json` holds tile indexes, asset
  manifests and points documents;
- **section 6,** the glb profile, does not change.

**Bounds cover anchors only.** They are exact and checkable, like glb
bounds. But a 30 m tree adds no height to its tile's bounds, so whatever
uses bounds for visibility (streaming today; levels of detail and exports
in M3) must add the objects' size.

### 7. Collider (amends RFC 0001 section 3)

Both layers must have `"physics": {"collider": "none"}`. As for water, the
rule is enforced in the asset-manifest schema, the tile-index schema (the
simulation reads colliders from tile indexes), `op_world`'s checks and the
Python tools.

**The cost:** players walk through about 1,000 trunks and every lamp post.
The simulation, and any future deterministic pedestrians, do not know
these objects exist. A travel landing
([ADR-0013](../docs/adr/0013-travel-by-starting-new-sessions.md)) can end
inside a trunk unless the client checks.

**Solid trunks and poles later** would need:
- a collider kind for points, for example 8-sided prisms built from
  integer offset tables, without trigonometry;
- a `SIM_CONTRACT_VERSION` bump, since that conversion joins the
  simulation contract;
- replays recorded again, with checkpoints that change near them;
- a new G9, because street trees and poles block its pavement probes;
- bounds that cover the colliders, and known mounts, since a guessed pole
  could block a lane.

### 8. Contributions and the gate (amends RFC 0002 section 3)

| Layer | Contributions in 0.1 | Collider | Notes |
|---|---|---|---|
| `street_objects` | **no: fix it in the source** | `none` | From open data (ADR-0015 for Zurich). The next baseline refresh brings fixes in. |
| `vegetation` | **no: fix it in the source** | `none` | Same. |

The gate's version becomes 0.1.3:
- **G3:** a contributed change fails, with that advice. A baseline refresh
  authored by `streetwide:ingest` gets `review`.
- **G7:** points do not count towards triangle budgets. New: at most 5,000
  points per tile over both layers, about 18 times the district's busiest
  tiles. Over it, a contribution fails and a refresh gets `review`, so
  dense real data (an orchard mapped tree by tree) can still be accepted.
  Starting values, to be set against ADR-0018.
- **G8:** every anchor lies within 50 mm of its tile's terrain surface,
  computed exactly from the terrain's millimetre vertices, which also
  checks that the point is in its tile. An anchor with no terrain below,
  or over terrain licensed with `ODbL-1.0`, fails: anchors are checked
  against non-OSM ground. Other mismatches fail a contribution and give a
  refresh `review` (over contributed terrain, for example). A terrain
  replacement lists the anchors it would strand, as `review`.
- **G9 and G10:** not applicable while the collider is `none`.
- **Review aid:** the report lists the points added, removed, moved and
  changed in each asset, with counts per tile, so a refresh that drops
  many points stands out.

**The 50-change cap.** RFC 0002 caps a change set at 50 changes, with no
exception for the ingest, and the district's first refresh adds 26 tree
assets and 29 light assets. Recommended: split refreshes by source for M2,
and amend RFC 0002 before M3 (see "Options considered").

### 9. Planet scale and one source per area

- **Elsewhere:** OSM `natural=tree` nodes become `tree` points, and
  `highway=street_lamp` nodes `light` points with `purpose: street`, in
  `tile:17/{x}/{y}/osm:node` assets licensed `ODbL-1.0` with the terrain's
  licence. The data is thin: 3.3% of OSM trees have a height, and the
  district has 76 OSM street lamps against the city's 1,959 light points.
  Tree rows (2.16 million OSM ways) and woods are not points.
- **Size:** about 11.6 GB of canonical JSON for OSM's trees and 1.6 GB for
  its lamps; M3's delivery ADR compresses or transcodes it.
- **One source per area.** As
  [ADR-0014](../docs/adr/0014-water-areas-from-openstreetmap.md) records,
  the OSMF guidelines keep share-alike off non-OSM data only if each kind
  of feature is all-OSM or all-non-OSM within a regional cut of at least a
  country, with no holes; a city is not one. On that reading, uncertain
  because the guidelines were written for 2D maps, register points in
  Zurich with OSM trees elsewhere in Switzerland would bring the register
  points under the ODbL. ADR-0015 drafts the Swiss choice.
- **Where the policy lives** follows ADR-0019.
  - Under its option A, the whole world under the ODbL, no table is
    needed: checking that each asset's licence covers its sources'
    licences is enough. A source choice per country, such as ADR-0015's
    for Switzerland, is then about data quality. It is kept as data: a
    source list per country and kind, with the ingest's source
    definitions, which any client gap-fill reads too. No gate check
    enforces it; the ingest and the review of each refresh do.
  - Under its option B, regional cuts, the policy is data in the world: a
    (country, kind, OSM or non-OSM) table in the world manifest and a
    country on each asset, checked across the whole world, because a
    check per tile cannot see mixing across tiles. This RFC would then
    carry that amendment, before anything is published. A simpler check
    for format 0.1, each point layer all-OSM or all-non-OSM within a
    world, is not proposed: nothing is published before the table exists,
    and it could not express ADR-0015's option (b).

Under ADR-0019's option B, two more rules are licence needs; under its
option A they are choices, which ADR-0015 keeps for Zurich:
- **No OSM in non-OSM points:** OSM roads, bridges, water and fallback
  buildings never place, drop, classify or correct a non-OSM point. They
  appear in QA reports only.
- **No gap-filling:** no lamps or trees generated along OSM streets in a
  country whose objects come from non-OSM data. Such decoration covers a
  whole country, or none.

### 10. Privacy

- **Public ground:** these layers hold real objects on public ground, and
  on private ground only where visible from public ground. An object on
  private ground in an enclosed courtyard or garden is never stored, from
  any source, including later fills from LiDAR or OSM. Objects that a
  public body records as its own, such as the city's register trees with
  an accepted status (ADR-0015 section 5), count as on public ground.
- **Private records:** a source that can publish objects on private
  ground needs a check in its ingest; ADR-0015 section 5 drafts the one
  for Zurich's register.
- **No place or personal text:** no addresses, street or quarter names,
  owners or operators. The register's `strasse` and `quartier` are never
  stored.
- **Lit windows** are client decoration: seeded, never derived from
  occupancy, energy or register data, with no interior detail, and off in
  F2.
- **G6** does not apply while the layers are closed. Each source's ADR
  says how its ingest keeps to these rules, and the refresh review checks
  them. Raw source answers kept outside the world (ADR-0015 section 4)
  are not world data; ADR-0015 drafts who may read them.

## Options considered

### Format

**A. glb meshes per tile, with new palette materials.**
- Pros: the existing glb path; materials added after `water` leave
  existing meshes byte-identical; trunks could collide later through the
  trimesh path.
- Cons:
  - a glb profile change, which older readers reject;
  - at the crudest look, about 5 to 6 MB and 151,000 triangles for the
    district (14% more than its 1.08 million), and about 100 GB for the
    planet;
  - one look frozen into canonical data, against ADR-0012; every better
    tree generator rewrites the world;
  - no genus, season or level of detail, nothing that says where a lamp's
    light is, and no lineage per tree.

**B. A points document per tile and source (recommended).**
- Pros:
  - it keeps what is recorded or measured, and marks what is guessed;
  - about 0.9 MB for the district, and no triangles;
  - no glb profile change; existing bytes and the store layout stay;
  - the client can draw instanced trees with levels of detail and seasons,
    and lit lamps at night;
  - each asset is single-source by construction.
- Cons:
  - a second kind of content: every place that assumes a glb changes;
  - lineage per tile and source, not per object, which is why RFC 0001
    gave buildings their own assets. One changed tree is a new document;
  - bounds cover anchors only;
  - a mesh in these layers (say, a contributed landmark tree) would need a
    contract change;
  - other consumers, such as a 3D Tiles export, must generate shapes.

**C. glTF instancing (`EXT_mesh_gpu_instancing`) in the glb profile.**
- Pros: the smallest, about 44 bytes per instance (0.13 MB for the
  district) plus models; a standard that 3D Tiles 1.1 and engines read.
- Cons:
  - a glb profile 0.2 with canonical instance transforms. Quaternions need
    trigonometry and risk byte differences between Rust and Python, unless
    yaw is a custom integer;
  - attributes need code lists or `EXT_structural_metadata`, and lights
    need `KHR_lights_punctual`, one node each;
  - the model is still frozen into the data.

  An exporter can produce it from B later (M3).

**D. One asset per object.**
- Pros: lineage per object, like buildings.
- Cons: about 3,000 assets in the district (1,491 today), 124,000 in the
  city and 42 million on the planet. A manifest (about 1 KB) and a
  tile-index entry (about 400 bytes) outweigh a point four to six times.

**E. The `furniture` layer, or one generic layer.**
- `furniture` is per building and must be `fictional: true`; real trees
  and lights break that privacy rule. Rejected.
- One generic layer is a naming variant of B that loses what differs by
  kind: contributions, future colliders, streaming distance and the source
  per country.

Fetching the open data in the client at run time was also rejected: the
world id would not pin it, and nothing would carry its provenance.

### Smaller choices

- **Asset ids.** One grammar with an optional `/{source}` is recommended
  (section 2). The alternatives: the suffix for water now (its ids change
  again, with nothing gained until a second source exists); the plain form
  for points (a border tile would need every asset renamed later); the
  suffix for terrain and transport (every replay checkpoint changes).
- **Records at one position.** A source can record several lights at one
  coordinate (up to 36 in the district, ADR-0015 section 6).
  - **One point per record (recommended, as in ADR-0015):** each keeps its
    id and orientation; the client groups points at one anchor, so models
    do not overlap and light is not multiplied.
  - One point per position, with `count`, named by its lowest id: a rule
    picks the id, which changes when that record goes, and orientations
    are merged.
- **Lights on bridges.**
  - **Anchors on the ground (recommended):** no OSM in the points, and G8
    checks every anchor exactly. About 10% of the district's light anchors
    lie 2 to 4 m below the object.
  - Deck heights from swissSURFACE3D inside non-OSM bridge outlines, such
    as the city's cadastral survey: correct anchors, but one more source,
    whose terms differ by canton. swissSURFACE3D alone is not enough
    (ADR-0015 section 6).
  - An estimated or unknown height over water: the anchor's height set by
    a rule and marked as estimated, or left `null` for the client to
    place. Against: G8 can no longer check those anchors exactly, and
    sections 3 and 5 and G8 change, as they would for the deck heights; a
    `null` height also leaves the bounds undefined.
  - Anchors on the OSM decks: allowed only under ADR-0019's option A, and
    then the tile's light asset becomes ODbL and G8 must accept OSM ground.
  - Leaving them out: unlit bridges.
- **Estimated attributes.**
  - **This RFC lists which attributes may be estimated (recommended,
    section 4):** a new estimated attribute amends it, so formula values
    enter canonical data only through an RFC.
  - Each source ADR may define rules for any attribute: quicker, but data
    gains guessed values without a contract change.
- **Refreshes over 50 changes.**
  - **Split them by source (recommended for M2):** no rule change. A
    city-wide refresh, about 2,800 tiles per layer, would need over a
    hundred change sets.
  - **No cap for `streetwide:ingest` (recommended before M3),** amending
    RFC 0002 section 2: one refresh, one review, helped by the per-point
    lists, but a large refresh is harder to review.

## Compatibility and migration

**Contract version, one decision for RFC 0003 and this RFC:**
- **A. Amend 0.1.0 in place (recommended).** RFC 0001 is a Draft and no
  world has been published. Both RFCs keep `contract_version` at `0.1.0`,
  which is then frozen when RFC 0001 is accepted or the first canonical
  world is published (M1), whichever comes first.
  - Pros: existing manifests, meshes and golden ids in `fixtures/format`
    keep their bytes. A bump is likely soon anyway: RFC 0001 expects
    textures to come with a profile version bump, and M2 plans that RFC.
  - Cons: three formats will have been called 0.1.0. An older reader fails
    with "unknown layer", not RFC 0001's clean "unknown contract version".
- **B. Move both to 0.2.0.**
  - Pros: older readers fail cleanly; one version names one format; the
    moment to add an explicit content key to manifests.
  - Cons: every manifest, tile-index and world id changes, golden ids
    included, though no mesh or replay checkpoint does; it spends a version
    before anything is published.

**Existing worlds.** A re-ingest adds point assets and tile indexes, so
world ids change; under A, existing assets stay byte-identical. Replays
must be recorded again, with unchanged checkpoints.

**Fixtures:** `fixtures/world` gains two assets in tile 17/68646/45903
(33 trees and up to 136 light points), a tile index and a world id;
`sample.oprep` is recorded again (its header only); the facade sample is
regenerated (gate 0.1.3); `fixtures/format` gains a golden points asset.
ADR-0015 section 4 drafts how the raw source answers stay available.

**Refreshes.** Each refresh of a point source is a new world id, so the
replay suite that G10 uses is recorded again (headers only) at each
merge. How often a source is refreshed is its ADR's choice (ADR-0015).
A later RFC could give replay headers an id that covers only colliders.

## Determinism, provenance and privacy

- **Determinism:** these assets are never colliders, so they never enter
  the simulation's active set or state hash; `SIM_CONTRACT_VERSION` stays
  0.2.0. The implementing pull request shows the sample replay's
  checkpoints byte-identical on Linux, Windows and macOS, in debug and
  release, with only the header's world id changed.
- **Rendering** may differ between machines: trees, lamps and night lights
  are decoration, and F2 shows only the data.
- **Provenance:** like any asset, listing every source used to place or
  measure a point (section 6); ADR-0015 gives Zurich's licence and
  credits. Each source gets an ATTRIBUTION.md entry first
  ([ADR-0001](../docs/adr/0001-licensing.md)). Baseline refreshes skip G4
  and G5 today, so the check that each licence covers its sources'
  licences (ADR-0017, ADR-0019) must run on them too.
- **One source per area** and **privacy:** sections 9 and 10.

## Tests and checks

- **Schemas:** `points.schema.json`; the two layers; collider `none` and
  the asset id pattern in the manifest and tile-index schemas.
- **`op_world`:** a canonical points reader and writer; ids sorted, unique
  and carrying the asset's prefix; kinds per layer; `estimated` and
  `rules` in agreement; bounds equal to the anchors; point ids unique per
  layer across the world.
- **Every place that assumes a glb:** `validate`, the gate (G3, G7, G8,
  triangle counts), `merge` (it copies content as a glb), `op_stream` (a
  points loader; colliders unchanged) and the client's streaming loop. The
  new layers need explicit serde names: the lowercase rule would write
  `streetobjects`.
- **Python:** a byte-identical writer, parity tests with Rust, and mutation
  tests (unsorted ids, a wrong prefix, a kind in the wrong layer, a trimesh
  collider, floats, omitted nulls, an estimate without a rule).
- **Gate:** G3's advice, `review` for refreshes, the point budget, and G8,
  including terrain licensed with `ODbL-1.0`.
- **No new crate or Python package.**

## Unresolved questions

- **The lighting data:** what ewz's orientation and stacks mean, and
  whether light and tree numbers survive a replacement (ADR-0015's
  questions for the city).
- **Bridges:** a Switzerland-wide non-OSM source of deck heights, and the
  OSMF's answer on lifting at render time (ADR-0019, question 5).
- **Planet scale:** checking point ids across the world without holding
  them all in memory; tree rows and woods; bounds for levels of detail.
- **New kinds:** signs carry text, which needs its own privacy and licence
  review.
- **Takedowns:** RFC 0002 section 7 removes or reverts whole assets.
  Taking down one point needs a new version of its tile's document.
- **Contributions:** contributed point sets
  (`tile:17/{x}/{y}/streetwide`, CC BY-SA 4.0), only for kinds not taken
  from OSM in that country.
- **Terrain contributions:** where a tile's terrain is a contribution, the
  ingest must place anchors on that terrain.
