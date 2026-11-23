# ADR-0006: Build the baseline world with a Python ingest from swisstopo and OpenStreetMap

- **Status:** Proposed (stage 3 checkpoint)
- **Date:** 2026-11-23
- **Deciders:** @victorpicaud
- **Supersedes / superseded by:** none

## Context

The first world is about 1 km² of Zurich's old town, built only from open
data: swissBUILDINGS3D 3.0 Beta (buildings), swissALTI3D (terrain) and
OpenStreetMap (roads, paths, bridges, and building footprints where
swisstopo has no building). The output must be a world store in the format
of [RFC 0001](../../rfcs/0001-world-format.md). Constraints from CLAUDE.md
and the project brief:

- every download must be scriptable, and no large data goes into Git;
- OSM-derived data stays in separate assets (ODbL);
- every asset carries provenance, licence and attribution;
- ingest output must be reproducible: the same inputs give the same world
  id.

## Decisions and options considered

### 1. Language: Python (`tools/ingest`)

- **A. Python** (chosen): PROJ (pyproj), GEOS (shapely), TIFF (tifffile)
  and earcut are mature and pinned. `tools/worldfmt` is a byte-compatible
  Python implementation of the world format, tested against `op_world`.
- **B. Rust**, sharing `op_world` directly. One language, but geodesy and
  polygon tooling in Rust are much less mature, and ingest is not part of
  the deterministic simulation.

### 2. Sources and formats

| Source | Format used | Alternatives rejected |
|---|---|---|
| swissBUILDINGS3D 3.0 Beta | CityGML 2.0 LOD2, one zip per map sheet, parsed as a stream with the standard library (expat) | FileGDB (needs GDAL and a proprietary driver), DWG (closed) |
| swissALTI3D | 0.5 m Cloud-Optimized GeoTIFF (LZW; tifffile + imagecodecs) | 2 m (too coarse for streets), XYZ text (10× larger to parse) |
| OpenStreetMap | one Overpass API query per area (`out body geom`) | Geofabrik country extract (~500 MB, needs osmium) |

Files are found through the swisstopo STAC API. An item is used if its
**footprint geometry** (not its lon/lat envelope) overlaps the area. Per
map tile, the **latest year** that has the wanted asset is used. Every
file is checked against the SHA-256 that STAC publishes. Search answers and
the Overpass answer are cached, so a re-run from cache picks the same
inputs. The Overpass `timestamp_osm_base` is recorded as the OSM version in
every OSM-derived manifest.

**CityGML details.** Each building carries its geometry twice:
- typed boundary surfaces (`bldg:boundedBy`: roof, wall, ground);
- untyped closed solids (`lod2Solid`, on the building or its parts).

Only the typed surfaces are used. They give materials for free and
include roof overhangs (about 1.6 % more area than the solids). Walls and
roofs keep the source orientation, which is outward: 99.7 % of decidable
wall triangles. Ground plates face into the building in the source, so they
are turned down. Buildings of type `Unterirdisches Gebaeude` (underground)
are skipped.

### 3. Coordinates

LV95 → WGS84 is an **explicit PROJ pipeline**: Swiss oblique Mercator
inverse, then the CH1903+ → WGS84 geocentric translation (674.374, 15.056,
405.346 m). This is the transformation PROJ itself picks for EPSG:2056 →
EPSG:4326. Pinning it means results never depend on which datum grids a
machine has installed.

The region frame is PROJ's topocentric conversion at the RFC 0001 anchor.
Source heights (LN02) are used as they are (RFC 0001 §1). All 2D work on
OSM geometry (clipping, offsets, overlaps) happens in LV95, the metric
projection the swisstopo data uses.

### 4. Terrain

- **A. A 128 × 128 grid per tile on Web Mercator z24 pixel corners**
  (chosen; about 1.6 m in Zurich), sampled bilinearly from the 0.5 m data.
  Neighbouring tiles evaluate the same function at shared corners, so
  seams are identical by construction. Normals use the neighbouring grid
  points across tile edges, so shading is continuous too. Cells without
  data are dropped, never guessed.
- **B. A metric grid per tile in the region frame.** Tile edges are not
  axis-aligned there, so seams would need stitching.
- **C. Full 0.5 m resolution.** 16× the triangles for detail that the
  buildings mostly cover.
- **D. A simplified TIN.** Smaller, but seams are harder. Left for levels of
  detail.

### 5. Buildings and the OSM fallback

