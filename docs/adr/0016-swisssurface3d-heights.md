# ADR-0016: Use swissSURFACE3D for measured heights

- **Status:** Proposed
- **Date:** 2026-11-24
- **Deciders:** @victorpicaud
- **Supersedes / superseded by:** none (adds a source to ADR-0006; serves
  RFC 0004 and RFC 0002 section 5)

## Context

Milestone M2 of the [roadmap](../roadmap.md) plans "an ADR for a LiDAR
source (swissSURFACE3D), used for tree heights and for RFC 0002's measured
metrics in the gate". This is that ADR. It covers only how heights are
measured, for two users:

- **Tree heights.** [ADR-0015](0015-zurich-trees-and-lights.md) proposes
  taking trees from Zurich's tree register (CC0), as point features of
  [RFC 0004](../../rfcs/0004-point-features.md). The register has no height.
- **RFC 0002's metrics.**
  [RFC 0002](../../rfcs/0002-contributions-and-merge-rules.md) section 5
  scores shells by their distance from "LiDAR roof and wall points" and
  terrain by its distance from "LiDAR ground points", once an ADR and an
  ATTRIBUTION.md entry exist (its question 3). No metric exists yet, so
  shell and terrain changes are judged by review alone.

The constraints:
- **Data, not decoration**
  ([ADR-0012](0012-client-realism-from-procedural-detail.md)): the world
  records what was measured; what the client invents stays in the client.
- **Licence before ingest** ([ADR-0001](0001-licensing.md)): a licence
  review and an [ATTRIBUTION.md](../../ATTRIBUTION.md) entry come first,
  and the roadmap lists the licence allowlist
  ([ADR-0017](0017-licence-allowlist.md)) as unblocking new M2 sources.
