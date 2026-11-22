# RFC 0001: World format

- **Status:** Draft
- **Authors:** @victorpicaud
- **Created:** 2026-11-22
- **Contract version:** world format `0.1.0`

## Summary

This RFC defines how StreetWide stores the world on disk:

- how the planet is cut into **regions** and **tiles**, and which coordinate
  frame geometry uses;
- the **asset**: a glTF 2.0 binary mesh plus a JSON **manifest** that
  carries its content id, layer, contract version, parent version and
  provenance (sources, licence, attribution);
- the **tile index** and the **world manifest** that tie assets together;
- **canonical JSON** and **BLAKE3 content ids**, so the same content always
  has the same id, in every language, on every platform;
- the directory layout of a world **store**.

The JSON Schemas in [`schemas/`](../schemas/) and the `op_world` crate
implement this RFC. Where the prose and the schemas disagree, that is a bug;
the stricter reading applies until it is fixed.

## Motivation

StreetWide's world is assembled from many sources (open data today, player
contributions later) and must only ever get better. That needs:

- **Stable identity.** An asset's id must depend only on its bytes, so two
  people producing the same content get the same id and anyone can verify
  what they downloaded.
- **Lineage.** Each version of an asset points at the version it replaces,
  so we can compare a contribution against what it would replace.
- **Provenance and licence on every asset** (CLAUDE.md), so licences with
  different obligations (CC BY-SA, ODbL, swisstopo terms) never get mixed
  up.
- **A frame the deterministic simulation can use exactly**, without
  trigonometry at load time.
- **Planet scale**, even though the first slice is one Zurich district.

## Design

### 1. Coordinate reference and tiling

**Tile addressing.** Tiles are cells of the Web Mercator (EPSG:3857)
quadtree, written `z/x/y` with the usual XYZ convention (x grows east, y
grows south, origin at the north-west corner). Format 0.1 uses:

- **regions** at zoom **12** (about 9.8 km wide at the equator, 6.6 km in
  Zurich), and
- **tiles** at zoom **17** (about 306 m at the equator, 207 m in Zurich).

Each z17 tile lies in exactly one z12 region: region `x = tile.x >> 5`,
`y = tile.y >> 5`. Web Mercator excludes latitudes beyond ±85.05°; polar
coverage is out of scope for 0.1.

**Region frame.** Each region has a local Cartesian frame:

- **origin**: the region's anchor point, the geodetic point (WGS84) at the
  centre of the z12 cell, at height 0;
- **axes** (the glTF convention): **+X = east**, **+Y = up** (ellipsoid
  normal at the anchor), **+Z = south**. This is right-handed, matches glTF,
  Bevy and Rapier, and is an East-North-Up frame with Y and Z renamed;
- **unit**: metre.

Positions are converted to the frame exactly (geodetic → ECEF → local ENU);
the frame is not a map projection, so distances are true. Over a region
(±5 km) the curvature of the Earth makes the ground drop by up to ~2 m
towards the region edge and tilts "up" by up to 0.05°; both are real
geometry, not error.

**Heights.** Format 0.1 stores heights as given by the source data,
treated as heights above the ellipsoid at the anchor. For Swiss data these
are orthometric heights (LHN95). The geoid undulation varies by only a few
centimetres across a region, so this is consistent within a region. A
future version may switch to ellipsoidal heights with a geoid model.

**Tile origin.** Each tile has an **origin** in its region frame, stored as
integer millimetres `[x, y, z]`. Ingest places it at the tile's centre at
height 0 (`y = 0`). All geometry of the tile's assets is stored **relative
to the tile origin**.

Because tile origins are exact integers in a shared region frame, placing
tiles next to each other is an exact integer translation: the simulation
assembles a region with no trigonometry and no rounding.

**Out of scope for 0.1**, to be specified by a later RFC: crossing region
boundaries (the 0.1 world is one region), moving the simulation origin
(origin rebasing) for planet-wide travel, and levels of detail above z17.

