# ADR-0022: Draw trees, street lamps and lit windows in the client

- **Status:** Proposed
- **Date:** 2026-11-25
- **Deciders:** @victorpicaud
- **Supersedes / superseded by:** none (extends ADR-0012)

## Context

M2's exit screenshots, at 18:00 on 21 June and 21 December at three
named places, must show trees, and street lamps lit in December
([roadmap](../roadmap.md), M2).
[RFC 0004](../../rfcs/0004-point-features.md) (Draft) stores trees and
light points as points, and says that how the client draws them "needs
its own ADR extending ADR-0012, before that client work".
[ADR-0018](0018-performance-tier.md) (Proposed) lists what that work
should bring, and the roadmap's section 8 lists this ADR among the
decisions waiting. This ADR drafts the options for four parts: trees,
street lamps, lit windows, and the night exposure and sky they are seen
under. Floodlit landmarks are a fifth, briefly.

**Constraints:**
- **The simulation never depends on rendering**
  ([ADR-0002](0002-crate-layout-and-dependency-direction.md)), and only
  the simulation must be deterministic
  ([ADR-0003](0003-determinism-strategy.md)). Everything here is client
  code. Trees and lamps are visual only in M2: RFC 0004 gives both layers
  `collider: none`, and [ADR-0021](0021-pedestrians.md) (Proposed)
  recommends people drawn by the client, so that "trunks and poles stay
  visual in M2".
- **Decoration is not data**
  ([ADR-0012](0012-client-realism-from-procedural-detail.md), Proposed).
  What the client invents must never pass for data. F2 shows the plain
  look, with only the world data's palette colours. Nothing drawn is
  saved, merged or checked by the gate. Textures are generated in code,
  with "no files and no new licences".
- **Licences** ([ADR-0017](0017-licence-allowlist.md), Proposed): client
  assets under CC0 1.0 or CC BY 4.0, no CC BY-SA, nothing
  non-commercial or no-derivatives; code under MIT OR Apache-2.0. Every
  source is credited in ATTRIBUTION.md ([ADR-0001](0001-licensing.md)).
- **The budget** (ADR-0018, Proposed): a 99th percentile frame time of
  at most 16.7 ms on the high preset, on an 8 GB card of the RX 6600
  class at 1080p, at Lindenhof, Hirschenplatz and Rathausbrücke, at 18:00
  on 21 June, 22:30 on 21 June and 18:00 on 21 December. It asks this
  ADR for levels of detail, no shadows from distant levels, occlusion
  culling and a distance limit for tree shadows; on fast, no tree
  shadows and alpha-to-coverage; shadowless lights aimed down for the
  nearest 32 to 64 lamps, glowing heads beyond, one model and one light
  per position; lit windows by a shader extension or a coarser effect.
