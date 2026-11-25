# ADR-0015: Take trees and lights in Zurich from Stadt Zürich open data

- **Status:** Accepted (2026-11-25)
- **Date:** 2026-11-24
- **Deciders:** @victorpicaud
- **Supersedes / superseded by:** none (adds two sources to ADR-0006;
  fills the point layers proposed in RFC 0004; waits for ADR-0017; its
  Swiss source choice follows ADR-0019)

## Context

Roadmap M2 brings trees from Zurich's tree register and street lights from
the city's lighting data ([roadmap](../roadmap.md), M2).
[RFC 0004](../../rfcs/0004-point-features.md) proposes how the world format
stores such points. This ADR covers where they come from, how the ingest
fetches and checks them, what it keeps, and which source each kind of
feature has in each country. Tree heights belong to
[ADR-0016](0016-swisssurface3d-heights.md).

Facts gathered on 2026-11-24, for the zurich-altstadt district unless
stated otherwise:

- **Two CC0 datasets** from the City of Zurich. Both are updated weekly,
  both were dated 22.11.2026, and both recommend the credit "Quelle: Stadt
  Zürich":
  - the tree register (Baumkataster), WFS layer
    `baumkataster_baumstandorte`;
  - the public lighting of ewz, the city's electricity utility, WFS layer
    `ewz_brennstelle_p`.
- **Trees:** 81,403 city-wide; 1,005 in the district (643 street trees,
  362 park trees). Each has an id (`baumnummer`, e.g. `AA-1656`), genus,
  species, names, a size class, crown diameter, planting year, category,
  status, street, quarter and survey method, but no height. The register
  holds only trees the city manages: the canton's LiDAR finds 2,015 tree
  crowns in about the same area.
- **Lights:** 42,434 light points city-wide; 1,959 in the district. Each
  has an id (`nisnr`, e.g. `LEU105227`), internal ids, a type code, an
  orientation and a 2D position. There is no mount, height, colour or
  power. The data covers street and path lighting, lighting for shipping,
  and special lighting for objects, art and decoration.
- **OpenStreetMap** has 841 trees (none with a height) and 76 street lamps
  in the district.
- **One source per area.** The OSMF guidelines that ADR-0014 relies on
  keep share-alike off non-OSM data only while each feature type is all
  OSM or all non-OSM within a regional cut of at least a country (RFC 0004
  section 9). Filling gaps counts as mixing. Applying these 2D-map
  guidelines to a 3D world is our reading, until
  [ADR-0019](0019-odbl-and-the-published-world.md) is decided and the
  OSMF Licensing Working Group has answered.

## Options considered

Each question has a recommended option. On 2026-11-25 the maintainer,
@victorpicaud, accepted the recommended options (see Decision).

### 1. Which source supplies trees and street lamps in Switzerland

The choice depends on ADR-0019, which decides how the published world
meets the ODbL. Under (a) and (b), nothing from OpenStreetMap places,
filters, classifies or corrects a Zurich point. The points then hold no
OSM data, so the choice can change later.

#### (a) No OSM trees or street lamps anywhere in Switzerland (recommended)

The city's data in Zurich; later, other non-OSM sources such as other
cities' registers or trees detected from LiDAR; OSM only in other whole
countries.

- Pros:
  - under ADR-0019's options B and C, the Zurich points stay outside the
    ODbL. Under A, their manifests stay CC0, but the published world
    offers them under the ODbL;
  - the same rule as water: one source per kind of feature and country;
  - it can still move to (b) or (c);
  - the city's data is far richer here: 1,959 lights against 76 lamps.
- Cons:
  - the rest of Switzerland has no trees or lamps until another non-OSM
    source exists;
  - it binds every later Swiss source: no OSM to fill gaps, and no
    procedural lamps along OSM streets, in the data or in the client.

#### (b) OSM elsewhere in Switzerland, the Swiss layers also under the ODbL

- Pros: trees and lamps wherever OSM has them; CC0 allows offering the
  city's data under the ODbL.
- Cons:
  - the Zurich points must be offered under the ODbL in the published
    world too, which ends their separate licensing there;
  - OSM is thin: 76 lamps, and no tree with a height, in the district;
  - the density of trees and lamps changes at the city boundary.