- A building belongs to the tile that contains its footprint centroid
  (RFC 0001).
- An OSM footprint is extruded only if **at most 20 %** of its area is
  covered by swissBUILDINGS3D footprints. In the old town, 10 of 1,449 OSM
  buildings qualify.
- Height comes from `height`, else `building:levels` × 3 m, else 10 m.
- The base is the lowest terrain point under the footprint.
- These are skipped: `building=roof|construction|proposed|ruins|…`,
  `location=underground`, and negative `layer`.

### 6. Transport

- **Roads and paths** become flat ribbons cut at tile edges, one visual
  asset per tile with `collider: none`. Each is 5 cm above the terrain
  (paths 6 cm), and its width is the `width` tag or a per-class default.
- **Excluded:** squares mapped as areas, tunnels, indoor ways, ways below
  ground, and `highway=construction|platform|corridor|…`. This means
  Rathausbrücke, currently tagged as under construction, is absent; the
  temporary bridge next to it is present.
- **Bridges:** each bridge way is its own asset with `collider: trimesh`,
  a 0.5 m deck. Deck heights are anchored to the terrain where the bridge
  meets a non-bridge highway, or at a loose end. In between they are
  harmonic: linear by distance along a chain of bridge ways.

### 7. Licence of OSM geometry placed on swisstopo heights

RFC 0001 §2 says that an asset never mixes geometry from sources with
different licences. Ribbons, bridge decks and fallback buildings are OSM
geometry, placed at swisstopo terrain heights. Options:

- **A. Licence them `ODbL-1.0 AND LicenseRef-swisstopo-OGD`** (chosen).
  Provenance lists both sources and both credits. swisstopo's terms allow
  processing and redistribution, including commercially, with a source
  reference. So the combination can be offered under the ODbL as long as
  "© swisstopo" stays with it.
- **B. Licence them `ODbL-1.0` only.** This hides an attribution
  obligation, which breaks the provenance principle.
- **C. Keep OSM geometry at a constant height and place it at load time.**
  The assets stay pure ODbL, but the simulation would need terrain-dependent
  placement at load time, and an asset would no longer hold its own
  geometry.

With A, the ODbL part of the world can still be extracted mechanically:
every asset whose licence expression contains `ODbL-1.0`. RFC 0001 §2 is
amended (while still Draft) to say exactly this.

### 8. Caching and untrusted input

- Downloads go to `data/cache/` (gitignored) under source-specific
  directories, written to `*.part` files and renamed only when complete
  and checksum-verified. Only HTTPS URLs are accepted, there are size
  limits, and nothing is extracted to disk.
- The User-Agent is generic (`streetwide-ingest/<version>`) and carries no
  personal contact.
- XML is parsed with expat, which resolves no external entities.
- Parsed buildings are cached as JSON in `data/cache/derived/`, never as
  pickle, because parsing a sheet takes about 100 s. Cache files are keyed
  by sheet file and area.

### 9. Reproducibility

- Input elements are sorted by id.
- No unordered string-keyed iteration is used.
- Positions are rounded half-up to the millimetre in the region frame, so
  tiles sharing a point agree on it.

Running the ingest twice from cache gives the same world id; this is
checked at every checkpoint. The output is reproducible on one platform.
Across platforms, differences in libm, PROJ or GEOS can in rare cases move
a vertex by one millimetre and so change ids. This does not affect the
simulation, which consumes a world by its id; ingest is not part of the
deterministic core.

## Consequences

- `python -m tools.ingest --world zurich-altstadt` builds the district in
  about 3.5 minutes on a first run. It downloads about 275 MB: one building
  sheet, one terrain tile and one Overpass answer.
  - Output: 30 tiles, 1,483 assets (1,377 swissBUILDINGS3D shells, 10 OSM
    fallback shells, 36 bridge decks, 30 ribbon assets, 30 terrain tiles).
  - About 1.08 M triangles and 48 MB.
- One tile is committed as a test fixture (`fixtures/world/`, 1.7 MB). Its
  OSM-derived assets are ODbL and carry that licence in their manifests.
- Known gaps, written up as good-first-issue drafts in stage 6:
  - no water surface (the Limmat is walkable terrain at water level);
  - squares mapped as areas are not drawn;
  - bridges that end at building doors are anchored to the ground;
  - no levels of detail.
- Revisit when:
  - another country's data is added (sources and the coordinate pipeline
    become per-region);
  - ingest has to be cross-platform reproducible (for example, to let
    contributors verify baseline ids).
