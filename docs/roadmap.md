# Roadmap

Where StreetWide is going, where it stands today, and the order in which we
plan to close the gap. [progress.md](progress.md) records what has been
done; this page looks ahead.

The roadmap is a plan, not a set of decisions. Foundational choices are
made in decision records (ADRs, in [docs/adr](adr/)) and changes to
contracts such as the world format in proposals open for comment (RFCs, in
[rfcs](../rfcs/)), as [GOVERNANCE.md](../GOVERNANCE.md) describes. Nothing
here has a date. The page is reviewed at every checkpoint and changes by
pull request.

**Now:**
- **M1** waits on three decisions: hosting (ADR-0010), how the published
  world meets the ODbL (ADR-0019) and the licence allowlist (ADR-0017).
  Work that needs no decision goes on: the client's inspect mode, which
  marks improvable buildings and gives the facade command, is on `main`.
  Every reader of untrusted files is now fuzzed, one of the steps before
  announcing.
- **M2** has started with water for the Limmat: RFC 0003 (a water layer,
  Draft) and ADR-0014 (water areas from OpenStreetMap, Proposed) are
  implemented on `main` while they are open for comment, as RFC 0001 and
  RFC 0002 were.
- **M2's next drafts** wait for the maintainer: RFC 0004 (trees and street
  lights), ADR-0015 (their Zurich sources), ADR-0016 (measured heights)
  and ADR-0018 (performance tier). RFC 0004 recommends deciding the
  pedestrian ADR first. It is drafted as
  [ADR-0021](adr/0021-pedestrians.md) (Proposed), which recommends
  people drawn by the client from a shared seed, outside the
  simulation's state, so that trunks and poles stay visual in M2.

*Last reviewed: 2026-11-25.*

## 1. Where we are going

**Walk any street on Earth in a world that looks and feels like a modern
AAA open-world game, built from open data, improved by its players, and
open to everyone.**

"AAA open-world quality" is a high bar, so here is what we mean by it:

