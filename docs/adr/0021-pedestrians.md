# ADR-0021: Decide where pedestrians live: the client, the simulation or both

- **Status:** Accepted (2026-11-25)
- **Date:** 2026-11-25
- **Deciders:** @victorpicaud
- **Supersedes / superseded by:** none (answers the roadmap's M2
  question on pedestrians; RFC 0004 and the navigation-data RFC follow
  it)

## Context

The [roadmap](../roadmap.md)'s M2 opens with "Pedestrians: an ADR with
options": in the client only, not colliding; deterministic agents in
`op_sim`, recorded in replays; or a hybrid. If they enter `op_sim`, an
RFC follows for the simulation contract and the replay header, and the
date and time become inputs.
[RFC 0004](../../rfcs/0004-point-features.md) asks for this decision
before its own: agents in `op_sim` in M2 would make tree trunks and
lamp posts obstacles, and navigation data simulation input, under an
RFC of its own.
[progress.md](../progress.md#m2-decisions-drafted-2026-11-24-branch-m2-decisions-drafts-only)
lists it third among the decisions waiting. The goal is "People on the
pavements" (roadmap, section 1), and M2's exit screenshots show
pedestrians "if their ADR puts them in M2".

**Constraints:**
- **Determinism** ([ADR-0003](0003-determinism-strategy.md)): a fixed
  60 Hz tick; one seeded RNG, drawn in schedule order (rule 3); a BLAKE3
  hash of the whole state after every tick (rule 9); a small math
  vocabulary. Any change to a state hash bumps `SIM_CONTRACT_VERSION`
  and re-records the fixtures, and replays recorded before are then
  refused.
- **Replays** ([ADR-0007](0007-replay-file-format.md)): every tick's
  input and a hash every 60 ticks. The header holds the seed, the world
  id and the spawn, but no date or time. The client's clock "never
  reaches the simulation" (`op_client/src/sky.rs`), so a replay does
  not record its sun either.
- **The simulation never depends on rendering**
  ([ADR-0002](0002-crate-layout-and-dependency-direction.md)), so it
  does not know the camera.
- **One main thread**
  ([ADR-0008](0008-client-architecture-and-streaming.md)): the
  simulation steps inside Bevy's `FixedUpdate`, in the main update. The
  client's Bevy build has pipelined rendering, which runs the render
  work on its own thread alongside, so a share of a 16.7 ms frame below
  is what the work takes from the main update's budget, and an upper
  bound on what it adds to frame time.
  [ADR-0018](0018-performance-tier.md) (Proposed) asks for a 99th
  percentile frame time of at most 16.7 ms on an 8 GB card of the
  RX 6600 class at 1080p, and names no CPU.
- **Rollback, if M5's networking ADR chooses it** (the roadmap's
  example for small groups): each machine re-runs recent ticks when a
  late input arrives. GGRS predicts up to 8 frames by default
  ([GGRS 0.13.0](https://docs.rs/ggrs/0.13.0/ggrs/struct.SessionBuilder.html)),
  so one frame can run up to 9 ticks. NetherRealm's target for a whole
  fighting game was "8 Frames in 16ms"
  ([GDC 2018](https://gdcvault.com/play/1025471/8-Frames-in-16ms-Rollback)).
- **The gate** ([ADR-0009](0009-simulation-based-gate-checks.md)): G9
  places the player capsule on public ways; G10 re-runs registered
  walks and needs every waypoint reached within 2 m.
- **Which side a feature falls on** (roadmap, section 5): "Anything
  that changes gameplay state is simulated in `op_sim`", and "Anything
  that only changes the look stays in the client."
- **Privacy:** interiors are fictional, never replicas of real homes
  (CLAUDE.md, README.md). No rule covers people yet; this ADR proposes
  one ([Privacy](#privacy)).
- **Licences:** every source carries its licence
  ([ADR-0001](0001-licensing.md)), and client assets get manifests with
  theirs (roadmap, M2). [ADR-0017](0017-licence-allowlist.md) and
  [ADR-0019](0019-odbl-and-the-published-world.md), both Proposed,
  draft which licences are allowed, for world data and client assets,
  and how the ODbL applies.

**Evidence.** Gathered on 2026-11-25 in three research notes (open-world
games; crowd models and walking behaviour; open data for paths, counts
and destinations) and a prototype, and checked the same day by a review
of this draft. None of them is in the repository yet (Consequences).
Outside facts carry their source. "Measured" means the notes, the
prototype or the review measured it; "estimate" means arithmetic on
measured numbers.

### What open-world games do

- **Tiers, never a whole crowd in full detail.** Assassin's Creed Unity
  worked "with the limit of 40 real AIs and 120 high resolution models"
  and put "10,000 crowd NPCs" on screen
  ([GDC 2015](https://www.gdcvault.com/play/1022141/Massive-Crowd-on-Assassin-s)):
  full AI within 12 m, cheaper "puppets" to 40 m, and beyond that a
  crowd that follows the navigation mesh, never leaves its
  predetermined path and, on contact, only shifts position without
  reacting
  ([GAME Watch, 2015](https://game.watch.impress.co.jp/docs/news/20150307_691715.html)).
  Watch Dogs: Legion deletes ignored passers-by once the player is far
  away, and "uprezzes" those the player engages; their routes are
  simulated "at a high enough rate that you could catch them on any
  point on their path"
  ([Game Developer, 2020](https://www.gamedeveloper.com/design/how-watch-dogs-legion-s-play-as-anyone-simulation-works)).
- **Online, someone owns the crowd.** The Division runs all its AI on
  servers at 10 Hz, and clients animate it
  ([Game Developer, 2018](https://gamedeveloper.com/design/how-tom-clancy-s-the-division-manages-ai-online);
  the article covers combat AI). On GTA V's FiveM servers, an entity is
  controlled by one owning client and migrates when it leaves that
  client's range, and a grid over the world sets who owns the ambient
  population
  ([FiveM](https://docs.fivem.net/docs/scripting-reference/onesync/)).
  Lockstep games simulate everything on every machine, with cheap units
  and slow ticks: Supreme Commander at 10 Hz
  ([Forrest Smith, 2011](https://www.forrestthewoods.com/blog/synchronous_rts_engines_and_a_tale_of_desyncs/)),
  Age of Empires (its article's title: "1500 Archers on a 28.8",
  [Bettner and Terrano, 2001](https://www.gamedeveloper.com/programming/1500-archers-on-a-28-8-network-programming-in-age-of-empires-and-beyond)).
  For Honor runs "a hundred NPCs" for 8 players with "a variation on
  deterministic simulation that uses time travel"
  ([GDC 2019](https://gdcvault.com/play/1026077/Back-to-the-Future-Working)),
  on "a distributed peer-to-peer deterministic simulation"
  ([GDC 2017](https://gdcvault.com/play/1024035/Deterministic-vs-Replicated-AI-Building)):
  soldiers, not ambient pedestrians. The notes found no published game
  with rollback networking and hundreds of ambient pedestrians.
- **What players notice.** "NPC AI and animation is the thing that
  breaks the illusion more than anything": "It doesn't matter that your
  models look photo realistic if an NPC walks into a wall" (Obbe
  Vermeij,
  [GTA BOOM, 2026](https://www.gtaboom.com/exclusive-if-an-npc-walks-into-a-wall-the-illusion-breaks-how-gta-really-feels-alive-aa3d)).
  Pedestrians that vanished when the player turned round were reported
  in Cyberpunk 2077
  ([T3, 2020](https://t3.com/news/cyberpunk-2077-ps4-and-xbox-one-plot-thickens-after-shocking-revelation-on-bugs)).
  Repeated looks are spotted far more easily than repeated motion, and
  eyes go first to heads and upper bodies
  ([McDonnell et al. 2008](https://doi.org/10.1145/1360612.1360625),
  [2009](https://history.siggraph.org/?p=108813)).
- **Crowds are tuned by trying.** Cyberpunk 2077 changed its crowds in
  patch 1.2 and again in patch 1.5 ("Improved crowd reactions,
  pathfinding and despawning")
  ([1.2](https://www.cyberpunk.net/en/news/37801/patch-1-2-list-of-changes),
  [1.5](https://www.cyberpunk.net/en/news/41435/patch-1-5-next-generation-update-list-of-changes)).

### Pedestrians in numbers

| | Value | Source |
|---|---|---|
| Free walking speed | 1.34 m/s, standard deviation 0.26 m/s | Weidmann 1993, ETH Zürich ([doi](https://doi.org/10.3929/ethz-b-000242008)) |
| Speed by trip purpose and on steps | shopping 1.16 m/s and leisure 1.10 m/s, 13 and 18% below the mean if each purpose is a quarter of the traffic; on stairs, about half the horizontal speed | Weidmann 1993 |
| Speed and density | speed falls slowly at first: nearly 10% lower at 0.5 people per m², about half at 1.5 | Weidmann 1993 |
| People walking in groups | 55% in a public place on a working-day afternoon and 70% in a busy shopping walkway on a Saturday afternoon (Toulouse; the authors relate the difference to the setting), mostly in groups of 2 to 4, abreast at low density; each extra member slows a group by about 0.04 to 0.08 m/s | Moussaïd et al. 2010 ([PLoS ONE](https://www.ncbi.nlm.nih.gov/pmc/articles/PMC2850937/)) |
| Reaction time | 0.17 to 0.80 s | DIN 2013 and Still 2000, as compiled by Bosina and Weidmann ([STRC 2018](https://strc.ch/2018/Bosina_Weidmann.pdf)) |
| Bahnhofstrasse, the busiest counted street, at its Saturday peak | about 135 people per 100 m of street, or 0.06 to 0.1 per m² | estimate from the counts below, at 1.34 m/s |
| Through the year | Bahnhofstrasse at 18:00, weekdays and Saturdays: 1.4 to 2.6 times as many people in December as in June (2025). The Limmatquai: about twice as many a day in June as in January (2024) | measured, from the counts below |
| People within 160 m of the player | about 180, 530 or 1,800 at 0.01, 0.03 or 0.1 per m² | estimate: streets, paths and squares cover 22% of the district's tiles |
| People within 600 m, the client's drawing radius | about 2,500, 7,500 or 25,000 at the same densities, fewer where that radius reaches past the district | estimate, as above |

### Data for the district

- **Paths: OpenStreetMap is nearly complete.** The ingest's cached
  answer (OSM data of 2026-11-23) holds 433 sidewalk ways, 267 crossing
  ways, 252 steps and 232 pedestrian ways, 34 of them squares mapped as
  areas. 99% of the city's surveyed sidewalk area is in polygons that
  an OSM pedestrian way passes within 1 m of (90% for OSM's sidewalk
  ways); 96% of OSM's sidewalk length lies within 1 m of a surveyed
  sidewalk; and 92% of the city's walking network lies within 5 m of an
  OSM pedestrian way (the notes, against the city's CC0
  [survey](https://data.stadt-zuerich.ch/dataset/geo_amtliche_vermessung_stadt_zuerich)
  and
  [walking network](https://data.stadt-zuerich.ch/dataset/geo_fuss__und_velowegnetz)).
- **The ingest keeps no walking graph.** Sidewalks become 2 m ribbons
  without topology, and squares mapped as areas are dropped. The query
  asks for no nodes (the district has 219 crossing points, 368 kerbs
  and 208 entrances) and no `highway` relations, so the Münsterhof is
  missing as an area (`tools/ingest/overpass.py`, `transport.py`).
- **Counts:**
  - Bahnhofstrasse: three areas, and Lintheschergasse beside it,
    hourly, counted by hystreet in a pilot of CBRE, hystreet.com, Swiss
    Life Asset Managers, the city's urban development office and the
    Bahnhofstrasse association
    ([dataset](https://data.stadt-zuerich.ch/dataset/hystreet_fussgaengerfrequenzen)),
    under "Creative Commons Attribution" with no version. In 2025 an
    area saw 9,165 (Lintheschergasse) to 38,597 people on an average
    weekday; weekdays peak at 17:00, Saturdays at 15:00 or 16:00.
  - The city's own counters, per 15 minutes, under CC0
    ([dataset](https://data.stadt-zuerich.ch/dataset/ted_taz_verkehrszaehlungen_werte_fussgaenger_velo)).
    The district's only one, on the Limmatquai, has counted almost
    nothing since December 2024; its data from 2018 to 2024 remain, and
    its Sundays are nearly as busy as its weekdays.
  - Boardings per tram and bus stop, trip and day type, under CC0
    ([VBZ](https://data.stadt-zuerich.ch/dataset/vbz_fahrgastzahlen_ogd)):
    yearly averages computed per scheduled trip, from counters in about
    a fifth of the vehicles; 135,539 a day at the district's 13 stops
    in 2025.
- **Destinations** (OSM nodes, not in the cache): 802 shops and 300
  places to eat or drink, 70% and 75% of them with opening hours; 36
  tram stop positions (`railway=tram_stop`, several per stop); 353
  benches; and about 140 places tagged `outdoor_seating=yes` (a count
  during the review).

### The prototype

A pedestrian module written like `op_sim`'s: `op_kernel`'s `Module`,
`Rng` and `StateHasher`, a walk graph in integer millimetres, and every
agent updated from the previous tick's state. The graph comes from the
ingest's OSM cache, projected as the ingest projects; its largest
connected part has 7,020 edges and 59.98 km. Three variants:
- **a. Graph and separation:** steer at the next node; neighbours
  within 1 m push apart.
- **b. Graph and ORCA,** written from the RVO2 algorithm: the 10
  nearest neighbours within 4 m, no static obstacles.
- **c. Capsules:** each agent is the player's capsule, moved by
  `op_sim`'s own character controller against the real shells, terrain
  and bridge decks of 4 tiles (481 colliders, 205,073 triangles, 10.44
  km of graph).

**Setup:** an AMD Ryzen 9 3900 (Zen 2) under Windows 11, Rust 1.100.0,
a release build with the workspace's profile, one thread; 3,600
measured ticks after 600 (600 after 60 for 10,000 capsules); each
configuration run twice in separate processes, and ranges go from one
run to the other. The machine was shared with other work: the first
four-tile pair of a and b overlapped outside load and was run again,
so the ranges understate the uncertainty. Every tick is hashed as
`Schedule::tick` hashes it, with one `StateHasher` call per field, as
`op_sim` hashes the player.

**Per tick,** median (99th percentile):

| Agents | a. Separation | of which hashing | b. ORCA | c. Capsules |
|---|---|---|---|---|
| 100 | 0.024–0.025 ms (0.031–0.033) | 0.019–0.020 ms | 0.025–0.026 ms (0.030–0.044) | 3.6–3.9 ms (7.7–7.9) |
| 500 | 0.12–0.13 ms (0.21–0.22) | 0.095–0.098 ms | 0.13 ms (0.22–0.23) | 20.8–22.0 ms (32.4–33.0) |
| 2,000 | 0.52–0.53 ms (0.83–0.90) | 0.385–0.390 ms | 0.62 ms (0.99–1.06) | 76–79 ms (125–130) |
| 10,000 | 3.04 ms (4.6–4.9) | 1.95–1.96 ms | 5.1–5.3 ms (7.3–7.6) | 471–475 ms (525–642), over 600 ticks after 60 |

**Under rollback,** the worst case on every frame: one new tick, a
restore to 8 ticks back and 8 ticks re-simulated, each hashed. Median
frame time and its share of a 16.7 ms frame, with the 99th percentile,
which ADR-0018 budgets, in brackets:

| Agents | a. Separation | b. ORCA | c. Capsules |
|---|---|---|---|
| 100 | 0.23–0.24 ms, 1.4% (0.41–0.42 ms, 2.4–2.5%) | 0.24–0.25 ms, 1.5% (0.40–0.43 ms, 2.4–2.6%) | 31.6 ms, 190% (55.0–55.9 ms, 330–336%) |
| 500 | 1.15–1.16 ms, 6.9% (1.92–1.94 ms, 11.5–11.6%) | 1.30 ms, 7.8% (2.16–2.38 ms, 13–14%) | 244–252 ms, over 600 frames |
| 2,000 | 4.9–5.2 ms, 29–31% (7.8–8.0 ms, 47–48%) | 6.4–6.6 ms, 38–40% (10.0–10.1 ms, 60%) | not run |
| 10,000 | 28.6–29.5 ms, 172–177% (42.4–45.2 ms, 254–271%) | 47.9–49.7 ms, 287–298% (78.5–85.9 ms, 471–515%) | not run |

What it showed:
- **Hashing costs more than moving.** At 2,000 agents, one
  `StateHasher` call per field costs 0.19 µs per agent and tick; the
  same bytes in one call cost 0.024 µs. BLAKE3 depends only on the
  bytes, so a hasher that buffers them would keep every existing hash.
  With it, the worst frame for 2,000 agents of a would take about 1.6
  to 1.8 ms (10 to 11% of a frame) while every re-simulated tick is
  still hashed, as ADR-0003's rule 9 requires, or 1.3 to 1.4 ms (8%)
  with one hash per frame, which would amend rule 9 (estimates).
- **Capsules cost about 150 times as much** per agent and tick as a
  (145 to 180 times at the same number of agents), and moving them,
  without hashing, 400 to 780 times as much; almost all of it is the
  controller's shape casts. They also break exact rollback: at 100
  agents, 423 of 28,800 re-simulated ticks gave another hash, the same
  ones in both runs. Kept out of Rapier's world, they rolled back
  exactly. The prototype traces this to Rapier 0.36.1's broad phase,
  whose tree depends on its history, which restoring the agents does
  not restore.
- **Otherwise deterministic.** All 35 configurations gave the same hash
  chain in both runs; 460,800 re-simulated ticks of a and b matched the
  first run; a Linux build (x86_64, in a container on the same CPU)
  matched Windows for a and b at 2,000 agents and c at 100. Not tested:
  arm64, macOS, debug builds.
- **OSM ways and building shells disagree.** In the 4 tiles, 269 of
  1,120 edges (1.77 of 10.44 km) have at least one point where the
  player capsule overlaps a shell, by G9's capsule test at points about
  1 m apart. Part of it is by design: of the 343 walkable ways there,
  44 end on a building outline, as paths to a door do, and 6 pass
  through buildings (`tunnel=building_passage`); the graph keeps both.
  In c, about a quarter of the agents got stuck within 70 s; a and b
  walk through those walls. Why the rest disagree was not examined.
- **ORCA jams:** at 10,000 agents, 64 had to start again after 20 s
  without progress, against none in a. Where they jammed was not
  logged.
- **Sparse crowds, overlaps not counted.** Agents started at random
  nodes, so 2,000 on the 59.98 km graph is about 3.3 per 100 m of graph
  and 10,000 about 17; on the 4 tiles' 10.44 km, 2,000 is about 19.
  Bahnhofstrasse's peak is about 135 per 100 m of street. Separation is
  a soft push, and no overlap between agents was counted.
- **Kernel gaps:** `Schedule` and `Module` have no snapshot or restore;
  no module can read another's state (`StepContext` carries the tick,
  the input and the RNG); one RNG drawn in agent order stops agents
  being split across threads; `dmath` has no `exp` or `ln`.

### Crowd models and crates

- **Social force** with Helbing's contact terms diverged at 60 Hz in
  the notes' runs, reaching 84 m/s at 0.03 people per m²; Köster et al.
  report "oscillations, collisions, and instabilities" even at very
  small steps
  ([2013](https://link.aps.org/doi/10.1103/PhysRevE.87.063305)).
- **The collision-free speed model**
  ([Tordeux et al. 2016](https://arxiv.org/abs/1512.05597)) never let
  bodies overlap in the notes' runs, but jammed head-on from 0.56
  people per m².
- **Separation,** the prototype's a, let bodies overlap in the same
  head-on runs: up to 0.36 m at 0.56 people per m² and 0.45 m at 1.67,
  under 5 cm at 0.17.
- **No crowd crate fits `op_sim` as it is** (sources read on
  2026-11-25). `dodgy_2d` (ORCA) calls the unseeded `rand::random()`
  and the platform's `cos` and `sin` in one degenerate case; `landmass`
  iterates `HashSet`s; `bevy-flock-core` and `bevy_orca` depend on
  `bevy_*` crates, which ADR-0002 bans; `navmesh-rs` claims identical
  results across platforms with its `libm` feature, but was under a
  month old. Path search with integer costs (`pathfinding`) fits.
- **Signed zeros.** `f32::min`, `max` and `clamp` may return either
  zero when given +0.0 and −0.0
  ([Rust](https://doc.rust-lang.org/std/primitive.f32.html)), and
  `StateHasher` hashes the bit pattern. On x86_64 the result followed
  the argument order in the notes' test (the second argument), and the
  review saw a release build return −0.0 for `max(−0.0, +0.0)` when
  `max(+0.0, −0.0)` was computed beside it, and +0.0 alone: the
  compiler may treat `max` as commutative. Arm's scalar instruction
  orders −0 below +0
  ([FMAXNM](https://www.scs.stanford.edu/~zyedidia/arm64/fmaxnm_float.html)).
  ADR-0003 lists `min` and `max` as safe.

## Options considered

Each option gives what it is, what changes in which crate and contract,
its tests, its cost, what it means for multiplayer (M5) and its effort.
Drawing people (models, animations, levels of detail) is the same work
and cost in every option but D, and was not measured.

### A. In the client only, not colliding

`op_client` spawns, steers and animates people around the camera with
its own state, as single-player games do: out of view, with fades, any
steering model, reactions to the player. The simulation never hears of
them, and the player walks through them.

**Seeded or not:** a crowd that draws only from the session seed, steps
on the simulation's ticks and spawns from the camera's pose at each
tick, never from frame times or what the GPU drew, shows the same
people on every scripted run of one build on one machine, given a
pinned crowd tick as for C2 ([What the crowd must
do](#what-the-crowd-must-do)). Another machine, or another camera path,
may see other people. Recommended within A: seeded.

- **Changes:** `op_client` gets the crowd and an option to turn it off;
  the world format gets paths as a navigation layer, read by the client
  only (the navigation-data RFC); the gate gets that layer's rules and
  a lane check, with a new gate version; `tools/ingest` builds the walk
  graph (see Data); character models and animations get manifests
  (roadmap, M2). `SIM_CONTRACT_VERSION`, the replay header and the
  state hash do not change.
- **Tests and CI:** client unit tests; screenshots with `--screenshot`;
  the benchmark route (ADR-0018). The determinism suite does not
  change.
- **Cost:** nothing in the simulation. On the client, in the main
  update: steering, animation and drawing, not measured.
- **Multiplayer:** each player sees a crowd of their own; reactions stay
  local; nothing to synchronise.
- **Effort:** small: the navigation-data RFC and its ingest, and the
  client's crowd.
- Pros:
  - the cheapest; every rendering trick is allowed;
  - tuning changes no contract;
  - replays, the simulation's part of the gate and rollback are
    untouched.
- Cons:
  - unseeded, other people on every run: screenshots and benchmark
    stops change from run to run. Seeded, the same people only on one
    machine and along one camera path;
  - a replay plays back with another crowd, unless the crowd is seeded
    and played back on the same machine;
  - other people for each player in M5;
  - people never block the player, and the player walks through them;
  - if gameplay needs pedestrians later, their behaviour is written
    again for `op_sim`.

### B. Deterministic agents in `op_sim`, recorded in replays

A `pedestrians` module in `op_sim`'s `Schedule`. People near the player
are spawned by a seeded rule from what the simulation knows (where
players are, never the camera), walk on a lane graph, push apart, and
are hashed every tick. Players and pedestrians can block each other,
which takes changes to the kernel and to the player's controller
(below).

Choices within B:
- **Motion:** graph and separation (a) is the cheapest and needs only
  `+ - * /` and `sqrt`, but lets people overlap in dense head-on flows;
  the collision-free speed model never let them overlap, but needs a
  `dmath::exp` and jams head-on; ORCA (b) costs more and jammed in the
  prototype; social force is unstable at 60 Hz. Recommended within B:
  a, with people as discs the module tests itself, never Rapier bodies
  (option E), once overlaps are counted at street density (0.01 to 0.1
  people per m², estimate), which the prototype did not do.
- **Rollback:** re-run the crowd on every rollback, 9 ticks in the
  worst frame; or, recommended within B, a **perception delay**: people
  see players only as they were D ticks before, with D at least the
  prediction window plus one (9 ticks, 150 ms, below the 0.17 to 0.80 s
  of human reaction times). The crowd then depends only on confirmed
  inputs, steps once per tick and keeps its states across a rollback; a
  sync test, as GGRS has, checks it. People then react to a player 9
  ticks late, after walking about 0.2 m further. Under rule 9 every
  re-simulated tick is still hashed, crowd included, so the delay saves
  the crowd's steps but not its hashing unless rule 9 is amended. If
  people ignore players instead, they are option C2's function inside
  `op_sim`.

- **Changes:**
  - **simulation contract** (an RFC, as the roadmap says): the module,
    its state in the hash, a `SIM_CONTRACT_VERSION` bump, the fixtures
    recorded again;
  - **replay header** (the same RFC; ADR-0007 sends header changes
    through one): the date and time become inputs, so the replay
    contract version and `replay-header.schema.json` change. The
    client's clock becomes a session setting: changing it starts a new
    session, as travel does
    ([ADR-0013](0013-travel-by-starting-new-sessions.md));
  - **`op_kernel`:** a navigation type that brings lanes to `op_sim` in
    integer millimetres, as `ColliderMesh` brings geometry. A tick
    stalls until its lanes have loaded (ADR-0003 rule 8), so people
    must stay within the 600 m the client loads. A way for one module
    to read another's state, since people must see players and the
    player's controller must see people. Randomness per agent from
    (seed, agent, tick), so that one more agent does not shift
    everyone's draws, which amends ADR-0003's single RNG (rule 3); a
    buffered hasher; snapshot and restore before M5; for rollback,
    either hashing only the ticks that a peer or a replay compares
    (confirmed ticks; every 60th for replays) or hashing each module
    separately under a new encoding, which amends rule 9; explicit
    comparisons wherever `min` or `max` could meet two zeros;
  - **the player**
    ([ADR-0004](0004-physics-and-character-controller.md)): people
    block the player only if its controller tests their discs itself,
    which amends ADR-0004, or if people become colliders that only the
    player's queries see, at E's cost and with its rollback mismatches;
  - **world format:** navigation data as simulation input (the
    navigation-data RFC), pinned by the world id;
  - **RFC 0004:** trunks and poles become obstacles within M2 (its
    section 7: colliders for points, a new G9), before it is accepted;
  - **gate:** G10 would re-run walks among people who can block the
    player, so a change that only moves people could fail it. The gate
    runs without people, or people never block players;
  - **`op_client`:** draws the agents from the simulation's state,
    interpolated like the player.
- **Tests and CI:** two simulations in one process, and a stalled run
  against an unstalled one, with pedestrians; the fixture replay with
  pedestrians on Linux, Windows and macOS arm64, in debug and release;
  a rollback test (restore and re-simulate equals a straight run);
  pinned bits for any new `dmath` function; a timing run on the
  district's graph at each checkpoint, since CI's timings are too noisy
  for a budget.
- **Cost** (prototype, a; the rest estimates): 0.12 to 0.53 ms per tick
  for 500 to 2,000 agents. Under worst-case rollback, 1.15 to 5.2 ms
  per frame at the median (7 to 31% of 16.7 ms) and 1.9 to 8.0 ms at
  the 99th percentile (12 to 48%). With a buffered hasher, about 0.38
  to 1.8 ms while every re-simulated tick is hashed (rule 9), or 0.26
  to 1.4 ms with one hash per frame. A perception delay saves the
  crowd's steps but not its hashing: for 2,000 agents, 0.57 to 0.64 ms
  with the buffered hasher under rule 9 (3.6 to 3.9 ms with today's),
  or 0.18 to 0.20 ms with one hash per frame. Re-run on every rollback,
  10,000 agents would take 9.9 to 10.5 ms with one hash per frame and
  11.7 to 12.5 ms under rule 9: 59 to 75% of a frame at the median, in
  the main update.
- **Multiplayer:** the same people for everyone, and a desync shows in
  the hashes. But rollback has no interest management: every machine
  simulates the people around every player, so four players far apart
  pay for four crowds. Servers per area, M5's option for larger groups,
  could own their crowds, as The Division's servers own its AI.
- **Effort:** the largest. Before any code: the navigation-data RFC as
  simulation input, the simulation and replay RFC, RFC 0004's colliders
  and amendments to ADR-0003 (rules 3 and 9) and ADR-0004. Then kernel,
  simulation, ingest, client and gate work, and every fixture recorded
  again.
- Pros:
  - the same people in every replay and, later, for every player, bit
    for bit;
  - bumping, blocking and fleeing are real, testable simulation;
  - one implementation, never moved later.
- Cons:
  - every tuning change bumps `SIM_CONTRACT_VERSION`, re-records the
    fixtures and makes earlier replays unreadable, while crowds are
    tuned by trying;
  - the most contract work, while six other M2 drafts wait (roadmap,
    section 8);
  - it shares the main update with the rest of the client, and the
    tier names no CPU;
  - the simulation cannot use the camera to hide where people appear:
    it knows where players look (yaw and pitch are inputs) and holds
    the shells and terrain within 160 m, but not the field of view,
    what only the client draws, or anything farther;
  - no gameplay needs it before M4 (interactions) and M5 (other
    players).

### C. A hybrid

#### C1. A near population in `op_sim`, ambient people in the client

`op_sim` owns the people within a few tens of metres of each player,
the ones who can touch gameplay. The client draws more people beyond
them from C2's function; A's crowd, whose state lives in one client,
could hand a person to `op_sim` only through a new recorded input. A
person who crosses the boundary is handed over:
- **into `op_sim`:** `op_sim` starts them where C2's function puts
  them, so only the client's own offsets (reactions, avoidance) have to
  fade;
- **back to the client:** a person who was blocked or reacted in
  `op_sim` is no longer where the function puts them. Options:
  `op_sim` keeps them until their trip ends at a door or stop, so its
  population grows; it removes them out of view; or the function
  absorbs a bounded delay.

- **Changes:** all of B's contracts, for a small population, and the
  hand-over rules. The function's version joins `SIM_CONTRACT_VERSION`,
  since the simulation's state then depends on it, and its randomness
  falls under B's amendment to rule 3.
- **Tests and CI:** B's, and a test that a person handed to `op_sim`
  starts where the function puts them.
- **Cost** (prototype, a): 0.024 to 0.13 ms per tick for 100 to 500
  agents; in the worst rollback frame, 0.23 to 1.16 ms at the median
  (1.4 to 6.9%) and 0.41 to 1.94 ms at the 99th percentile (2.4 to
  11.6%).
- **Multiplayer:** the near people are the same for everyone; the
  ambient ones as in C2.
- **Effort:** the largest: B's documents and kernel work, and two
  crowds.
- Pros: the pattern of the games above (Assassin's Creed Unity's 40
  real AIs in a crowd of 10,000, Legion's uprezzed citizens), with For
  Honor's hundred soldiers in a deterministic simulation with rollback
  as the nearest precedent; deterministic agents only where they
  matter, cheap people elsewhere.
- Cons: two systems, and a hand-back that needs its own rule; tuning
  the function then bumps `SIM_CONTRACT_VERSION` too; all of B's
  contracts in M2 regardless.

#### C2. People in the client from a shared seed (recommended)

The client draws people from a population that is a pure function of
the world (its lanes, places and population profiles), the session
seed, the crowd's clock, the population's version and the
character-asset catalogue ([What the crowd must
do](#what-the-crowd-must-do)). Who walks which lane, who sits where,
when, how fast, in which group and in which clothes follows from those
inputs alone. Each machine works out only the people near its own
camera. In the client only, people near the camera avoid each other,
give way to players and glance, as offsets that fade back to the
function's path and never reach the simulation; the player walks
through anyone they miss.

- **Changes:**
  - **the population function,** outside the simulation's `Schedule`,
    state and hash: it keeps no state and is never stepped or hashed,
    so `SIM_CONTRACT_VERSION` stays 0.2.0. It works in integer
    millimetres and ticks, with a counter-based hash of the session
    seed for its randomness, so every platform gives the same people.
    That randomness never touches the simulation's RNG or state, so
    ADR-0003's rule 3 is not changed in M2; if `op_sim` later places
    agents with it (C1), it needs B's amendment. The function has a
    version of its own, which screenshots and benchmark reports record.
    Where it lives: [below](#where-the-code-lives);
  - **world format:** the navigation layer, its places and its
    population profiles (the navigation-data RFC), not simulation
    input;
  - **gate:** that layer's rules and a lane check (the navigation-data
    RFC; RFC 0002, a new gate version);
  - **`op_client`:** drawing, animation, levels of detail, offsets and
    fades; people hidden in F2's plain look, as other decoration is
    ([ADR-0012](0012-client-realism-from-procedural-detail.md)); an
    option to turn them off;
  - **`tools/ingest`:** as in A, and the places and profiles;
  - the simulation's state, the replay format, G9 and G10 do not
    change.
- **Tests and CI:** a golden test, in which a fixture world, seed,
  clock and tick give pinned bytes, run in debug and release on Linux,
  Windows and macOS, as the Rapier canary is; working out the same
  place twice, or places in another order, gives the same people;
  client tests, the benchmark route and the prototype's checks ([What
  the crowd must do](#what-the-crowd-must-do)).
- **Cost:** nothing in the simulation. On the client, in the main
  update: working out each person near the camera, animation,
  transforms and draws, not measured.
- **Multiplayer:** every machine derives the same people from the same
  inputs, with no network traffic and nothing to roll back, provided
  the session agrees on the crowd's clock, the population's version and
  the character-asset catalogue, for example by a hash at session
  start. State hashes do not cover the crowd, so only that hash would
  catch a mismatch. Offsets differ a little between players: people
  give way to every avatar, but each client works out its own.
- **Effort:** medium, with an unproven core: A's work, the function
  with its golden tests, and the requirements below.
- Pros:
  - the same people in screenshots and at every benchmark stop, on
    every machine and along any camera path, at a pinned crowd tick;
    later, for every player;
  - turn round and the same people are there: the camera never decides
    who exists;
  - no change to the simulation's state, the replay format or G9 and
    G10; tuning bumps only the population's own version;
  - no machine works out people that nobody near it can see, from one
    district to the planet;
  - the function can later place a near population in `op_sim` (C1),
    so the hand-over into `op_sim` does not show.
- Cons:
  - unproven: no game found documents a crowd that keeps no state. The
    closest is Assassin's Creed Unity's crowd beyond 40 m, which never
    leaves its path; C2 uses that kind of crowd down to the player's
    feet, with no near tier that has state of its own;
  - people avoid each other only by construction or by client offsets,
    both unproven;
  - people appear and leave somewhere: at entrances, stops, squares and
    the world's edge. In M2, doors are a client texture, OSM's
    entrances are not queried yet, and stops have no trams until M4;
  - people never block the player, and M2's visible character walks
    through anyone it misses;
  - a replay shows its recorded crowd only when played back at the same
    date and time, which it does not record (open question 1).

### D. Not in M2

No pedestrians until M4 (interactions) or M5 (other players); this
ADR's evidence goes to those decisions. No crate, contract or test
changes now, nothing costs anything and no effort is spent until then;
the networking ADR decides the multiplayer side.

- Pros: M2 stays on water, trees, lights, sound and the visible
  character; the choice is made when gameplay and networking are known;
  RFC 0004 can go ahead as drafted.
- Cons: the district stays empty, although "People on the pavements"
  is the first thing the roadmap's section 1 lists under "It feels
  alive"; the navigation data waits too.

### E. Pedestrians as Rapier bodies (rejected by measurement)

Each pedestrian as the player is: a capsule moved by `op_sim`'s
character controller. The prototype's variant c rules it out: 34 to 47
µs per agent and tick, about 150 times a graph agent's tick (145 to 180
times at the same number of agents); 3.6 to 3.9 ms a tick for 100 of
them; and, under rollback, 31.6 ms a frame at 100, with reproducible
mismatches while the capsules sit in Rapier's world. Kept out of that
world they roll back exactly, but still cost 30.4 to 30.9 ms a frame at
100.

### How the options compare

| | A | B | C1 | C2 | D |
|---|---|---|---|---|---|
| People in `op_sim` | none | all near players | those within tens of metres of a player | none | none |
| `SIM_CONTRACT_VERSION` and state hash | unchanged | bumped; agents hashed every tick | as B, and the function's version joins it | unchanged | unchanged |
| Replay header | unchanged | date and time as inputs | as B | unchanged (open question 1) | unchanged |
| Navigation data | world layer, for the client | simulation input | simulation input | world layer, ready for the simulation | waits |
| Gate | the layer's rules and a lane check | as A, and G10 among people | as B | as A | unchanged |
| Trunks and poles in M2 | visual | obstacles | obstacles | visual | visual |
| Worst rollback frame, median (prototype, a) | none | 1.15 to 5.2 ms for 500 to 2,000 agents | 0.23 to 1.16 ms for 100 to 500 | none | none |
| Same people at every screenshot and benchmark stop | on one machine, if seeded and stepped on ticks | yes, at a pinned tick | the near ones, at a pinned tick | yes, on every machine, at a pinned tick | no people |
| Same people for every player (M5) | no | yes | the near ones | yes, if sessions share the crowd's inputs; offsets apart | no people |
| People avoid each other | any model | separation | the near ones in `op_sim` | by construction or client offsets, unproven | no people |
| Reactions to players | any, with state, each player's own | simulated, 150 ms late with a perception delay | the near ones simulated | client offsets that fade | no people |
| Where people appear | anywhere out of view | where the simulation's rule puts them, without the camera | as B, near players | places in the world data; the camera only delays drawing | no people |
| Turn round, same people | only if the client keeps them | yes | yes | yes | no people |
| Seated and standing people | possible | possible, as simulation state | possible | possible, with no trips to hide | no people |
| People can block the player | no | yes, with an ADR-0004 change | the near ones | no | no people |
| Effort | small | the largest | the largest | medium, with an unproven core | none now |

## Decision

*Accepted on 2026-11-25 by @victorpicaud:* the options marked
recommended below. C2 in M2: people drawn by the client from a shared
seed, outside the simulation; a seeded A is the fallback if the client
prototype fails its checks; C1 is left to M4 and M5. The open questions
at the end stay open.

**Recommended: C2 in M2.** The client draws people from a shared seed.
They stay out of the simulation (out of `op_sim`'s `Schedule`, state
and hash), out of replays and out of the gate, and do not collide.
Whether people who block or react enter `op_sim`, as C1, is left to M4
and M5.

**Why C2 rather than B, C1 or D:**

1. **M2 needs people to look at, not to play with.** Interactions
   arrive with M4's RFC, and other players with M5. By the roadmap's
   own rule (section 5), what only changes the look stays in the
   client.
2. **Tuning stays cheap.** Crowds are tuned by trying. In `op_sim`
   every try bumps `SIM_CONTRACT_VERSION` and makes every earlier
   replay unreadable; in C2 it bumps only the population's own version.
3. **RFC 0004 can go ahead.** Trunks and poles stay visual in M2.

These three hold for A as well.

**Why C2 rather than A:**

4. **The same people on every machine.** At a pinned crowd tick, the
   exit screenshots and every benchmark stop show the same people on
   any machine and along any camera path. A, seeded and stepped on
   ticks, gives the same on the reference machine, which is all
   ADR-0018 compares, so in M2 this edge is small.
5. **One crowd for every player in M5, without interest management.**
   No machine works out people that nobody near it can see, and players
   share one crowd with no traffic and nothing to roll back, if their
   sessions agree on the crowd's inputs.
6. **The road to `op_sim` stays open.** If gameplay later needs people
   who block or react, a near population of graph agents is affordable
   (0.024 to 0.13 ms a tick for 100 to 500 agents, measured), and C2's
   function places it, so the hand-over into `op_sim` does not show.

**So:** C2's larger benefits arrive in M4 and M5; its risk, an
unproven look, arrives in M2. The maintainer weighs C2 now against a
seeded A now, then C2 or C1 when M5's networking ADR is drafted.

**Games** tier their crowds, with full AI near the player (Assassin's
Creed Unity's 40 within 12 m) and cheap people farther away. C2 is
their cheapest tier used everywhere, with no near tier that has state
of its own in M2. That is part of its risk, not a reason for it; C1
can add the near tier later.

**The main risk:** a crowd with no state of its own has no published
precedent. A client prototype comes first and is judged by checks set
before it runs (Consequences); a seeded A is the fallback, which
changes no simulation or replay contract either.

### What the crowd must do

Requirements for the client's design, from the evidence above:
- **Inputs:** the world's lanes, places and population profiles, the
  session seed, the crowd's clock, the population's version and the
  character-asset catalogue. Nothing else, and nothing from the camera,
  decides who is where.
- **The crowd's clock:** a full date and time in Swiss civil time, with
  the year, since the year fixes the day type, plus the crowd's ticks at
  60 Hz. Options for those ticks: the simulation's, which stop during
  stalls and restart at 0 with each new session (travel and R, behind a
  cut, ADR-0013); or a count the client starts when loading ends.
  Recommended: the simulation's ticks; the benchmark mode instead
  counts from the end of loading, and screenshots draw the crowd at a
  whole tick, without the frame's interpolation, so loading and frame
  timing never change the people. Moving the clock (T, 30 minutes)
  cross-fades the crowd.
- **Lanes only:** people walk on lanes clear of building shells, never
  through a wall, except where a lane ends at an entrance or passes
  through a building passage, which the matching keeps.
- **Never appear or vanish in view:** trips start and end only at
  places in the world data: entrances of shops and public buildings
  (brought into the navigation layer and drawn as doors there), stops,
  squares and the world's edge. The area the client works out ends in
  a distance fade, as OpenMW fades actors in and out at the edge of its
  processing range
  ([OpenMW](https://openmw.readthedocs.io/en/latest/reference/modding/settings/game.html)),
  never in a trip's end. The camera may delay drawing, never move
  anyone: a person whose trip starts in view is drawn once they have
  been out of view or beyond the fade distance. Until trams run (M4),
  trips at stops start and end out of sight, or stops wait.
- **Like real walkers:** more than half of the people in groups of 2 to
  4, abreast at low density; speeds by trip purpose, group size and
  lane kind (slower for shopping and leisure and in larger groups,
  about half on steps, with stair animations), around 1.34 m/s overall.
- **Through the day and the year:** more or fewer people by hour, day
  type and month. The profiles carry a month or season factor per kind
  of place (shopping street, river front, square), from the
  Bahnhofstrasse and Limmatquai series; clothes follow the season; the
  crowd's part of M2's ambient sound follows the same profiles.
- **Apart from each other:** people do not walk through each other in
  view. Options: client offsets near the camera that fade back to the
  function's path, as reactions do (a client-only near tier, as
  Assassin's Creed Unity's full AI works within 12 m); or avoidance
  built into the function (sub-lanes by direction, one speed per
  sub-lane or overtaking worked out in closed form, group formations by
  lane width). The prototype tries both and counts overlaps.
- **Sitting and standing:** people sit and stand where the data has
  places for it: benches, café terraces (OSM's `outdoor_seating`),
  stops, viewpoints and fountains, with occupancy by hour, day type and
  season from the same profiles, set by eye. No trip, so nothing
  appears in view.
- **Variety where eyes go:** heads, upper bodies and colours, and
  animation that does not move in step.
- **Reactions in the client only:** people give way to every player's
  avatar, which every client knows from the simulation, and glance, as
  offsets that fade out and never reach the simulation.
- **A radius and levels of detail:** people are worked out and drawn
  within a set radius inside the client's 600 m, with full skeletons
  near the camera and cheaper animation farther away, towards a density
  per stop and time that the profiles give.
- **A budget:** animation and drawing within a fixed share of the
  frame, as Unreal's animation budget allocator works (1.0 ms by
  default,
  [Epic](https://dev.epicgames.com/documentation/unreal-engine/animation-budget-allocator-in-unreal-engine)),
  measured on ADR-0018's route, CPU time in the main update included.
- **Decoration** (ADR-0012): hidden in F2's plain look; never saved,
  merged or checked by the gate.

**Checks for the prototype,** pass or fail over each stop's 20-second
turn, with thresholds set before it runs:
- nobody appears or vanishes in view inside the fade distance;
- overlaps between people in view stay under a set number a minute;
- nobody stands inside a building shell;
- the number of people stays within the profile's band;
- the crowd stays within its share of the frame.

### Where the code lives

- **`op_sim`, outside the `Schedule` (recommended):** clippy's
  determinism bans and the determinism job's tests on three platforms
  already apply, and `op_sim` can call the function later (C1) without
  a new dependency, since `op_sim` depends only on `op_kernel`
  (CLAUDE.md, ADR-0002). The cost: code that only changes the look sits
  in the simulation crate, a departure from the roadmap's section 5,
  named here.
- **A crate of its own** (for example `op_people`, depending on
  `op_kernel`): the same bans, since the root `clippy.toml` covers
  every crate but `op_client`, once it joins the determinism job, and
  the simulation crate keeps only the simulation. It needs an edge in
  ADR-0002's dependency policy now, and for C1 later either `op_sim`
  depending on it, which changes CLAUDE.md's rule, or the function
  moving into `op_sim`.
- **`op_client`:** nothing enforces the bans there, so machines could
  disagree.

In each, the function takes lanes as a plain type defined beside it,
which the client fills from the navigation layer, so no crate gains a
dependency on `op_world`. An `op_kernel` navigation type is a kernel
interface, which needs an RFC; it waits for the RFC that streams lanes
into the `Schedule` (B or C1).

### What this means for other decisions

- **RFC 0004:** trunks and poles are not obstacles in M2, so it can be
  decided as drafted, with collider `none`. People keep clear of them
  when drawn, by client offsets that fade, as reactions do; lanes routed
  round them by the ingest would let non-OSM points
  ([ADR-0015](0015-zurich-trees-and-lights.md)) shape OSM lanes (open
  question 6). If a later ADR puts people into `op_sim`, its section 7
  ("Solid trunks and poles later") applies then.
- **The navigation-data RFC** drafts a world layer for walking, read by
  the client in M2 and shaped to become simulation input later without
  a format change:
  - lanes in integer millimetres with their kind (sidewalk, footway,
    crossing, steps, pedestrian or living street), and walkable
    squares;
  - places where trips start and end, or people sit and stand:
    entrances of shops and public buildings, stops, squares, benches
    and terraces;
  - population profiles: an hourly shape per day type, a month or
    season factor per kind of place, an intensity per lane and an
    occupancy per place, marked as estimated where a rule set them;
  - lanes matched to building shells by the ingest, keeping lanes that
    end at entrances or pass through building passages, and a G9-style
    check that a change does not newly block a lane, which changes the
    gate (RFC 0002, a new gate version);
  - provenance and licence per asset, with the open licence questions
    settled first (open question 6); closed to contributions while
    under the ODbL, with fixes made in OSM (G3);
  - whether the profiles live in the navigation assets or in their own.
- **ADR-0018:** people are client work in Bevy's main update. The route
  measures CPU frame time with and without people, and the tier may
  need a CPU once people are drawn, so ADR-0018's open item on a CPU
  stays open. A stop shows the same people on every run only if its
  route entry gives a full date or a day type (21 June is a Sunday in
  2026 and a Monday in 2027) and the crowd's tick is pinned (above);
  then the 10% regression rule compares like with like. The counts'
  busiest case is a Saturday in December at 16:00 or 17:00, well above
  the yearly Saturday peak. Character variety costs video memory more
  than frame time
  ([NVIDIA on GTA V](https://www.nvidia.com/en-gb/geforce/news/grand-theft-auto-v-pc-graphics-and-performance-guide/)),
  which matters at 8 GB.
- **M4 and M5:** the interaction RFC and the networking ADR decide
  whether people who block or react enter `op_sim` (C1). This ADR's
  evidence for that choice:
  - graph agents with separation, never Rapier bodies. The prototype's
    crowds were sparse and overlaps were not counted, so a near
    population at street density (for example 500 agents within
    160 m, 0.03 to 0.1 people per m²) is measured first, counting
    overlaps for a and b;
  - a perception delay of at least the prediction window plus one tick
    (9 ticks with GGRS's defaults), so the crowd never re-runs; people
    then react to players 150 ms late;
  - rule 9 hashes every re-simulated tick, crowd included: 0.57 to
    0.64 ms for 2,000 agents with a buffered hasher, 3.6 to 3.9 ms with
    today's, even with the delay. Either those costs, or an amendment:
    hash only the ticks that a peer or a replay compares, or each
    module separately under a new encoding;
  - a buffered hasher, and randomness per agent from (seed, agent,
    tick);
  - players blocked by people: an ADR-0004 amendment with a kernel way
    for one module to read another's state, or colliders that only the
    player's queries see, at E's cost and with its rollback mismatches;
  - the hand-over: C2's function places people as they enter `op_sim`;
    the way back needs a rule (C1), and the function's version then
    joins `SIM_CONTRACT_VERSION`;
  - For Honor's talks on its deterministic battlefield (GDC 2017 and
    2019, above), as reading for the networking ADR;
  - Rapier's own state under rollback: the prototype's capsules broke
    exact rollback through its broad phase, and the player's collider
    set changing inside a rollback window may do the same. Restoring
    Rapier's world as well was not tried;
  - the prototype's numbers come from one shared machine; it is kept
    for runs on others (Consequences).

### Data

| What | Source | Licence | Status |
|---|---|---|---|
| Lanes: sidewalks, footways, crossings, steps, pedestrian and living streets, squares | OpenStreetMap, on swissALTI3D heights and kept clear of swissBUILDINGS3D shells | `ODbL-1.0 AND LicenseRef-swisstopo-OGD` if keeping clear of shells counts as placing under RFC 0001; open (open question 6) | ways in the cache; crossing nodes, kerbs, entrances, `highway` relations and `area:highway` need new queries |
| Places: entrances, stops, benches, terraces, shops and places to eat, with opening hours | OpenStreetMap | `ODbL-1.0` | a query for nodes; entrances, stops and benches first |
| Checks, never copied | the city's walking network and surveyed sidewalks | `CC0-1.0` | quality reports; no allowlist entry needed (ADR-0017) |
| Hourly shapes by day type, and month factors | Bahnhofstrasse counts (hystreet's pilot); the Limmatquai counter, 2018 to 2024; VBZ boardings by stop and hour | "Creative Commons Attribution", no version; `CC0-1.0`; `CC0-1.0` | hystreet's version first (open question 2); the CC0 sources fit ADR-0017's list |
| Intensity per lane and occupancy per place | estimated from lane kinds, places and stops, calibrated at the counted places | open: OSM-derived places mixed with counts (open question 6) | the navigation-data RFC |
| Character models and animations | client assets such as MakeHuman's (CC0) and the 100STYLE motion data (CC BY 4.0), both on ADR-0017's list | `CC0-1.0`, `CC-BY-4.0` | manifests and downloads (roadmap, M2); Mixamo, and Quaternius's own licence, are on its forbidden list |

Every source gets its ATTRIBUTION.md entry before its first byte enters
the world (roadmap, section 5).

**Where there is no data:**
- **No sidewalks mapped:** lanes by road class and frontage, marked as
  estimated. In the district, sidewalks on both sides of residential
  streets would be right on 83% of their sides, and on 88% where
  buildings line them; service roads get none, which is right on 67%.
  Pedestrian streets and shared zones are walkable across their width,
  and crossings go at junctions (the notes, against the city's survey).
- **No counts:** Zurich's hourly shapes and month factors, scaled by
  nearby destinations, stops and population, and marked as estimated;
  calibrated wherever counts with a cleared licence exist. Population
  grids such as GHS-POP (CC BY 4.0) need ADR-0017's review first.
- **No lane at all:** no people.
- **Always** baked into the world by the ingest and pinned by the world
  id. Nothing comes from live services.

### Privacy

- **Fictional people,** a rule this ADR proposes (today's rule covers
  interiors and furniture only): looks come from seeds and from
  character assets with manifests. Motion capture under an allowed
  licence, such as 100STYLE, may drive them; a scan, photograph or
  other likeness of a real person's face or body may not.
  Character-asset manifests state this in a field that ADR-0017's
  client-asset check reads; the gate never checks decoration, so that
  check is the rule's only enforcement.
- **No homes:** trips start and end at public places (stops, squares,
  entrances of shops and public buildings, the world's edge), never at
  a residential address, and no person lasts from one trip to the next.
- **Aggregates only:** hourly or quarter-hourly totals. Never individual
  tracks (GPS traces, including OSM's public traces, cameras, Wi-Fi or
  Bluetooth), and never accident records, which news reports can tie
  to people.
- **OSM without user data:** no user or changeset fields; never
  `out meta`.

## Consequences

- **Simulation:** unchanged in M2. `SIM_CONTRACT_VERSION` stays 0.2.0;
  the simulation's state, the replay format, the state hash, G9, G10
  and the sample replay's checkpoints do not change.
- **Contracts that do change,** for C2 and A alike: the world format
  gets the navigation layer with its places and profiles (the
  navigation-data RFC), and the gate gets that layer's rules and a lane
  check (RFC 0002, a new gate version). The population function has a
  version of its own. No `op_kernel` interface changes in M2.
- **World:** the navigation layer gives worlds new ids, so replays are
  recorded again with only their headers changed, as for water
  ([ADR-0014](0014-water-areas-from-openstreetmap.md)).
- **Order of work in M2,** if C2 is chosen:
  1. a throwaway client prototype of C2 on the pedestrian prototype's
     graph, at the benchmark route's three places, judged by the checks
     above;
  2. the navigation-data RFC, shaped by what the prototype found, with
     the ingest's new queries and lanes matched to shells;
  3. character models and animations under ADR-0017's client-asset
     licences, with manifests;
  4. the route measured with and without people.
- **The pedestrian prototype kept:** before M4 or M5 relies on its
  numbers, its source, configurations and results are kept where others
  can run them again (a branch, or a folder with a README), without the
  OSM-derived graph, which its extractor rebuilds from the ingest
  cache. Until then, those decisions have only the numbers recorded
  here.
- **Fallback:** if the client prototype fails its checks, A, seeded,
  changes no simulation or replay contract either; only the same people
  across machines and camera paths is lost.
- **Licences:** hystreet's version is settled before its counts shape
  anything, and the lane and profile licences before the ingest builds
  lanes (open questions 2 and 6); ATTRIBUTION.md gets an entry for
  every count source used, and the client credits its character assets.
- **Privacy:** if the rule for fictional people is accepted, README.md,
  CONTRIBUTING.md and CLAUDE.md's "Private by design" name people too,
  and character-asset manifests get the field.
- **What gets harder:** players and people never touch in the
  simulation, and people in `op_sim` later need C1's hand-over and
  hand-back.
- **Revisit:**
  - when M4's interaction RFC wants people in gameplay (blocking,
    missions, reactions that matter);
  - when M5's networking ADR is drafted;
  - if C2's prototype fails;
  - if the route shows that people do not fit the budget.

## Open questions

1. **Replays and the date and time.** Under C2, a replay played back
   shows the crowd for the playback's clock, as the client already does
   for the sun. To show the recorded crowd and sun, the header could
   carry the date and time, and the population's version, as
   presentation fields, not inputs. That changes the replay contract
   (an RFC). Recommended: leave it until bug reports need it. M5's
   sessions carry the same data anyway (C2, Multiplayer).
2. **hystreet's licence.** The city's portal says "Creative Commons
   Attribution" with no version, and links that label to the Open
   Definition's CC BY page, whose main link is version 4.0
   ([Open Definition](https://opendefinition.org/licenses/cc-by/)).
   opendata.swiss records its own "terms_by" terms (the notes). The
   counts come from a pilot of private and public partners, so who
   holds the rights matters too. Confirm the version and the city's
   right to license with the city, or add a `LicenseRef-` entry with
   the stored text, under ADR-0017. Until then the CC0 counts give a
   first profile.
3. **Day types.** The counts separate weekdays, Saturdays and Sundays;
   holidays need a calendar per place, which the navigation-data RFC
   could carry as data.
4. **Signed zeros in ADR-0003.** `min` and `max` are listed as safe but
   do not fix which zero they return. StreetWide's own simulation code
   never gives them two zeros today: its only use limits the player's
   fall speed to −50 m/s. New code could, and Rapier and parry already
   call `max` and `clamp` on values that can be −0.0 (Rapier 0.36.1's
   character controller, parry 0.31.1's segment distances). A `dmath`
   helper or a clippy rule would close it for StreetWide's own code,
   whatever this ADR decides; dependency code stays covered only by the
   cross-platform replay check and the Rapier canary.
5. **Stopping, sitting, shopping.** The notes found no peer-reviewed
   rates for how often passers-by stop, sit or enter shops. A short
   count at three named places, recording totals only, would give
   starting values. It is optional: until then, occupancy is set by
   eye, as games set it.
6. **Lane and profile licences.** RFC 0001 lets an OSM-derived asset
   use another source "only to place its OSM geometry", for example
   terrain heights to drape a road
   ([RFC 0001](../../rfcs/0001-world-format.md)). Moving lanes sideways
   to clear swissBUILDINGS3D shells goes further. Shells are also open
   to contributions under CC BY-SA 4.0
   ([RFC 0002](../../rfcs/0002-contributions-and-merge-rules.md)):
   re-matching an ODbL lane to a contributed shell would combine ODbL
   with CC BY-SA geometry, which ADR-0001 and G4 forbid, while not
   re-matching it lets the lane check judge a contributed shell, moved
   within G8's 1 m, against a lane contributors cannot edit, as G9
   already does with ribbons. Options: amend RFC 0001 section 2 so
   that clearance counts as placing, and settle the contributed-shell
   case there; keep lanes pure OSM and keep people off shells only when
   drawing them (a Produced Work); or keep clearance offsets in an
   asset of their own, outside the ODbL. Intensities and occupancies
   that mix OSM-derived places with CC BY and CC0 counts are ADR-0019's
   question, since using OSM to select or correct other data counts as
   mixing. Recommended: settle both in the navigation-data RFC, with
   ADR-0019, before the ingest builds lanes.

**These stay open after the decision.** The maintainer, @victorpicaud,
decides each and records it here. Each is drafted with a
recommendation.