- **The data** (RFC 0004; [ADR-0015](0015-zurich-trees-and-lights.md)
  and [ADR-0016](0016-swisssurface3d-heights.md), both Proposed):
  - **trees:** an anchor, genus, species, taxon, planted year, crown
    diameter (null at the register's 8 m default) and a measured height
    (null where the LiDAR rule failed). `leaf_cycle` and `leaf_type`
    exist in RFC 0004, but no Zurich field fills them, and RFC 0004 does
    not allow them to be estimated;
  - **lights:** an anchor and an estimated `yaw`. `count`,
    `light_height_mm`, `method`, `mount` and `purpose` are null for all
    1,959 district points, because ewz publishes none of them. One point
    per record; the client groups points at one position;
  - **no gap-fill:** no trees or lamps generated along OSM streets where
    they come from non-OSM data (ADR-0015).
- **Privacy:** interiors are fictional, never replicas of real homes
  (CLAUDE.md). RFC 0004 section 10: lit windows are "seeded, never
  derived from occupancy, energy or register data, with no interior
  detail, and off in F2".

**Evidence.** Gathered on 2026-11-25 in three research notes and a
prototype. The engine note reads Bevy 0.20.1's features and costs from
its source. The games note covers what games and renderers do, and how
Zurich is lit. The data note counts the district's trees and light
points in the City of Zurich's open data (CC0). The notes and the
measurements are outside the repository; the prototype's code is on
branch `drawing-prototype` (Consequences). Outside facts carry their
source. "Measured" means the prototype or a note measured it;
"estimate" means arithmetic on measured numbers.

### The district's trees and lamps

| | Value | Source |
|---|---|---|
| Trees | 1,005: 643 street and 362 park trees, of 44 genera; lime (*Tilia*) 28.6%, the top 10 genera 78.6% | data note, section 5 |
| Leaf habit | 948 deciduous (94.3%) and 57 evergreen, by the note's own genus table; every tree within 100 m of a stop is deciduous | data note, section 5 |
| When they are bare | median leaf drop on 24 October (horse chestnut) and 27 October (beech); leaves unfold between 12 and 27 April by species (MeteoSwiss, three Zurich stations, 2006 to 2026) | games note, M3 |
| Heights | median 10.8 m (5th to 95th percentile 5.3 to 20.2 m); about 112 trees without a measurement | data note, section 5 |
| Crowns | median 7 m where not the 8 m default; 183 at the default | data note, section 5 |
| Trees within 50 / 100 m | Lindenhof 56 / 92; Hirschenplatz 10 / 32; Rathausbrücke 0 / 7 | data note, section 7 |
| Light points | 1,959 records at 1,717 positions; 74 positions hold 316 records, up to 36 | data note, section 6 |
| Spacing | median distance between neighbouring light positions 10.0 m | games note, M1 |
| Light positions within 50 / 100 m | Lindenhof 14 / 68; Hirschenplatz 26 / 77; Rathausbrücke 19 / 83; the 64th nearest is 89 to 98 m away | data note, section 7 |
| Likely mounts | by an unvalidated geometric heuristic: 420 records within 1.5 m of a facade (wall brackets?), 82 in the middle of narrow lanes (hung from wires?), 34 off-centre in narrow lanes (bracket or pole), 217 over the Limmat, 140 inside footprints, the rest in open places and wide streets (poles?) | data note, section 6 |

### What games and renderers do

- **A lamp is split in two:** light cast on surfaces near the camera,
  and a cheap glow at any distance. GTA V draws far lights as instanced
  quads with a 32 x 32 texture
  ([Courrèges, 2015](https://www.adriancourreges.com/blog/2015/11/02/gta-v-graphics-study-part-2/),
  a frame analysis, not Rockstar's); X-Plane separates "billboard" and
  "spill" lights
  ([X-Plane](https://developer.x-plane.com/article/billboard-and-spill-lights-for-objs/));
  Unreal fades local lights out over a "Max Distance Fade Range ... so
  it doesn't pop"
  ([Epic](https://dev.epicgames.com/documentation/unreal-engine/point-lights-in-unreal-engine)).
- **Shadows from street lights are rationed.** Cyberpunk 2077's artists
  get "a fixed amount of shadow maps" per frame
  ([SIGGRAPH 2021](https://history.siggraph.org/wp-content/uploads/2022/06/2021-Talks-Dybisz_Shadows-Optimizations-in-Cyberpunk-2077.pdf)).
  Many shadowed lights need ray tracing: Unreal's MegaLights shades 941
  shadowed area lights in 5.51 ms on a PS5 at 1080p, with hardware ray
  tracing
  ([SIGGRAPH 2025](https://www.advances.realtimerendering.com/s2025/content/MegaLights_Stochastic_Direct_Lighting_2025.pdf)).
- **Night is exposure and contrast more than light count.** Far Cry 5
  found physically valued night scenes too contrasty, retargeted its
  automatic exposure through an artist curve, treated moonlight as a
  separate blue light and raised the moon's brightness
  ([SIGGRAPH 2018 notes](https://www.advances.realtimerendering.com/s2018/The%20Challenges%20of%20Rendering%20an%20Open%20World%20in%20Far%20Cry%205%20(With%20Notes).pdf)).
  Two 2025 photographs of the old town from Lindenhof, with the sun about
  7.5° below the horizon, were exposed at EV100 3.0 and 4.0. They show an
  overcast sky lit orange-brown by the city, lit upper windows and
  bright ground floors (games note, M5 and 6.5).
- **Trees:** levels of detail cross-faded by a dither (GTA V, per
  Courrèges); alpha-tested leaf cards (Crysis,
  [GPU Gems 3, ch. 16](https://developer.nvidia.com/gpugems/gpugems3/part-iii-rendering/chapter-16-vegetation-procedural-animation-and-shading-crysis));
  octahedral impostors far away
  ([an MIT implementation](https://github.com/wojtekpil/Godot-Octahedral-Impostors));
  bare trees in winter (Forza Horizon 4, press). The permissive tree
  generators are ez-tree (MIT) and its Bevy port; Blender's Sapling is
  GPL and SpeedTree proprietary, so both are out (games note, 4.1).
- **Windows:** interior mapping ray-casts fictional rooms behind windows
  in the pixel shader, and lights each room with a probability
  ([van Dongen, 2008](https://www.proun-game.com/Oogst3D/CODING/InteriorMapping/InteriorMapping.pdf)).
  Window light has been measured: a lit flat's window averages
  6 cd/m², shop windows and adverts 50 cd/m², and at most 50% of
  residential windows were lit
  ([CIE 2025](https://files.cie.co.at/x051_2025/P0796_ID298TomasNovak_OA_with_cover.pdf),
  a Czech housing estate). No Swiss data on windows lit by hour was
  found (games note, 5.3).
- **Zurich at night.** The city's
  [Plan Lumière](https://www.stadt-zuerich.ch/de/planen-und-bauen/stadtplanung/strategien-und-konzepte/plan-lumiere.html)
  lights landmarks, uses no ground-mounted lights and switches building
  lighting off at midnight; its open concept plan has no landmark points
  in the district (games note, M4). Swiss guidance gives 2700 to 3000 K
  for pedestrian areas
  ([SLG, 2025](https://www.slg.ch/wAssets/docs/06-lichtwissen/04-oeffentliche-beleuchtung/oeB-ratgeber2025-aussenbeleuchtung-ig-strassenlicht-d.pdf)).
  Lucerne's old town, a comparable town, asks for 5 to 11.25 lx on
  average and at least 1.0 to 1.5 lx in its lanes
  ([Lucerne, 2010](https://www.stadtluzern.ch/_docn/5350666/Allgemeinbeleuchtung.pdf)).

### What Bevy 0.20.1 offers

From the engine note, read from Bevy's source:
- **Many lights, priced per pixel.** Point and spot lights are clustered
  on the GPU with no cap on their number; each fragment evaluates every
  light of its cluster (3.1).
- **Shadowed lamps are expensive in this client.** A shadowed spot light
  takes the sun's shadow-map size: a 64 MiB layer and a 4096 x 4096 pass
  on high. A shadowed point light renders six 1024 x 1024 faces. Bevy's
  docs advise one or two shadowed lights (3.2).
- **Emissive and bloom are cheap.** Emissive is in cd/m² and follows
  the exposure when `emissive_exposure_weight` is 1; it lights nothing
  else. The camera already has bloom (3.3).
- **Levels of detail, but no impostors.** `VisibilityRange` cross-fades
  levels with a dither. Occlusion culling exists and needs the depth
  prepass. Bevy has no impostors (3.7).
- **Leaves.** Under TAA, `AlphaToCoverage` is an alpha test at 0.5; with
  MSAA, on fast, it spreads coverage. Alpha-tested leaves are drawn in
  the prepass, the main pass and every cascade they fall in (3.8).
- **Windows without a shader:** a facade material can read its emissive
  texture through a second set of texture coordinates (3.11).
- **The reference card:** an RX 6600 is about 1.65 times slower than the
  RTX 3070 in GPU-bound synthetic tests (1.33 to 1.93,
  [Notebookcheck](https://www.notebookcheck.net/Review-of-the-AMD-Radeon-RX-6600-Mid-Range-Desktop-GPU.577558.0.html),
  press). ADR-0018's 16.7 ms is therefore about 10.1 ms of GPU time on
  the RTX 3070 (16.7 / 1.65), or 8.8 ms at a cautious ratio of 1.9
  (estimate, section 6).
- **Today at night,** the client lights with the sun and an environment
  map from the atmosphere, which give almost no light, and has no
  ambient light (2.1, 3.5). Exposure falls from EV100 13.8 by day to a
  floor of 6.5 (`sky.rs`).

### The prototype

On branch `drawing-prototype` (commit 68b149f), about 3,650 lines in
`crates/op_client/src/proto/`, with small hooks in the client's
`main.rs`, `materials.rs`, `scene.rs` and `sky.rs`. It is rendering only
and not for merging as is. Without its options (`--proto-*`, `--bench`,
`--preset`), the client behaves as before. It draws:
- **trees** (`--proto-trees`): models generated in code at start-up for
  four genus groups (broad, small, columnar, conifer), three variants
  each, each with three levels of detail cross-faded by
  `VisibilityRange`, ending at 40, 120 and 600 m by default. Bark is
  tapered tubes, leaves alpha-tested cards, and evergreens keep needles.
  Each tree is scaled to its height and crown and kept out of the
  nearest facade. The two nearest levels cast sun shadows. A model's
  near level has 1,256 to 5,080 bark triangles and 168 to 600 leaf-card
  triangles; all models and their textures take about 110 ms at
  start-up;
- **leaves by date:** horse chestnut, and beech (whose dates hornbeam
  and oak share), leaf out and fall on MeteoSwiss's medians for Zurich;
  lime and maple leaf out on them too. Lime's leaf fall is set a week
  after its colouring, because MeteoSwiss records no leaf drop for limes
  (games note, M3), and maple's on beech's date. Every other genus,
  plane and cherry among them, keeps leaves from 20 April to 27 October.
  Those leaf falls and that default are assumptions; limes are 28.6% of
  the district's trees;
- **lamps** (`--proto-lamps <n>`): a model per mount (a lantern on a
  pole, on a wall bracket, or hung from a wire across the lane), with a
  glowing head at 3000 K on every lamp; and a pool of the n nearest
  lamps as real shadowless lights of 2,500 lm and 20 m range, spot
  lights aimed down or, with `--proto-lamp-point`, point lights, faded
  towards the pool's edge. Stacks are drawn once. Lamps more than 1 m
  inside a footprint are left out (47 positions), a choice of the
  prototype. Of the 201 positions over the river, 175 stand on the
  surface found within 5 m in the drawn terrain and transport meshes, a
  bridge deck or a quay; 19 keep their own ground, which is above the
  water; 7 with nothing within 5 m are left out;
- **lit windows** (`--proto-windows`): each facade and ground-floor
  style has one mask of 16 bays by 8 storeys, read through a second set
  of texture coordinates on facade parts. A cell of the mask is lit when
  a hash of the style and the cell falls below the hour's share. Each
  building and wall only shifts its style's mask by whole cells, so the
  pattern repeats every 16 bays and 8 storeys, and every wall of a style
  shows the same pattern, shifted. The shares come from hand-set curves:
  one for upper floors, the same every day, and one for ground floors per
  day type (weekday, Saturday, Sunday). Upper windows average 6 cd/m²,
  shop windows 25 cd/m² (half of CIE's average), other ground floors
  10 cd/m²; nothing is drawn behind the glass;
- a **benchmark mode** (`--bench`), with `--preset high|fast` and
  `--proto-night-ev <EV100>` for the night exposure floor.

Its trees and lamps come from a scratch file converted once from the
city's data, with the data note's heights and mount classes, adapted by
the conversion's own rules (points up to 1 m inside a footprint count
as wall lamps, 38 positions, and so do off-centre lane points within
2.5 m of a facade): 1,137 poles, 451 wall lamps, 82 lamps on wires and
47 inside buildings. That file is not world data and is not committed.

**Determinism:** on the prototype's build, the client played the sample
replay (`fixtures/replays/sample.oprep`) and logged that it matches
every checkpoint, and `op_replay check` passed (20 checkpoints, final
state `212bd353...`).

**Setup:** an RTX 3070 (Vulkan, driver 610.88), an AMD Ryzen 9 3900 and
Windows 11; 1920 x 1080 with vsync off. 88 runs: a matrix of 80, then 8
on the committed build. Each run waits for loading, then turns the
camera once round at eye height over 600 frames, which took 2.5 to
4.7 s: much shorter than ADR-0018's 20-second turn. Every run used a
night exposure floor of EV100 3 (`--proto-night-ev 3`), which changes
only the night stops; the base runs used nothing else of the
prototype's but `--bench` (and `--preset fast` for the fast ones). GPU
time is the sum of Bevy's top-level render passes per frame; GPU timings
were recorded for 509 to 600 of each run's 600 frames. Bevy 0.20.1
counts no draw calls.

**Caveats:**
- **One run per configuration,** except four repeated on the committed
  build (`_b2`), and the machine may have been in use. Between a run and
  its repeat, GPU median moved by 0.00 to 0.05 ms, GPU 99th percentile
  by 0.01 to 0.24 ms and frame 99th percentile by 0.08 to 1.14 ms
  (`Lind_jun_base` 7.45 ms, `Lind_jun_base_b2` 6.49 ms). Frame-time
  differences of about 1 ms are therefore not evidence.
- **Batches.** The six Lindenhof sweeps ran as one batch, and it ran
  slower than the later runs: with the same shadow settings, its shadow
  pass took 0.27 to 0.32 ms more than `Lind_jun_trees_b2`'s. The sweeps
  compare with each other.
- **Two builds.** The first 70 runs of the matrix were built before the
  prototype's commit. Its last 10 (`Lind_jun_trees_shadow0`,
  `_shadow1`, `_shadow3`, `_lod25`, `_lod60`, `_far300`,
  `Hirs_dec_all64_r10`, `_r30`, `Rath_n2230_all32` and `_all32p`) and
  the 8 later runs were built from the commit. It changed how trees pick
  their level, and it narrows fewer crowns at facades (113 trees instead
  of 213), so 100 trees have another crown size. Base runs draw no trees
  and use none of that code (`Lind_jun_base` 4.18 ms, `Lind_jun_base_b2`
  4.22 ms). Comparisons below are within one build unless stated.
- **One card:** an RTX 3070, faster than the reference class; Vulkan
  only; no AMD card and no DX12.
- **Not measured:** video memory, popping while walking, shimmer in
  motion.

**Run names:** the stop (`Lind`, `Hirs`, `Rath`); the time (`jun` 18:00
on 21 June, `dec` 18:00 on 21 December, `n2230` 22:30 on 21 June); then
what was on: `base` (today's client), `trees`, `lamps32` (lamp models
and 32 real lights), `windows`, `all<n>` (everything, with n real
lights), `p` for point lights, `fast` for the fast preset, `r10` and
`r30` for the light range, `shadow<n>` for the levels casting shadows,
`lod25`, `lod60` and `far300` for level distances, `b2` for a repeat on
the committed build, and `rec` for the recommended configuration.

**The default configuration** (two levels casting shadows, 32 spot
lights), earlier build. GPU time per frame on high, median in ms, with
the difference from the stop's base in brackets:

| Stop and time | Base | Trees | Lamps, 32 lights | Windows | All, 32 lights | All: GPU p99 | All: frame p99 |
|---|---|---|---|---|---|---|---|
| Lindenhof, June 18:00 | 4.18 | 6.66 (+2.49) | 4.23 (+0.05) | 4.30 (+0.13) | 6.79 (+2.61) | 7.80 | 9.29 |
| Hirschenplatz, June 18:00 | 4.71 | 5.10 (+0.39) | 4.78 (+0.07) | 4.84 (+0.13) | 5.30 (+0.59) | 5.56 | 9.09 |
| Rathausbrücke, June 18:00 | 4.62 | 4.66 (+0.04) | 4.67 (+0.04) | 4.70 (+0.08) | 4.79 (+0.17) | 5.17 | 9.93 |
| Lindenhof, December 18:00 | 2.77 | 3.17 (+0.40) | 2.87 (+0.10) | 2.81 (+0.04) | 3.32 (+0.55) | 3.60 | 6.82 |
| Hirschenplatz, December 18:00 | 2.97 | 3.05 (+0.08) | 3.18 (+0.21) | 3.04 (+0.06) | 3.31 (+0.34) | 3.44 | 6.59 |
| Rathausbrücke, December 18:00 | 2.89 | 2.93 (+0.04) | 3.02 (+0.13) | 2.94 (+0.04) | 3.11 (+0.22) | 3.28 | 8.12 |

Runs `<stop>_<time>_base`, `_trees`, `_lamps32`, `_windows` and
`_all32`. At 18:00 in June the sun is up, so lamps and windows are
unlit: those columns measure lamp models and the windows' extra texture
coordinates only. At 22:30 on 21 June, with everything on and 32 spot
lights, GPU time was 3.23 to 3.59 ms at the median and 3.51 to 3.97 ms
at the 99th percentile (`*_n2230_all32`, Rathausbrücke's on the
committed build); those stops have no base run.

**The recommended configuration** (`*_rec`: sun shadows from the
nearest tree level only, `--proto-tree-shadows 1`; 64 point lights,
`--proto-lamps 64 --proto-lamp-point`; trees and windows on), committed
build, in ms:

| Stop and time | GPU median | GPU p99 | Frame p99 | Over the base: GPU median, GPU p99 |
|---|---|---|---|---|
| Lindenhof, June 18:00 | 6.69 | 7.77 | 9.25 | +2.47, +3.08 (`Lind_jun_base_b2`) |
| Lindenhof, December 18:00 | 3.47 | 3.76 | 8.00 | +0.70, +0.78 (`Lind_dec_base`) |
| Hirschenplatz, December 18:00 | 3.44 | 3.60 | 6.32 | +0.47, +0.54 (`Hirs_dec_base`) |
| Lindenhof, June 22:30 | 3.87 | 4.36 | 7.06 | no base run |

The December bases are from the earlier build, which differs only in
tree code that base runs do not use.

**What the runs show:**
- **Real lights are nearly free.** From 0 to 256 shadowless spot
  lights, GPU median rose by 0.08 to 0.20 ms, most of it with the first
  16 (`*_dec_all0` to `*_dec_all256`); on the committed build, 0 to 64
  added 0.17 ms at Hirschenplatz (`Hirs_dec_all0_b2`, `_all64_b2`).
  Point lights cost 0.08 to 0.16 ms more than spot lights in December,
  and 0.03 to 0.28 ms more at 22:30 in June, the most at Lindenhof,
  where leaves are lit from below (the `p` runs). 64 point lights cost
  0.19 to 0.31 ms of GPU median over none at the December stops
  (`*_dec_all64p` against `*_dec_all0`). A range of 10 or 30 m instead
  of 20 m changed GPU median by -0.08 and +0.07 ms (`Hirs_dec_all64_r10`
  and `_r30` against `Hirs_dec_all64_b2`). The prototype kept Bevy's
  default cluster capacities; the cluster-list resize that the engine
  note said could happen with hundreds of lights (3.1) never showed: no
  run logged a warning, even with 256.
- **Lit windows are nearly free:** 0.04 to 0.06 ms at night. In June,
  with no window lit, they cost 0.08 to 0.13 ms, of which 0.04 to
  0.06 ms in the sun's shadow pass. Our reading: the extra texture
  coordinates on facade parts are drawn in every pass.
- **Trees are the cost, but only in leaf.** At Lindenhof in June, trees
  raised GPU median from 4.18 to 6.66 ms (+2.50 ms on the committed
  build, `Lind_jun_trees_b2`): the sun's shadow pass from 1.28 to
  2.89 ms, the main pass from 0.39 to 0.99 ms and the prepass from 0.23
  to 0.42 ms. Main-pass fragments per pixel rose from 0.59 to 2.67, so
  on this card hidden leaf layers are shaded, not rejected before
  shading (the engine note's open question, section 7). Bare December
  trees cost 0.04 to 0.40 ms, though their twig cards still tripled
  main-pass fragments at Lindenhof (0.59 to 1.82). Where few trees
  stand they cost little: 0.04 ms at Rathausbrücke in June.
- **The cost is the near trees and their shadows.** In the sweep
  batch: no tree shadows, 5.37 ms (shadow pass 1.42 ms); the nearest
  level only (within 40 m), 6.85 ms (2.93 ms); all three levels,
  7.12 ms (3.31 ms). The nearest level makes about 85% of what tree
  shadows cost (1.48 of 1.75 ms). With two levels casting and other
  level distances (`_lod25`, `_lod60`, `_far300`), 6.96 to 7.11 ms:
  drawing trees only to 300 m (`_far300`, 7.07 ms) was not cheaper, so
  what trees beyond 300 m cost is below what these runs resolve. The
  middle level's larger cards shade more: main-pass fragments per pixel
  were 2.90 with the near level ending at 25 m and 2.50 at 60 m.
  Against two levels, shadows from the nearest level only saved 0.11 ms
  of shadow pass in the later runs (`Lind_jun_rec` 2.78 ms,
  `Lind_jun_trees_b2` 2.89 ms) and about 0.28 ms within the batch
  (`_shadow1` against `_far300`).
- **The fast preset** has no prepass, so every leaf layer is shaded in
  the main pass. At Lindenhof in June, trees, lamps and windows added
  1.90 ms of GPU median on fast (`Lind_jun_fast_all32` against
  `Lind_jun_fast_base`), and main-pass fragments per pixel went from
  1.82 to 4.74. The fast runs kept tree shadows, which ADR-0018 would
  drop on fast. They started on fast, so whether switching with F4
  leaves the prepasses on (engine note, finding 11) is still open.
- **The worst case** is Lindenhof in June, in the recommended
  configuration (`Lind_jun_rec`): GPU 7.77 ms and frame 9.25 ms at the
  99th percentile. On the reference class, 7.77 ms of GPU time would be
  about 12.8 ms (x 1.65) to 14.8 ms (x 1.9), against 16.7 ms for the
  whole frame (estimate). Put the other way: the budget is 8.8 to
  10.1 ms of GPU time on the RTX 3070; today's client takes 4.68 ms of
  it there (`Lind_jun_base_b2`, GPU p99), trees, lamps and windows
  3.08 ms more, which leaves 1.0 to 2.3 ms for the rest of M2:
  reflections in the water (ADR-0014's open decision), people
  (ADR-0021) and the visible character (estimate). The frame is close
  to GPU-bound at that stop (frame median 7.01 ms, GPU median 6.69 ms),
  so the ratio applies. The default configuration measured the same
  (`Lind_jun_all32`, earlier build: 7.80 and 9.29 ms): shadows from the
  nearest level only save little against two levels, because the
  nearest level is the cost.
- **The highest frame 99th percentile** in the default configuration,
  9.93 ms, came at Rathausbrücke in June with a GPU 99th percentile of
  5.17 ms (`Rath_jun_all32`): a hitch on the CPU side or from the
  machine, in one run.
- **CPU.** Trees and lamps add about 9,500 entities (6,030 for trees,
  six per tree, and 3,490 for lamps), as ADR-0018's design expected
  (about 9,000). At the December stops, where the frame is not GPU-bound
  (frame median 4.2 to 4.3 ms against GPU 2.8 to 3.0 ms in the base
  runs), frame median rose by 0.58 to 0.74 ms with everything on
  (`*_dec_all32` against `*_dec_base`). Our reading: the main-thread
  cost of the new entities and the light pool, from one run each.
  People share that main update (ADR-0021). Placing every point on the
  terrain took 1.5 to 2.0 s of frame time after loading, at 4 ms a
  frame.

**What the screenshots show,** at 18:00 on 21 June and 21 December,
with everything on at EV100 3 and 64 point lights, on the committed
build:
- **June at Lindenhof looks right:** lime crowns over the square and
  dappled shadows on the setts. In still images, some crowns in the
  middle distance show a dotted pattern, which looks like the cross-fade
  dither; how it looks in motion under TAA was not checked.
- **December at Hirschenplatz reads as an old town at night:** lit
  windows on upper and ground floors, lanterns on poles and brackets,
  warm pools of light on the setts.
- **Shop windows blow out to a flat white,** even at 25 cd/m², half of
  CIE's average. At EV100 3 mid-grey is 1.73 cd/m² (games note, M5), and
  25 cd/m² is about 3.9 stops above it (our arithmetic).
- **The sky is pitch black,** with no city glow and no moon, where the
  2025 photos show a sky lit orange-brown.
- **Away from lamps the foreground is nearly black:** on Lindenhof's
  open square and at the Rathausbrücke stop. Nothing lights surfaces
  between lamps.
- **Point or spot:** two earlier screenshots at Lindenhof in December,
  taken outside the matrix and the screenshot script, on the earlier
  build, with 32 lights each: spot lights aimed down leave the facades
  and bare crowns above the lanterns dark; point lights show them.

## Options considered

Options are grouped by part. Their names follow the games note where
they match it. Costs are the prototype's where measured, otherwise
estimates or the notes' expectations.

### Trees

#### T0. Proxies

A trunk cylinder and a crown shape per tree, scaled by its height and
crown.
- Pros: almost free; opaque, so no overdraw or shimmer.
- Cons: does not look like a tree, against "It looks real" (roadmap,
  section 1).

#### T1. Models generated per genus group, with levels of detail (recommended)

What the prototype draws: a few seeded variants per genus group, each
level a shared mesh, so Bevy instances them; alpha-tested leaf cards;
bare in winter by leaf dates per genus.
- Pros:
  - Bevy's own tools only; meshes and textures generated in code, so no
    licences;
  - seasons from the date;
  - draws grow with variants and levels, not with trees;
  - measured: +2.49 ms GPU median at Lindenhof in June, 0.04 to 0.40 ms
    elsewhere.
- Cons:
  - leaf overdraw in the prepass, the main pass and the shadow pass is
    the largest cost measured;
  - the cross-fade dither may show in the middle distance;
  - the tables per genus (group, crown form, defaults) are guesses;
  - bare crowns are a few twig cards, not fine branches.

#### T2. T1 plus impostors far away

Views of each variant baked at start-up and drawn as one card beyond
100 to 200 m.
- Pros: the cheapest far trees; `bevy_flora` measured 4.2 ms falling to
  3.3 ms for 1,000 trees with them, on another GPU (engine note, 3.6).
- Cons: a shader and baking code; lit only roughly at night; the
  prototype's far trees already cost little (`Lind_jun_trees_far300`),
  so there is little to save.

#### T3. One model per register tree

Each tree grown, for example by space colonisation, inside its own
crown and height.
- Pros: every tree different; the data used in full.
- Cons: no instancing; start-up time and memory grow with the trees.
  ez-tree's default tree alone has about 13,000 bark triangles (games
  note, M6).

#### T4. A crate (`bevy_flora`) or virtual geometry

- Pros: ready on Bevy 0.20, and measured by its author.
- Cons: `bevy_flora` calls itself "Vibe Coded" with no line-by-line
  human audit, its wind writes no motion vectors, and its textures are
  third-party images unless turned off. Bevy's meshlets need opaque
  materials and allow no custom vertex shader, so not leaves (engine
  note, 3.7).

**Tree shadows,** within T1 or T2:
- **S1. From the nearest level only (recommended to start):** trees
  within 40 m cast; 6.85 ms against 7.12 ms with all levels and 5.37 ms
  with none (the sweeps). Against two levels it saves little, 0.1 to
  0.3 ms of shadow pass, because the nearest level is the cost.
- **S2. Shadow-only crowns:** an opaque low-poly crown that only the sun
  sees, with leaf cards casting nothing. It attacks the largest measured
  cost. Plausible from Bevy's source, not tried (engine note, 3.2).
- **S3. No tree shadows:** saves 1.5 to 1.7 ms at Lindenhof in June
  (the sweeps), and loses the dappled shade that makes the June
  screenshot.
- **On fast:** none, as ADR-0018 recommends (not measured).

**Wind:** none in M2, or sway in a vertex shader that also gives TAA
last frame's position (engine note, 3.9). Sway costs in the prepass,
the main pass and the shadow pass; not measured.

### Street lamps

#### L0. Glowing heads only

A model per lamp with an emissive head; bloom does the glow.
- Pros: almost free.
- Cons: glowing heads over black streets, the problem ADR-0018's option
  C describes.

#### L1. L0 plus painted pools of light

Unlit, additive quads or decals on the ground under lamps.
- Pros: cheap; every street looks lit.
- Cons: looks painted on walls and slopes; clustered decals need
  Bevy's `pbr_clustered_decals` feature, which the client does not
  enable, and binding arrays on the GPU, while forward decals need no
  feature (engine note, 3.4); not tried.

#### L2. Real lights near the camera, glows everywhere (recommended)

L0 on every lamp, plus a pool of the nearest N lamps as real shadowless
lights, faded towards the pool's edge so that a lamp joining or leaving
the pool does not pop. What the prototype does.
- Pros:
  - lit streets where the camera is;
  - measured, GPU median at the December stops: 0 to 256 spot lights
    cost 0.08 to 0.20 ms, and the recommended 64 point lights 0.19 to
    0.31 ms over none (`*_dec_all64p` against `*_dec_all0`). With trees
    and windows also on, those runs took 0.29 to 0.80 ms more GPU time
    at the 99th percentile than the base runs;
  - the pattern of GTA V, X-Plane and Unreal.
- Cons:
  - light without shadows passes through walls into lanes and
    courtyards;
  - trunks and poles cast no shadows at night;
  - popping was not checked while walking;
  - beyond the pool, streets are dark under glowing heads.

Choices within L2:
- **Pool size:** ADR-0018's review asked for 32 to 64; the cost is flat
  from 16 to 256. With 64, the pool covers the lamps within 89 to 98 m
  of the stops (data note, section 7). Recommended: 64.
- **Point or spot:** point lights light the trees and facades above the
  lantern, as a clear lantern does, for 0.03 to 0.28 ms more.
  Recommended: point.
- **Range:** 20 m keeps 78% of a lamp's illuminance 10 m away (games
  note, M2), and 10 or 30 m changed cost by under 0.1 ms. Recommended:
  20 m.

#### L3. L2 plus a few shadowed lamps

The one to four lamps nearest the camera cast shadows.
- Pros: poles, trunks and people cast shadows under the nearest lamps.
- Cons: a 64 MiB layer and a 4096 x 4096 pass per shadowed spot light
  on high, six faces per point light (engine note, 3.2); Bevy gives
  shadows by entity order, not distance, so the client must choose; not
  measured.

#### L4. Every lamp a real light, or ray-traced light

- Pros: the simplest logic, or every glowing head and window lighting
  the street.
- Cons: 398 to 532 light positions within 250 m of each stop (engine
  note, 3.1). Bevy's ray-traced lighting is experimental, and its
  denoiser runs on NVIDIA only (engine note, 3.10). Not for the tier.

**Mount, height, colour and flux.** RFC 0004 has `mount` and
`light_height_mm`, null for every Zurich point.
- **M-A. The client infers the mount from geometry (recommended):** a
  wall bracket within 1.5 m of a facade, a lamp on a wire across the
  lane for points in the middle of narrow lanes, a pole elsewhere; a
  height per model; 3000 K; one default flux. Decoration, as ADR-0015's
  option A allows. Unvalidated: the wire class grows from 39 to 166
  positions as its thresholds change (data note, section 6).
- **M-B. Store `wall` in the world** (ADR-0015's option B): about 355 to
  420 points; poles and wires still unknown.
- **M-C. Ask ewz** for mount, height and luminaire type (ADR-0015's
  questions to the city), whatever is chosen meanwhile.
- **M-D. One model for all:** wrong for the 400 or so points at
  facades.

### Lit windows

#### W0. A glow per building

- Pros: trivial.
- Cons: does not read as windows.

#### W1. A seeded share of the procedural windows (recommended)

A window is lit when a seeded hash falls below a share for the hour
and day type, on upper and ground floors. Either with emissive masks on
a second set of texture coordinates, as the prototype does (one mask
per facade style, shifted by whole cells per building and wall;
StandardMaterial only, so F2 keeps working), or with a small shader
extension that hashes each window's building, wall, bay and storey and
can vary over time.
- Pros:
  - measured: 0.04 to 0.06 ms at night;
  - uses nothing about any real building, so it fits RFC 0004's rule
    as written;
  - the same windows on every run.
- Cons:
  - the shares are set by hand: no Swiss data by hour was found;
  - with the prototype's masks, the pattern repeats every 16 bays and
    8 storeys, every wall of a style shows the same pattern shifted,
    and a building's pattern changes only in steps of the share; a
    shader extension, or more masks, would avoid the repeat;
  - one more vertex attribute on facade parts, drawn in every pass.

Inputs within W1:
- **One curve for all buildings (recommended for M2):** the clock, and
  what the client itself draws (a shop window where a building's
  ground-floor style is a shop).
- **Curves by building class,** from OSM building tags and opening hours
  through the navigation layer (data note, section 8). It needs
  ADR-0019's answer on OSM changing how swisstopo shells are drawn.

#### W2. W1 plus interior mapping near the camera

Fictional rooms generated in code behind the nearest windows.
- Pros: depth behind the glass; the cost follows the pixels, not the
  number of rooms (van Dongen, 2008).
- Cons: RFC 0004 says "no interior detail", so that rule changes first;
  more shader work on every facade pixel near the camera.

#### W3. Animated rooms

Screens flickering, blinds moving.
- Cons: W2's, and more.

### Night exposure and sky

ADR-0018 owns the exposure decision and recommends a floor between
EV100 3 and 5 (its night-exposure option A). The screenshots show that a floor alone
does not make the night: the sky, the dark between lamps and the
brightest windows decide the look as much.

#### N1. A lower floor only

ADR-0018's night-exposure option A: a floor between EV100 3 and 5 once
the sun is well down (its review suggested 3 to 4), with lamp heads and windows in
cd/m² following the exposure.
- Pros: reproducible; no cost; the photos' EV100 3.0 and 4.0.
- Cons: what the prototype shows at EV100 3: a black sky, and black
  ground away from lamps.

#### N2. N1 plus a night fill (recommended)

- **Moonlight:** a dim, bluish directional light from the moon's
  position and phase, without shadows, as Far Cry 5 treats it. Cost:
  one more light in every pixel (estimate: small). With shadows it
  would bring back the sun's cascades at night: 1.64 to 2.94 ms in the
  June runs with trees in leaf (`*_jun_all32`), as they are at 22:30 in
  June.
- **Sky glow:** a dim, warm term in the night sky and in the light it
  gives, for the city's glow on clouds. Bevy's atmosphere has no term
  for artificial light (games note, 3.8), so this is client code; how
  is not established.
- Pros: dim rather than black between lamps, as Lucerne's 1.0 to 1.5 lx
  minimum keeps real lanes; a sky that reads as a city's.
- Cons: invented light, set by eye; the photos' sky was overcast, and
  weather is not in M2.

#### N3. Automatic exposure, retargeted

ADR-0018's night-exposure option B, with a curve as Far Cry 5 used.
- Pros: adapts between lit squares and dark lanes.
- Cons: each image depends on what the camera saw before, so stops and
  screenshots depend on the way to them; a compute pass every frame.

**Window and shop luminance,** within any of these:
- **Physical:** 6 cd/m² for lit flats and 50 cd/m² for shop windows
  (CIE 2025). Shop windows are about 4.9 stops above mid-grey at
  EV100 3: white.
- **The prototype's:** 6, 25 and 10 cd/m²; shop windows still white.
- **Lower for shop windows,** as a stated look choice.
- **Structure in the glass:** a gradient, frames and blinds, so that a
  bright window is not a flat panel, still without interior detail.

### Floodlit landmarks

- **F0. None in M2 (recommended):** no open dataset says which
  buildings are lit, and the concept plan has no landmark points in the
  district (games note, M4).
- **F1. Floodlight the buildings typed as sacred:** swissBUILDINGS3D's
  `OBJEKTART` marks 5 sacred buildings in the district (data note,
  section 9), but the world does not keep it (a format change), and
  which are lit would still be invented, against ADR-0012's rule that
  decoration must not pass for data.
- **F2. A source decision, or contributions:** lit landmarks from a
  source chosen in a source ADR, or added by contributors with their
  facades.

### How the options compare

| Part | Option | Cost on the RTX 3070 | Look | Needs | Recommended |
|---|---|---|---|---|---|
| Trees | T0 proxies | almost none (estimate) | not trees | nothing | no |
| Trees | T1 generated per genus group, levels | +2.49 ms at Lindenhof in June, 0.04 to 0.40 ms elsewhere (measured); S1 saves little | June right; dither in stills | genus tables, leaf dates | **yes**, with S1, then S2 tried |
| Trees | T2 T1 plus impostors | far trees already cheap (measured) | rough at night | baking, a shader | not now |
| Trees | T3 one model per tree | large (estimate) | each tree its own | a mesh per tree | no |
| Trees | T4 a crate, meshlets | not measured here | | an unaudited crate; meshlets cannot draw leaves | no |
| Lamps | L0 heads only | almost none | black streets | models | as part of L2 |
| Lamps | L1 painted pools | low (estimate) | painted | a Bevy feature for clustered decals, none for forward decals | later, if long views need it |
| Lamps | L2 heads and a pool of real lights | 0.08 to 0.20 ms for 0 to 256 spot lights; 0.19 to 0.31 ms for 64 point lights (measured) | lit lanes near the camera | pool code, mount inference | **yes**, 64 point lights, 20 m |
| Lamps | L3 plus shadowed lamps | a 64 MiB layer and a pass per light (engine note) | shadows under lamps | a shadow budget | not in M2 |
| Lamps | L4 every lamp, or ray tracing | over the tier (estimate) | | hardware beyond the tier | no |
| Windows | W0 a glow per building | almost none | not windows | | no |
| Windows | W1 a seeded share | 0.04 to 0.06 ms at night (measured) | reads as windows; shops white | hand-set curves | **yes**, one curve |
| Windows | W2, W3 rooms | per facade pixel (estimate) | rooms behind glass | an RFC 0004 change | no |
| Night | N1 a floor only | none | black sky and gaps | ADR-0018 | as part of N2 |
| Night | N2 a floor and a fill | small without shadows (estimate) | dim gaps, a city sky | screenshots against photos | **yes** |
| Night | N3 automatic exposure | a compute pass | adapts | a curve | no |
| Landmarks | F0 none | none | no floodlit towers | | **yes, in M2** |

## Decision

*Proposed, not decided.* The options marked recommended are a proposal
to the maintainer (@victorpicaud), who decides and records the choice
here. Until then nothing in this ADR applies.

**Recommended:**
- **trees:** T1, with sun shadows from the nearest level only (S1), and
  shadow-only crowns (S2) measured before the client work merges; no
  wind and no impostors in M2;
- **lamps:** L2, with a pool of 64 shadowless point lights of 20 m
  range, and mounts inferred in the client (M-A);
- **lit windows:** W1, with one share curve for all buildings;
- **night:** N2, under ADR-0018's night-exposure option A, with EV100 3
  as the starting value for the floor (3 or 4, per open question 2) and
  the values set by comparing screenshots with photos;
- **landmarks:** none in M2 (F0).

**Why:**
1. **It spends where the look is.** Lights and windows cost a few
   tenths of a millisecond; trees in leaf cost 2.5 ms at Lindenhof,
   mostly in shadows and overdraw near the camera. The recommendation
   keeps June's shade and December's lit lanes, and leaves out what
   costs without showing: shadows from far trees, impostors for far
   trees that are cheap already, and shadowed lamps.
2. **It keeps decoration apart from data.** Everything invented
   (models, leaf dates, mounts, heights, colours, which window is lit)
   is client code. The world keeps only what was recorded or measured
   (ADR-0012, RFC 0004).
3. **It needs no new asset licence.** Meshes and textures are generated
   in code. The one outside input, MeteoSwiss's leaf dates, is a
   handful of medians, credited (open question 6); the dates it lacks
   (lime's leaf fall, most genera) are assumptions in code.
4. **The night look is part of the decision.** At EV100 3, lamps alone
   gave a black sky and black ground between lamps; a lower floor is
   necessary but not enough.

**The budget risk.** In the recommended configuration (`Lind_jun_rec`),
Lindenhof in June would take about 12.8 to 14.8 ms of GPU time on the
reference class (estimate), against 16.7 ms for the whole frame, before
reflections, people and the visible character. S1 alone barely changes
that, since the nearest level is the cost. S2 and occlusion culling are
the first things to try; a share of the budget for this work is open
question 1.

### What the client draws

**Trees:**
- a model group (broad, small, columnar, conifer) from genus and
  cultivar, and the leaf habit from genus, both by client tables; leaf
  habit stays out of the world, as RFC 0004 drafts it (open question
  5);
- leaves from leaf-out to leaf fall, by per-genus dates for Zurich:
  MeteoSwiss's medians where its network observes the phase, and
  stated assumptions elsewhere (lime's leaf fall, and a default for the
  genera it does not observe); bare twigs otherwise; evergreens always
  in needle or leaf;
- a few seeded variants per group, scaled to the tree's measured height
  and crown; where those are null, defaults from crown, age or genus
  (ADR-0016's policy 1);
- three levels cross-faded by `VisibilityRange`, with shared meshes and
  materials so that Bevy instances them, and leaf textures whose
  mipmaps keep their coverage;
- sun shadows from the nearest level only; none on fast;
- crowns kept out of the nearest facade; no wind in M2.

**Lamps:**
- one model and one light per position; stacks drawn once; points more
  than 1 m inside a footprint not drawn, a choice of the client.
  ADR-0015's recommended option keeps those points in the data and says
  only that "The client does not light the street from inside a
  building";
- the mount from RFC 0004's `mount` where a source fills it, otherwise
  inferred from swissBUILDINGS3D shells and terrain only, never from
  OSM data; the height from `light_height_mm`, otherwise per model;
  3000 K and a default flux as client constants;
- heads glowing in cd/m², following the exposure; a pool of 64
  shadowless point lights of 20 m range, faded over a band at the
  pool's edge, keeping a lamp in its slot while it stays among the
  nearest;
- on once the sun is 2° below the horizon, the prototype's choice: no
  switching threshold was found for Zurich (games note, section 9).
  Zurich reduces street lighting after midnight where it can; both
  night stops come before midnight (data note, section 6);
- lamps over the river drawn from their anchors, or stood on the drawn
  deck or quay if ADR-0019's question 5 allows it. Of 201 positions over
  the river, the prototype stood 175 on a deck or quay within 5 m, kept
  19 on their own ground above the water and left out 7.

**Lit windows:**
- a share by hour and day type, set by hand and documented as such, for
  upper floors and ground floors;
- which window is lit follows only from the building id, wall, bay,
  storey and clock, without the repeat of the prototype's masks (a
  shader extension, or more masks); nothing behind the glass; off in
  F2.

**Night:**
- ADR-0018's floor, with EV100 3 as the starting value, as in the
  prototype (3 or 4, per open question 2); moonlight without shadows
  and a sky glow; window and shop luminance; all set by screenshots
  against the 2025 photos.

**The plain look (F2):** trees and lamps drawn as plain shapes from
their stored values only (a trunk and crown where a height is measured
and a crown recorded, a post at each light point), with no leaves,
glow, real lights or lit windows. The city calls every crown in its
register "only estimated" (data note, section 3), so a crown in F2 is a
recorded value, not a measurement. The alternative is to hide them in
F2.

### What RFC 0004 carries and what the client derives

| What drawing needs | RFC 0004 attribute | Zurich | The client |
|---|---|---|---|
| Where a tree stands, how tall, how wide | anchor, `height_mm`, `crown_diameter_mm` | about 112 heights and 183 crowns null (data note, section 3) | defaults where null |
| Which model, which crown form | `genus`, `species`, `taxon` | filled | a table from genus and cultivar to group and form |
| Bare in winter or not | `leaf_cycle`, `leaf_type` | null; may not be estimated | a table from genus to habit |
| When leaves come and go | none | none | per-genus dates in code |
| Age, for default heights | `planted_year` | 979 of 1,005 | defaults by age |
| Where a light stands | anchor, one point per record | filled | groups points at one position |
| Which way it faces | `yaw`, estimated | 1,799 of 1,959 | wall lamps face away from the wall |
| How it is mounted | `mount` | null (unless ADR-0015's option B) | inferred from shells; a stored value wins |
| How high | `light_height_mm` | null | per model |
| What it lights | `purpose` | null | a street lamp by default |
| Colour and flux | none | none | 3000 K and a default flux |
| How far it reaches up | bounds cover anchors only | | adds the objects' size when streaming (RFC 0004, section 6) |

So RFC 0004 needs no new attribute for M2's drawing. What drawing needs
and no source records (crown form, leaf dates, where a wire is
anchored, colour, flux, switching times) stays a client default (data
note, section 4). RFC 0004 should keep `mount`, `light_height_mm` and
`purpose`, nullable, because other sources fill them (39 of the
district's 76 OSM lamps carry a `lamp_mount`, and the canton's data a
construction type; data note, section 6), and the client reads them
before its own guesses.

## Consequences

- **Simulation:** unchanged. Only `op_client` changes;
  `SIM_CONTRACT_VERSION`, the replay format, the state hash and the gate
  stay as they are. On the prototype's build, the client played the
  sample replay matching every checkpoint, and `op_replay check`
  passed.
- **RFC 0004:** no new attribute (above). The client's streaming loop
  gets a branch that spawns instances from points instead of cutting
  meshes (engine note, 2.2), and pads asset bounds by object size.
- **ADR-0018:**
  - the route already shows where the budget is at risk: Lindenhof in
    June;
  - its night floor: EV100 3 as the starting value, with N2's fill
    (open question 2);
  - the prototype's `--bench` mode already does much of its measuring
    tool (option A under "Measuring tool"): a 1080p window without
    vsync, GPU time per pass, a JSON report. Its turn is 600 frames,
    which took 2.5 to 4.7 s on the RTX 3070, against ADR-0018's 20
    seconds, so the tool needs a timed turn; it can start from the
    prototype;
  - the 22:30 June stop needs a base run, and the fast preset drops
    tree shadows.
- **ADR-0019:** lifting lamps onto bridge decks follows its question 5.
  The mount inference uses swisstopo shells and terrain only. The data
  note's heuristic, from which the prototype's mounts were adapted,
  also looked at OSM water polygons; and the prototype stood lamps on
  the nearest surface in the drawn terrain and transport meshes, so
  some of them on OSM-derived decks. Both are the same render-time
  question.
- **ADR-0021:** people share the main update with about 9,500 more
  entities, which cost 0.58 to 0.74 ms of frame time at the December
  stops (our reading, from one run each).
- **Licences and credits:** no asset files. MeteoSwiss's leaf dates are
  credited "Source: MeteoSwiss" in ATTRIBUTION.md and the client's
  credits once open question 6 is settled. A port of ez-tree, if used,
  keeps its MIT notice; Sapling (GPL) and SpeedTree are not used.
- **Privacy:** the rule for lit windows stays in RFC 0004 and is
  repeated in the client's code and help: which window is lit is
  invented.
- **The prototype** stays on branch `drawing-prototype` and is not
  merged. Its results are recorded here and in
  [progress.md](../progress.md). The real work is written in pieces
  once RFC 0004's points load, reusing the prototype's tree generator,
  lamp pool, window mask and benchmark mode where review accepts them.
  The scratch points file and the script that converts the city's data
  stay outside the repository: the city's data reaches the world
  through the ingest (RFC 0004). The branch is kept until the route
  runs on the reference card, so that the same configurations can be
  measured there.
- **Order of work,** if accepted:
  1. on an RX 6600-class card and on DX12, the prototype's matrix again,
     two runs per configuration; S2 and occlusion culling tried at
     Lindenhof in June;
  2. the night calibrated by screenshots against the 2025 photos;
  3. RFC 0004 decided; the client reads the point layers; trees, lamps
     and windows land behind the route;
  4. M2's exit screenshots.
- **Tests:** unit tests for seeds, leaf dates, share curves and mount
  rules (the prototype has some); screenshots with `--screenshot`; the
  route. CI has no GPU.
- **What gets harder:** the client's tables are code to maintain per
  region. The city has 121 genus values against the district's 44 (data
  note, section 5), and other countries need their own leaf dates.
- **Revisit:**
  - when the reference card's run is in;
  - when RFC 0004 or ADR-0015 change, or ewz publishes mounts and
    heights;
  - when Bevy gains impostors or a cheaper path for leaves;
  - when weather (clouds, wet streets) or building types enter the
    world.

## Open questions

1. **A share of the budget for trees, lamps and windows.** Options: no
   share, and the route decides at the end of M2; or a share, as
   ADR-0021 asks for people. Recommended: a share, with 2 ms of GPU time
   at the 99th percentile on the RTX 3070 at every stop as a starting
   value: less than half of the 4.1 to 5.4 ms left at Lindenhof in June
   (estimate, against `Lind_jun_base_b2`). In the recommended
   configuration the prototype meets it at Lindenhof and Hirschenplatz
   in December (0.78 and 0.54 ms more) and misses it at Lindenhof in
   June (3.08 ms more); the other stops were not run in it. In the
   default configuration, six stop-and-time pairs have a base run: it
   meets the share at five (the June stops at Hirschenplatz and
   Rathausbrücke and the three December stops, 0.21 to 0.62 ms more) and
   misses it at Lindenhof in June (3.33 ms more).
2. **The night's values:** the floor (EV100 3 or 4), moonlight (on or
   off, with the real phase or a fixed one), the sky glow's strength,
   and window and shop luminance (CIE's 6 and 50 cd/m², the prototype's
   25 for shops, lower, or structure in the glass). Recommended: start
   from EV100 3, and choose the values by comparing screenshots at
   EV100 3 and 4 with the two 2025 Lindenhof photos, at the three
   stops at 18:00 on 21 December and 22:30 on 21 June, and record the
   values here, with any departure from CIE's figures named as a look
   choice.
3. **Tree shadows:** S1, S2 or a distance limit; and none on fast.
   Recommended: S1 now, S2 measured before merging, none on fast.
4. **Point or spot lights.** Recommended: point, for 0.03 to 0.28 ms
   more; spot lights if the route needs the time back.
5. **Leaf habit in the client or the world.** Options: a client table
   (as RFC 0004 drafts it); or an amendment to RFC 0004 letting a named
   rule estimate `leaf_cycle` and `leaf_type` from genus, so that every
   reader draws the same seasons (data note, section 10). Recommended:
   the client table.
6. **MeteoSwiss's leaf dates.** Its STAC collection says "CC-BY" with no
   version; its documentation says the data "may be used without
   restriction" with the credit "Source: MeteoSwiss"; ADR-0017 lists CC
   BY 4.0 only (data note, section 4). Options: credit MeteoSwiss in
   ATTRIBUTION.md and the client's credits after ADR-0017's review; or
   take dates from general botany instead. Recommended: the first.
7. **Lamp mounts:** M-A, M-B or M-C. Recommended: M-A, and ask ewz
   anyway, with ADR-0015's questions.
8. **Windows by building class:** one curve in M2, classes after
   ADR-0019. Recommended: one curve.
9. **Floodlit landmarks:** F0 in M2; F2 later, when a source or
   contributors can say which buildings are lit.

**Nothing here is decided.** The maintainer, @victorpicaud, decides and
records the choice and its reasons in the Decision section. Each open
choice is drafted with a recommendation.