If ADR-0019 puts the whole world under the ODbL, (b) costs nothing more in
licence terms, and the choice is about data quality only. Inside the
city, (a) and (b) give the same data; they differ only where a world
reaches beyond it.

#### (c) OSM only

- Pros: one source and one licence everywhere.
- Cons: 841 trees without heights and 76 lamps in the district, against
  1,005 trees and 1,959 lights. M2's screenshots of lamps lit in December
  would rest on 76 lamps.

**Not used as sources:**
- the canton's LiDAR tree crowns: the canton and the city portal label
  their licence differently, and the crowns were split with swisstopo's
  swissTLM3D. They stay a quality check, like the cadastral survey in
  ADR-0014;
- the city's Bauminventar: not updated since 2022, and it puts swissTLM3D
  trees under a CC0 label;
- the canton's lighting data: cantonal roads only, none in the district.

### 2. Fetching

The WFS is the only scriptable access: the portal's download links return
an HTML form. With `SRSNAME=EPSG:2056` the GeoJSON answer is in LV95 to
the millimetre; without it, in WGS84 rounded to about 1 m. An answer
carries only a `Date` header: no checksum, version or total. It is
byte-stable while the data is unchanged: three fetches of the trees in
square 2683-1247 gave one SHA-256. `RESULTTYPE=hits` gives the count:
1,027 trees and 2,023 lights in that square, as many as the answers held.
Whether the server caps larger answers is not known.

| Option | For | Against |
|---|---|---|
| **A. One request per LV95 kilometre square** (recommended) | integer boxes give the same cache key on every platform; the fixture and the district share answers, so a tile's points are the same in both; about 0.6 MB each | integrity rests on our own hash; a change in the service stops the ingest until the code follows |
| B. One request per area, as for Overpass | fewer requests | the fixture and the district get different answers; keys from floating-point boxes can vary by platform |
| C. The portal's files | the whole city at once | a form, not a scriptable download (ADR-0006) |

### 3. The version of a source

`sources[].version` says which data an asset was built from. The city
publishes no version, and serves only its current data.

| Option | For | Against |
|---|---|---|
| **A. The publisher's date plus the SHA-256 of the cached answer**, e.g. `2026-11-22 sha256:d11bec11…` (recommended) | one rule, readable and exact: the date tells the week, the hash pins the bytes | the date is city-wide, so every manifest from these sources changes at each refresh, even where no point changed |
| B. The SHA-256 alone | unchanged squares keep their manifests | the week is not visible without the report |
| C. The date alone, or the fetch time | simple | neither pins the bytes; the fetch time makes every fetch a new world |

The date is the portal's `dateLastUpdated` at fetch time; the hash is
written in full. With A, local builds get a new world id at each
refresh. In the canonical world, a refresh replaces an asset made by the
ingest only when its content changed (RFC 0002 section 7).

### 4. Keeping the inputs

After a weekly update, the city's earlier data cannot be fetched again.

| Option | For | Against |
|---|---|---|
| **A. Archive every raw answer that a published baseline uses, in a source archive beside the world store, addressed by its SHA-256** (recommended) | the baseline can be checked again after the city's data moves on; the fixture can be rebuilt from it; CC0 allows it | needs the bucket of [ADR-0010](0010-hosting-the-canonical-world.md), not set up yet; about 0.6 MB per changed square and dataset at each refresh |
| B. Commit the answers to Git | no new service | data in Git, against CLAUDE.md; the fixture's two answers alone are about 1.1 MB |
| C. Rely on the city | nothing to do | a baseline's inputs disappear within a week |

**The archive is not a world store.**
[RFC 0001](../../rfcs/0001-world-format.md) section 7 keeps downloaded
source data out of the world store, and names its objects by BLAKE3. The
archive is a separate area of ADR-0010's bucket, for example
`sources/sha256/<aa>/<hex>.json`. No world id covers it and no tile index
or manifest points to it; source entries name each answer's SHA-256 in
their version (section 3). It adds that area to ADR-0010's proposed
layout, as ADR-0010 now notes, and leaves RFC 0001 unchanged.

