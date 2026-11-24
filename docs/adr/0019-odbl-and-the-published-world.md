# ADR-0019: Decide how the published world meets the ODbL

- **Status:** Proposed
- **Date:** 2026-11-24
- **Deciders:** @victorpicaud
- **Supersedes / superseded by:** none (builds on ADR-0001, which it
  leaves as it is)

## Context

The roadmap's M1 opens with this decision: before StreetWide publishes a
canonical world, it must know how that world meets the Open Database
License (ODbL 1.0) of the OpenStreetMap (OSM) data in it
([roadmap](../roadmap.md), M1). [ADR-0001](0001-licensing.md) already
licenses OSM-derived assets under the ODbL and keeps them in their own
assets. This ADR covers the published world as a whole, and its notices.

**This is not legal advice.** The readings below are the project's own,
from the licence and the OpenStreetMap Foundation's (OSMF) guidelines.

**The ODbL** ([text](../../LICENSES/ODbL-1.0.txt)):
- a derivative database used publicly is offered under the ODbL only, and
  takes in no contents incompatible with it (4.4 a and d). Its individual
  contents may keep their own licences (2.4);
- a produced work, such as an image, may carry any licence, with a notice
  that it uses data under the ODbL (4.3);
- public use of either means offering, free of charge over the internet,
  the whole derivative database, or a file of its changes or of the
  method that made them (4.6);
- the database travels with the licence or its URI (4.2);
- independent databases beside it form a collective database, which
  share-alike does not reach (4.5 a).

**The OSMF guidelines** (guidance, not law):
- **Produced Work** (2014): a result "intended for the extraction of the
  original data" is a database.
- **Collective Database** (2016) and **Horizontal Map Layers** (2014):
  other data stays outside share-alike only while each data type is
  "either all OSM or all non-OSM within the same regional cut", in
  whatever layer it sits. Using OSM to fill gaps in, select, filter or
  correct the other data counts as mixing.
- **Regional Cuts** (2014): a cut is at least a country, with no holes
  and no cherry-picking. The guideline "primarily relates to" 2D maps.
- **Attribution** (2021): a game may credit OSM on a start-up screen, in
  the game view or on a credits page, making clear that the data is under
  the ODbL, for example with a link to openstreetmap.org/copyright.

**Database or produced work?** Unsettled. A world stores meshes per
feature, with the OSM element ids they came from, in a format built to be
read back by software: most likely a derivative database. Rendered frames,
screenshots and videos are produced works. The OSMF Licensing Working
Group (LWG) has not been asked.

**The world today.** zurich-altstadt on this branch has 1,491 assets:
- 1,407 from swisstopo only (`LicenseRef-swisstopo-OGD`): 30 terrain
  tiles and 1,377 swissBUILDINGS3D shells;
- 84 OSM-derived (`ODbL-1.0 AND LicenseRef-swisstopo-OGD`): 30 ribbon
  assets, 36 bridge decks, 8 water assets
  ([ADR-0014](0014-water-areas-from-openstreetmap.md)) and 10 fallback
  shells.

**The ten fallback buildings** are OSM footprints extruded where
swissBUILDINGS3D has no building
([ADR-0006](0006-ingest-pipeline-and-data-sources.md)): 10 of the
district's 1,449 OSM buildings, between about 3 and 9 m across, one of
them in the fixture tile. They share the `shell` layer with swisstopo's
shells, so one feature type has two sources in one country. Separate
assets do not help, because the guidelines look at feature types. While
they stay,
[ATTRIBUTION.md](../../ATTRIBUTION.md)'s claim that the ODbL part of the
world is exactly the assets licensed `ODbL-1.0` does not hold.

**Planned data.** Zurich's trees and lights are CC0: 1,005 trees and
1,959 light points in the district, where OSM has 841 trees and 76 lamps.
[ADR-0015](0015-zurich-trees-and-lights.md) asks whether Switzerland
takes trees and lamps from OSM at all. Tree heights
([ADR-0016](0016-swisssurface3d-heights.md)) are swisstopo data. New
sources wait for the allowlist ([ADR-0017](0017-licence-allowlist.md)).