### 2. Layers and assets

An **asset** is one piece of the world in one **layer**. Layers:

| Layer | What | Granularity |
|---|---|---|
| `shell` | Building outer hull: walls, roofs, ground plate | one asset per building |
| `facade` | Facade detail on top of the shell (windows, doors, ornaments) | one asset per building |
| `interior` | Interior rooms (must be fictional) | one asset per building |
| `furniture` | Furniture and props inside interiors (must be fictional) | one asset per building |
| `interactivity` | Interactive elements (doors that open, signs, triggers) | one asset per building |
| `terrain` | Ground surface | one asset per tile |
| `transport` | Roads, paths, steps, bridge decks | one asset per tile, plus one per bridge |

`terrain` and `transport` are additions to the five building layers named
in the project brief: the world needs ground to stand on and streets to
read.

Building assets are **per feature** so that a contribution can improve one
building's facade without touching anything else, and so that each
building's layer has its own version history (`parent_version_id`). A
building belongs to the tile that contains the centroid of its footprint,
even if it extends into a neighbouring tile.

**Licence separation.** An asset never mixes geometry from sources with
different licences. In particular, geometry derived from OpenStreetMap
(ODbL) is always in its own assets. Every tile index lists each asset's
licence, so the ODbL-licensed part of the world can be identified and
extracted mechanically as its own database.

**Feature ids** are stable identifiers of the real-world (or fictional)
thing an asset describes, namespaced by source:
`swisstopo:swissbuildings3d:{UUID}`, `osm:way:123`,
`osm:relation:456`, `tile:17/68646/45908` (for per-tile layers),
`streetwide:<uuid>` (for contributed features). Pattern: a lowercase
namespace, a colon, then printable ASCII without spaces.

### 3. Asset files

Each asset is two files:

1. a **mesh**: a glTF 2.0 binary file (`.glb`) in the *StreetWide glb
   profile* (section 6);
2. a **manifest**: canonical JSON (section 5) validated by
   [`schemas/asset-manifest.schema.json`](../schemas/asset-manifest.schema.json).

Manifest fields:

| Field | Meaning |
|---|---|
| `format` | Always `"streetwide.asset"`. |
| `contract_version` | World format version, `"0.1.0"`. |
| `content_id` | `blake3:<hex>` of the `.glb` bytes. |
| `size_bytes` | Size of the `.glb` in bytes. |
| `layer` | One of the layers above. |
| `feature_id` | What the asset describes (section 2). |
| `tile` | The z17 tile key the asset belongs to. |
| `bounds_mm` | Axis-aligned bounds `{min, max}` of the mesh in integer millimetres, relative to the tile origin. Must equal the bounds of the glb positions. |
| `physics` | `{"collider": "trimesh"}` if the simulation collides with this mesh, `{"collider": "none"}` otherwise. |
| `fictional` | `true` if the geometry is invented rather than derived from measurements of the real world. **Must be `true` for `interior` and `furniture`** (private by design). |
| `parent_version_id` | Version id (section 5) of the manifest this one replaces, or `null` for the first version of this layer of this feature. |
| `provenance` | See below. |

`provenance`:

| Field | Meaning |
|---|---|
| `license` | SPDX licence expression for the asset, e.g. `CC-BY-SA-4.0`, `ODbL-1.0`, `LicenseRef-swisstopo-OGD`. |
| `attribution` | Credits that must be shown with the asset, e.g. `["© swisstopo"]`. Sorted, unique, non-empty. |
| `sources` | Non-empty list of datasets or contributions it was derived from: `dataset`, `version`, `license`, `attribution`, `url`, `source_ids` (ids of the source records used, sorted). |
| `generator` | `{name, version}` of the tool that produced the asset. |

### 4. Tile index and world manifest

