# ADR-0018: Set a performance tier and a benchmark route

- **Status:** Proposed
- **Date:** 2026-11-24
- **Deciders:** @victorpicaud
- **Supersedes / superseded by:** none (sets the budget for ADR-0012's
  look, and would change its night exposure)

## Context

The [roadmap](../roadmap.md) promises "a steady frame rate on mainstream
hardware" and leaves the exact target to an ADR that opens M2; its
section 8 lists it among the decisions waiting. One of M2's exit
criteria is that "the benchmark route meets the performance-tier
budget". The tier, the budget and the route are not defined yet.

M2 also brings the most expensive things the client has drawn so far:
trees and street lights ([RFC 0004](../../rfcs/0004-point-features.md),
[ADR-0015](0015-zurich-trees-and-lights.md)). An independent review of
their design asks for this ADR, and for a measurement of today's client
at 1080p, before that client work merges.

Facts gathered on 2026-11-24 from the repository, and from two working
notes of the same day that are not in the repository: a design for M2's
trees and lights ("the design") and an independent review of it ("the
review"). The figures this ADR uses from them are given here.

**Players' hardware**
([Steam Hardware Survey](https://store.steampowered.com/hwsurvey/),
October 2026, all platforms):

| | Most common | Next |
|---|---|---|
| Primary display | 1920 × 1080: 50.23% | 2560 × 1440: 21.82% |
| Video memory | 8 GB: 26.92% | 16 GB: 26.35%; 12 GB: 12.91% |
| System memory | 16 GB: 41.42% | 32 GB: 37.06% |

It is a self-selected sample of Steam users and changes every month.
The figures gathered give no shares or speeds for GPU models, and
nothing on CPUs.

**The only measurement:** 169 frames per second (5.9 ms a frame) at
1280 × 720 on an RTX 3070, an 8 GB card, on the high setting, at
Hirschenplatz with the district loaded
([ADR-0012](0012-client-realism-from-procedural-detail.md)). Its limits:
- one view, one time, before water;
- 720p is the window's default size, which no command-line option
  changes. 1080p has 2.25 times the pixels; the review estimates
  75–105 fps there on the same card, unmeasured;
- the F3 panel shows a running average, which hides single slow frames;
- the client presents with vsync (Bevy's default `Fifo` mode, which the
  client keeps), so 169 fps may be the display's refresh rate rather
  than the GPU's limit. The record does not say which.

**What the client draws**
([ADR-0008](0008-client-architecture-and-streaming.md), ADR-0012):
everything within 600 m of the camera, and nothing beyond. The district,
about 1.08 million triangles in 30 tiles, fits in one radius, so from
its middle almost all of it is drawn. Both presets have a physically
based atmosphere, bloom, ACES tonemapping and four sun shadow cascades
reaching 450 m. F4 switches between them:

| | High (default) | Fast |
|---|---|---|
| Anti-aliasing | temporal (TAA) | MSAA 4× |
| Ambient occlusion | on | off |
| Shadow maps | 4096², temporal filter | 2048², Gaussian filter |

On high, geometry is drawn in a depth prepass (TAA and ambient occlusion
need it), in the main pass and in the cascades. Sun shadows switch off
once the sun is 3° below the horizon, so tree shadows and lamp lights
would overlap only briefly at dusk.

**What M2 adds** (from the design; its costs are estimates):
- **Trees:** about 1,000 in the district, 70 of them at Lindenhof. Their
  leaves are alpha-tested cards, so on high each is drawn in the
  prepass, the main pass and up to four cascades.
- **Lights:** about 2,000 light points in the district. Bevy 0.20.1
  clusters lights on the GPU with no cap on their number, but a pixel
  costs more the more lights reach it. Only the nearest lamps get real
  lights (384 within 250 m in the design, 32–64 in the review); glowing
  lamp heads stand in for the rest.
- **Cost:** about 1–2 ms more GPU time, under 0.1 GB more video memory
  and about 9,000 more entities, on an 8 GB card at 1080p.
- **Doubts:** the review expects this to fit an RTX 3070-class card only
  with levels of detail, limits on tree shadows and occlusion culling.
  It notes that some 8 GB cards, such as the RTX 3050 8 GB, run at about
  half an RTX 3070's speed and may miss 60 fps before any trees.

**Night exposure.** The client sets exposure from the sun's elevation
(`sky.rs`) and holds it at EV100 6.5 once the sun is 10° below the
horizon. A surface then needs about 19.5 cd/m² to look mid-grey, while
lamp-lit paving has 1–3 cd/m², so lit streets come out almost black:
- the design estimates linear values of 0.01–0.03 before tonemapping,
  against 0.18 for mid-grey;
- the review estimates about 9/255 after tonemapping for a street lit to
  8.8 lux by a 4,000 lm lamp 6 m up, and about 91/255 at EV100 3.

Yet M2's screenshots must show street lamps lit at 18:00 on 21 December,
with the sun about 13.2° below the horizon. Bevy also adds emissive
light without exposure unless a material asks for it, so glowing lamp
heads and windows would not match either.

**Testing.** CI has no GPU: it builds and tests the client on Linux,
Windows and macOS, but never draws a frame. Rendering never feeds the
simulation, which runs at a fixed 60 Hz whatever the frame rate
([ADR-0003](0003-determinism-strategy.md), ADR-0008).

**Gate budgets.** Check G7 of
[RFC 0002](../../rfcs/0002-contributions-and-merge-rules.md) allows up
to 1,000,000 rendered triangles per tile, as a starting value; the
whole district has about 1.08 million. G7 counts mesh assets only, so
trees and lamps that the client draws from points, as RFC 0004
proposes, would be outside it.

## Options considered

### Reference hardware

#### A. An 8 GB card of the RTX 3060 / RX 6600 class, at 1080p (recommended)

- Pros:
  - the most common display and video-memory size in the survey;
  - a speed as well as a memory size, which matters because some 8 GB
    cards are about half as fast as others (review);
  - one NVIDIA and one AMD card cover both main drivers.
- Cons:
  - slower than the maintainer's RTX 3070, so measuring needs a card of
    the class; the RTX 3060 comes with 12 GB or 8 GB, so the reference
    must name one card;
  - the figures gathered do not show how many players have a card at
    least this fast.

#### B. The maintainer's RTX 3070 (8 GB), at 1080p

- Pros: available now, at no cost; today's only measurement is on it.
- Cons: faster than the slower 8 GB cards, so a pass says little about
  them; one vendor's driver.

#### C. Two tiers, or another floor

For example the class of A on fast as a minimum and the class of B on
high as recommended; or a floor of 6 GB cards (5.20% in the survey),
integrated graphics or handhelds; or 1440p, with 1.78 times the pixels
of 1080p.

- Pros: tells more players what to expect.
- Cons: more cards and budgets to keep; nothing gathered on the speed
  of the lower devices.

### Preset and budget

#### A. 99th percentile frame time at or under 16.7 ms, on high (recommended)

16.7 ms is 60 frames per second. The 99th percentile looks at the
slowest frames, so stutter counts.

- Pros: the tier promises the look of ADR-0012's screenshots; it catches
  hitches that an average hides.
- Cons: it is strict. If today's client already misses it on the
  reference class, the high preset must get lighter before trees
  arrive. It needs a tool that records every frame.

#### B. A lower bar: an average of 60 frames per second, or 30 (33.3 ms)

- Pros: easier to meet with the full look; an average is close to what
  F3 shows.
- Cons: a route can pass on average while it stutters; at 30 the picture
  moves half as often as the 60 Hz simulation steps.

#### C. The same budget on the fast preset only

- Pros: the most likely to pass.
- Cons: the tier would promise a look without TAA or ambient occlusion,
  with coarser shadows; leaves shimmer without TAA, so trees would need
  alpha-to-coverage there (review).

### Measuring tool

#### A. A benchmark mode in the client (recommended)

`op_client --benchmark <route>` would:
- open a 1920 × 1080 window with vsync off, on a given preset;
- go to each stop of a route file (a named place, a date and a time) as
  `--place` does: a new session, facing the place's heading
  ([ADR-0013](0013-travel-by-starting-new-sessions.md));
- wait until every mesh in range is loaded, then turn the camera once
  round at eye height over 20 seconds, camera only as in flight, and
  record every frame;
- take the stop's screenshot, facing the place's heading, with the ODbL
  notice on it ([ADR-0019](0019-odbl-and-the-published-world.md)). Today
  the client hides its help panel with `--screenshot`, and the credit
  line names OpenStreetMap but not the ODbL;
- write a report: per stop and preset, the median, 99th percentile and
  worst frame time, and GPU time per render pass where Bevy measures it
  (Vulkan and DX12, not Metal); then the commit, world id, GPU, driver,
  CPU, memory, system, resolution and preset.

- Pros: one command and the same views every time; M2's exit-criterion
  screenshots come from the same run; client code only.
- Cons: new client code to maintain, with options for the window size,
  vsync and the preset; it still needs a machine with a GPU.

#### B. The client's existing options

`--place`, `--date`, `--time` and `--details`, with `--screenshot`,
`--screenshot-tick` and `--exit-when-done`, already save a screenshot at
a place and time with the F3 frame rate on it.

- Pros: no new code; usable today.
- Cons: 1280 × 720 with vsync on, and no option for the preset; one
  smoothed number per screenshot, no percentiles.

Two other ways help but do not replace A. A walk played back with
`--replay` follows the same path every time, but names its world id, so
each rebuilt world needs a new recording; it could become a kind of stop
in A later. Profilers run by hand (GPU vendors' tools, RenderDoc) find
causes, but give no repeatable budget.

### Where and when it runs

#### A. On a reference machine at every checkpoint, results in progress.md (recommended)

- Pros: no new infrastructure or secrets; it fits how checkpoints
  already work. Contributors can still run the route on their own cards
  and post reports, as information.
- Cons: a regression shows only at the next checkpoint; it depends on
  one person and one machine.

#### B. In CI, on a GPU runner or rented cloud GPUs

- Pros: catches a regression in the change that causes it.
- Cons:
  - a runner for a public repository can run code from anyone's pull
    request, and a desktop in daily use gives noisy timings;
  - cloud GPUs are mostly data-centre cards, not the reference class;
  - someone must own the machine or the bill (roadmap, M1).

### Night exposure (a related decision for ADR-0012's look)

#### A. A lower night exposure, still set from the sun (recommended)

Exposure stays a function of the sun's elevation, with a lower floor
between EV100 3 and 5: the design suggests about 5, the review 3–4 once
the sun is 6° below the horizon. The value, and where the ramp from day
ends, are chosen by comparing screenshots, as the tonemapper was. Lamp
heads and lit windows give their brightness in cd/m² and follow the
exposure (Bevy's `emissive_exposure_weight` at 1).

- Pros: the same place and time always give the same exposure, so
  screenshots and benchmark stops are reproducible; no cost per frame.
- Cons: one exposure for the whole view, a dark lane as much as a lit
  square; it changes ADR-0012's dusk and night look, so those
  screenshots are taken again.

#### B. Bevy's automatic exposure

Bevy 0.20.1's `AutoExposure` meters a histogram of each frame and adapts
towards mid-grey at set speeds.

- Pros: it adapts between dark lanes and lit squares, as eyes do.
- Cons:
  - by default it brightens night towards mid-grey, so it needs a
    compensation curve and limits to keep night looking like night;
  - each image depends on what the camera saw before, so stops and
    screenshots depend on the way to them;
  - a compute pass every frame (small, not measured).

#### C. Keep EV100 6.5, as it is or with brighter lamps

- Pros: no change to the exposure.
- Cons: as it is, the December screenshot shows glowing lamp heads over
  black streets; lamps brighter than real ones give up ADR-0012's
  physically based light.

## Decision

*Proposed, not decided.* The options marked recommended are a proposal
to the maintainer (@victorpicaud), who decides and records the choice
here. Until then nothing in this ADR applies.

**Recommended tier** (hardware A, budget A):
- **Machine:** a desktop GPU of the RTX 3060 / RX 6600 class with 8 GB
  of video memory, 16 GB of system memory and a 1920 × 1080 display. No
  CPU is named, because the figures gathered have none; each result
  records it.
- **Budget:** at every stop of the route, on the high preset, the 99th
  percentile frame time is at or under 16.7 ms. The fast preset is
  measured and reported, with no budget in M2.
- **Reference card:** an AMD Radeon RX 6600 (8 GB). It comes only with
  8 GB, unlike the RTX 3060, and beside the maintainer's NVIDIA card it
  covers the second main driver. The card chosen, and any substitute
  limit, are recorded in this section when the maintainer decides, or
  later in a superseding ADR. Each result records its driver;
  progress.md holds results only.
- **Until the project has one,** the route runs on the RTX 3070, and its
  results are marked as faster than the tier.
- **M2's exit check:** the maintainer runs it on the reference card,
  bought or borrowed for the purpose, because the route runs at every
  checkpoint and needs a card the project can count on. Buying needs an
  owner for the cost (roadmap, M1). A contributor's run on a card of the
  class is welcome as a second result. A stricter limit on the RTX 3070
  is not recommended: setting it needs a run on a card of the class
  anyway.

**Recommended tool:** option A, with one route file per world in the
repository. Until it exists, option B gives the screenshots but no
budget result.

**Recommended route for zurich-altstadt:** the three named places the
design proposed:
- **Lindenhof:** 70 trees, the design's worst case for trees;
- **Hirschenplatz:** narrow old-town lanes, and today's only
  measurement;
- **Rathausbrücke:** the river, with water, bridges and long views.

Each at three times, on high and then on fast:

| Date and time | Sun | What it loads |
|---|---|---|
| 21 June, 18:00 | about 31.6° up | full sun, four cascades, trees in leaf |
| 21 June, 22:30 | about 8.7° down | trees in leaf and lamps lit |
| 21 December, 18:00 | about 13.2° down | lamps lit, deciduous trees bare |

The 18:00 stops give M2's exit-criterion screenshots. A dusk stop, when
sun shadows and the first lamps overlap, is added once lamps switch on
in the client.

**Recommended practice** (where and when A): the maintainer runs the
route on the reference machine at every checkpoint that changes the
client, the world or the pinned Bevy version
([ADR-0005](0005-toolchain-and-engine-pinning.md)), and adds a summary
table to [progress.md](../progress.md). Raw frame logs stay out of Git.
- **First run:** today's client at 1080p on both presets, before any
  tree or lamp code merges.
- **Regressions:** a stop more than 10% slower than at the previous
  checkpoint is explained in progress.md. 10% is a starting value, to be
  set once the spread between runs is known.

**Recommended night exposure** (option A): the floor and the end of the
ramp are chosen with screenshots of the three places at 22:30 on
21 June and 18:00 on 21 December, before the exit criterion "street
lamps lit in December" is claimed. ADR-0012 is still Proposed; if both
are accepted, this replaces its night exposure.

## Consequences

- **Simulation:** unchanged, whichever options are chosen. The benchmark
  mode, the presets and the exposure are client code. Stops arrive as
  new sessions, as travel does, so no replay format or contract changes.
- **Order of work in M2,** if the recommendation is accepted:
  1. the benchmark mode, and today's client measured at 1080p;
  2. if it already misses the budget on the reference class, the
     maintainer chooses between a lighter high preset, the fast preset
     as the tier's preset, or another tier, before trees and lamps
     merge;
  3. trees, lamps and lit windows measured on the route as they arrive.
- **What the trees and lights work should bring.** Recommended here,
  from the review, for the client ADR on trees, lamps and lit windows
  that RFC 0004 names, extending ADR-0012 and not yet drafted then (now
  [ADR-0022](0022-drawing-trees-lamps-and-windows.md)):
  - **trees:** levels of detail by distance; no shadows from distant
    levels or impostors; occlusion culling on the camera (TAA and
    ambient occlusion already give it the depth prepass it needs); a
    distance limit for tree shadows; on fast, no tree shadows and
    alpha-to-coverage for leaves;
  - **lamps:** short-range, shadowless lights aimed down for the nearest
    32–64 lamps, raised only while the night stops stay within budget;
    glowing heads beyond; one model and one light per position. The
    review asked for stacked points to be merged into one point with
    `count`; ADR-0015 recommends one point per record instead, so the
    client groups them when drawing;
  - **lit windows:** a client shader extension or a coarser effect per
    building, because today's materials have nothing per window. They
    stay seeded decoration: never from occupancy, energy or register
    data, with no interior detail, and off in F2.
- **Gate:** this ADR sets no gate numbers; merge criteria change by RFC.
  G7's per-tile triangle numbers are RFC 0002's starting values, and
  RFC 0004 proposes a starting budget of 5,000 points per tile. The
  route can give both evidence, for example a synthetic world with every
  tile within 600 m at its budget.
- **Comparisons:** until M1 publishes one canonical world,
  zurich-altstadt is a local build, and results compare only on the same
  world id.
- **Reflections:** the separate decision on reflecting buildings in water
  ([ADR-0014](0014-water-areas-from-openstreetmap.md)) would be measured
  on the same route.
- **CI:** still no GPU, so timings come from checkpoints, not pull
  requests. Whether CI could run the route on a software renderer, for
  crashes and screenshots but not timings, is untested.
- **Open:**
  - who buys or lends the reference card;
  - whether today's high preset meets the budget at 1080p on that class
    at all;
  - video memory: Bevy's diagnostics report mesh buffer sizes, not total
    use, so the total comes from outside tools for now;
  - if pedestrians enter `op_sim`, their cost per tick shares the main
    thread with rendering (ADR-0008), and the tier may then need a CPU.
- **Revisit:**
  - when 8 GB is no longer the most common video memory in the survey,
    or 1080p no longer about half of displays;
  - at M3, when levels of detail and the whole city extend the route;
  - when a GPU runner or cloud GPUs become affordable and someone owns
    the cost.