**swisstopo's terms** allow use, processing, redistribution and
commercial use, with the credit "© swisstopo". They do not say which
licence derived data may carry, and they refer to the FSDI terms, which we
have not checked. ADR-0006 already puts swisstopo heights into ODbL
assets. The OSM wiki records that the OSM community held swisstopo's
credit requirement to be incompatible with the ODbL until swisstopo
clarified central attribution on 14 September 2021. We have not read that
clarification itself.

**Today:**
- for ingest data, nothing checks that an asset's licence covers its
  sources' licences: the gate skips G4 and G5 for baseline refreshes;
- G3 closes every asset whose licence contains `ODbL-1.0` to
  contributions;
- the client always shows the credits, and its help panel, open at
  start-up except in scripted screenshots, names the ODbL with a link to
  openstreetmap.org/copyright. The credit line names OpenStreetMap but not
  the ODbL;
- the world store carries no licence notice;
- contributions are CC BY-SA 4.0 under the DCO, without a CLA, so a later
  change needs every contributor's consent (ADR-0001). None from outside
  has been merged yet.

## Options considered

| | A. Whole world under the ODbL | B. Regional cuts | C. No OSM |
|---|---|---|---|
| Ten fallback buildings | stay | dropped in Switzerland (B1), or all Swiss shells under the ODbL (B2) | dropped |
| Water (ADR-0014) | unchanged | unchanged, unless display-only use counts as mixing | removed |
| Trees and lamps (ADR-0015) | any source, mixed freely | no OSM in Switzerland, or the Swiss layers under the ODbL | non-OSM sources only |
| Contributions | CC BY-SA 4.0 contents in an ODbL world, to confirm | unchanged | unchanged |
| CC0 and swisstopo data in the published world | under the ODbL | own terms | own terms |
| Work before publishing | notices; the swisstopo and contributor questions | an RFC for countries and a source table | new sources for streets, bridges and water |

### A. The whole world under ODbL 1.0 (recommended)

The published world, as one database, is offered under the ODbL. Each
manifest keeps the licence and credits of its own sources, as today, so
provenance stays exact and the OSM-derived assets can still be listed.

- **Obligations:** notices with the world and in the client (below), and
  the offer (4.6), which the public store proposed in
  [ADR-0010](0010-hosting-the-canonical-world.md) would meet.
- **Inputs** must fit the ODbL (4.4 d), a stricter test than "open":
  - CC0 fits;
  - CC BY 4.0 appears to fit with its credit kept: Overture's ODbL
    buildings theme includes CC BY 4.0 footprints;
  - swisstopo: see "What any option needs";
  - Copernicus GLO-30, planned for terrain in M7, asks for a notice and a
    liability sentence to be passed on: to be checked.

  A source that does not fit cannot enter the world.
- **Contributors** add to an ODbL world. Their content can probably stay
  CC BY-SA 4.0, as contents (2.4). Whether it fits 4.4 d is uncertain
  (question 3 below).
- **Reusers** get our copy of CC0 and swisstopo data under the ODbL; the
  originals stay available on their own terms. A subset made without OSM,
  such as the swisstopo shells alone, could probably still be offered on
  its own terms.

- Pros:
  - one rule, easy to state and check, with no source table and no
    country in the format;
  - each place can use its best data, which serves "the canonical world
    only moves upward";
  - a slip, such as OSM used to place or filter other data, cannot break
    the licence.
- Cons:
  - share-alike reaches data that is CC0 or credit-only upstream;
  - every source needs a compatibility check;
  - the contributors' question must be answered first;
  - hard to undo: published versions stay under the ODbL.

### B. Regional cuts: each kind of feature from OSM or from elsewhere, per country

ADR-0001 as it is, with the OSMF rule made checkable:
- in each country, each kind of feature (buildings, streets, water,
  trees, lamps) comes all from OSM or all from other sources;
- the rule is data: a source table of (country, kind, `osm` or
  `non-osm`) in the world manifest, which `op_world` and the gate check
  across the whole world, not per tile. Assets record their country. Both
  amend RFC 0001, and [RFC 0004](../../rfcs/0004-point-features.md) could
  carry them;
- a layer that changes source at a border is cut exactly there, along a
  border not taken from OSM;