**Who may read it.** The raw answers hold `strasse`, `quartier` and
`status`, which never enter a world (section 5, RFC 0004 section 10).
- **Public, like the rest of the bucket (recommended; decided
  2026-11-25):** anyone can check a baseline. The answers are the city's
  own CC0 publication, about trees it manages and its lights, with street
  and quarter names but nothing about people. Under section 5's option
  A, an answer with a status outside the accepted six stops the run, so
  it never backs a baseline and is never archived.
- **Maintainers only:** street names stay out of public storage, but only
  maintainers can check a baseline.

### 5. Trees on private ground

The register's status list defines "Privat" (trees on private ground) and
"Bund", and its description says private trees are added. The published
layer has none today: its statuses are Strassenbaum, Strassenbaum (A),
Grünanlage, Obst, Kanton and Schulen. A weekly update could change that,
and no gate check would notice: G6 covers interiors, furniture and facades
only.

| Option | For | Against |
|---|---|---|
| **A. Accept those six statuses; anything else (Privat, Bund, empty or new) stops the run, naming the records** (recommended) | nothing on private ground enters unseen; a renamed status fails loudly instead of emptying the layer | a harmless new status stops refreshes until someone reviews it |
| B. The same list; leave out anything else and flag it in the report | refreshes continue | relies on a reviewer reading the report |
| C. Leave out only Privat and Bund | simple | a new value passes unseen |

In every option, `strasse` and `quartier` are never stored. The six
statuses cover street, park, fruit and school trees and the canton's
trees: trees of public bodies, which RFC 0004 section 10 counts as on
public ground.

### 6. What a light record is

A record gives a light point at a position, with an id and an
undocumented orientation. It does not say whether the point is a street
lamp, a floodlight or a light for shipping, how it is mounted, or how
high. Its type code is 0 ("Beleuchtung") for all but 10 points city-wide.