- **No OSM in non-OSM data,** a proposed rule: ADR-0015 ("No OSM in
  these points") and RFC 0004 section 9 propose that OpenStreetMap never
  selects, masks or places anything in a non-OSM asset. It is a licence
  need under option B of
  [ADR-0019](0019-odbl-and-the-published-world.md) and a choice under A.
  [RFC 0001](../../rfcs/0001-world-format.md) section 2 already keeps OSM
  geometry out of other assets.

Facts gathered for the zurich-altstadt district (2026-11-24):

- **The register:** 1,005 trees, none with a height. `baumtyp` is the
  species' size at maturity: 177 of the 201 trees in its "over 20 m" class
  measure under 20 m. Crown diameters are estimates; 183 carry the 8 m
  default.
- **swissSURFACE3D** is on swisstopo's STAC in two forms, in swissALTI3D's
  1 km tiles and under its terms:
  - **Raster:** a 0.5 m surface model with vegetation and buildings,
    without power lines and masts; ±20 cm in position, ±10 cm in height.
    The district's tile (2683-1247) has items from 2018 and 2024. The 2024
    file is 13.5 MB, a little less than the terrain tile (16.4 MB), and
    matched its published SHA-256.
  - **Classified point cloud:** COPC LAZ, 155 MB for the same tile in
    2024, about 200 MB per km² since 2024.
  - About one sixth of Switzerland is updated each year.
- **Leaf-off:** the LiDAR NFI model, built from swissSURFACE3D, describes
  its data as leaf-off, so deciduous crowns read low.
- **Data years:** the surface model is from 2024, the terrain from 2026,
  and the register is updated weekly. The tile's flight date, and whether
  an item's year is its flight year, were not checked.
- **The canton's crowns** (Einzelbäume 2021/2022, GIS-ZH 608) come from a
  leaf-off flight between 31 December 2021 and 19 April 2022. There are
  2,015 in the district. 847 register trees stand in one and 33 more
  within 2 m; 125, most planted since 2000, have none; 70 crowns are
  shared by 193 trees in rows. By the canton's own account, its 2021/22
  heights are systematically lower than its 2017/18 ones.

**Heights measured** as the largest value of the raster minus swissALTI3D
near each trunk, with buildings masked. Two rules were tried; both give
every register tree a value:

| Rule | Searched | Median (5th–95th percentile) | Low values | Against the canton's crowns (847 trees) |
|---|---|---|---|---|
| 1.5 m rule | 1.5 m around the trunk; swissBUILDINGS3D footprints plus 0.5 m masked | 10.4 m (0.5–20.3 m) | 85 under 3 m | 0.63 m higher (median), middle half −0.12 to +1.09 m; 56% within 1 m, 81% within 2 m |
| Crown rule | half the register's crown diameter, kept between 1 and 3 m; building pixels masked | 11.2 m (1.3–21.0 m) | 63 under 2 m | not compared |

- The rule changes the result, and both rules rest on one 1 km² tile. A
  third proposal, the maximum over the whole crown, was not measured.
- **Planted after the flight:** 50 district trees were planted in 2024 or
  later. By the crown rule 15 of them measure under 2 m; the other 35 take
  a height from something the flight saw (an earlier tree, a neighbour, or
  the young tree itself), and the data cannot tell which.

## Options considered

### Source of tree heights

#### A. swissSURFACE3D Raster minus swissALTI3D (recommended)

The largest height of the surface above the terrain within a radius of
the trunk, with building footprints masked. From swisstopo data only.

- Pros:
  - a value for every register tree in the district;
  - the terrain's licensor, terms and path: no new licence or credit;
    STAC, the same tiles, a SHA-256 per file
    ([ADR-0006](0006-ingest-pipeline-and-data-sources.md)), and older
    years kept so far (the 2018 item);
  - one 13.5 MB file per km² in the district, and no new dependency: the
    ingest's `Dem` class reads it as it is. Only the item-id pattern in
    `tools/ingest/stac.py` must accept the hyphen in
    `swisssurface3d-raster`;
  - the 1.5 m rule reads a median 0.63 m above the canton's crowns, and
    81% of trees agree within 2 m.
- Cons:
  - no classes: the highest point near a trunk may be a neighbour's crown,
    a vehicle, a wall or an arcade; only buildings are masked;
  - leaf-off, and the result depends on the rule;
  - it does not serve RFC 0002's metrics (below);
  - it does not measure lamps: at street lights it stands a median 0.8 m
    above the terrain;
  - Switzerland only.

#### B. The classified swissSURFACE3D point cloud

Heights from vegetation points alone, as the LiDAR NFI model does with
classes 3 to 5.

- Pros:
  - no building mask needed;
  - the only swisstopo product with the classified points that RFC 0002's
    metrics need: one product for both jobs.
- Cons:
  - 155 MB for the district's tile, about 11 times the raster;
  - a COPC/LAZ reader, which is not among the pinned dependencies
    (`tools/requirements.txt`);
  - reading parts of files by HTTP range would give up ADR-0006's
    whole-file SHA-256 check, as ADR-0014 found for swissTLM3D;
  - more ingest work before M2 needs it; still leaf-off.

#### C. The canton's crowns or the NFI height models, as checks only

- **The canton's single trees:** "OGD - freie Nutzung und Weitergabe",
  with no licence id on opendata.swiss and no SPDX identifier (only the
  city's portal calls it CC0), so as a source it would need a LicenseRef
  or the canton's written confirmation. Its crowns were split with
  swissTLM3D 2025 points, so it also carries swisstopo data. It is older
  than the surface model and misses 125 register trees.
- **The NFI model from stereo images** (EnviDat, DOI 10.16904/1000001.1):
  1 m and leaf-on, licensed ODbL with the Database Contents License.
  Copying its heights would bring the ODbL into the tree assets. As a
  check it could show how much the leaf-off flights lower heights; the
  canton's crowns, also leaf-off, cannot.
- **The NFI model from LiDAR** (EnviDat, DOI 10.16904/envidat.664):
  CC BY 4.0, 0.5 m, made from the same swissSURFACE3D points. It is one
  national file per year: about 148 GB for 2025 (17 GB for 2022). From
  the same flights, it checks the building mask, not the flights.
- **The city's other tree data:** the Bauminventar is frozen since 2022
  and labelled CC0 over swissTLM3D positions marked "© swisstopo"; a 2022
  canopy model (CC0) comes only through a download form or WMS.

- Pros: independent or differently made comparisons, and no licence
  question while nothing is copied.
- Cons: none can be a source today, for reasons of licence, size, age or
  access.

#### D. No heights in the data

Trees carry no height, and the client sizes them.

- Pros: no new source, download or code; nothing measured can be wrong.
- Cons:
  - every size is a client guess: the register cannot help, and a formula
    from genus and crown (about ±25%) needs measured heights to calibrate;
  - no simpler licence: tree anchors stand on swissALTI3D anyway;
  - the roadmap's "heights from LiDAR" and RFC 0002's metrics stay open.

### Height policy for the data

Some trees get no reliable measurement in any option.

#### 1. The measured height, or null (recommended)

A height is stored only when the rule measured one. Otherwise it is null,
and the client draws a default it chooses, as decoration (ADR-0012).

- Pros:
  - the data holds only measurements, and a null shows where measurement
    failed;
  - changing the client's defaults never changes the world.
- Cons:
  - 85 to 135 of the 1,005 district trees have no height: 85 under 3 m,
    and up to 50 planted in 2024 or later (the groups overlap);
  - each reader needs its own default;
  - the format must be able to say "unknown". RFC 0004's proposed point
    sets can; with meshes, every tree needs a size, so a default would
    enter the data.

#### 2. The measured height, else an estimate by a named rule

Where measurement fails, the ingest stores a formula value, listed in
`estimated` (RFC 0004) with the rule named: for example a genus × crown
formula calibrated on the district's measured heights (about ±25%), or the
genus's median height where the crown is unknown (a crown of exactly 8 m
counts as unknown).

- Pros: every tree has a height, and every reader draws the same size.
- Cons:
  - formula values in canonical data, which pass for measurements in any
    reader that ignores `estimated`;
  - they rest on crown diameters that are estimates or defaults;
  - changing the formula changes canonical data.

### Where a height is measured from

RFC 0004 defines `height_mm` from the anchor, the ground at the trunk, to
the top. The figures above measured from the ground under the highest
pixel instead. On a slope the two differ: within 1.2 m of the district's
trunks, the lowest ground lies a median 0.10 m below the trunk (90th
percentile 0.49 m, maximum 2.42 m), and the median slope at the trunks is
3.7°.

#### (a) From the anchor (recommended)

The top is still the pixel with the largest surface minus terrain; the
height is that pixel's surface minus the ground at the trunk.

- Pros: the field means what RFC 0004 says, the height above the tree's
  base; the client, which draws from the anchor, puts the top where the
  surface model saw it.
- Cons: the comparison with the canton's crowns must be run again.

#### (b) From the ground under the top, as tested

- Pros: the figures above hold as they are.
- Cons: RFC 0004 must define `height_mm` as the top above the ground
  beneath it, and a tree drawn from its anchor misses the top by the
  difference.

### RFC 0002's measured metrics

**The raster does not serve them.** It keeps one top height per 0.5 m
cell, without classes:
- no wall points, and no way to tell a roof from a tree over it;
- no ground under trees and buildings;
- its terrain counterpart, swissALTI3D, is the baseline terrain's own
  source (ADR-0006), so scoring terrain against it would favour the
  baseline over better data.

**Only the classified point cloud (option B) can.** The gate would need:
- the point cloud of each tile a change touches, wherever the gate runs
  (155 MB for the district's tile);
- a COPC/LAZ reader, in Rust if `op_world`'s gate computes the metric;
- the reference pinned by STAC item and SHA-256 and named in the check
  report, so a result can be checked again;
- a deterministic computation (RFC 0002 section 4) and tolerances (its
  question 2);
- a look at the classes: whether they separate roofs from walls, or only
  mark buildings, was not checked;
- outside Switzerland, another reference per country (its question 3).

## Decision

*Proposed, not decided.* The maintainer (@victorpicaud) chooses; the
choice and its reasons are recorded here when made. The recommendation:

- **Tree heights:** option A with the 1.5 m rule, under policy 1,
  measured from the anchor.
- **Checks:** the canton's crowns in the ingest report (option C), never
  copied into the world.
- **RFC 0002's metrics:** the classified point cloud (option B), adopted
  in its own ADR when the gate's metrics are built.

A measures every register tree with data already used for the terrain,
for one more file per km² and no new dependency. The 1.5 m rule is the
one checked against an independent reference, and it does not use the
register's crown diameters. Measuring from the anchor keeps the meaning
RFC 0004 gives the field, at the cost of repeating that check. Policy 1
keeps the world to measurements, as ADR-0012 asks. B alone serves
RFC 0002, but it costs 11 times the bytes and a new reader before any
metric is designed.

If the recommendation is accepted, the ingest works as follows.

**Product.** `ch.swisstopo.swisssurface3d-raster`: the 0.5 m GeoTIFF of
each 1 km tile (`_0.5_2056_5728.tif`), the latest year per tile, checked
against its STAC SHA-256.

**Height.** For each tree, the top is the 0.5 m pixel with the largest
value of surface minus terrain, among the pixels whose centres lie within
1.5 m of the trunk, leaving out pixels within 0.5 m of a swissBUILDINGS3D
footprint. The height is that pixel's surface minus the swissALTI3D ground
at the trunk, where the anchor stands (RFC 0004 section 5); rounded half
up to the millimetre. It depends only on the tree and the source tiles
around it, not on the area being ingested, so the fixture tile and the
same tile in zurich-altstadt agree. A disc that crosses a 1 km tile edge
reads both tiles. The comparison with the canton's crowns is run again
with this rule before the ingest code merges.

**Null**, with the reason in the report, when:
- the value is under 3 m or over 50 m, the band the canton's crown
  product keeps;
- the register's planting year is the surface model's year or later;
- every pixel of the disc is masked or has no data.

**Never used:** OpenStreetMap, to select, mask or correct a height (the
mask uses swissBUILDINGS3D only, so the ten OSM fallback buildings are not
masked); `baumtyp`; heights from the canton's crowns, the Bauminventar or
the NFI models.

**Only at register trees.** The surface model also shows private gardens
and courtyards. Nothing from it enters the world except a height at a
register tree. Detecting other trees in it would need its own ADR, and
RFC 0004 section 10 keeps private courtyards and gardens empty from any
source: only trees on private ground that are visible from public ground
could be added.

**Provenance.** Each tree asset lists the swissSURFACE3D Raster items it
used, like its swissALTI3D entries: the item ids (for example
`swisssurface3d-raster_2024_2683-1247`), the year as version,
`LicenseRef-swisstopo-OGD` and "© swisstopo". The licence stays
`CC0-1.0 AND LicenseRef-swisstopo-OGD`, as ADR-0015 proposes, because the
anchors already stand on swissALTI3D.

**Report**, per world:
- measured and null heights, each null with its reason, and percentiles;
- trees within 3 m of a building, where the mask cuts into the disc;
- as checks only: trees within 3 m of an OSM fallback building, which the
  mask does not cover, and the comparison with the canton's crowns. The
  comparison is optional, and the world's bytes never depend on it.

**Licence first.** No height enters any world, the fixture included,
until:
- ADR-0017 (licence allowlist) is accepted. The review is short: the
  terms are the terrain's;
- ATTRIBUTION.md has a row for swissSURFACE3D Raster;
- validation checks that every asset's licence covers each source's
  licence, as ADR-0015 also requires (baseline refreshes skip G4 and G5);
- RFC 0004 and ADR-0015 are accepted.

ADR-0017 should also give `LicenseRef-swisstopo-OGD` one text and one URL:
ADR-0001 and ATTRIBUTION.md link different swisstopo pages, and
`LICENSES/` has no text for it. If
[ADR-0019](0019-odbl-and-the-published-world.md) puts the whole world under
the ODbL, these heights go with it, as terrain heights in OSM-derived
assets already do (ADR-0006 section 7).

## Consequences

If the recommendation is accepted:

- **Simulation:** unchanged. Heights belong to visual-only points
  (RFC 0004); `op_sim` never reads them, and no replay checkpoint changes.
  World ids change when heights do, as for any asset.
- **Contracts:** none change here; the height field, null and `estimated`
  are RFC 0004's.
- **Ingest:** one more STAC product, the item-id fix (with a test that
  existing ids parse as before) and one more file per km²; no new
  dependency. Rebuilding the fixture needs the 2024 surface tile
  2683-1247 in the cache, as it needs the terrain tile today.
- **Refreshes:** a new surface-model year changes the tree heights in its
  tile, through a baseline refresh (RFC 0002 section 7).
- **RFC 0002:** question 3 narrows to the classified point cloud, in its
  own ADR, needed at the latest for M6, whose RFC asks for measured
  metrics where reference data exist. Until then, shell and terrain
  changes are judged by review.
- **Bridges:** lamp heights on bridges stay ADR-0015's question. The
  raster sees the decks, but alone it cannot tell which lamps stand on
  one (ADR-0015 section 6).
- **Known limits:**
  - leaf-off flights: deciduous trees read low;
  - one 1 km² tile so far: the radius, the floor and the agreement with
    the canton may not hold elsewhere;
  - the highest point within 1.5 m can belong to a neighbour; 255 trunk
    positions were digitised on screen (0.5 to 2 m), and 56 trees stand
    within 3 m of a building, where the mask removes part of the disc;
  - surface and terrain come from different years, and neighbouring tiles
    may come from different flights;
  - trees under 3 m get no height.
- **Open questions:**
  - whether a STAC item's year is its flight year: the planting rule
    assumes the flight was not later;
  - whether the point cloud's classes separate roofs from walls;
  - how far heights measured from the anchor move from the tested
    figures, and from the canton's crowns, on sloping ground.
- **Revisit:**
  - when more tiles are ingested, to check the radius, the floor and the
    agreement again;
  - when the gate adopts the point cloud: tree heights could then use its
    vegetation classes, with no building mask;
  - if trees become colliders (the pedestrian ADR, RFC 0004): heights
    could then enter the simulation, and a new surface year would change
    replays near trees;
  - outside Switzerland. The rule works wherever an open surface and
    terrain model pair exists, such as the Netherlands' AHN (CC0). Canopy
    maps made by machine learning, such as Meta and WRI's (CC BY 4.0), are
    estimates: under policy 1 they never count as measured.