- OSM never selects, filters, corrects, places or fills gaps in other
  data: no OSM bridge decks under CC0 lamps in the data, and no
  procedural lamps along OSM streets in a country whose lamps come from
  elsewhere. Uses for display only are discussed under "What any option
  needs";
- one (country, kind) may go under the ODbL as a whole: option A for that
  cell.

**Obligations:** the OSM-derived assets form the derivative database,
with the notices and the offer. The rest is a collective database
(4.5 a) and keeps its own terms.

**Fallback buildings:** B1 drops them in Switzerland before the first
publication. B2 keeps them and offers all Swiss shells under the ODbL.

- Pros:
  - non-OSM data keeps its own, often freer, terms;
  - follows the guidelines as written;
  - can move to A later; the reverse is harder.
- Cons:
  - a format change and world-wide checks before publishing;
  - a city's better data cannot be used while OSM supplies that kind
    elsewhere in the country, unless the cell goes under the ODbL;
  - fragile: implicit uses of OSM (decks, centrelines, borders, filters)
    do not show in manifests, and checks see only declared sources;
  - "a country, no holes" for a 3D world is our reading of a 2D
    guideline.

### C. No OSM in the canonical world

**Obligations:** none from the ODbL; each remaining source keeps its
credit.

- Pros: the simplest licensing; every layer keeps its own terms.
- Cons:
  - removes the streets, bridges, water
    ([RFC 0003](../../rfcs/0003-water-layer.md)), fallback buildings and
    named places;
  - swissTLM3D has roads and water, but only as whole-country files of
    2.9 to 4.8 GB, which ADR-0006 cannot read without an amendment;
  - abroad, Overture's transportation theme is ODbL too;
  - removing the bridge decks removes colliders, so replays near them
    change.

## Decision

*Proposed:* option A, with the ten fallback buildings staying in the
`shell` layer. **Not decided:** the maintainer, @victorpicaud, decides,
and the decision and its reasons are recorded here when made.

**Why A.**
- Reviews of the water and tree designs found four cases of OSM data
  placing, filling or filtering other data: bridge decks under CC0 lamps,
  procedural lamps along OSM streets, survey water with OSM filling its
  gaps under bridges, and the fallback buildings. Under A none of them
  breaks the licence.
- At planet scale, many of B's cells would go under the ODbL anyway: M7
  plans to combine footprints from OSM, Overture, Microsoft and Google.
- The cost falls on reuse of non-OSM data through our copy, and the
  original sources stay open.

**Before publishing under A:** confirm swisstopo (below). Answer the
contributors' question before the first outside contribution: if CC BY-SA
4.0 contents do not fit, a new ADR changes the contribution terms and
supersedes that part of ADR-0001.

**If B is chosen:** B1. The ten buildings are small, and a contributor
can add each one as a shell, with evidence (RFC 0002).

### What any option needs

- **Checks:** the licence rules proposed in ADR-0017, on baseline
  refreshes too: every source's licence appears in the asset's licence,
  and every source's credit in its credits. A feature id's prefix never
  decides a licence.
- **Notices with the world:** the ODbL notice, the licence texts and the
  credits sit at the store's root, beside `current.json` (ADR-0010). The
  fetch command planned for M1 copies them and prints the notice. This
  needs no format change; a licence field in the world manifest can come
  with a later RFC 0001 amendment.
- **Notices in the client:** keep the credit line and the help panel, and
  add a credits page listing each source, its licence and a link.
  Screenshots and videos the project publishes carry the 4.3 notice.
  Scripted screenshots hide the help panel, so the credit line, which they
  show, should name the ODbL too.