| A player expects | Which means |
|---|---|
| **It looks real** | Physically based light and materials, water, vegetation, night lighting, weather, and a skyline that does not end a few hundred metres away |
| **It feels alive** | People on the pavements, traffic, trams, sound that changes from street to river to square |
| **There are things to do** | Activities, vehicles, places worth finding, other players |
| **It runs well** | A steady frame rate on mainstream hardware: 1080p with 8 GB of video memory is the most common setup in the [Steam Hardware Survey](https://store.steampowered.com/hwsurvey/) of October 2026. The exact target is set in an ADR (M2) |

StreetWide cannot compete with commercial studios at hand-authoring one
invented map with hundreds of artists. It aims at what they do not offer:

- **Real places.** Any street, including your own, rather than an invented
  city. Most places will start at a simpler baseline than Zurich
  (section 4).
- **Built by its players.** The canonical world only moves upward: a
  change merges when it passes the automated checks (the gate) and beats
  what is there ([RFC 0002](../rfcs/0002-contributions-and-merge-rules.md)).
- **Open.** Code under MIT or Apache-2.0, content under CC BY-SA 4.0, and
  data under its sources' licences, all recorded per asset.
- **Verifiable.** The simulation is deterministic, every asset carries its
  provenance, and every replay can be checked bit for bit.

Out of scope: replicas of real homes (interiors are always fictional), a
planning tool or digital twin, and hand-written story campaigns for the
whole planet. Whether StreetWide has an in-game economy is not decided
(section 8).

## 2. Where we are (2026-11-24)

StreetWide has working foundations (a world format, a deterministic
simulation with replays, a contribution gate) and a small, empty, silent
world.

The **AI world compiler** in the README is the pipeline from open data to
the baseline world. Its first part exists: a reproducible ingest that
converts open data into tiles. Its machine-learning part, which will fill
in what the data lacks, is not started.

A **region** below is the format's map region: a fixed square about 6.6 km
wide in Zurich ([RFC 0001](../rfcs/0001-world-format.md)).

| Area | Today | Missing for the goal |
|---|---|---|
| **World** | 0.95 km² of Zurich's old town: 30 tiles, 1,387 buildings, from swisstopo and OpenStreetMap (OSM) data. Switzerland only, one region | Every other place; data sources outside Switzerland |
| **Data and AI** | The ingest: the same inputs on the same platform give the same world id (a hash of the whole world's content, ADR-0006). No machine learning | Height and roof inference; facades and materials stored in the world data (today the client draws generic windows at render time); an ingest reproducible across platforms; a planet-scale pipeline |
| **World format** | RFC 0001 (Draft): content-addressed tiles, provenance on every asset, 7 layers, an 8-colour palette, no textures | Water, vegetation and other layers; textures; levels of detail; several regions per world; mesh compression |
| **Scale** | Everything within 600 m is loaded, nothing beyond. 48 MB of content for the district (about 51 MB per km²) | Levels of detail, streaming over HTTP, a disk cache, crossing region boundaries, moving the origin as the player travels |
| **Simulation** | Deterministic at 60 Hz, bit-identical replays on Linux, Windows and macOS. One thing moves: the player | People, vehicles, interactions, anything else that moves |
| **Look** | Physically based sky, the real sun for any date and time, shadows, ambient occlusion; procedural windows, roof tiles and paving drawn by the client ([ADR-0012](adr/0012-client-realism-from-procedural-detail.md)) | Water, trees, street furniture, night lights, weather, real facades, a visible character |
| **Sound** | None | Ambience, footsteps, traffic, spatial audio |
| **Gameplay** | Walking, flying, a map and named places | Things to do, vehicles, saves, menus, gamepad |
| **Multiplayer** | None (determinism and per-tick state hashes are the groundwork) | Networking, accounts, moderation |
| **Contributions** | A gate with ten checks, merges, takedowns and a facade tool, all working locally | Hosting ([ADR-0010](adr/0010-hosting-the-canonical-world.md)), submissions, reviewers, baseline refreshes, a first real contribution |
| **Community** | One maintainer, no external contributors yet, 13 ADRs, 2 Draft RFCs | A second person who can merge, a Code of Conduct contact, an announcement, an owner for costs |
| **Engineering** | About 14,600 lines of Rust and 4,200 of Python; 106 Rust and 91 Python tests; 12 CI jobs, including the client built and tested on Linux, Windows and macOS; seeded fuzz tests of every reader of untrusted files | GPU and performance tests, releases |

## 3. Where you can help

- Read [CONTRIBUTING.md](../CONTRIBUTING.md) first: sign-off and the checks
  to run.
- **Pick an issue draft:** [docs/issues](issues/README.md) has eight small
  or medium tasks in the ingest, the client and CI; none needs an RFC.
- **Model a facade** with [the facade guide](contributing/facades.md) and
  check it with the gate on your machine; submissions open with M1.
- **Comment on the drafts:** RFC 0001, RFC 0002 and, soon, RFC 0003.
- **Bring the skills the next milestones need:**
  - open-data licensing, reviewing and security (M1);
  - water, trees, sound and performance (M2);
  - streaming and levels of detail (M3).

## 4. The size of the challenge

The gap is large. These facts set the order of the work.

**Data.** Open data now has footprints for almost every building on Earth:
2.54 billion in Overture's November 2026 release
([Overture](https://docs.overturemaps.org/blog/2026/11/18/release-notes/)).
Heights and roof shapes are rare, and many existing heights are
machine-learning estimates:
- about 8% of Overture's buildings have a height (many of them Microsoft's
  estimates) and 0.35% a roof shape, by our count from the release files;
- in OpenStreetMap about 8% have a height or a number of levels and 1.3% a
  roof shape ([taginfo](https://taginfo.openstreetmap.org/keys/building));
- 3D models with real roof shapes (CityGML LoD2), like Zurich's, are open
  almost only in Europe and Japan: by our rough count, 2–4% of Overture's
  buildings.

Most of the world will therefore start as extruded blocks with inferred
heights and procedural roofs. The compiler's machine-learning part is core
work, not polish.

**Terrain.** High-resolution open terrain exists in a number of countries:
Switzerland, the Netherlands, England, the United States, France, Norway
and Finland. Elsewhere the best open global model is Copernicus GLO-30.
It is a 30 m surface model that includes buildings and trees, free for any
use with attribution
([licence](https://dataspace.copernicus.eu/sites/default/files/media/files/2025-06/copernicus_contributing_mission_data_access_v2_cop_dem_licenses.pdf),
section "COP-DEM-GLO-30-F" at the end). The well-known bare-earth versions,
FABDEM and FathomDEM, are non-commercial and cannot enter the world.

**Volume and cost.** Our own arithmetic, from the measured district:
- **Buildings** at today's detail come to roughly 13–200 TB before
  compression.
  - **Low estimate:** the district's 27 MB of building shells per km² over
    the 464,586 km² of built-up surface measured by the EU's Global Human
    Settlement Layer in 2020
    ([JRC](https://publications.jrc.ec.europa.eu/repository/bitstream/JRC133256/JRC133256_01.pdf)).
  - **High estimate:** the whole district's 51 MB per km² over the
    3.9 million km² of towns and villages in the same report.
- **Terrain** is the real problem. At today's detail (a 128 × 128 height
  grid per tile, about 1.6 m apart in Zurich), all land would take
  1.2–5 PB; on a 30 m grid, about 8 TB. Terrain resolution must adapt to
  the source and the distance.
- **Delivery, not storage, drives the cost.** Sending data to players
  costs between nothing on Cloudflare R2 and USD 0.085–0.12 per GB on
  Amazon CloudFront's pay-as-you-go prices, depending on the region
  ([R2](https://developers.cloudflare.com/r2/pricing/),
  [CloudFront](https://aws.amazon.com/cloudfront/pricing/pay-as-you-go/)).
  Request fees favour packing a tile into one file over thousands of
  small objects.
- **Precedent:** Microsoft Flight Simulator streams a planet built from
  more than 2.5 PB of data
  ([Microsoft](https://developer.microsoft.com/en-us/games/articles/2021/07/microsoft-flight-simulator-the-future-of-game-development/)).

**Licences.**
- **OpenStreetMap:** a world that uses OSM data is very likely a
  "derivative database" under the ODbL; if we publish it, that database
  must also be offered under the ODbL
  ([ODbL 1.0](https://opendatacommons.org/licenses/odbl/1-0/)).
  [ADR-0001](adr/0001-licensing.md) already publishes OSM-derived assets
  under the ODbL and keeps them separate.
- **Mixing sources:** under the OpenStreetMap Foundation's guideline,
  data from other sources stays outside the ODbL only if, in a given area,
  each kind of feature (roads, buildings and so on) comes entirely from OSM
  or entirely from elsewhere
  ([guideline](https://osmfoundation.org/wiki/Licence/Community_Guidelines/Collective_Database_Guideline_Guideline)).
  Zurich's buildings already mix both: where swisstopo's model has no
  building, the ingest extrudes ten buildings from OSM footprints.
- **Licences change:** Microsoft's building footprints moved from ODbL to
  CDLA-Permissive-2.0 in March 2026
  ([repository](https://github.com/microsoft/GlobalMLBuildingFootprints)).
- **Tempting sources we cannot use:** Google's Photorealistic 3D Tiles
  forbid derived models
  ([terms](https://developers.google.com/maps/documentation/tile/policies)),
  and several attractive datasets are non-commercial.

**People and time.**
- **Volunteer projects take a long time:**
  - Veloren, an open-source Rust RPG, is at version 0.18 after eight
    years;
  - OpenMW is at 0.51 after about eighteen;
  - 0 A.D. left alpha in 2026, after about twenty-five.
- **Machines do the bulk:**
  - Microsoft Flight Simulator's planet baseline was built with machine
    learning by Blackshark.ai, a company of about 50 people. Its chief
    executive estimated that perhaps 20% of the 1.5 billion buildings were
    off
    ([TechCrunch, 2020](https://techcrunch.com/2020/08/17/meet-the-startup-that-helped-microsoft-build-the-world-of-flight-simulator/)).
  - Google retired crowd-modelled 3D buildings in 2013 in favour of
    automated capture.
- **Community research:**
  - **A complete-looking start can mean fewer edits:** in OpenStreetMap,
    US counties seeded with a complete map later received fewer
    contributions, although seeding helped dense urban areas and did not
    deter committed contributors
    ([Nagaraj 2021](https://ideas.repec.org/a/inm/ormnsc/v67y2021i8p4908-4934.html)).
  - **Automated rejection drives newcomers away:** on Wikipedia,
    newcomers whose first edits were rejected by automated tools were less
    likely to keep editing
    ([Halfaker et al. 2013](https://meta.wikimedia.org/wiki/Research:The_Rise_and_Decline)).

## 5. How we order the work

1. **Prove it in the district, then scale it.** Each capability is built
   end to end (data, simulation, client and tests) in Zurich's old town
   before it meets a larger world.
2. **Contracts first.** Every milestone opens with its ADRs and RFCs, then
   schemas and tests, then features.
3. **Determinism stays.** Anything that changes gameplay state is
   simulated in `op_sim`, with Bevy-free crates audited for determinism.
   Anything that only changes the look stays in the client. Which side a
   new feature falls on is decided in its ADR.
4. **Licence before ingest.** Every new source gets a licence review and
   an ATTRIBUTION.md entry before its first byte enters the world.
5. **Machines do the routine; players do what machines cannot.**
   - The compiler fills terrain, roads and building shells today, and
     later heights, roofs, facades and materials.
   - Players own fictional interiors, real facades, activities, landmarks
     and curation.
   - Baseline content is visibly marked as improvable.
   - Compiler runs go through the same gate as people. They are reviewed
     like a bulk import into OpenStreetMap: announced, documented and
     checked by people before they land.
6. **Measure.** Each milestone ends with exit criteria we can check and a
   checkpoint in progress.md.

## 6. Milestones

Milestones are numbered by priority, not by date. Each says what it opens
with and what it needs. After M3, milestones M4, M5 and M6 can run in
parallel.

### M0. Foundations (built; decisions pending)

The walkable, deterministic district (progress.md's stages 1–6), the
contribution gate and merge rules, the facade tool, the fix for the player
shaking in concave corners, the detailed look and fast travel. All of it is
on `main`. ADR-0011 to ADR-0013 are Proposed, and RFC 0001 and RFC 0002 are
drafts (section 8).

### M1. Open for contributions

**Goal:** someone other than the maintainer improves the canonical world.

**Opens with:**
- **Licence of the world, before publishing.** An ADR, building on
  ADR-0001, on how the published world meets the ODbL. Options:
  - the whole world under ODbL 1.0;
  - OSM-derived layers under the ODbL, with each kind of feature from a
    single source in a given area.

  It also decides whether the ten OSM fallback buildings stay in the shell
  layer, and the ODbL notices in the client and with the download. We ask
  the OpenStreetMap Foundation's Licensing Working Group whether a world
  format is a database or a produced work.
- **Hosting:** accept ADR-0010, which proposes a world repository, pull
  requests, CI running the gate and a merge bot; choose a provider and an
  owner for its costs.
- **Licence allowlist:** an ADR amending ADR-0001 that lists the licences
  allowed for world data and client assets, and the forbidden ones (non-
  commercial and no-derivatives licences, Google's 3D Tiles). The gate
  checks manifests against it, and the ingest records each source's
  licence when it downloads.

**Needs:** nothing.

**Includes:**
- **Publish:** one canonical Zurich world, with the same world id for
  everyone, so players stop building their own.
- **Download:** an HTTP-backed store, or a fetch command, that downloads
  the world and verifies every object against its content id.
- **Submissions:** contributions arrive as pull requests; CI runs the gate
  without secrets and posts the report; the merge bot gets its write
  access only after a maintainer approves.
- **Baseline refresh:** the ingest emits RFC 0002's refresh change sets,
  authored by `streetwide:ingest`, so new data never overwrites
  contributions. A migration tool carries contributed assets to a new
  world-format version with their authors and lineage.
- **Reviewers:** a GOVERNANCE.md change that adds a reviewer role with merge
  rights per layer, and a trainee path (RFC 0002, question 7).
- **Newcomers:**
  - every failed check says why and how to fix it;
  - every first contribution gets a human reply, within a target time
    tracked in progress.md;
  - the client marks baseline buildings as improvable and links to the
    facade guide.
- **Before announcing:**
  - the licence ADR decided;
  - a Code of Conduct contact;
  - branch protection with the required checks;
  - fuzzing of the strict file readers (glb, canonical JSON, replays,
    bundles): done (see
    [progress.md](progress.md#reader-fuzzing-2026-11-24));
  - dependency advisories in CI;
  - the issue drafts filed;
  - RFC 0001 and RFC 0002 decided.
- **Costs:** options for who owns hosting costs and receives donations
  (the maintainer, a fiscal host such as Open Source Collective, or a
  foundation).

**Exit criteria:**
- an external contributor's facade is merged through a pull request, and
  a player who downloads the world sees it;
- a reviewer added under GOVERNANCE.md has reviewed and merged a
  contribution they did not write.

### M2. A living district

**Goal:** Zurich's old town looks and sounds like a place, day and night.

**Opens with:**
- **Water:** RFC 0003 (a water layer) and ADR-0014 (its source), drafted
  now.
- **World format:** an RFC for vegetation, street objects (lamps, benches,
  signs) and navigation data.
- **Performance tier:** an ADR naming a reference GPU class, resolution and
  graphics preset, a frame-time budget, and a scripted benchmark route
  through the named places.
- **Pedestrians:** an ADR with options:
  - in the client only, not colliding;
  - deterministic agents in `op_sim`, recorded in replays;
  - a hybrid.

  If they enter `op_sim`, an RFC follows for the simulation contract and
  the replay header (date and time become inputs).
- **Measured heights:** an ADR for a LiDAR source (swissSURFACE3D), used
  for tree heights and for RFC 0002's measured metrics in the gate.
- **Client assets:** sounds, character models and animations get manifests
  with source, licence and attribution, are downloaded rather than
  committed, and are credited in the client.

**Needs:** nothing to start. Once M1 publishes the canonical world, M2's
changes enter it as baseline refreshes.

**Includes:**
- **Water** for the Limmat, from OpenStreetMap water areas.
- **Trees** from Zurich's tree register (CC0, about 1,000 trees in the
  district), with heights from LiDAR.
- **Street lights** from Zurich's open lighting data (CC0); lit windows at
  night.
- **Ground:** squares and other paved areas drawn from land-cover data,
  not only street ribbons; street objects where data exists.
- **Sound:** ambience by place and time, and footsteps by surface.
- **A visible character:** a third-person view.
- **Pedestrians,** as their ADR decides.
- **Textures:** an RFC for textures in the world format (glTF materials
  with compressed textures) and orthophotos on roofs and open ground
  (ADR-0012, option C), at the latest before M3 fixes the delivery format.

**Exit criteria:**
- **Screenshots:** scripted screenshots at 18:00 on 21 June and 21
  December, at three named places, show:
  - the Limmat drawn from the water layer;
  - trees;
  - street lamps lit in December;
  - pedestrians, if their ADR puts them in M2.
- **Performance:** the benchmark route meets the performance-tier budget.
- **Determinism:** replays stay bit-identical on Linux, Windows and macOS.

### M3. A whole city

**Goal:** all of Zurich, about 88 km², walkable and visible from end to
end.

**Opens with:**
- **World format:** the RFC that RFC 0001 defers: crossing region
  boundaries, moving the origin as the player travels, and levels of
  detail based on geometric error.
- **Delivery:** an ADR on packs per tile, compression, the disk cache and
  budgets for bytes per km² and gigabytes per player-hour, textures
  included.

**Needs:** M1 (hosting).

**Includes:**
- **Ingest:**
  - builds many tiles in parallel and incrementally;
  - reads OpenStreetMap from dated regional extracts, not the public
    Overpass API;
  - refreshes on a schedule, so fixes made upstream in OpenStreetMap come
    back;
  - rebuilds any area from a clean cache in CI.
- **Regions:** worlds that span several regions, with precise positions
  everywhere.
- **Levels of detail:** distant buildings and terrain drawn as simplified
  tiles, so the skyline reaches the horizon; terrain resolution that adapts
  to distance and source.
- **Delivery:**
  - compressed meshes, packed per tile;
  - streamed over HTTP;
  - kept in a verified disk cache.
- **Contributions that survive rebuilds:** an RFC for entity ids
  independent of any one source (a registry that maps swisstopo, OSM and
  later ids, as Overture's GERS does), and a rule for carrying
  contributions over when a feature is split, merged or re-sourced.
- **Releases:** signed builds for Windows, macOS and Linux that download
  the world, and a decision on supported platforms.
- **Interoperability:** options for exporting the world as OGC 3D Tiles,
  for web previews and GIS tools.

**Exit criteria:**
- **Skyline:** on a scripted flight across the city, every tile within the
  view distance is drawn at some level of detail in every frame, checked
  automatically.
- **Budgets:** bytes per km² and gigabytes per player-hour stay within the
  delivery budgets.
- **Determinism:** a replay that crosses a region boundary stays
  bit-identical on all three platforms.

### M4. Things to do

**Goal:** a new player finds an hour of things to do.

**Opens with:**
- **Interactions:** an RFC for an interaction module (new input buttons
  and a simulation contract change) and the interactivity layer.
- **First game loop:** an ADR with options, for example exploration and
  discovery, collections, or deliveries.
- **Vehicles:** an ADR with options, for example trams following their
  tracks, bicycles with their own controller, and cars on Rapier's
  ray-cast vehicle model.
- **Interiors and furniture:** an RFC opening those layers (doors, the
  walkable connection to the street, always fictional), with options for
  interiors generated from a seed so that only contributed ones are
  stored.
- **Facade law:** a legal review before contributed facades may carry
  textures (RFC 0002, question 4).

**Needs:** M3 (releases for playtests; the whole city for traffic).

**Includes:**
- **Activities and missions** that contributors can write, checked by the
  gate.
- **Player-facing basics:**
  - saves, menus and settings;
  - gamepad support and key rebinding;
  - the basic level of the Game Accessibility Guidelines.

**Exit criteria:**
- at least half of ten or more first-time players, on a release build,
  play for 60 minutes unprompted;
- the first activity written by a contributor is merged.

### M5. Play together

**Goal:** friends explore the same place at the same time.

**Opens with:** a networking ADR with options, for example peer-to-peer
sessions with rollback networking for small groups (each machine runs the
deterministic simulation and re-runs recent ticks when a late input
arrives) and servers per area for larger groups.

**Needs:** M1 for small groups; M3 for servers per area.

**Includes:**
- accounts designed for privacy;
- before any sandbox opens (a multiplayer space where players change
  things live, which never reaches the canonical world without the gate):
  moderation, rate limits and a way to undo changes.

**Exit criteria:**
- four players share a session, and desyncs are detected by the state
  hashes;
- a moderator can undo staged vandalism in a sandbox, restoring its
  earlier state hash.

### M6. A second country, then Europe

**Goal:** prove that the pipeline is not Swiss-only.

**Opens with:**
- **Sources:** an ingest ADR for each new source, as ADR-0006 was for
  swisstopo.
- **Compiler:** an ADR for the compiler's first version.
- **Machine output:** an RFC amending RFC 0002 for it:
  - measured metrics where reference data exist;
  - plausibility checks on heights and storeys;
  - sampled human review elsewhere;
  - a published diff with a review window;
  - a one-step revert.

**Needs:** M3.

**Includes:**
- **Ingest adapters:** first the Netherlands' 3DBAG (CC BY 4.0, all
  buildings with roofs), then German states' open LoD2 models.
- **Coordinates:** a conversion from each country's national grid, as
  ADR-0006's is for Switzerland.
- **EU-wide buildings:** footprints, and floor counts where public bodies
  hold them, under Implementing Regulation (EU) 2023/138, which requires
  them to publish such data under CC BY 4.0 or a freer licence.
- **Compiler, first version:**
  - height and roof inference;
  - procedural facades;
  - materials from open orthophotos.
- **Regional maintainers:** people with merge rights for their region's
  tiles, recorded in GOVERNANCE.md.

**Exit criteria:**
- a Dutch city passes the same gate, benchmark and exit checks as Zurich
  in M2 and M3, with 3DBAG roofs on its buildings;
- at least two people outside Switzerland hold merge rights for Dutch
  tiles and have merged changes there in the last three months.

### M7. The planet baseline

**Goal:** any populated place on Earth is walkable at baseline quality.

**Opens with:**
- **Terrain:** an ADR on bare-earth terrain derived from GLO-30 by the
  compiler (flattened under buildings and roads, canopy removed using
  canopy-height data), with options.
- **Buildings:** an ADR on combining footprints from Overture,
  OpenStreetMap, Microsoft and Google in a way their licences allow.

**Needs:** M6.

**Includes:**
- **Terrain:** national LiDAR where it is open, the derived GLO-30
  elsewhere.
- **Order of coverage:** urban centres first, then towns, then villages.
- **Hosting:** planet-scale hosting and delivery, with funding to match.

**Exit criteria:**
- scripted spawns in a random sample of populated places load within the
  time set in the delivery ADR, on its reference connection;
- the monthly cost per active player is known and covered.

## 7. Risks

| Risk | Why it matters | What we do (status) |
|---|---|---|
| **Licences** | ODbL share-alike, mixed-source layers, licences that change between releases, non-commercial data | **In place:** a licence on every asset; forbidden sources in gate check G4. **M1:** the licence ADR and the allowlist; licences recorded at download |
| **Security** | Untrusted bundles and downloads reach our file readers in CI and in clients; a merge bot holds write access | **In place:** strict readers, fuzzed in CI; content ids checked on read. **M1:** dependency advisories, a gate without secrets, write access only after approval |
| **One maintainer** | A single point of failure for reviews, decisions and knowledge | **M1:** a reviewer role and a second person with merge rights. **M6:** regional maintainers |
| **Cost** | Delivering data to players dominates the bills | **M1:** a cost owner, with egress cost as a criterion in ADR-0010. **M3:** packed tiles, compression, levels of detail and a disk cache |
| **Compiler quality and ingest scale** | A wrong baseline at scale; an ingest far too slow for a planet | **M3:** parallel ingest from regional extracts, with its speed measured in CI. **M6:** measured metrics, plausibility checks and review like an import |
| **Contributor dynamics** | Complete-looking baselines (outside dense cities) and automated rejection both reduce contributions; human review becomes the bottleneck | **M1:** clear gate feedback, a human reply to first contributions, baseline marked as improvable, reviewer tiers |
| **Determinism versus the ecosystem** | Bevy releases every few months and crates lag behind | **In place:** a Bevy-free simulation; Bevy and Rapier pinned (ADR-0005). **Each new simulation crate:** a determinism audit and a cross-platform replay test |
| **Privacy and facade law** | Street imagery shows people and number plates; some countries restrict reproducing buildings | **In place:** interiors are fictional; the privacy check G6. **M4:** a legal review before textured facades; no third-party street-level imagery inside assets |
| **Scope creep** | Trying to be both a game and a planning tool would split a small team | The goal in section 1; anything outside it is drafted as options and decided in an ADR |
| **Griefing** | Live sandboxes get vandalised quickly | **In place:** changes to the canonical world go through the gate. **M5:** moderation and undo before any sandbox opens |

## 8. Decisions waiting

| Decision | Record | Status | Unblocks |
|---|---|---|---|
| Hosting, provider and who pays | [ADR-0010](adr/0010-hosting-the-canonical-world.md) | Proposed; the maintainer chooses | M1 |
| Corner fix, detailed look, fast travel | [ADR-0011](adr/0011-stop-corner-oscillation.md), [ADR-0012](adr/0012-client-realism-from-procedural-detail.md), [ADR-0013](adr/0013-travel-by-starting-new-sessions.md) | Proposed | closing M0 |
| World format and contribution rules | [RFC 0001](../rfcs/0001-world-format.md), [RFC 0002](../rfcs/0002-contributions-and-merge-rules.md) | Draft | announcing (M1) |
| How the published world meets the ODbL; the OSM fallback buildings | [ADR-0019](adr/0019-odbl-and-the-published-world.md), building on ADR-0001 | Proposed | publishing (M1) |
| Licence allowlist | [ADR-0017](adr/0017-licence-allowlist.md), amending ADR-0001 | Proposed; its gate and validation rules need an RFC | M1, new sources in M2 |
| Water layer and water source | [RFC 0003](../rfcs/0003-water-layer.md), [ADR-0014](adr/0014-water-areas-from-openstreetmap.md) | Draft and Proposed, implemented; comments until at least 2026-12-01; the contract version is chosen with RFC 0004 | M2 |
| Trees and street lights in the world format | [RFC 0004](../rfcs/0004-point-features.md) | Draft; comments until at least 2026-12-01 | M2 |
| Trees and lights from Zurich's open data; measured heights | [ADR-0015](adr/0015-zurich-trees-and-lights.md), [ADR-0016](adr/0016-swisssurface3d-heights.md) | Proposed | M2 |
| Performance tier | [ADR-0018](adr/0018-performance-tier.md) | Proposed | M2 |
| Pedestrians | [ADR-0021](adr/0021-pedestrians.md) | Proposed; RFC 0004 recommends deciding it first | M2 |
| Navigation data; how the client draws trees, lamps and lit windows | an RFC after the pedestrian ADR; an ADR extending ADR-0012 | to be drafted | M2 |
| Code of Conduct contact, branch protection, filing the issue drafts, announcing | tasks, see M1 | open | announcing (M1) |
| Who owns costs and receives donations | with ADR-0010 | to be drafted | M1 |
| Whether there is an in-game economy | none yet | not drafted | later |
| The first game loop, vehicles, networking, the compiler | ADRs | drafted when their milestones start | M4, M5, M6 |

## 9. Sources

The figures above come from research done for this roadmap on 2026-11-24:

- **Buildings:**
  - Overture Maps release notes and release files, 2026-11-18;
  - OpenStreetMap taginfo;
  - Microsoft Global ML Building Footprints, including its licence change
    of 2026-03-11;
  - Google Open Buildings;
  - 3DBAG;
  - Germany's LoD2-DE documentation (BKG);
  - EU Implementing Regulation 2023/138.
- **Terrain and imagery:**
  - the Copernicus DEM licence;
  - the FABDEM licence;
  - swisstopo product pages;
  - national LiDAR programmes' licence pages.
- **Scale and cost:**
  - the JRC Global Human Settlement Layer report (built-up surface
    464,586 km² and towns and villages 3.9 million km² in 2020; about
    146 million km² of land);
  - Cloudflare R2 and Amazon CloudFront pay-as-you-go price pages;
  - Microsoft's Flight Simulator article;
  - Planetiler and Protomaps documentation.
- **Precedents and community:**
  - Veloren, OpenMW and 0 A.D. project pages;
  - TechCrunch on Blackshark.ai (2020-08-17);
  - Google's 2013 notices retiring Building Maker and user-made 3D models;
  - Nagaraj, *Management Science* 67(8), 2021;
  - Halfaker et al., *American Behavioral Scientist* 57(5), 2013;
  - OpenStreetMap Foundation licence guidelines;
  - Google Map Tiles API policies.
- **Zurich:**
  - Stadt Zürich open data: the tree register (`geo_baumkataster`) and
    public lighting, both CC0;
  - Statistik Stadt Zürich for the city's area.
- **Players:** Steam Hardware Survey, October 2026.

Volumes and the LoD2 share are our own arithmetic, from the measured
district (48 MB over 0.95 km², of which 25.5 MB are building shells) and
the figures above. They are orders of magnitude, not plans.