A **tile index** (`format: "streetwide.tile"`) lists the assets of one tile:
`tile`, `region`, `origin_mm`, and `assets`, a list of
`{layer, feature_id, manifest_id, content_id, license, collider, bounds_mm}`
sorted by `(layer, feature_id)` byte order, with no duplicate
`(layer, feature_id)` pair. The copy of `license`, `collider` and
`bounds_mm` lets streaming and the simulation decide what to load without
opening every manifest. They must match the manifest.

A **world manifest** (`format: "streetwide.world"`) describes a playable
world: `name`, `region` (`key` and `anchor_e9`, the anchor's latitude and
longitude in integer nanodegrees), `attribution` (the sorted union of every
asset's attribution, shown by clients), `spawn` (`position_mm` in the
region frame and `yaw` in units of 1/65536 of a turn, clockwise from north),
and `tiles`, a list of `{tile, tile_id, origin_mm, bounds_mm}` sorted by
tile `x` then `y`, where `bounds_mm` is the union of the tile's asset bounds
relative to its origin.

### 5. Canonical JSON and ids

All manifests, tile indexes and world manifests are stored as
**canonical JSON**, a strict subset of JSON (RFC 8259):

- UTF-8, no byte-order mark, no whitespace outside strings, no trailing
  newline;
- object keys are sorted by byte value, with no duplicates; keys match
  `[a-z][a-z0-9_]*`;
- numbers are integers only, in `[-(2^53 - 1), 2^53 - 1]`, written without
  sign for zero, without leading zeros, fraction or exponent. Coordinates
  are integer millimetres, angles integer fractions of a turn, so no float
  is ever needed;
- strings contain no control characters (U+0000–U+001F, U+007F); the only
  escapes are `\"` and `\\`; all other characters, including non-ASCII such
  as `©`, are written as raw UTF-8;
- `true`, `false`, `null` as usual. Optional values are written as `null`,
  never omitted.

A reader **rejects** a file whose bytes differ from the canonical
serialisation of its parsed value. This makes the mapping from value to
bytes one-to-one.

**Ids.** A content id is the string `blake3:` followed by the 64 lowercase
hex digits of the 256-bit BLAKE3 hash of a file's exact bytes:

- the **content id** of an asset is the hash of its `.glb`;
- the **version id** (`manifest_id`) of an asset version is the hash of its
  canonical manifest;
- the **tile id** is the hash of the canonical tile index;
- the **world id** is the hash of the canonical world manifest.

The chain world → tiles → manifests → meshes is a Merkle tree: the world id
pins every byte of the world. Replays record the world id they were made
in.

### 6. StreetWide glb profile 0.1

Meshes are glTF 2.0 binary files restricted to a profile that is easy to
validate and has exactly one byte representation per mesh:

- one JSON chunk and one BIN chunk; JSON padded with spaces, BIN with
  zeros, to 4 bytes; no external URIs, images, textures, samplers, cameras,
  skins, animations or extensions;
- `asset = {"generator": "StreetWide world format 0.1", "version": "2.0"}`;
- one scene `0` containing every node; nodes have a `name` (the feature id
  of what they show) and a `mesh`, and no transform: positions are already
  in the tile frame;
- each mesh primitive is triangles (`mode` 4) with `POSITION` and `NORMAL`
  (float VEC3) and `UNSIGNED_INT` indices, each in its own buffer view, laid
  out primitive after primitive, positions, then normals, then indices;
- **positions are snapped to the millimetre**: every coordinate is the f32
  nearest to the f64 value `k / 1000` for an integer `k`. The accessor
  `min`/`max` of positions are written as those millimetre values in
  shortest decimal form (`12.345`, `-0.5`, `7`);
- materials come from a fixed **palette** and are identified by name:
  `roof`, `wall`, `ground`, `terrain`, `road`, `path`, `bridge`, `water`.
  Each has a fixed base colour, metallic 0, roughness 1. A mesh lists only
  the materials it uses, in palette order;