- **OSM in how the client draws other data.** The client can use OSM data
  to change how it draws a non-OSM layer, without changing any asset:
  - ADR-0014's client already draws swisstopo terrain lower under OSM
    water;
  - it could lift CC0 lamps onto OSM bridge decks
    ([RFC 0004](../../rfcs/0004-point-features.md) section 5);
  - procedural lamps along OSM streets would fill gaps in a lamp layer,
    which ADR-0015 proposes to rule out where lamps come from non-OSM
    data.

  Under A, none of this is a licence question. Under B, our reading: a
  rendered frame is a produced work (4.5 b), so drawing a layer
  differently does not change the data. But the Horizontal Map Layers
  guideline is written for produced maps, and may count a non-OSM layer
  adjusted with OSM as mixing. Filling gaps is the guideline's own example
  of mixing, so gap-fill stays out under B. **Recommended under B:** allow
  the two adjustments that change no data and add no feature (terrain
  under water, lamps on decks), and ask the LWG (question 5). If it counts
  them as mixing, the client drops both. The reviews of the water and
  tree designs accepted render-time masks and lifting, and counted
  gap-fill as mixing. This reading follows them, but it is ours, not the
  OSMF's.
- **The offer:** every published world stays in the public store, and
  the ingest stays open source.
- **swisstopo inside an ODbL database.** Our reading: swisstopo data can
  be contents of an ODbL database (2.4), with "© swisstopo" given
  centrally, in the client's credits and in ATTRIBUTION.md, as the OSM
  community has accepted since 2021. To confirm before publishing: quote
  swisstopo's clarification or ask swisstopo, and check the FSDI terms.

### The question for the Licensing Working Group

The maintainer sends it before publishing; the answer is recorded here.

StreetWide is an open-source game world built from OpenStreetMap and
other open data. A world is a content-addressed store of glTF meshes per
feature or map tile (roads, bridges, water, a few buildings), each with a
JSON manifest listing the OSM element ids and snapshot it came from. A
game client renders it.

1. Is such a world a Derivative Database or a Produced Work? Is its
   rendered view a Produced Work?
2. If it is a database, do the Collective Database, Horizontal Map Layers
   and Regional Cuts guidelines apply as they do to 2D maps, with a kind
   of 3D object (buildings, roads, water, trees, street lamps) as the
   feature type? For example: swisstopo building shells, plus ten
   buildings extruded from OSM footprints where swisstopo has none.
3. If we offer the whole world under the ODbL, may its contents keep
   their own licences (section 2.4): players' meshes under CC BY-SA 4.0,
   and data whose terms require a credit, such as swisstopo's or
   CC BY 4.0?
4. Does a public store holding every published version, free of charge,
   with the open-source ingest, meet section 4.6?
5. The client draws some non-OSM data differently where OSM data lies,
   for display only: terrain from another source drawn lower under OSM
   water, or street lamps from a city's open data lifted onto OSM bridge
   decks. The world's data does not change. Is such a rendering a Produced
   Work that leaves the other data outside share-alike, or mixing under
   the Horizontal Map Layers guideline?

## Consequences

- **Simulation:** licences do not reach it. Removing fallback shells (B1)
  or bridge decks (C) removes colliders: worlds get new ids, replays are
  recorded again, and checkpoints change where those colliders were
  loaded. B1 also removes the fixture tile's fallback shell.
- **Privacy:** unchanged. The credit names "OpenStreetMap contributors",
  never a mapper.
- **Under A:**
  - ATTRIBUTION.md, the README and the roadmap say that the world is
    offered under the ODbL, and ATTRIBUTION.md drops its extraction claim;
  - CONTRIBUTING.md ("Contributing world content") and the facade guide
    (`docs/contributing/facades.md`) tell contributors that their content
    is published inside a world offered under the ODbL. This is done
    before the first outside contribution, with the answer to question 3;
  - RFC 0001 says that an asset's licence is that of its contents;
  - ADR-0014's one-source rule for water and ADR-0015's rule keeping OSM
    out of the Zurich points become choices, not licence needs;
  - G3 keeps working, since asset licences do not change;
  - ADR-0017's list records whether each licence may be offered under
    the ODbL.
- **Under B:** the RFC for countries and the source table, before
  publishing; the B1 or B2 choice; ADR-0015 and RFC 0004 follow the
  table. The client's display-only uses of OSM follow the LWG's answer to
  question 5.
- **Under C:** new sources for streets and bridges, and ADRs replacing
  ADR-0006's OSM sources and ADR-0014.
- **Revisit:**
  - when the LWG answers, in particular if a world is a produced work, or
    if display-only uses count as mixing;
  - if a key source turns out not to fit the ODbL (under A);
  - when a second country is added (M6);
  - if the OSMF changes its guidelines.
