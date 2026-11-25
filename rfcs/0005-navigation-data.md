# RFC 0005: Navigation data: lanes, places and population profiles

- **Status:** Draft
- **Authors:** @victorpicaud
- **Created:** 2026-11-25 (comment period until at least 2026-12-09)
- **Contract version affected:** world format 0.1, amended in place or
  moved to 0.2.0, in one decision with RFC 0003 and RFC 0004; contribution
  contract 0.1, amended in place or bumped (a choice); the next gate
  version (`GATE_VERSION` is 0.1.2 today; 0.1.4 only if RFC 0004's 0.1.3
  lands first). See "Compatibility and migration".

## Summary

The world format gets a layer for people on foot. The client reads it in
M2 to draw its crowd, as [ADR-0021](../docs/adr/0021-pedestrians.md)
decided (option C2: people drawn by the client from a shared seed,
outside the simulation).
- **`navigation`: where people walk, where they go, and how many.** One
  canonical-JSON `streetwide.navigation` document per tile and source
  holds:
  - a graph of nodes and lanes: polylines in integer millimetres, each
    with a kind (sidewalk, footway, crossing, steps, pedestrian or
    living street, and three kinds this RFC adds) and a width;
  - walkable areas: squares, and pedestrian streets mapped as areas;
  - places where trips start and end or people sit and stand:
    entrances, stops, squares, benches, terraces, viewpoints and
    fountains;
  - the population: hourly curves per day type with a factor per month,
    a holiday calendar, people per day on each lane, and trips or
    occupants at each place. Every value a rule set names its rule.
- **Visual only in M2.** The layer has `collider: none` and never enters
  the simulation. Its shape (integer millimetres in the region frame, one
  graph per tile joined at tile edges, stable ids, lanes checked against
  colliders) is meant to become simulation input later without a format
  change.
- **Lanes matched to building shells.** The ingest moves lanes, and
  snaps doors onto walls, by at most 1 m, and marks what it cannot
  clear. A new gate check, G11, keeps any change from newly blocking a
  lane. G9 and G10 stay as they are, as ADR-0021 says.
- **Closed to contributions.** Lanes and places come from OpenStreetMap
  (ODbL) and are fixed there. The population's numbers come through the
  ingest's rules and sources (G3).

This amends [RFC 0001](0001-world-format.md) sections 2, 3, 5 and 7, as
[RFC 0004](0004-point-features.md) does for points, and
[RFC 0002](0002-contributions-and-merge-rules.md) sections 3 and 4. It
drafts an answer to ADR-0021's open question 6, the licences of lanes and
profiles.

**Nothing here is decided.** The maintainer, @victorpicaud, decides
after the comment period. Each open choice is drafted with a
recommendation. Five choices are tied to other documents:
- **ADR-0021's open question 6** is settled with this RFC and
  [ADR-0019](../docs/adr/0019-odbl-and-the-published-world.md), before
  the ingest builds lanes (section 10). The maintainer records the
  answer in ADR-0021.
- **The client prototype first.** ADR-0021's order of work is a
  throwaway client prototype first, then "the navigation-data RFC,
  shaped by what the prototype found". Drafting this RFC now departs from
  that order. Recommended: revise it after the prototype has run its
  checks, and accept it only then.
- **The contract version** is chosen once for RFC 0003, RFC 0004 and this
  RFC. RFC 0003 and RFC 0004 describe that choice for two RFCs; their
  texts follow if the maintainer takes this RFC into it ("Compatibility
  and migration").
- **The order of a refresh's sets** is
  [ADR-0020](../docs/adr/0020-baseline-refreshes.md#5-the-workflow)'s,
  accepted: visual-only sets first. This RFC recommends keeping it (section 9, "Refreshes").
  The options that change it would need their own decision: an ADR that
  supersedes that row of ADR-0020 section 5, or an amendment beside RFC
  0002's questions 9 to 16 (A1 to A8).
- **Refreshed lanes that a contributed shell blocks:** recommended, a new
  hold in ADR-0020's refresh planner (section 9).

## Motivation

- **ADR-0021 needs world data.** The client's population is a pure
  function of the world's lanes, places and population profiles, the
  session seed, the crowd's clock, the population's version and the
  character-asset catalogue. Under "What this means for other
  decisions", ADR-0021 asks for "a world layer for walking, read by the
  client in M2 and shaped to become simulation input later without a
  format change".
- **The format and the ingest have none of it.** The ingest keeps no
  walking graph: sidewalks become 2 m ribbons without topology, squares
  mapped as areas are dropped, and its OSM query asks for no nodes and
  no `highway` relations (ADR-0021, "Data for the district"). Ribbons
  carry no ids, kinds or connections, so nothing can walk on them.
- **OpenStreetMap has the paths.** The ingest's cached answer (OSM data
  of 2026-11-23) holds 433 sidewalk ways, 267 crossing ways, 252 steps
  and 232 pedestrian ways, 34 of them mapped as areas. "99% of the city's
  surveyed sidewalk area is in polygons that an OSM pedestrian way passes
  within 1 m of", and 96% of OSM's sidewalk length lies within 1 m of a
  surveyed sidewalk (ADR-0021). The data notes behind ADR-0021 measure
  about 46 km of walkable ways in the district, 19.2 km of them
  sidewalks, and 46,506 m² of pedestrian areas.
- **What is missing is in nodes and relations:** 219 crossing points,
  208 entrances, 36 tram stop positions, 353 benches and about 140
  terraces (`outdoor_seating=yes`), and the Münsterhof, a pedestrian
  multipolygon (ADR-0021).
- **OSM ways and shells disagree.** In the 4 tiles of ADR-0021's
  prototype, 269 of 1,120 graph edges (1.77 of 10.44 km) have a point
  where the player capsule overlaps a building shell. Of the 343
  walkable ways there, 44 end on a building outline, as paths to a door
  do, and 6 pass through buildings. Capsule agents on that graph got
  stuck, about a quarter of them within 70 s. People drawn on unmatched
  lanes would walk through walls, which ADR-0021's sources name as what
  breaks the illusion: "It doesn't matter that your models look photo
  realistic if an NPC walks into a wall".
- **Counts exist.** Hourly counts on Bahnhofstrasse, the Limmatquai
  counter and boardings per stop give hourly shapes per day type and
  factors per month (ADR-0021). The city's counter and the boardings are
  CC0; the version of Bahnhofstrasse's licence is still open (ADR-0021,
  open question 2).
- **Contracts before features** (CLAUDE.md): the client prototype, the
  ingest and the gate need one shape to build against.

## Design

### 1. The layer (amends RFC 0001 section 2)

| Layer | What | Granularity |
|---|---|---|
| `navigation` | Where people walk, where they go, and how many: lanes, walkable areas, places and the population | one asset per tile and source |

- **Content:** a canonical-JSON document, as RFC 0004's points
  documents are. No manifest gains a key; the layer says what the
  content is.
- **Collider:** `none`. As for water, the rule is enforced in the
  asset-manifest schema, the tile-index schema (the simulation reads
  colliders from tile indexes), `op_world` and the Python tools.
- **Sort order:** in byte order, `navigation` sorts between `interior`
  and `shell`. Existing tile-index entries keep their relative order,
  and tiles without the layer keep their bytes.
- **Feature ids:** `tile:17/{x}/{y}/{source}`, where `{source}` is the
  namespace of the elements' ids: `osm` for OpenStreetMap, as in
  `tile:17/68646/45903/osm`. RFC 0004 defines `{source}` as the point
  ids' prefix, the id without its last part (`osm:node`), and its
  section 9 writes `tile:17/{x}/{y}/osm:node`. A navigation document
  mixes OSM nodes, ways and relations, so that rule cannot apply here.
  How to align the two rules is an open question; this RFC does not
  change RFC 0004.
- **Fictional:** `false`. Estimates do not make an asset fictional, as
  in RFC 0004.

The draft's first version proposed a second layer for the population.
"Where the profiles live", under "Options considered", explains why it
is now inside the navigation document.

### 2. The navigation document (amends RFC 0001 sections 5 and 7)

Canonical JSON (RFC 0001 section 5) with
`format: "streetwide.navigation"`, validated by
`schemas/navigation.schema.json`. It is stored as
`objects/<aa>/<hex>.json`, named by its hash, so the store layout does
not change.

**Top level** (all keys required): `areas`, `contract_version`,
`format`, `lanes`, `nodes`, `places` and `population`. The four lists
are sorted by `id` in byte order, without duplicates. `null` means
unknown.