- **Kind.**
  - **A (recommended):** a generic light point (RFC 0004's `light`), with
    purpose, mount and height unknown.
  - **B:** every record a street lamp, like OSM's `highway=street_lamp`.
    Against: the data also holds lighting for shipping and special
    lighting, and a wrong class in published data is costly to correct.
- **Mount.** About 355 points stand 0.3–1.5 m from a facade, with their
  orientation pointing away from it: likely wall lamps.
  - **A (recommended):** leave the mount unknown. The client may infer it
    for drawing, as decoration (ADR-0012).
  - **B:** store `wall` for those points as an estimate, under rule
    `adr-0015:wall-near-shell:1`. Against: it rests on our reading of the
    orientation, and poles and wire-hung lamps still cannot be told apart.
- **Stacks.** The 1,959 records sit at 1,717 positions: 74 positions hold
  two or more records at exactly the same coordinate, up to 36. That stack
  faces two opposite directions, like a floodlight mast.
  - **A (recommended):** one point per record, each with its `nisnr` and
    orientation, and `count` unknown, because a record states no number of
    luminaires. The client groups points at one position when drawing, so
    a stack gives one model and one light.
  - **B:** one point per position, with `count` the number of records,
    named by its lowest `nisnr`. For: one point per structure, as drawn.
    Against: an id the city did not give, which changes when that light
    goes; a stack's different orientations must be merged; and it assumes
    one coordinate is one structure, which the data does not say.

  An independent review of the design asked for B, so that a stack is not
  drawn as overlapping models with multiplied light. A meets that in the
  client and keeps the city's ids, so it stays recommended.
- **Inside buildings.** 140 to 153 points, depending on the analysis, lie
  inside swissBUILDINGS3D footprints, most of them more than 1 m inside.
  The data does not say why.
  - **A (recommended):** keep them, and list them in the report. They are
    the city's records. The client does not light the street from inside
    a building.
  - **B:** leave out those more than 1 m inside (104 in one analysis).
    Against: it drops the city's records on a guess.

  Only swissBUILDINGS3D footprints count, never the OSM fallback buildings.
- **Orientation.** Read as the direction of the lamp's arm,
  counter-clockwise from grid east, `orientierung` points away from the
  facade for 89% of 398 lamps near facades (13% if read clockwise from
  north). That reading is ours, not ewz's. 160 values are exactly 0, about
  five times as many as any other value.
  - **A (recommended):** keep it as `yaw`, converted under our reading and
    marked as estimated (rule `adr-0015:yaw-from-orientierung:1`) until
    ewz confirms it. Exactly 0 is stored as unknown.
  - **B:** leave it out until then. Against: wall lamps cannot face the
    street.
- **Heights on bridges.** 191 points stand over the Limmat within 5 m of an
  OSM bridge deck. There swissALTI3D is the river surface, and the deck is
  2.0–4.1 m higher.
  - **A (recommended):** terrain only. These points sit at the river's
    level in the data. Until ADR-0019 is decided, the client draws them
    from their anchors, so a lamp head at a typical height ends up 1–3 m
    above the deck. Whether it may lift them onto the deck it draws, for
    display only, follows ADR-0019 (RFC 0004 section 5).
  - **B:** a swisstopo-only rule (surface model 1.5–6 m above flat
    terrain). It flags 265 points: 175 of the 200 that a check against
    OSM puts on bridges, and 90 elsewhere.
  - **C:** bridge outlines from the cadastral survey, with swissSURFACE3D
    heights: one more source, with its own licence review (ADR-0017).
    Left for later.

  OSM bridge decks are ruled out: they would put OSM data into the points.

### 7. Which tree attributes to keep

Crown diameters are "only estimated", and 8 m is used where none is known:
13,813 trees city-wide, 183 in the district. `baumtyp` is the species'
size when mature, not a measurement: 177 of the 201 district trees in its
"> 20 m" class measure under 20 m by one LiDAR estimate, and 446 of the
1,027 trees in square 2683-1247 have no class.

- **A (recommended): what the register records about the tree.**
  - Kept: genus (`baumgattunglat`), species (with `baumartlat`), taxon
    (`baumnamelat`, with any cultivar), planting year (`pflanzjahr`) and
    crown diameter (`kronendurchmesser`). A crown of exactly 8 m, and any
    missing value, is stored as unknown.
  - Not kept: `baumtyp`, which the report compares with measured heights;
    `strasse` and `quartier`; `objectid`, an internal database id; the
    German name; `status`, used only for the check in section 5.
  - Against: trees whose crown really is 8 m lose it.
- **B: everything as published.** Against: street names in the world, a
  size class that reads like a height, and a default that reads like a
  measurement.

## Decision

*Accepted on 2026-11-25 by @victorpicaud:* the recommendation, as one
package: (a) in section 1 and A in sections 2 to 7, with the rules below.
That is:
- **Source:** no OSM trees or street lamps anywhere in Switzerland; the
  city's data in Zurich (section 1, (a));
- **Fetching:** one WFS request per LV95 kilometre square (section 2, A);
- **Version:** the publisher's date plus the SHA-256 of the cached answer
  (section 3, A);
- **Inputs:** every raw answer that a published baseline uses is archived
  beside the world store, addressed by its SHA-256, and public like the
  rest of the bucket (section 4, A); the fixture's two answers as a
  release asset until then ("Archive" below);
- **Private ground:** the six statuses are accepted, and anything else
  stops the run (section 5, A);
- **Lights:** a generic light point per record, with mount unknown,
  `count` unknown, points inside buildings kept and listed, the
  orientation kept as an estimated `yaw`, and heights from the terrain
  only, bridges included (section 6, A in each part);
- **Tree attributes:** what the register records about the tree, without
  `baumtyp`, street, quarter, status or internal ids (section 7, A).

(a) is recommended whatever ADR-0019 decides. Under its options B and C,
it keeps the Zurich points outside the ODbL. Under A, it costs nothing in
the district, where (a) and (b) give the same data, and it keeps a later
move to B open. Under A, (b) is worth a new look once a world reaches
beyond the city.

[ADR-0019](0019-odbl-and-the-published-world.md) was accepted the same
day with option A: the whole published world is offered under the ODbL.
So the rules "No OSM in these points" and "No gap-fill" below are
choices, not licence needs, and the policy lives as written under
"option A" in "Where the policy lives". For the same reason, whether the
client lifts bridge lights onto the deck it draws is not a licence
question (ADR-0019, "OSM in how the client draws other data");
[ADR-0022](0022-drawing-trees-lamps-and-windows.md) decides how the
client draws them.

**Before any byte is ingested.** Nothing from these sources enters any
world, the fixture included, until:
- [ADR-0017](0017-licence-allowlist.md), which the roadmap lists as
  blocking new sources in M2, is accepted and allows `CC0-1.0` for world
  data;
- [ATTRIBUTION.md](../../ATTRIBUTION.md) lists both datasets, as ADR-0001
  requires of every new source;
- `op_world` validation checks that each asset's licence expression covers
  the licence of each of its sources. The gate skips G4 and G5 for
  baseline refreshes, so nothing checks this today. ADR-0016 asks
  ADR-0017 for the same check;
- RFC 0004 is accepted, after the pedestrian ADR, as RFC 0004
  recommends.

ADR-0017 was accepted on 2026-11-25, with `CC0-1.0` on its list for
world data, and the pedestrian ADR ([ADR-0021](0021-pedestrians.md))
and [RFC 0004](../../rfcs/0004-point-features.md) the same day. The list
itself, the ATTRIBUTION.md rows and the validation check are still to
come.

**No OSM in these points.** OSM roads, bridges, water and the OSM fallback
buildings never place, filter, classify or correct a Zurich point. They
appear only in the ingest report, as checks.

**No gap-fill.** No procedural trees or lamps along OSM streets, in the
data or in the client, in a country whose trees or lamps come from
non-OSM data. Where it is wanted, it covers a whole country that takes
that kind of feature from OSM, or nowhere.

**Where the policy lives** (RFC 0004 section 9). A check per tile cannot
see a breach of a country-wide rule.
- **Under ADR-0019's option B,** the source of each kind of feature in
  each country is a table in the world manifest, checked across the
  whole world before publishing. RFC 0004 would carry it.
- **Under option A,** the two rules above are choices, not licence needs.
  The Swiss choice is data: a source list per country and kind, kept with
  the ingest's source definitions, which any client gap-fill reads too.
  No gate check enforces it; the ingest and the review of each refresh
  do.

**Fetching:**
- one GetFeature per dataset and square, with the layer,
  `OUTPUTFORMAT=application/vnd.geo+json`, `SRSNAME=EPSG:2056` and the
  square as `BBOX=…,EPSG:2056`. Plain boxes only: complex filters met a
  firewall's 403;
- with each, a `RESULTTYPE=hits` request and the portal's
  `dateLastUpdated`;
- the raw answer is cached under `data/cache/stadt-zuerich/` like the
  Overpass answers (written to `.part`, then renamed). A sidecar holds the
  URL, fetch time, publisher's date, SHA-256, size, feature count and hit
  count. Every run checks the hash again;
- HTTPS to the city's hosts only, at most 32 MB per answer, JSON only.

**Archive** (section 4). Until ADR-0010's bucket exists, the answers stay
in local caches, as Overpass answers do today. For the fixture's two
answers (about 1.1 MB):
- **A release asset (recommended; decided 2026-11-25):** attach them to a
  GitHub release of this repository, with their SHA-256 and the
  publisher's date in `fixtures/README.md`. The fixture can then be
  rebuilt byte for byte without anyone's cache, and no data enters Git. A
  release is not content-addressed, so the SHA-256 is what pins the
  bytes.
- **Commit them:** simplest, but an exception to CLAUDE.md's "No large
  data in Git", to be recorded where it is made.
- **Local caches only:** once the city's data moves on, a rebuild depends
  on someone's cache.

**Validation.** The run stops on a coordinate system other than
`urn:ogc:def:crs:EPSG::2056`, a feature count that differs from the hits,
a record that differs between two squares, or a tree status outside the
list of section 5. A record that is not a point inside its square (plus
1 m), or whose id or attributes have the wrong form, is left out and
counted. The GeoJSON `bbox` member, which is in WGS84, is ignored.

**Ids and placement.**
- Point ids are `stadt-zuerich:baumkataster:<baumnummer>` and
  `stadt-zuerich:beleuchtung:<nisnr>`, the local id byte for byte. Never
  `objectid` or `objid`, which are internal database ids.
- The position is the source's. The height is the ground as RFC 0004
  defines it: the tile's terrain, from swissALTI3D.
- A point belongs to the tile that contains it, with half-open edges as
  for buildings. Points outside the area are left out. Records are sorted
  by id.

**Rules for estimated values** (RFC 0004). Only `yaw` is set by a rule:
`adr-0015:yaw-from-orientierung:1` reads `orientierung` as degrees
counter-clockwise from LV95 grid east, the direction of the arm, and
converts it to RFC 0004's yaw in the region frame. Exactly 0 gives no
value. If ewz answers otherwise, a new version of the rule follows.

**Provenance:**
- licence `CC0-1.0 AND LicenseRef-swisstopo-OGD`;
- credits "Quelle: Stadt Zürich", the publisher's wording, and
  "© swisstopo". CC0 requires no credit, but StreetWide credits every
  source;
- one source entry per answer used, so an asset that spans two squares
  lists two. Each has the version of section 3, licence `CC0-1.0`, the
  credit and the dataset page as URL;
- the swissALTI3D items that give the anchors' heights (item ids, the
  year as version, `LicenseRef-swisstopo-OGD`, "© swisstopo"), and for
  trees the swissSURFACE3D Raster items of ADR-0016.