- the JSON chunk is canonical JSON (section 5) except that the numbers
  listed above may be millimetre decimals.

`op_world` provides the reference writer; a `.glb` is valid only if
re-writing its parsed content with the reference writer reproduces it byte
for byte.

### 7. Store layout

A world on disk is a directory:

```
world.json                         the world manifest
objects/<aa>/<hex>.json            tile indexes and asset manifests
objects/<aa>/<hex>.glb             meshes
```

where `<hex>` is the 64-digit hash of the file and `<aa>` its first two
digits. Every object file is named by its own hash, so the store verifies
itself. Downloaded source data never lives here.

## Options considered

**Tiling.**

1. *Web Mercator quadtree* (chosen). Ubiquitous tools and integer keys;
   tiles shrink towards the poles (at 60° a z17 tile is 153 m), which is
   acceptable for a street-level game.
2. *Geodetic (equal-angle) quadtree*, as in Cesium. Worse distortion near
   the poles, less common tooling.
3. *S2 cells.* Near-uniform area and 64-bit ids, but quadrilaterals that
   are not axis-aligned and unfamiliar tooling for contributors.
4. *National grids* such as Swiss LV95 kilometre tiles. Exact match to
   swisstopo, but not planet-scale; every country would differ.

**Geometry frame.**

1. *Region ENU frame with integer tile origins* (chosen). Exact tile
   placement with integer maths; one frame per ~7 km region.
2. *One ENU frame per tile.* Tiles are self-contained, but placing them
   requires a rotation per tile (trigonometry in the simulation).
3. *ECEF.* Global and exact, but "up" differs everywhere, so every tile
   needs a rotation for gravity, and f32 precision is poor at
   Earth-radius magnitudes.

**Asset granularity.**

1. *Per feature for building layers, per tile for terrain and transport*
   (chosen). Natural unit for contributions and lineage.
2. *Per tile per layer.* Fewer files, but every contribution rewrites a
   whole tile layer and lineage becomes meaningless.

**Canonical form.** RFC 8785 (JCS) supports floats but needs a careful
number serialiser in every language; banning floats from manifests makes
the canonical form trivial to implement identically in Rust and Python.

**Hash.** BLAKE3 (chosen; fast, 256-bit, single algorithm for all sizes)
vs SHA-256 (ubiquitous but slower; swisstopo uses it for download
checksums, which is unrelated to our ids).

## Compatibility and migration

This is the first version; there is nothing to migrate. Readers must reject
`contract_version` values they do not know. Future versions bump
`contract_version`; a change of version changes ids, so migrations rewrite
assets with `parent_version_id` pointing at the old versions.

## Determinism, provenance and privacy

- The simulation consumes geometry only through the integer tile origins
  and the mm-snapped positions of `collider: trimesh` assets, loaded in
  `(tile, layer, feature_id)` order.
- Every asset carries licence and attribution; validation rejects assets
  without sources or attribution.
- `interior` and `furniture` assets must be marked `fictional: true`.
  Ingest never produces interiors from real data.

## Tests and checks

- JSON Schemas for every document in `schemas/`; examples and fixtures
  validate in Rust and Python.
- Golden vectors in `fixtures/format/` (manifests, tile index, world
  manifest, a small glb) with their expected ids, checked by Rust and
  Python on Linux, Windows and macOS.
- Rejection tests for non-canonical JSON (whitespace, unsorted keys,
  floats, escapes, duplicate keys, missing `null`s) and invalid glbs.
- `op_world validate <dir>` checks a whole world: canonical form, hashes,
  references, bounds, licence rules.

## Unresolved questions

- Region boundaries and origin rebasing (next world-format RFC).
- Levels of detail and aggregated meshes for distant tiles.
- Textures and richer materials (likely glTF PBR with KTX2, behind a
  profile version bump).
- How contribution quality is scored for "beats what is there" (separate
  RFC).
