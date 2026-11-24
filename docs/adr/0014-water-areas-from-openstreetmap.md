# ADR-0014: Take water areas from OpenStreetMap and place them as level surfaces

- **Status:** Proposed
- **Date:** 2026-11-24
- **Deciders:** @victorpicaud
- **Supersedes / superseded by:** none (adds a source to ADR-0006; uses the
  layer proposed in RFC 0003)

## Context

[RFC 0003](../../rfcs/0003-water-layer.md) proposes a `water` layer. This
ADR decides where its data comes from, how the ingest places it, and how
the client draws it.

Facts gathered for the Limmat in the zurich-altstadt world (2026-11-24):

- **The terrain carries the river's surface.** swissALTI3D models the
  river as its surface, at about 405.70 m (LN02). In the fixture tile,
  almost all terrain points in the river lie within 6 cm of one level.
  In region-frame coordinates the same level reads about 405.36 m,
  because the frame's height drops with the Earth's curvature.
- **The terrain model has a false ramp.** In the river north of the
  Rathausbrücke, swissALTI3D rises up to 1.3 m above the water over about
  280 m (LV95 N 1,247,380–1,247,660). A flat water surface there is
  hidden under the terrain.
- **OpenStreetMap:**
  - the Limmat is multipolygon relation 1598599 (`natural=water`,
    `water=river`), which runs on past the district to the Aare;
  - along the banks it matches the cadastral survey within 0.2 m
    (median) and 0.9 m (90th percentile), away from bridges;
  - it continues under bridges, which are mapped as separate areas.
- **swisstopo swissTLM3D:**
  - it has water areas (land-cover class with object types for flowing
    and standing water);
  - but it is published only as whole-country files: 2.9 GB (FileGDB) to
    4.8 GB (GeoPackage) per release, with no files per tile and no API
    for areas.
- **The cadastral survey (amtliche Vermessung):**
  - its land cover has survey-grade water areas;
  - the terms vary by canton, and six cantons require approval or a
    contract;
  - the canton of Zurich and the City of Zurich publish theirs openly.
- **OpenStreetMap Foundation guideline:** data from other sources stays
  outside the ODbL only if, in a given area, each kind of feature comes
  entirely from OSM or entirely from elsewhere. A canton or a city is not
  accepted as such an area; a whole country is.

## Options considered

### Source

#### A. OpenStreetMap water areas (recommended)

- Pros:
  - worldwide;
  - one small extra Overpass answer (211 KB for the district);
  - the ingest already handles OSM, its licence and its relations;
  - accurate enough along the Limmat;
  - continuous under bridges.
- Cons:
  - ODbL, so the water assets are ODbL;
  - one bad edit in OSM can break a river's relation;
  - tagging needs rules (fountain basins are also mapped as water).

#### B. swissTLM3D water areas

- Pros: swisstopo's licence, like the terrain; Switzerland-wide.
- Cons:
  - only as whole-country files, 13 to 17 times the rest of the ingest's
    downloads;
  - reading one needs either extracting a 10.8 GB GeoPackage, which
    ADR-0006 forbids, or fetching parts of the archive, which gives up
    ADR-0006's SHA-256 check;
  - Switzerland only, so other countries still need OSM.

#### C. Cadastral survey land cover

- Pros: the most precise outlines.
- Cons:
  - Swiss only;
  - terms differ by canton;
  - a canton is not a valid area under the OSMF guideline, so combining it
    with OSM elsewhere would put it under the ODbL.

It stays useful for checking quality, never copied into the world.

#### D. Detecting water from the terrain model

- Pros: no new data.
- Cons:
  - flat squares look like water;
  - the false ramp is missed;
  - nothing records where the water really is.

### Placement

#### A. Level surfaces (recommended)

Each water body gets one flat level per swissALTI3D tile.

- Pros: clean shorelines, because the banks rise above the surface and
  hide its edge.
- Cons: the false ramp hides the water unless the client deals with it.

#### B. Draped on the terrain

Like the street ribbons.

- Pros: never hidden.
- Cons: the water climbs the smoothed quay walls by up to 1.6 m, and
  follows the ramp.

#### C. Changing the terrain

Flattening or cutting it under the water.

- Cons:
  - it changes the collision surface and every replay near water;
  - with OSM outlines, it would make the swisstopo terrain ODbL.

It needs its own decision, with non-OSM data.

## Decision

Source A with placement A.

**Query.** A second Overpass query, with its own cache file, for:
- ways and relations tagged `natural=water`;
- the deprecated `waterway=riverbank`;
- `landuse=reservoir`.

Each statement has its own bounding box, and the answer includes geometry,
so relations come back whole. The existing query stays byte-identical, so
the existing OSM assets keep their bytes and their snapshot time. ADR-0006
spoke of one query per area; with this ADR there are two.