**ATTRIBUTION.md**, in the same change as the ingest code: a row for each
dataset, a Stadt Zürich section (dataset pages, WFS layers, weekly
updates, the version rule and the archive), and the fixture's row.

**Errors** go to the city: Grün Stadt Zürich for trees, ewz for lights.
The next refresh brings the fixes in.

**Refreshes** come in batches, for example monthly, not weekly. The
cadence is the maintainer's choice.

**Report**, as for water: each answer with its date, hash, features and
hits; records left out, by reason; crowns at the 8 m default; survey
methods (689 surveyed, 255 digitised on screen, 61 unknown); stacks,
points in footprints and orientations of 0; and, as checks only, points
near OSM bridges or water, and the canton's crowns against the register.

## Consequences

With the recommendation accepted:

- **Simulation:** unchanged, if RFC 0004 keeps these points visual only.
  The change that adds them carries RFC 0004's replay proof.
- **Fixture:** tile 17/68646/45903 gains 33 street trees and 136 light
  points, with the fixture changes listed in RFC 0004 ("Compatibility and
  migration"). Of the lights, 13 are inside building footprints, 3 on an
  OSM bridge deck and 22 over the water beside it. A rebuild needs the two
  answers for square 2683-1247 (from a cache or the release, see
  "Archive") and the swissSURFACE3D tile (ADR-0016).
- **Weekly churn:** every refresh is a new world id, with RFC 0004's
  costs: the replays that G10 runs are recorded again, and RFC 0002 caps
  a change set at 50 changes. The district's first refresh alone adds 55
  assets (trees in 26 tiles, lights in 29), so it needs two change sets
  or an amendment to RFC 0002 (RFC 0004 section 8). Batching bounds this.
- **Ids:** whether a replacement tree keeps its `baumnummer`, and whether
  `nisnr` survives work on a light, is undocumented. If not, refreshes
  show removals and additions, and lineage per point breaks. Two weekly
  answers for one square are compared before the ingest code merges.
- **Known limits:**
  - private courtyards and gardens look emptier than they are, and stay
    so: RFC 0004 section 10 stores nothing there, from any source. Only
    objects on private ground that are visible from public ground may be
    added later, from a source chosen in its own ADR;
  - every light's height, mount and colour is a client default, and bridge
    lights sit at the river's level in the data;
  - about 2,000 light points per km² need a night budget
    ([ADR-0018](0018-performance-tier.md));
  - under (a), the rest of Switzerland has no trees or lamps yet.
- **Questions for the city,** which the maintainer sends (ADR-0022, open
  question 7, asks ewz the same):
  - ewz: what `orientierung` measures, and from which direction; whether 0
    means "not set"; what a stack at one coordinate is; why points lie
    inside buildings; whether `nisnr` is stable; whether mount, height or
    luminaire type could be published;
  - Grün Stadt Zürich: whether a replacement tree keeps its number.
- **Revisit** when the Licensing Working Group answers ADR-0019's
  questions (ADR-0019 itself was accepted on 2026-11-25), when ewz
  answers, when another Swiss source of trees or lamps appears, or if the
  city's service changes.