**Positions** are `[x, y, z]` in integer millimetres in the region frame,
relative to the tile origin (RFC 0001 section 1).

**Heights.** `y` is on the surface people walk on, with no lift: the
surface of the tile's terrain asset at that horizontal position, as RFC
0004 section 5 defines ground; or, for an element marked `bridge`, the
top surface of a bridge deck. Ribbons sit 5 or 6 cm above the height
model and sample it between the terrain's vertices (a grid of about
1.6 m), so lanes do not copy their heights.

#### Nodes

A node is where lanes meet or end: a junction, a dead end, a place, or a
cut at a tile edge or at the world's edge.

| Key | Meaning |
|---|---|
| `id` | The node id (section 4). |
| `position_mm` | Where it is. |
| `edge` | `null` inside the tile; `tile` for a cut on an edge shared with another tile of the world; `world` for a cut where the world's area ends (section 3). |

Every node is the end of at least one lane of the same document.

#### Lanes

A lane is a part of one OSM way between two nodes, or a lane the ingest
makes. People walk along it, within its width.

| Key | Meaning |
|---|---|
| `id` | The lane id (section 4). |
| `kind` | One of the kinds below. |
| `from`, `to` | Two different node ids of the same document. |
| `points_mm` | The positions between `from` and `to`, in order. Consecutive vertices, the nodes included, are at most 1,500 mm apart horizontally (checked on squared integer distances), as the ingest already densifies ribbons, so that heights follow the ground. |
| `width_mm` | 300 to 50,000: the walkable width, centred on the polyline. |
| `bridge` | `true` if the lane lies on a bridge deck. |
| `control` | For `crossing`: `signals`, `marked` or `unmarked`, or `null` if unknown. `null` for the other kinds. |
| `moved_mm` | 0 to 1,000: the largest sideways move the matching made (section 6), from the OSM geometry. |
| `ranges` | The parts of the lane that people must not simply walk, as `{kind, first, last}` with vertex indices (0 is `from`, then the points, then `to`), sorted and not overlapping. Kinds: `entrance`, the end of a lane that enters a building at a door; `passage`, a part through a building (`tunnel=building_passage`); `blocked`, a part nobody walks: an overlap the matching could not clear, or the end of a lane at a door that is not a place. An `entrance` range ends at a node that is an entrance's `node`. |
| `curve` | The id of a curve in `population`. |
| `people_per_day` | 0 to 1,000,000: the people who pass a point of the lane on a mean day of the year, both ways together. |
| `forward_permille` | 0 to 1,000: the share walking from `from` to `to`; `null` for half. At Bahnhofstrasse's middle counting area, 51.8% walk one way; at the Limmatquai, 59.6% (data notes). |
| `estimated` | The values a rule set (below). |

**Kinds:**

| Kind | What | Made from |
|---|---|---|
| `sidewalk` | a pavement beside a carriageway | OSM `footway=sidewalk`; or estimated beside a road, from its `sidewalk` tags or, where it has none, by the fallback rule (section 6) |
| `footway` | a path for people on foot | `highway=footway` of no other kind, `highway=path`, and cycleways or bridleways open to people on foot |
| `crossing` | where people cross a carriageway | `footway=crossing` and traffic islands; crossings estimated at junctions |
| `steps` | stairs | `highway=steps` |
| `pedestrian` | a pedestrian street, walkable across its width | `highway=pedestrian` as a line |
| `living_street` | a shared street | `highway=living_street` |
| `road` | a carriageway that people walk along where no sidewalk is mapped or estimated, such as an old-town alley | a road open to people on foot, by rule |
| `area` | a lane across or along a walkable area | made by the ingest from an area (below) |
| `connector` | the last metres between a place and the nearest lane | made by the ingest |

- **ADR-0021's six kinds** come first. `road`, `area` and `connector`
  are added by this RFC ("Smaller choices"). A new kind needs an
  amendment of this RFC.
- **Which tags make which kind,** and which ways are left out (no foot
  access, private, tunnels other than building passages, indoor, below
  ground), is the navigation ingest's ADR, to be drafted as ADR-0014 was
  for water.

#### Areas

A walkable area mapped as such: `highway=pedestrian` with `area=yes`, a
pedestrian multipolygon, or `area:highway=pedestrian`.

| Key | Meaning |
|---|---|
| `id` | The OSM id of the area, `osm:way:…` or `osm:relation:…`. Unique in the document. |
| `kind` | `square`, or `street` for a pedestrian street mapped as an area. |
| `bridge` | `true` if the area lies on a bridge deck. |
| `polygons_mm` | The area's pieces in this tile: each piece is an outer ring, then any holes; each ring has at least 3 positions on the ground, with the first not repeated at the end. Clipping at tile edges and at the world's edge, as for water, can leave several pieces in one tile. |
| `nodes` | The sorted ids of the nodes where lanes reach the area. `area` lanes join them, across a square or along a street. |

- **Streets as areas.** In the cache, Bahnhofstrasse has no
  `highway=pedestrian` line: it is six pedestrian areas, beside
  carriageways tagged `foot=use_sidepath` and `sidewalk=separate`
  (counted for this RFC). The 34 pedestrian areas include those six, one
  of Rennweg, and two on the Rathausbrücke, one of them tagged
  `man_made=bridge` and `layer=1`. So areas have two kinds, and squares
  are not all the areas there are.
- **Not drawn as ground.** Drawing squares is M2's ground work, from
  land-cover data ([roadmap](../docs/roadmap.md), M2).

#### Places