**Selection.**
- **Kept:** `natural=water` with no `water` tag, or with `water` set to
  `river`, `canal`, `lake`, `oxbow`, `lagoon`, `reservoir` or `pond`;
  `waterway=riverbank`; `landuse=reservoir`.
- **Left out:**
  - `amenity=fountain` and `amenity=drinking_water`;
  - `water=reflecting_pool` and `water=basin`;
  - `covered=yes`, `tunnel=culvert` and `location=underground`.

**Assembly.**
- Ways and multipolygon relations become polygons with holes, in LV95,
  repaired if invalid.
- **Nested rings:** they count as deep as they are mapped. A pond on an
  island in a lake is water.
- **Relations that cannot be assembled:** a relation that reaches the area
  but gives no area stops the ingest with an error naming it, rather than
  dropping the river silently. That covers rings that do not close, a
  member way without geometry or with a role other than outer or inner,
  and rings that enclose nothing.

**Placement.**
- **Outline:** buffered by 1 m, so the shoreline tucks under the banks,
  then clipped to each tile and to the world's area.
- **Level:** one per water body and per swissALTI3D tile (1 km²). It is
  the median terrain height at 2 m grid points at least 3 m inside the
  outline, within that terrain tile, plus 3 cm. It depends only on the
  body and the terrain tile, not on the area being ingested, so the
  fixture tile and the same tile in zurich-altstadt get the same level.
  Their meshes match except within a millimetre of where the area's edge
  cuts the tile.
- **Water too narrow for a level:** water with no such grid point in a
  terrain tile (narrower than about 6 m there) has no level and is not
  drawn. The report lists it. In zurich-altstadt that leaves out one
  15 m² pond.
- **Rivers across terrain tiles:** a river that crosses into another
  terrain tile gets a separate level there, so the surface can step at
  that 1 km boundary.
- **Mesh:** the surface is triangulated in LV95. Each vertex is converted
  to the region frame separately, with up normals.

**Asset.**
- one water asset per tile (RFC 0003);
- collider `none`;
- licence `ODbL-1.0 AND LicenseRef-swisstopo-OGD`;
- sources: the OSM water snapshot with the ids of the relations and ways
  that survive the clip, and the swissALTI3D items used for the levels.

**Report.**
- water area per tile;
- each level with its spread;
- the area of water hidden under terrain (the false ramp).

**One source per area.** Within any area, water stays all-OSM. A future
non-OSM water source must replace OSM across a whole country, clipped
exactly at its border, and needs its own ADR.

**Client** (decoration, like ADR-0012):
- **Water:** an opaque material:
  - dark green-blue;
  - water's reflectance (F0 about 0.02);
  - low roughness;
  - a procedural ripple normal map that drifts slowly;
  - it reflects the sky.
- **Terrain under water:** drawn 10 cm below the water surface, for
  display only, where all three hold:
  - it is flat (within 10 degrees of horizontal);
  - the water surface reaches at least 1 m beyond it in every direction
    (the ingest's buffer), so it lies inside the mapped outline;
  - it is at most 1.6 m above the water.

  Banks stay as they are. This shows the river along the false ramp
  (flat terrain up to 1.33 m above the water in zurich-altstadt). Terrain
  higher than 1.6 m over water is a bridge or a quay: the terrain model
  carries the Rathausbrücke, 2.68 m or more above the water, as ground.
  Collision is unchanged.
- **Travel:** landings at named places and from the map avoid water,
  meaning points inside the water whose highest surface is at most 1.6 m
  above it. A water mesh that fails to load counts as dry.
- **Credit:** the help panel shows the OpenStreetMap credit and the ODbL
  link.

## Consequences

- **Simulation:** unchanged. The fixture world gains one asset and a new
  world id. The sample replay is recorded again: its 20 checkpoints are
  unchanged, and only the world id in its header differs.
- **Fixtures:** the facade sample is regenerated for the new world id.
- **Your recordings:** walks recorded in a local zurich-altstadt world
  built before water no longer match it once it is rebuilt. Keep a copy of
  the old world to check them.
- **Known limits:**
  - players still walk on the river: the terrain is the collision
    surface;
  - the 1.6 m bridge-or-quay limit is measured in Zurich; a river whose
    terrain model rises higher above its water stays partly hidden;
  - along the false ramp the water is drawn correctly, but collision
    follows the ramp, up to 1.3 m above the visible water;
  - reflections show the sky but not the buildings; reflecting the
    buildings needs screen-space or planar reflections, which is a
    rendering decision of its own;
  - the ripples drift in one direction everywhere, because the data has no
    flow direction for areas.
- **Revisit:**
  - when swimming is designed;
  - when a national water source would replace OSM in a whole country;
  - if OSM edits break rivers often.