| Key | Meaning |
|---|---|
| `id` | The place id (section 4). Unique across the world. |
| `kind` | One of the kinds below. |
| `anchor_mm` | Where the place is, on the ground: the door, the bench, the stop, the spot to stand at. |
| `bridge` | `true` if the place lies on a bridge deck. |
| `moved_mm` | 0 to 1,000: how far the matching moved the anchor from the OSM data (a door snapped onto a wall, section 6). |
| `node` | The node where people walking to or from the place join the lanes. `null` only for a place where trips neither start nor end, when no lane is near. |
| `area` | For `square`: the id of its area in the same document, or `null` for a square mapped only as a point. `null` for the other kinds. |
| `object` | The id of the object drawn there in another layer, such as a bench in RFC 0004's `street_objects` once that layer has a bench kind; otherwise `null`. |
| `yaw` | 0 to 65,535: which way the place faces (a door outward, a bench's seat), in 1/65536 of a turn clockwise from north, like the world manifest's spawn yaw; `null` if unknown. |
| `seats` | 1 to 1,000: the seats of a bench or terrace; `null` if unknown. |
| `hours` | `null` if unknown. Otherwise opening hours: keys `mo` to `su` and `ph` (public holidays), each a sorted list of `[open, close]` minutes after local midnight, with 0 ≤ open < close ≤ 2,880, not overlapping; `[]` means closed. |
| `curve` | The id of a curve in `population`. |
| `trips_per_day` | For a place where trips start and end: the trips that start or end there on a mean day of the year. Otherwise `null`. |
| `occupancy` | For a place where people sit or stand: the people there at the busiest hour of its curve. Otherwise `null`. |
| `estimated` | The values a rule set (below). |

| Kind | Trips start and end | People sit or stand | In the district (ADR-0021 and its data notes) |
|---|---|---|---|
| `entrance` | yes | no | 208 OSM entrances; doors estimated for shops and public buildings that have none (802 shops, 300 places to eat or drink) |
| `stop` | yes | stand | 13 tram and bus stops, 36 `railway=tram_stop` positions |
| `square` | yes | stand | 15 `place=square`; squares among the 34 pedestrian areas |
| `bench` | no | sit | 353 OSM benches |
| `terrace` | no | sit | about 140 places with `outdoor_seating=yes` |
| `viewpoint` | no | stand | `tourism=viewpoint` |
| `fountain` | no | stand | 33 `amenity=fountain` |

- **Entrances are public.** An entrance place leads into a shop, a place
  to eat or drink, or a public building such as a church, museum or
  library. Never a door mapped as a home's door or as private, and never
  a door of a building that holds only homes. This is ADR-0021's privacy
  rule: trips never start or end at a residential address.
- **The world's edge** is a place too, but it is not stored: a node with
  `edge: world` is where trips enter and leave the world.

#### The population

`population` holds what the tile's lanes and places share: `curves`,
sorted by `id`, `holidays`, `time_zone` and `week`. Each lane and place
carries its own numbers (above).

A **curve** is the shape of a kind of place through the day and the
year: a shopping street, a river front, a square, a stop, a shop. The
ingest chooses the catalogue.

| Key | Meaning |
|---|---|
| `id` | `[a-z][a-z0-9_]*`, at most 64 characters, unique in the document. |
| `days` | One entry for each day type: `weekday`, `saturday` and `sunday`. |

Each day type:

| Key | Meaning |
|---|---|
| `share_permille` | The day type's daily total, per mille of the mean day. Over the seven days of `week`, the shares add up to 7,000. |
| `hours` | 24 integers: each local clock hour's share of the day's total, from 00:00, in parts per 10,000. They add up to 10,000. |
| `months` | 12 integers, January to December: that day type's daily total in the month, per mille of its yearly mean. They add up to 12,000. |

People passing a point of a lane in an hour, with the lane's curve and
the date's day type `d`, month `m` and hour `h`:

```text
people_per_day × share_permille[d] / 1,000
               × months[d][m] / 1,000
               × hours[d][h] / 10,000
```

Trips at a place follow the same formula with `trips_per_day`. The
population function rounds as its version defines. The formula's yearly
mean is close to `people_per_day`, not exactly equal: holidays count as
Sundays, and every month weighs the same whatever its length.

The counts show why curves differ by kind of place. At 18:00 on
Bahnhofstrasse, December brings 1.4 to 2.6 times as many people as
June, on weekdays and Saturdays; the Limmatquai sees about twice as
many a day in June as in January (ADR-0021). The Limmatquai's Sundays
are nearly as busy as its weekdays, while Bahnhofstrasse's middle area
counts 22,590 people on a mean Sunday against 38,597 on a weekday (data
notes, 2025). Months are given per day type, 36 values a curve instead
of 12, so that a December Saturday can differ from a December weekday;
whether the counts need that was not measured ("Smaller choices").

**The calendar:**
- **`time_zone`:** the IANA time zone of the hours, such as
  `Europe/Zurich`. The crowd's clock is a full date and time in Swiss
  civil time (ADR-0021); a world elsewhere needs its own zone.
- **`week`:** the day type of each day, Monday to Sunday. In
  Switzerland, `["weekday", "weekday", "weekday", "weekday", "weekday",
  "saturday", "sunday"]`. Countries with other weekends map their own.
- **`holidays`:** sorted, unique `{date, day}` entries: a date as
  `YYYY-MM-DD` and the day type it counts as, usually `sunday`. Dates
  outside the list count as their day of the week. ADR-0021's open
  question 3 asks for holidays as data per place; here the place is the
  tile, so a city or canton can have its own.
- **Clock changes:** hours are local clock hours. When the clocks go
  forward, the missing hour has nobody; when they go back, the repeated
  hour repeats.

**Measured or estimated.** A number is measured only where a count's
cross-section crosses one lane or area alone, or where a stop's
boardings and alightings give its trips. Bahnhofstrasse's counting
areas span its pedestrian areas and the sidewalks beside its
carriageways, so its numbers are estimates calibrated to the counts.
Occupancies are set by eye until there are counts of people stopping
and sitting (ADR-0021, open question 5).

#### Estimates and their rules

`estimated` is an object on each lane and place. It maps each value a
rule set, rather than the source or a count, to the id of that rule,
such as `adr-xxxx:width-by-kind:1` for a rule in the navigation
ingest's ADR. Keys are sorted, as canonical JSON sorts them; `{}` when
nothing is estimated. One key can come from different rules in one
document (a width by default per kind, or narrowed near a wall), which
is why the rule is named per element, not per kind as RFC 0004 does
("Smaller choices").

What may be estimated:
- **lanes:** `points_mm` for `sidewalk` (beside a road), `crossing` (at
  a junction), `area` and `connector`; `width_mm`, `people_per_day` and
  `forward_permille` for every kind;
- **places:** `anchor_mm` for `entrance` (a door estimated for a shop or
  public building with none mapped) and `terrace` (in front of its
  building, towards the nearest lane); `yaw`, `seats`, `trips_per_day`
  and `occupancy` for every kind.

Nothing else. A move within 1 m by the matching is placement, recorded
in `moved_mm`, not an estimate.

#### Example

Keys in reading order; the canonical form sorts them. Values are
illustrative.

```json
{
  "format": "streetwide.navigation",
  "contract_version": "0.1.0",
  "population": {
    "time_zone": "Europe/Zurich",
    "week": ["weekday", "weekday", "weekday", "weekday", "weekday", "saturday", "sunday"],
    "holidays": [{ "date": "2026-12-25", "day": "sunday" }],
    "curves": [
      {
        "id": "shopping_street",
        "days": {
          "weekday": {
            "share_permille": 990,
            "hours": [30, 10, 5, 5, 10, 40, 110, 260, 380, 370, 480, 680, 920, 900, 840, 830, 870, 1020, 880, 550, 350, 290, 130, 40],
            "months": [850, 880, 950, 980, 1010, 1000, 1020, 1000, 1010, 1020, 1060, 1220]
          },
          "saturday": {
            "share_permille": 1500,
            "hours": [100, 60, 40, 30, 20, 20, 20, 40, 100, 250, 500, 750, 900, 1000, 1100, 1150, 1150, 1000, 750, 450, 250, 150, 100, 70],
            "months": [850, 880, 950, 980, 1010, 1000, 1020, 1000, 1010, 1020, 1060, 1220]
          },
          "sunday": {
            "share_permille": 550,
            "hours": [100, 50, 30, 20, 20, 20, 30, 60, 150, 300, 500, 700, 800, 900, 1000, 1050, 1050, 1000, 800, 600, 400, 250, 120, 50],
            "months": [850, 880, 950, 980, 1010, 1000, 1020, 1000, 1010, 1020, 1060, 1220]
          }
        }
      }
    ]
  },
  "nodes": [
    { "id": "osm:node:2401", "position_mm": [-99280, 408960, 13052], "edge": null },
    { "id": "osm:node:5120", "position_mm": [-97850, 409010, 15020], "edge": null },
    { "id": "osm:way:3317/n1", "position_mm": [-103512, 408870, 12044], "edge": "tile" }
  ],
  "lanes": [
    {
      "id": "osm:node:5120/c",
      "kind": "connector",
      "from": "osm:node:2401",
      "to": "osm:node:5120",
      "points_mm": [[-98565, 408985, 14036]],
      "width_mm": 1500,
      "bridge": false,
      "control": null,
      "moved_mm": 0,
      "ranges": [{ "kind": "entrance", "first": 1, "last": 2 }],
      "curve": "shopping_street",
      "people_per_day": 180,
      "forward_permille": null,
      "estimated": {
        "people_per_day": "adr-xxxx:flow-from-places:1",
        "points_mm": "adr-xxxx:connector-to-nearest-lane:1",
        "width_mm": "adr-xxxx:width-by-kind:1"
      }
    },
    {
      "id": "osm:way:3317/2",
      "kind": "sidewalk",
      "from": "osm:way:3317/n1",
      "to": "osm:node:2401",
      "points_mm": [[-102100, 408900, 12380], [-100690, 408930, 12716]],
      "width_mm": 1800,
      "bridge": false,
      "control": null,
      "moved_mm": 120,
      "ranges": [],
      "curve": "shopping_street",
      "people_per_day": 5200,
      "forward_permille": null,
      "estimated": {
        "people_per_day": "adr-xxxx:flow-from-places:1",
        "width_mm": "adr-xxxx:width-narrowed-at-wall:1"
      }
    }
  ],
  "areas": [],
  "places": [
    {
      "id": "osm:node:5120",
      "kind": "entrance",
      "anchor_mm": [-97850, 409010, 15020],
      "bridge": false,
      "moved_mm": 240,
      "node": "osm:node:5120",
      "area": null,
      "object": null,
      "yaw": 49152,
      "seats": null,
      "hours": {
        "mo": [[540, 1110]], "tu": [[540, 1110]], "we": [[540, 1110]],
        "th": [[540, 1260]], "fr": [[540, 1110]], "sa": [[540, 1020]],
        "su": [], "ph": []
      },
      "curve": "shopping_street",
      "trips_per_day": 180,
      "occupancy": null,
      "estimated": {
        "trips_per_day": "adr-xxxx:trips-by-kind:1",
        "yaw": "adr-xxxx:door-faces-lane:1"
      }
    }
  ]
}
```

**Size:** about 2.5 to 3 MB of canonical JSON for the district's 30
tiles, under 0.1 MB a tile on average (estimate, not measured: about
46 km of lanes at 1.5 m spacing is about 31,000 positions of about 23
bytes; a few thousand lanes and up to about 2,000 places at 250 to 400
bytes each with their numbers and rules; and a catalogue of curves of
about 1 KB each, copied into each tile).

**Format limits:** at most 65,536 nodes, lanes and places, 1,024 areas
and 1,048,576 positions per document. Those limits allow documents of
tens of megabytes, close to RFC 0001's 64 MiB cap on a file. The
district's documents fit any option of RFC 0001's open question on JSON
size limits; the limits themselves would need that question's answer.

### 3. Tile edges and the world's edge

- **Lanes are cut at tile edges,** as ribbons and water are. The ingest
  clips in LV95 against one tile polygon for both neighbours, and puts a
  node with `edge: tile` at each cut. Both tiles list that node with the
  same id and the same position in the region frame (tile origin plus
  `position_mm`). Each part of the lane belongs to the tile it lies in.
- **Each tile's graph is self-contained:** every node that a lane,
  area or place names is in the same document, and every node lies in
  its tile or on its edge. Readers join tiles by matching `tile` nodes
  by id, as the simulation already assembles tiles by integer origins.
- **Areas** are cut the same way. The pieces of one area in one tile
  form one entry, with one polygon each in `polygons_mm`. A square's
  place belongs to the tile that holds its anchor.
- **The world's edge:** where the world's area cuts a lane (the
  district's bounding box runs through its outer tiles), the cut gets
  `edge: world`. Trips enter and leave the world there.
- **Check:** every `tile` node has exactly one partner in each
  neighbouring tile of the world that it touches, at the same
  region-frame position: one partner on an edge, up to three at a
  corner. A world-level check, like the terrain seams of G8.

### 4. Ids and lineage

- **Assets:** lineage per tile, layer and source, as for RFC 0004's
  point assets. One changed lane, or one changed number, is a new
  version of its tile's document.
- **Elements** keep the source's ids, in RFC 0004's point-id grammar:
  `{namespace}:{dataset}:{local id}`, at most 256 characters, the local
  id printable ASCII without spaces or colons, never lowercased or
  trimmed.
  - **Nodes:** an OSM node keeps `osm:node:{id}`. A node the ingest
    makes (a cut, the foot of a connector, the end of an estimated
    sidewalk) is named after the way it lies on, with a suffix:
    `osm:way:3317/n1`.
  - **Lanes:** `osm:way:{id}/{k}` for the k-th part of a way, counted
    from its first node across the whole way. A lane the ingest makes is
    named after what it comes from: an estimated sidewalk after its road
    (`osm:way:812/l0` on the left side), an `area` lane after its area,
    a `connector` after its place (`osm:node:5120/c`).
  - **Places:** the OSM id of the element (`osm:node:5120`, or
    `osm:way:…` for a platform). An estimated door or terrace is named
    after its point of interest, with a suffix
    (`osm:node:6620/door`).
  - **Areas:** their OSM id.
- **Rules for ids:** the ingest's ADR fixes the suffixes; this RFC fixes
  the rules. Ids derive from source ids. The same input gives the same
  ids. Lane, node and place ids are unique across the world, except that
  a `tile` node appears in two to four documents, always at one
  position.
- **Stable enough:** lanes are split at junctions, places and cuts, so a
  lane keeps its id while its way keeps all three. A new shop whose
  connector joins a way renames that way's later lanes. The crowd seeds
  its randomness from these ids (ADR-0021), so a refresh that renames
  lanes changes who walks there; the world id changes with any refresh
  anyway.
- **In manifests:** the OSM source of each asset lists the ids of the
  ways, nodes and relations it used.

### 5. Manifest fields (amends RFC 0001 sections 3, 5 and 7)

As in RFC 0004 section 6, an asset is its content and its manifest. For
a navigation asset:

| Field | Meaning |
|---|---|
| `content_id`, `size_bytes` | Of the document. |
| `feature_id` | `tile:17/{x}/{y}/{source}` (section 1). |
| `bounds_mm` | The exact bounds of every position in the document. |
| `physics` | `{"collider": "none"}`. |
| `fictional` | `false`. |
| `parent_version_id` | Lineage per tile, layer and source. |
| `provenance` | The OSM snapshot and the ids used; the swissALTI3D items for heights; the swissBUILDINGS3D sheets used for the matching (section 6); each count dataset, with the counting stations or stops used. |

RFC 0004 changes RFC 0001's section 3 so that an asset's content is
"its content: a mesh, or a points document in `vegetation` and
`street_objects`". This RFC adds "or a navigation document in
`navigation`" to that phrase, and the same to sections 5 and 7. Section
6, the glb profile, does not change.

### 6. The ingest

The navigation ingest's ADR, to be drafted as ADR-0014 was for water,
chooses the tags, queries and rules. This RFC fixes what the output
must be.

- **One more query,** with its own cache file, as ADR-0014 added one for
  water, so that the existing query and the OSM assets built from it
  keep their bytes. It asks again for highway ways, so that lanes and
  the nodes on them come from one snapshot, and for:
  - the nodes that navigation needs: entrances, crossings, benches,
    stops, shops and places to eat, terraces, viewpoints and fountains;
  - `highway` relations (the Münsterhof and the Züghusplatz are
    pedestrian multipolygons), `area:highway` and `place=square`.

  The district's cached highway and building answer is 3.3 MB today.
  The query never uses `out meta`, so no user or changeset field is read
  (ADR-0021, Privacy).
- **Lanes:**
  - ways split at junctions, places and edges; heights on the terrain
    mesh, or on the top of the deck the ingest already computes for a
    bridge (section 2, "Heights");
  - widths from `width`, otherwise a default per kind, marked as
    estimated. 39% of the district's footway, pedestrian, path and steps
    length carries a `width` tag (data notes).
- **Roads:** where a road maps its sidewalks on itself
  (`sidewalk=both`, `left` or `right`) or not at all, sidewalks are
  estimated beside it, by ADR-0021's rule for places without data
  (both sides of residential streets, right on 83% of sides in the
  district; none along service roads, right on 67%). A road open to
  people on foot with no sidewalk becomes a `road` lane. In the
  district, only 12 roads carry a sidewalk on the road itself (data
  notes); most mark theirs as mapped separately, so few lanes are
  estimated there. Elsewhere, many are.
- **Areas:** squares and pedestrian streets mapped as areas, with
  `area` lanes that join the nodes where lanes reach them. An area or
  place over water with no deck under it is left out and reported.
- **Matching to shells.** ADR-0021 asks for lanes clear of building
  shells, except where a lane ends at an entrance or passes through a
  building passage. Under option K1 (section 10):
  1. probe each lane at points at most 1 m apart against every collider
     within reach, the neighbouring tiles' included, since a building
     can reach into a tile it does not belong to (RFC 0001 section 2).
     A point is blocked by G9's test: the player capsule overlaps a
     collider, or rays in the four horizontal directions and up all hit
     within 30 m. G11 uses the same test (section 9);
  2. mark as `entrance` the end of a lane at an entrance place; mark the
     end of a lane at any other door, such as a home's, as `blocked`, so
     that nobody walks into it; mark as `passage` the parts of ways
     tagged `tunnel=building_passage`;
  3. move the other blocked parts sideways, away from the shell, by at
     most 1 m, keeping junctions joined, and record the largest move in
     `moved_mm`;
  4. mark what is still blocked as `blocked`, and narrow `width_mm` (as
     an estimate) where a wall is nearer than half the width plus
     0.45 m, the distance people keep from house walls (Weidmann 1993,
     [ETH Zürich](https://doi.org/10.3929/ethz-b-000242008), p. 89);
  5. report every move, passage and blocked part, so that errors can be
     fixed in OpenStreetMap.

  **The 1 m bound** follows OSM's accuracy in the district (96% of its
  sidewalk length within 1 m of the survey) and G8, which lets a shell
  move by 1 m. Why the prototype's other overlaps occur was not
  examined, so how many lanes 1 m clears is not known. The first
  district run reports it, and the bound is a starting value until then.
  **Only ingest-made shells** are matched (swissBUILDINGS3D and the OSM
  fallback). A fresh ingest never sees contributed shells (ADR-0020
  section 1); section 9 says what happens when one blocks a lane.
- **Doors, terraces and connectors.** In the district, 1,377 of the
  1,387 shells are swissBUILDINGS3D's and 10 are OSM fallbacks
  (ADR-0019). Under K1:
  - a mapped entrance starts at its OSM node; a door estimated for a
    shop or public building starts on the OSM building outline that
    holds it, at the point nearest the nearest lane; a terrace starts
    in front of that outline, towards the lane;
  - each is then snapped onto the wall of the shell it belongs to, if
    that wall is within 1 m, and its move is recorded in `moved_mm`. A
    door further than 1 m from any wall is left out and reported, so
    that every door sits on a shell and its `entrance` range ends where
    the lane meets the wall;
  - a `connector` joins each place to the nearest lane within 30 m that
    it reaches without crossing a shell (starting value). A place with
    none is left out and reported, unless people only sit or stand
    there.
- **Places:** stops, squares, benches, viewpoints and fountains from
  OSM; opening hours parsed from `opening_hours` where the ingest's
  subset of its grammar reads them, otherwise `null`. 70% of the
  district's shops and 75% of its places to eat have the tag.
- **Population:**
  - curves from the counts whose licences are settled (section 10):
    Bahnhofstrasse's areas for shopping streets, the Limmatquai for the
    river front, VBZ boardings by hour for stops;
  - numbers per lane and place estimated from kinds, places and stops,
    and calibrated at the counted places (ADR-0021, "Data");
  - occupancies set by eye;
  - where no counts exist, Zurich's curves, scaled by nearby
    destinations and stops, marked as estimated (ADR-0021).
- **Nothing from live services:** everything is baked by the ingest and
  pinned by the world id (ADR-0021).

### 7. How the client reads it

- **Loading:** `op_stream` loads a tile's navigation document with the
  tile, as content that is not a collider, through `op_world`'s reader,
  which validates it. The simulation's collider path never sees it.
- **Joining:** `op_client` joins loaded tiles at `tile` nodes and turns
  lanes, areas, places and numbers into the population function's plain
  types, which are defined beside the function (ADR-0021, "Where the
  code lives"), so that no crate gains a dependency on `op_world`.
- **Radius:** the crowd works out people within a radius inside the
  600 m the client loads (ADR-0021). `world` nodes are where people
  enter and leave. A `tile` node whose neighbour is not loaded lies
  beyond that radius.
- **What the client draws from it:** a door at each entrance place, and
  a bench at each bench place until `object` names a bench in
  `street_objects`; both are decoration
  ([ADR-0012](../docs/adr/0012-client-realism-from-procedural-detail.md)),
  hidden in F2's plain look.
- **Passages:** recommended for M2, nobody walks a `passage` range (see
  "Passages" under "Options considered").
- **A help for fixing OSM** (a suggestion): the inspect mode could draw
  lanes and their `blocked` ranges, so that a player sees what to fix
  in OpenStreetMap.
- **Without the layer:** a world, or a tile, with no navigation asset
  has no people, and nothing fails. A reader built before this RFC
  rejects a world that has the layer, with "unknown layer", as for RFC
  0003 and RFC 0004.

### 8. Simulation input later

If a later decision puts people into `op_sim` (ADR-0021's B or C1, in M4
or M5), the format is meant to be ready:
- integer millimetres in the region frame, so tiles join by integer
  translation, as colliders do;
- one graph per tile, joined at `tile` nodes by id, so the simulation
  can load lanes by tile within its 160 m, as it loads colliders;
- sorted, stable ids, and no floats;
- heights on the surfaces players stand on, and lanes checked against
  the colliders by G11;
- pinned by the world id.

What that decision would still need, in its own RFC and without a
format change: an `op_kernel` navigation type (ADR-0021, "Where the
code lives"); `op_sim` hashing what it reads of the navigation assets it
loads, which bumps `SIM_CONTRACT_VERSION`; and a tick that stalls until
its lanes have loaded. With the population inside the navigation
document, a simulation that hashed whole content ids would see every
change of a number; it could hash only what it reads instead, which is
that RFC's choice.

### 9. Contributions and the gate (amends RFC 0002 sections 3 and 4)

| Layer | Contributions in 0.1 | Collider | Notes |
|---|---|---|---|
| `navigation` | **no: fix lanes and places in OpenStreetMap; the population comes through the ingest's rules and sources** | `none` | Derived from OSM (ODbL) and from counts. The next baseline refresh brings fixes in. |

**The gate** gets the next free version, because its output changes for
change sets that touch the layer or colliders near lanes.
`GATE_VERSION` is 0.1.2 today; this RFC's would be 0.1.4 only if RFC
0004's 0.1.3 lands first, and later still if ADR-0017's list or A1 to A8
change the gate before it.
- **G1:** validation covers the document, the world-level pairing of
  `tile` nodes (section 3), and world-wide unique lane, node and place
  ids.
- **G3:** a contributed change to the layer fails, with the advice
  above. A baseline refresh authored by `streetwide:ingest` gets
  `review`. G3 already closes every asset whose licence contains
  `ODbL-1.0`.
- **G7:** lanes and places count no triangles. A refresh whose document
  has more than ten times the positions or places of the district's
  busiest tile gets `review`; the numbers are set from the first
  district run.
- **G8:** every lane vertex, area ring position and place anchor lies
  within 50 mm of the terrain mesh of its tile, or, where marked
  `bridge`, of the top surface of a bridge deck, computed exactly from
  millimetre vertices; and `moved_mm` is at most 1,000. Starting values,
  as RFC 0004's G8 for anchors.
- **G9 and G10:** unchanged, as ADR-0021 says under C2.
- **G11 (new): lanes,** the "G9-style check" that ADR-0021 asks for.
  - **Probe points:** every lane vertex and points between them at most
    1 m apart, outside `entrance`, `passage` and `blocked` ranges.
  - **Blocked** means what it means in the ingest's matching (section
    6) and in G9: the player capsule overlaps a collider, or the point
    is enclosed.
  - **Colliders that change** (a shell, terrain or a bridge): on lanes
    within 2 m of the changed assets, a probe point that was free in the
    base world must stay free in the candidate world, as G9 checks
    ribbons. A contributed shell that newly blocks a lane fails, though
    its author cannot edit the lane: G9 already treats ribbons so. If
    ADR-0020's amendment A4 makes G9 give `review` when the blocking
    asset was made by the ingest, G11 follows it.
  - **Lanes whose geometry changes** (`from`, `to`, `points_mm`,
    `width_mm`, `bridge` or `ranges`): every probe point must be free in
    the candidate world. A change of numbers only probes nothing. On a
    refresh, the outcome is `review` with the findings, and the refresh's
    final world must pass (below).
  - **The report** lists, per tile, the length of lanes in `blocked`,
    `passage` and `entrance` ranges and how it changed, so that a
    refresh that blocks more stands out.
  - **The check report** gains an entry, G11, beside G1 to G10. That
    changes its schema and RFC 0002 section 4 ("one entry per check,
    `G1` to `G10`"). "Where the lane check lives" weighs putting it
    inside G9 instead.

**Refreshes.** ADR-0020 section 5 accepted the order of a refresh's
sets: "(a) visual-only sets first, then by layer and source, decision
sets last", for this reason: "Visual-only sets pass `op_world`'s gate
alone, and new ribbons merged first keep G9's probes on ways that exist
upstream." The planner orders `none` before `trimesh` in code. So a
refresh's navigation set merges before its shell sets, and its lanes,
matched to the new shells, are first probed against the old ones. Recommended (option (d) under "The
lane check during a refresh"): keep ADR-0020's order; G11 gives `review`
on refresh sets; and the planner's comparison of the pre-refresh and
final worlds, which ADR-0020 accepted for its Q19, runs G11's absolute
check on the final world, which must pass before the first set merges.
This changes G11 (this RFC) and the planner's comparison, not ADR-0020's
order. The other options change that order or ADR-0020's units, and
need their own decision.

**A contributed shell in a refresh.** A fresh ingest matches lanes to
baseline shells only. Where a contribution replaced a baseline shell,
a refreshed lane can cross the contributed one, and the final check
fails. Recommended: the planner holds that tile's navigation change and
lists it for review, as ADR-0020 holds refresh changes that meet
contributions. The current version was checked against the contributed
shell when that merged, so the tile stays consistent. This is a new
hold in the planner, decided with ADR-0020's holds. The other options:
merge it with `review`, so that people walk through the contributed
shell until OSM or the shell changes; or let the ingest read
contributed shells, which would combine CC BY-SA content with ODbL
lanes, against ADR-0001 and G4 (rejected).

**Takedowns.** RFC 0002 section 7 removes or reverts whole assets, and a
takedown still passes G1 and G2. With the population inside the
navigation document, a takedown always acts on lanes, places and
numbers together. A refresh never changes a key whose last change was a
takedown: a removal stays frozen, and a revert may be lifted by an accept
with a reason (ADR-0020's Q18 (b), now implemented). So taking down one
tile's navigation asset to remove one door freezes the whole tile's
lanes in later refreshes until a maintainer acts. Recommended:
- fix a wrong door in OpenStreetMap and run a refresh, which is not a
  takedown;
- if it cannot wait, a takedown that reverts the asset, lifted by an
  accept once OSM is fixed;
- a takedown that removes it only when the whole tile must go: its
  people go with it until a maintainer's own change set adds it back.

Taking down one place alone needs a contract change (unresolved).

**The cap of 50 changes.** The district's first refresh adds about 30
navigation assets: one set.

### 10. Licences and provenance (ADR-0021, open question 6)

**The asset's licence:**
`ODbL-1.0 AND LicenseRef-swisstopo-OGD AND CC0-1.0`: OSM geometry placed
on swissALTI3D heights and, under K1, matched to swissBUILDINGS3D shells,
with numbers from the city's counter and VBZ's boardings (CC0). The
hystreet counts add their licence once its version is settled (ADR-0021,
open question 2). Sources: the OSM snapshot, swissALTI3D,
swissBUILDINGS3D under K1, and each count dataset.

**Part 1 of the question: what the shells may do.** RFC 0001 lets an
OSM-derived asset use another source "only to place its OSM geometry".
The matching uses swisstopo's shells for more than heights: it moves
lanes sideways, snaps doors and terraces onto walls, narrows widths,
chooses which lane a connector joins, and marks what it cannot clear.
Options (compared under "Options considered"):
- **K1. Amend RFC 0001 section 2 (recommended):** placing OSM geometry
  includes moving a point or a line sideways by at most 1 m to clear
  another source's geometry or to meet it (a lane clear of a wall, a
  door on it); narrowing a width; choosing among OSM-derived candidates
  the one that clears it (the lane a connector joins); and marking what
  cannot be cleared. Never against contributed content (CC BY-SA 4.0),
  which ADR-0001 and G4 keep apart from ODbL geometry. Every door and
  terrace starts from OSM data (a node, or an OSM building outline), so
  no anchor is placed by swisstopo geometry beyond 1 m.
- **K2.** Amend it only to allow marking: no moves or snaps.
- **K3.** No amendment: lanes and doors stay pure OSM geometry, and the
  client keeps people off shells when drawing them (a produced work).
- **K4.** Clearance offsets in an asset of their own, outside the
  navigation asset.
- **K5.** Doors and terraces placed on the swissBUILDINGS3D wall nearest
  their shop, at any distance within its building.

**Part 2: counts mixed with OSM places.** ADR-0021 notes that using OSM
to select or correct other data counts as mixing (ADR-0019). Our
reading, which is not legal advice:
- the counts enter an ODbL asset; no asset outside the ODbL is selected,
  filtered or corrected with OSM, so nothing outside the ODbL changes;
- under ADR-0019's option A (the whole world under the ODbL), there is
  no question;
- under its option B (regional cuts), the curves become contents of an
  ODbL asset, and the original datasets stay available on their own
  terms. Walking data is then a feature type that is all OSM in
  Switzerland; the city's walking network and surveyed sidewalks stay
  checks, never copied (ADR-0021, "Data");
- every count source must fit inside an ODbL database: CC0 fits, and CC
  BY 4.0 "appears to fit" (ADR-0019). hystreet's licence comes first.

**Recommended for question 6:** K1, and part 2 as read above, settled
with ADR-0019. The ingest builds no lanes or curves until the maintainer
has decided question 6, [ADR-0017](../docs/adr/0017-licence-allowlist.md)
allows each source, and [ATTRIBUTION.md](../ATTRIBUTION.md) lists every
count source used (ADR-0001).

**Closed to contributions** while the licence contains `ODbL-1.0`, which
G3 already enforces.

**Drawing on shells** (ADR-0019, question 5):
- doors drawn at entrance places are OSM data drawn as objects of their
  own, on the wall K1 snapped them to. No question under option A;
  under B, our reading is that this draws OSM data and changes no
  shell;
- openings cut into shells for passages, or lit windows chosen by
  opening hours (ADR-0022's curves by building class), would change how
  swisstopo shells are drawn, which is question 5.

**Trees and lamp posts:** lanes are not routed around RFC 0004's points.
People keep clear of trunks and poles when drawn, by client offsets
that fade (ADR-0021). Routing in the data would let non-OSM points
shape OSM lanes, the same question as shells, for a gain in looks only.

### 11. Privacy

- **Public ground:** no lane on a way that is private or closed to
  people on foot, and no indoor ways.
- **No homes:** entrance places only for shops, places to eat or drink,
  and public buildings (section 2). Trips start and end at public places
  and the world's edge (ADR-0021).
- **No text:** no names, addresses, brands or operators, as RFC 0004
  keeps such text out of points. Opening hours are stored as minutes,
  never as the source's text, and the schema has no free-text field.
- **Aggregates only:** counts are hourly or quarter-hourly totals; never
  tracks (GPS traces, including OSM's public traces), cameras, Wi-Fi or
  Bluetooth data, and never accident records (ADR-0021).
- **OSM without user data:** no user or changeset fields.
- **A wrong door at a home** is fixed in OpenStreetMap and leaves with
  the next refresh; if it cannot wait, see "Takedowns" (section 9).
- **G6** does not apply while the layer is closed. The ingest's ADR says
  how its rules keep to this section, and the review of each refresh
  checks them.

## Options considered

### The content

**A. A graph document per tile and source (recommended).**
- Pros:
  - what the crowd needs: lanes with kinds and widths, joined, with
    places on them; Unreal's City Sample likewise spreads its people
    along lanes
    ([Epic](https://dev.epicgames.com/documentation/unreal-engine/city-sample-project-unreal-engine-demonstration));
  - the crowd notes behind ADR-0021 rank lanes on a graph with
    separation as the best fit for M2, and the prototype's graph agents
    were cheap and deterministic;
  - no glb profile change; each asset single-source by construction;
  - small: about 2.5 to 3 MB for the district (estimate).
- Cons:
  - a second kind of JSON content, after RFC 0004's points, with its
    own reader in Rust and Python;
  - lineage per tile, not per lane;
  - an area is walkable only along its lanes, unless client offsets
    spread people over it.

**B. A navigation mesh** (walkable polygons with portals) per tile.
- Pros: any point of a square or a wide street is walkable; Assassin's
  Creed Unity's far crowd follows its navigation mesh (ADR-0021).
- Cons: built from OSM lines, it needs a width and a polygon union for
  every way, all estimated; no kind per lane; larger and harder to
  check. An exporter can build one from A later.

**C. glb meshes,** lanes as thin triangles in a new palette material.
- Cons: a glb profile change; no ids, kinds, places or topology.

**D. The transport ribbons as they are.**
- Cons: no topology, kinds or ids. Ribbons stay for drawing roads and
  paths.

**E. One asset per lane or place.**
- Cons: thousands of manifests in the district, each outweighing its
  content, as RFC 0004's option D found for points.

**F. No world data:** the client derives lanes from OSM at run time.
Rejected: the world id would not pin it, nothing would carry its
provenance, and ADR-0021 bakes everything into the world.

### How lanes are stored

**L1. One graph per tile, cut at tile edges, joined by shared nodes
(recommended).**
- Pros: each tile's graph is complete and checkable alone; the
  simulation could load it by tile later; it cuts as ribbons and water
  already cut.
- Cons: nodes made at cuts, with ids of their own; a world-level check
  that both sides agree.

**L2. Lanes owned by one tile, uncut, joined across tiles by node id.**
- Pros: no cut nodes.
- Cons: a tile's lanes reach into its neighbours, so its bounds overlap
  theirs and it cannot be checked alone; a lane dangles until the
  neighbour loads.

**L3. A polyline per OSM way, with no topology.**
- Pros: the simplest ingest.
- Cons: every reader joins ends by position, which is fragile; places
  and junctions lose their meaning; the simulation would need topology
  anyway.

### Where the profiles live

ADR-0021 asks whether the profiles live in the navigation assets or in
their own. The draft's first version recommended their own layer, P3. A
review found that ADR-0020's refresh planner cannot keep two assets of
one tile in step: a unit's purpose is one (collider, layer, dataset), and
a set never mixes purposes, so a tile's navigation and population
changes go into different sets, merged one after the other.

**P1. Inside the navigation document (recommended):** curves and the
calendar in `population`, numbers on each lane and place.
- Pros:
  - one asset per tile, so no refresh, set or takedown can leave numbers
    that name missing lanes, or bounds that disagree;
  - no references across documents, so G1 stays strict;
  - G11 probes only lanes whose geometry changed, so a change of
    numbers probes nothing.
- Cons:
  - tuning the crowd, or settling a count's licence, writes a new
    version of every navigation asset;
  - the asset's licence includes the counts';
  - a later simulation that hashed whole content ids would see every
    change of a number (section 8);
  - the curves are copied into every tile.

**P2. Curves in an asset of their own per tile; numbers in the
navigation document.**
- Pros: the curves, which come only from counts, have their own asset.
- Cons: the curve asset and the navigation document still reference
  each other, so the planner splits them as for P3; the curves are
  still copied into every tile.

**P3. A `population` layer: curves and numbers in one asset per tile,
beside the navigation asset.**
- Pros: tuning leaves the lanes' bytes alone; a count source's licence
  touches only population assets.
- Cons:
  - any refresh that adds or renames a lane, or changes the navigation
    bounds, leaves a world between two sets in which the population
    document names missing lanes or has other bounds, so G1 fails on
    whichever set merges first. Only the first refresh, which adds both,
    works;
  - a takedown must act on both assets, or G1 fails;
  - the curves are copied into every tile's document, and those
    documents differ, so content addressing does not store them once.
- Remedies, each with a cost:
  - a unit that may span the two layers of one tile, which changes
    ADR-0020 section 5 and the planner;
  - a G1 that tolerates a stale population document between sets:
    readers ignore entries that name missing lanes, and a lane without
    an entry gets no people, with completeness checked only on a
    refresh's final world. Weaker validation, and a world in which some
    lanes have no people for a while.

**P4. Curves in the client,** with the population function's version.
- Pros: no world data for curves.
- Cons: against ADR-0021, which makes profiles world data, baked and
  pinned by the world id; curves differ by country and place.

If the maintainer prefers P3, the lighter remedy is the tolerant G1,
with takedowns acting on both assets.

### Matching lanes to shells

**K1. Move lanes and snap doors by at most 1 m, narrow, choose
connectors, and mark the rest (recommended).**
- Pros: people stay out of walls in the data, for the crowd now and
  the simulation later; doors sit on walls; G11 can hold every lane to
  it; the bound matches OSM's measured accuracy in the district.
- Cons: an amendment of RFC 0001 section 2; contributed shells are
  never used, so a contributed shell can fail G11 on a lane its author
  cannot edit; whether 1 m clears most overlaps is unknown, and doors
  more than 1 m from a wall are dropped, how many is unknown.

**K2. Mark only, never move.**
- Pros: a smaller amendment; lanes stay at OSM's positions.
- Cons: a sidewalk mapped a few centimetres into a wall loses its whole
  blocked part, and how many there are is unknown; doors stay off the
  walls.

**K3. No matching in the data.** The client keeps people off shells
when drawing them.
- Pros: no amendment; the cleanest licence position.
- Cons: the population function would need the shells as an input,
  which ADR-0021's list of inputs does not include; client offsets
  cannot fix a lane that runs inside a building; doors on OSM outlines
  can sit off the shell, so an `entrance` range does not end where the
  lane meets the wall; the simulation would need matching later anyway.

**K4. Clearance offsets in an asset of their own,** as ADR-0021 lists.
- Pros: what came from shells is visible as one asset.
- Cons: the offsets are computed from OSM positions, so on our reading
  they are OSM-derived too; and they would still combine with
  contributed shells, so the contributed-shell case is not solved.

**K5. Doors and terraces on the swissBUILDINGS3D wall nearest their
shop, at any distance.**
- Pros: every estimated door lands on a wall.
- Cons: the anchor is then placed by non-OSM geometry, often more than
  1 m from the OSM data, which needs a wider amendment than K1.

### Where the lane check lives

ADR-0021 says that under C2 "G9 and G10 do not change", and asks for "a
G9-style check that a change does not newly block a lane".

- **G11, a check of its own (recommended):** G9's rule for the player
  stays as ADR-0021 says. The cost: the check report gains an entry,
  which changes its schema and RFC 0002 section 4.
- **Inside G9:** the same capsule, probes and code, and the report keeps
  its entries. But G9 then changes, which departs from ADR-0021; an ADR
  superseding that part of ADR-0021 would come first.

### The lane check during a refresh

A refresh changes shells and lanes together, in different sets.
- **(a) ADR-0020's order, with G11 strict on every set:** the navigation
  set merges first and is judged against the old shells, so a lane moved
  to clear a new shell can fail on an old one. Not workable whenever
  shells and lanes change together.
- **(b) Collider sets first:** the shell set is judged by G11 against
  the base world's old lanes, so a refreshed shell over an old lane fails
  until A4 (ADR-0020's Q19 notes the same for buildings over ways). It
  also gives up ADR-0020's reason for its order. It changes an accepted
  ADR and the planner's code: an ADR superseding that row of ADR-0020
  section 5 first.
- **(c) A tile's shells and lanes in one unit, so in one set:** no world
  between the two. But a unit has one purpose (collider, layer,
  dataset), so ADR-0020 section 5 and the planner change, and a tile's
  unit can pass 50 changes: 1,377 shells over 30 tiles is about 46 a
  tile.
- **(d) ADR-0020's order; G11 gives `review` on refresh sets, and its
  absolute check runs on the refresh's final world (recommended):** no
  order or unit changes; it extends the comparison of the pre-refresh
  and final worlds that ADR-0020 accepted for Q19. The cost: between two
  sets, the canonical world can hold lanes that cross old shells, which
  changes only the crowd's look; and the final check runs in the
  planner's `verify`, outside the gate's report.
- **(e) `review` through an A4-style rule only,** when the lane or the
  blocking asset was made by the ingest: no final check, so every
  refresh relies on a reviewer reading the findings.

Options (b) and (c) need their own decision: an ADR that supersedes the
row of ADR-0020 section 5, or an amendment beside RFC 0002's questions 9
to 16 (A1 to A8).

### Passages

ADR-0021's matching keeps lanes that pass through building passages. The
cache holds 39 highway ways tagged `tunnel=building_passage` (counted for
this RFC; the prototype's 4 tiles had 6). swissBUILDINGS3D's shells have
no openings for them, so the wall stands where the passage is.
- **Nobody walks a passage in M2 (recommended for M2):** the data keeps
  the passage; the crowd routes around it. This narrows ADR-0021, which
  keeps passages for people to walk, and routes through them detour.
- **People walk through and fade at the mouths:** they vanish in view,
  against ADR-0021.
- **People walk through the wall:** what breaks the illusion.
- **The client opens or hides the wall at the passage:** this changes
  how a swisstopo shell is drawn, ADR-0019's question 5. Recommended
  once that question is answered, or where a contributed shell models
  the opening, with evidence (G11 then finds the passage free).

### Smaller choices

- **Kinds beyond ADR-0021's six.**
  - **Add `road`, `area` and `connector` (recommended):** old-town
    alleys without sidewalks get lanes; areas can be crossed and walked
    along; every place reaches the graph.
  - ADR-0021's six only: alleys have no lanes, Bahnhofstrasse's areas
    have none, and places need a mapped path to the door.
- **Area kinds.**
  - **`square` and `street` (recommended):** squares are places;
    pedestrian streets mapped as areas, such as Bahnhofstrasse, are
    not.
  - One kind for all areas: every piece of Bahnhofstrasse would be a
    square where trips start and end.
- **How places reach lanes.**
  - **A node, with a `connector` lane where none is mapped
    (recommended):** everything people walk is a lane, checked by G11.
  - A lane and a distance along it: no new lanes, but the last metres
    are unchecked, and distances along polylines need square roots.
  - No reference: each reader finds the nearest lane, with its own
    ties.
- **Areas and their lanes.**
  - The polygons only: readers route across them themselves.
  - **The polygons and `area` lanes made by the ingest (recommended):**
    people walk only on lanes, and the polygons still let client offsets
    spread them.
  - Mapped lines only: no polygon to stand on, and nothing along
    Bahnhofstrasse.
- **Entrances.**
  - E1. Mapped entrances only: 208 in the district, against 802 shops
    and 300 places to eat or drink.
  - **E2. Mapped entrances, and doors estimated for shops, places to
    eat and public buildings without one (recommended).** How many
    doors this estimates was not counted.
  - E3. None in M2: trips start and end only at stops, squares and the
    world's edge.
- **Benches.**
  - **B1. OSM benches as places, drawn by the client, `object` null
    (recommended for M2).**
  - B2. Benches in RFC 0004's `street_objects`, once it has a bench
    kind and a source, with places naming them in `object`. The city's
    CC0 register lists 383 benches in the kilometre square that holds
    the district (data notes); a source ADR would choose between it and
    OSM, as ADR-0015 does for trees and lights.
  - B3. Both, unlinked: two benches drawn for one. Rejected.
- **How estimates name their rules.**
  - **A rule id per element and key (recommended):** a width set by
    default and one narrowed near a wall name different rules in one
    tile. About 30 bytes for each estimated value.
  - RFC 0004's shape, one rule per kind and key at the top: smaller,
    but it cannot say which of two rules set a value.
  - Rule ids listed once at the top, and an index per element: smaller,
    but indices shift when the list changes.
- **Points along lanes.**
  - **Every 1.5 m at most (recommended):** as ribbons; simple; about
    0.7 MB of positions in the district.
  - Only where a straight line would leave the ground by more than a
    tolerance: smaller, but more ingest code and a less simple check.
- **Months per curve.**
  - **Per day type, 36 values (recommended):** a December Saturday can
    differ from a December weekday; 24 values more than one set of 12,
    about 100 bytes a curve.
  - One set of 12 per curve: simpler, and enough if the counts show the
    same months on every day type, which was not measured.
- **Holidays.**
  - **H1. A list of dates in the document, from a public list reviewed
    under ADR-0017; empty until then (recommended).**
  - H2. None: holidays count as their weekday.
  - H3. Rules in the client's code (Easter and the rest): code, not
    data, and different per canton.
- **Asset ids.**
  - **`tile:17/{x}/{y}/osm`, with `{source}` the namespace
    (recommended):** a second walking source in a tile (at a border)
    gets its own asset. It differs from RFC 0004's rule for `{source}`
    (section 1), an open question.
  - The plain `tile:17/{x}/{y}`, as water: a rename later if a second
    source appears.
- **Trees and lamp posts.**
  - **Client offsets (recommended, as ADR-0021 says).**
  - Lanes routed around them in the data: non-OSM points would shape OSM
    lanes (section 10).

## Compatibility and migration

**World format version, one decision for RFC 0003, RFC 0004 and this
RFC.** RFC 0004 drafts the options for itself and RFC 0003: amend 0.1.0
in place (its recommendation), frozen when RFC 0001 is accepted or the
first canonical world is published, whichever comes first; or move to
0.2.0.
- **Recommended:** choose once for the three RFCs, after this RFC's
  comment period (no earlier than 2026-12-09). Nothing is published
  before M1, so a week's wait costs nothing. RFC 0003, RFC 0004 and the
  roadmap describe that choice for two RFCs; their texts follow if the
  maintainer takes this RFC into it.
- **If RFC 0003 and RFC 0004 are decided first:** this RFC follows their
  choice. It amends in place while 0.1.0 is not frozen, and otherwise
  takes the next minor version.

**Contribution contract version.** RFC 0003 and RFC 0004 amend contract
0.1 in place. RFC 0002 says that changing its rules "needs an RFC that
bumps the contribution contract version", and ADR-0020 leaves the same
choice open for A1 to A8.
- **Amend 0.1 in place while RFC 0002 is a Draft (recommended):** as
  water did; no change set's bytes change.
- **Bump it:** the rule RFC 0002 states; every tool that writes change
  sets, and the fixtures, follow.

**Existing worlds.** A re-ingest adds navigation assets and new tile
indexes, so world ids change. Under in-place amendment, existing assets
keep their bytes. Replays are recorded again with only their headers
changed; their checkpoints do not change, because the layer does not
reach the simulation.

**Fixtures:**
- `fixtures/world` gains a navigation asset in tile 17/68646/45903, a
  new tile index and a new world id;
- `sample.oprep` is recorded again (its header only);
- the facade sample is regenerated for the new gate version;
- `fixtures/format` gains a golden navigation document;
- ADR-0021's golden test for the population function reads the fixture
  tile's document.

**Refreshes.** Each refresh of OSM or of a count source gives a new
world id, so the replay suite that G10 uses is recorded again (headers
only) at each merge, as RFC 0004 notes for points.

## Determinism, provenance and privacy

- **Determinism:** the layer is never a collider, so it never enters the
  simulation's active set or state hash; `SIM_CONTRACT_VERSION` stays
  0.2.0. The implementing pull request shows the sample replay's
  checkpoints byte-identical on Linux, Windows and macOS, in debug and
  release, with only the header's world id changed.
- **The crowd:** the document holds no floats, and the population
  function seeds its randomness from its ids, so every machine derives
  the same people from the same world (ADR-0021's golden test). Where a
  world's `time_zone` is not the zone of the crowd's clock, the
  function's version pins the time-zone rules it uses.
- **Rendering** may differ between machines: doors, benches and people
  are decoration, and F2 shows none of them.
- **Provenance:** section 10. Every source gets its ATTRIBUTION.md entry
  before its first byte enters the world.
- **Privacy:** section 11.

## Tests and checks

- **Schemas:** `navigation.schema.json`; the layer, collider `none` and
  the asset id pattern in the manifest and tile-index schemas; G11 in
  the check-report schema.
- **`op_world`:**
  - a canonical reader and writer;
  - sorting, uniqueness, references inside the document, vertex
    spacing, ranges, shares that add up, `estimated` keys allowed for
    their kind, bounds;
  - world-level checks: `tile` nodes paired at one position, and ids
    unique across the world.
- **Every place that assumes a glb,** as RFC 0004 lists them:
  `validate`, the gate, `merge`, `op_stream` and the client's streaming
  loop. The new layer needs an explicit serde name.
- **Python:** a byte-identical writer, parity tests with Rust, and
  mutation tests: unsorted ids, a dangling node, an unpaired `tile`
  node, points too far apart, a wrong range, a trimesh collider, floats,
  omitted nulls, shares that are off by one, an estimate of a key its
  kind may not estimate.
- **Gate:** G3's advice and `review` for refreshes; G8 on terrain and on
  decks; and G11: a shell moved onto a sidewalk fails, a lane with an
  unmarked blocked point fails, `entrance` and `passage` ranges pass, a
  change of numbers only probes nothing, a refresh set gets `review`.
- **Planner:** the final-world G11 check and the hold for lanes blocked
  by a contributed shell.
- **Ingest:** the district run reports moves, snaps, dropped doors,
  passages and blocked lengths, and how many of the prototype's 269
  overlapping edges it clears.
- **No new crate or Python package.**

## Unresolved questions

- **The client prototype** has not run. It may show that the crowd
  needs more than one width per lane (sub-lanes by direction), or other
  places. This RFC is revised after it.
- **Why lanes and shells disagree,** beyond doors and passages, was not
  examined, so the 1 m bound is a guess until the first district run.
- **`{source}` in asset ids:** this RFC uses the namespace (`osm`); RFC
  0004 uses the ids' prefix (`osm:node`). One rule for every layer, or
  one per layer.
- **Levels and indoor ways:** lanes over each other are told apart by
  height; indoor ways and stations (Zürich HB lies just north of the
  district) are left out.
- **Stopping, sitting and shopping:** occupancies are set by eye
  (ADR-0021, open question 5).
- **Opening hours:** which part of OSM's grammar the ingest reads, and
  seasonal hours, such as terraces in summer only.
- **Licences of sources not used yet:** the canton's crossing data and
  population grids (`LicenseRef-ZH-OGD`, ADR-0017's open item);
  population grids such as GHS-POP for places without counts.
- **The planet:** the size of walking data at planet scale, not
  estimated; sidewalks unmapped in most places, so many estimates;
  Overture's transportation theme as a ready-made snapshot, with no
  licence gain over OSM.
- **Takedowns of one place:** RFC 0002 section 7 removes or reverts
  whole assets. Taking down one door needs a new version of its tile's
  document, as RFC 0004 notes for one point.
- **Accessibility:** kerbs (368 kerb nodes in the district, 346 of them
  lowered) and wheelchair access are not modelled.
- **Traffic:** crossings carry signals, but nothing waits for them until
  vehicles and trams run (M4).
- **Simulation input:** the `op_kernel` type, what is hashed and how
  lanes stream, in the RFC that puts people into `op_sim`, if one does.
