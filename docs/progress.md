# Progress

The first milestone is a walkable, deterministic slice of Zurich's old town
(about 1 km²) built only from open data. Work is split into six stages, each
ending at a checkpoint where the checks are run and the result is reviewed.

| Stage | Scope | Status |
|---|---|---|
| 1 | Scaffold: workspace, licences, community docs, CI | ✅ done 2026-11-22, CI green |
| 2 | World format: JSON Schemas, `op_world`, RFC 0001 | ✅ done 2026-11-23, CI green |
| 3 | Ingest: swisstopo + OSM → world tiles, one fixture tile | ✅ done 2026-11-23, CI green |
| 4 | Kernel, simulation, replay: 60 Hz, Rapier capsule, state hashes, replay CLI | ✅ done 2026-11-23, CI green on 3 platforms |
| 5 | Client and streaming: Bevy first-person walking, replay recording | ✅ done 2026-11-23, CI green |
| 6 | Docs and review: ADRs, README run guide, good-first-issues | ✅ done 2026-11-23, CI green |

District bounding box (WGS84): `8.5377,47.3687,8.5507,47.3774`
(`worlds/zurich-altstadt.json`), the largest lon/lat rectangle inside the
LV95 km square E 2'683'000–2'684'000 / N 1'247'000–1'248'000, so it needs
one swissBUILDINGS3D sheet (1091-41) and one swissALTI3D tile (2683-1247).

---

## Stage 1: scaffold (2026-11-22)

**Done**

- Cargo workspace (edition 2024, resolver 3) with crates `op_kernel`,
  `op_sim`, `op_replay`, `op_world`, `op_stream`, `op_client`, as documented
  stubs. `op_client` opens an empty Bevy 0.20.1 window.
- Pins: Rust 1.100.0 (`rust-toolchain.toml`), `bevy =0.20.1`,
  `rapier3d =0.36.1` with `enhanced-determinism`. `Cargo.lock` committed.
- Determinism lints in `clippy.toml`: banned `HashMap`/`HashSet`/`RandomState`,
  `Instant`/`SystemTime`, unseeded `rand`, and platform trig/transcendental
  functions on `f32`/`f64`. `op_client` has its own relaxed `clippy.toml`.
  `unsafe_code` is forbidden workspace-wide.
- Rapier canary test (`crates/op_sim/tests/rapier_canary.rs`): eight balls,
  200 steps; same-process runs must match bit for bit and match a golden
  digest (`0xc9e3ca4f0c0c2bb1`, recorded on x86_64 Windows; identical in
  debug and release).
- Licences: `LICENSE-MIT` (holder now "The StreetWide contributors"),
  `LICENSE-APACHE`, `LICENSES/CC-BY-SA-4.0.txt`, `LICENSES/ODbL-1.0.txt`.
- Community docs: README, CONTRIBUTING (with DCO 1.1 text), GOVERNANCE,
  CODE_OF_CONDUCT (Contributor Covenant 2.1), SECURITY (GitHub private
  vulnerability reporting, now enabled on the repo), ATTRIBUTION (skeleton).
- Decision records: ADR-0001 licensing, ADR-0002 crate layout and dependency
  direction, ADR-0005 toolchain and engine pinning (*Accepted* at the
  checkpoint); ADR and RFC templates.
- CI (`.github/workflows/ci.yml`, actions pinned to SHAs): `fmt`, `clippy`,
  `test`, `python` (tests of the CI scripts), `deps` (dependency direction),
  `dco` (sign-off check).
- `tools/ci/check_deps.py` + `deps-policy.toml` and `tools/ci/check_dco.py`,
  each with tests that feed them deliberately broken cases.

**Checks** (local, Windows 11 x86_64, Rust 1.100.0)

| Check | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | pass (Bevy client included) |
| `cargo test --workspace --locked` | pass; Rapier canary 2/2 in debug and release |
| `python -m pytest` (CI script tests) | 14 passed |
| `python tools/ci/check_deps.py` | pass |
| Determinism bans fire on a deliberate violation (`HashMap`, `Instant`, `f32::sin`, `f64::powi`, `f64::atan2`) | 5/5 errors, then reverted |
| `op_client` relaxed `clippy.toml` accepts the same code | pass |
| `cargo run -p op_client` | window "StreetWide" opens (Vulkan, RTX 3070), runs 10 s without errors |
| GitHub Actions CI on `main` (commit `e1b2196`) | all 9 jobs green, incl. the Rapier canary on Linux, Windows and macOS arm64 ([run 45078352896](https://github.com/AndromedaSLS/StreetWide/actions/runs/45078352896)) |

**Open items**

- CODE_OF_CONDUCT contact address: placeholder until a private reporting
  address exists; must be filled before the project is announced.

---

## Stage 2: world format (2026-11-23)

**Done**

- [RFC 0001](../rfcs/0001-world-format.md) (*Draft*): Web Mercator tiles at
  z17 inside z12 regions; region East-North-Up frame (X east, Y up, Z south)
  with integer-millimetre tile origins; layers `shell`, `facade`,
  `interior`, `furniture`, `interactivity` plus `terrain` and `transport`;
  per-building assets with `parent_version_id` lineage; licence separation
  (OSM-derived geometry never shares an asset); `fictional: true` required
  for interiors and furniture; canonical JSON with integers only; BLAKE3
  ids forming a Merkle chain world → tiles → manifests → meshes; a strict
  glb profile with exactly one byte form per mesh; content-addressed store.
- JSON Schemas (draft 2020-12): `schemas/common`, `asset-manifest`,
  `tile-index`, `world-manifest`.
- `op_world`: canonical JSON, content ids, tile/region keys, typed
  documents with all rule checks, glb reference writer and strict reader,
  store, whole-world validator, and the `op_world validate|id` CLI. No
  workspace dependencies.
- Golden fixture `fixtures/format/`: a fictional house and ground plate in
  tile 17/68646/45908, built from `meshes.json` by
  `cargo run -p op_world --example make_format_fixtures -- fixtures/format`.
  World id `blake3:f5a3cc70b342251b7286d752e28f1df961f9aa7631064baf4fae0060f47b8fa3`.
- `tools/worldfmt`: Python implementation (canonical JSON, ids, glb writer,
  schema validation) that the stage 3 ingest will use.
- CI: format golden ids checked on Linux, Windows and macOS; Python parity
  tests; every committed `.glb` validated by the Khronos glTF validator
  (`tools/ci/gltf`, pinned 2.0.0-dev.3.10).

**Changes from the plan**

- The replay-header schema moves to stage 4, to be designed together with
  the replay format (ADR-0007).
- `op_world` parses glb with its own strict reader instead of the `gltf`
  crate: the profile is small and the reader must also prove canonical
  form. The Khronos validator in CI is the independent glTF 2.0 check.
- Added `tools/worldfmt` so ingest (Python) writes exactly the bytes
  `op_world` accepts; Python requirements moved to `tools/requirements.txt`.

**Checks** (local, Windows 11 x86_64)

| Check | Result |
|---|---|
| `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings` | pass |
| `cargo test --workspace --locked` | pass (23 `op_world` tests + Rapier canary) |
| Manifests, tile index and world manifest round-trip byte for byte | pass |
| Fixture world rebuilt in 3 separate processes | identical world id, byte-identical to the committed files |
| JSON Schema ⇔ Rust agreement: fixture documents valid; 24 mutations rejected by both; 3 Rust-only rules | pass |
| Non-canonical JSON (14 cases) and corrupted glb (8 cases) rejected | pass |
| `python -m pytest` (CI scripts + Python/Rust parity) | 41 passed |
| `op_world validate fixtures/format/world` | 1 tile, 2 assets, 12 triangles, 0 errors |
| Khronos glTF validator on committed `.glb` | 2/2 valid, 0 errors, 0 warnings |
| `python tools/ci/check_deps.py` | pass |
| GitHub Actions CI on `main` (commit `0b4e90e`) | all 10 jobs green, incl. format golden ids on Linux, Windows and macOS arm64 and the Khronos validator ([run 45135919320](https://github.com/AndromedaSLS/StreetWide/actions/runs/45135919320)) |

**Open items**

- RFC 0001 is *Draft*. GOVERNANCE.md asks for 7 days of comment before an
  RFC is accepted; the maintainer decides when to accept it.

---

## Stage 3: ingest (2026-11-23)

**Done**

- `tools/ingest` (`python -m tools.ingest`), decisions and options in
  [ADR-0006](adr/0006-ingest-pipeline-and-data-sources.md) (*Proposed*):
  - **Sources.** Found through the swisstopo STAC API by item footprint;
    the latest year per map tile; SHA-256 verified. OSM comes from one
    Overpass query. Everything is cached in `data/cache/` (gitignored),
    with a generic User-Agent.
  - **Buildings.** swissBUILDINGS3D 3.0 Beta CityGML is streamed out of the
    2.5 GB zip. Only the typed roof, wall and ground surfaces are used (the
    solids duplicate them), and ground plates are turned to face down.
    Underground buildings are skipped.
  - **Terrain.** swissALTI3D 0.5 m is sampled onto a 128 × 128 grid per
    tile, with crack-free seams by construction.
  - **Transport.** OSM roads, paths and steps become ribbons draped on the
    terrain, one visual asset per tile. Each bridge way becomes a deck
    asset with a collider, with heights interpolated between the streets
    it connects.
  - **Fallback.** OSM footprints are extruded where swissBUILDINGS3D covers
    at most 20 % of them (10 buildings in the district).
  - **Coordinates.** An explicit PROJ pipeline (CH1903+ → WGS84) and PROJ
    topocentric coordinates in the RFC 0001 region frame.
- World definitions in `worlds/`: `zurich-altstadt` (spawn on the
  Lindenhof, facing the Limmat) and `fixture-grossmuenster`.
- `fixtures/world/`: tile 17/68646/45903 (Grossmünster, Münsterbrücke, the
  Limmat), 44 assets, 90 files, 1.7 MB. Rust test `ingest_fixture`, Python
  `test_fixture_world`, and CI validates it on all three OSes.
- `tools/worldfmt/store.py`: Python store writer.
- ATTRIBUTION.md complete: dataset versions, terms, links, ODbL notice,
  licences of the committed data. `fixtures/README.md` added.
- RFC 0001 §2 (still *Draft*) amended: an OSM-derived asset may use another
  source to place its geometry (terrain heights) and then states the
  combined licence `ODbL-1.0 AND LicenseRef-swisstopo-OGD`.

**Result for `zurich-altstadt`**

- 30 tiles and 1,483 assets:
  - 1,377 swissBUILDINGS3D shells and 10 OSM fallback shells;
  - 36 bridge decks;
  - 30 ribbon assets;
  - 30 terrain tiles.
- 1,077,314 triangles, 48 MB in 2,997 files.
- About 3 minutes from a cold cache once downloaded, 2 minutes with the
  parsed-building cache.

**Changes from the plan**

- **Bounding box.** The planned bbox `8.5377,47.3685,8.5507,47.3775` stuck
  out of the LV95 km square: 12 m at the south-west corner, 2 m at the
  north-east. Checked against the STAC item geometries, it would have
  needed two more terrain tiles and a second 250 MB building sheet. It is
  now `8.5377,47.3687,8.5507,47.3774`.
- **Terrain resolution.** About 1.6 m (128 cells per tile on Web Mercator
  pixel corners) instead of a 1 m grid in the tile frame. Seams are exact
  without stitching, and the size is a quarter.
- **Fixture tile.** 17/68646/45903 instead of the plan's description. Its
  south edge is cut to the bbox so it needs no extra downloads.
- **Stage 2 golden fixture.** Its tile origin had been computed with a
  spherical shortcut (4 m off). It now uses the RFC's exact ECEF → ENU
  method, `[-1971116, 0, 930405]`, and a test recomputes it. New world id
  `blake3:f2d6f86933f9a8820ec057888b0c3b9642795b026bcd0312c824d25cb496bda1`.

**Checks** (local, Windows 11 x86_64)

| Check | Result |
|---|---|
| `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings` | pass |
| `cargo test --workspace --locked` | pass (27 tests, incl. 2 new `ingest_fixture` tests) |
| `python -m pytest` | 80 passed (39 new ingest tests, synthetic data, incl. the OSM fallback) |
| Every generated document against `schemas/` (Python jsonschema, during ingest) | pass |
| `op_world validate data/world/zurich-altstadt` | 30 tiles, 1,483 assets, 1,077,314 triangles, 0 errors |
| `op_world validate fixtures/world` | 1 tile, 44 assets, 42,227 triangles, 0 errors |
| Khronos glTF validator (district + fixtures) | 1,529/1,529 valid, 0 errors, 0 warnings |
| Ingest twice from cache (once parsing the CityGML zip, once from the parsed-building cache) | identical world id `blake3:30c4e880531add65944db631d9987438ebc555c9ec4bf4e6172f9ca9c7530022` |
| Fixture regenerated from cache (`--offline`) | byte-identical to `fixtures/world` |
| Downloads | SHA-256 match swisstopo's published checksums |
| `python tools/ci/check_deps.py` | pass |
| GitHub Actions CI on `main` (commit `59362ca`) | all 10 jobs green, incl. the Python ingest tests on Linux and the ingest fixture on Linux, Windows and macOS arm64 ([run 45142654990](https://github.com/AndromedaSLS/StreetWide/actions/runs/45142654990)) |

**Open items**

- ADR-0006 was accepted with the go-ahead for this checkpoint.
- Known gaps for good-first-issue drafts (stage 6):
  - no water surface (the Limmat is walkable terrain at water level);
  - squares mapped as areas are not drawn;
  - bridges that end at building doors are anchored to the ground;
  - no levels of detail.
- Rathausbrücke is tagged `highway=construction` in OSM (it is being
  rebuilt), so it is absent; the temporary bridge next to it is present.
- Ingest output is reproducible on one platform. Across platforms, libm,
  PROJ or GEOS differences could in rare cases move a vertex by 1 mm.

---

## Stage 4: kernel, simulation, replay (2026-11-23)

**Done**

- `op_kernel` (Bevy-free, no workspace dependencies):
  - `Module` trait and a fixed-order `Schedule`. Each tick prepares, then
    steps. A tick whose geometry is still loading stalls without changing
    anything.
  - `PlayerInput`: quantised, an 8-byte record.
  - BLAKE3 `StateHasher` (`streetwide.state.v1`).
  - Seeded `Rng` (xoshiro256** via SplitMix64).
  - `dmath` on libm.
  - The `GeometrySource` contract, in integer millimetres in a sim frame
    whose integer origin is at the spawn point.
- `op_sim`:
  - `Sim` with the `PlayerPhysics` module: Rapier 0.36
    `KinematicCharacterController` on a 0.3 × 1.8 m capsule.
  - Walk 1.4 m/s, sprint 4 m/s, jump about 0.6 m, autostep 0.3 m, 45°
    slopes, snap to ground.
  - Colliders within 160 m are added and removed in key order.
  - `SIM_CONTRACT_VERSION` 0.1.0.
- `op_stream`: `StoreSource`, the collision geometry of a world store with
  exact millimetre conversion (synchronous; stage 5 adds background
  loading).
- `op_replay`:
  - **`.oprep` format:** canonical-JSON header, 8-byte inputs, a
    checkpoint every 60 ticks and at the end. Strict reader.
  - **`schemas/replay-header.schema.json`**, the schema moved here from
    stage 2.
  - **Playback** against a world store, checked by world id.
  - **CLI:** `check`, `run`, `info`, `trace`, `synth`.
- `fixtures/replays/sample.oprep` (10.8 KB): a 20 s scripted walk in the
  Grossmünster fixture. It lands on the square, sprints into a house wall
  (blocked 88 ticks), jumps (42 ticks airborne), crosses the square and
  climbs the lane east of the Grossmünster (2.45 m up), 63.8 m in all.
- ADR-0003 (determinism strategy), ADR-0004 (physics and character
  controller) and ADR-0007 (replay format) are *Proposed*. ADR-0006 is
  *Accepted*.
- CI: the determinism job on Linux, Windows and macOS arm64 now also runs
  the kernel, sim, stream and replay tests, and `op_replay check` on the
  sample twice in debug (separate processes) and once in release.

**Checks** (local, Windows 11 x86_64)

| Check | Result |
|---|---|
| `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings` (determinism bans active) | pass |
| `cargo test --workspace --locked` | 56 passed (29 new) |
| `python -m pytest` | 80 passed |
| `op_replay check fixtures/replays/sample.oprep`, two separate debug runs | 20/20 checkpoints match, final `a1580200f7aa78dbd750220e35382ad105be40969e57a636f999350badbfdad2` |
| Same in release | identical |
| Two simulations in one process; a run with loading stalls | identical hash trails |
| `op_replay synth` regenerates the sample | byte-identical |
| A changed input at tick 701 | caught at checkpoint 720 |
| `python tools/ci/check_deps.py` | pass |
| GitHub Actions CI on `main` (commit `bc3a8bb`) | all 10 jobs green; `op_replay check` gives final state `a1580200…fdad2` on Linux x86_64, Windows x86_64 and macOS arm64, twice in debug and once in release each ([run 45151989757](https://github.com/AndromedaSLS/StreetWide/actions/runs/45151989757)) |

**Open items**

- ADR-0003, ADR-0004 and ADR-0007 were accepted with the go-ahead for this
  checkpoint.
- Known physics limitation (ADR-0004): pushing into an unclimbable concave
  corner can make the capsule oscillate by about 2 cm from tick to tick.
  It is deterministic; a good-first-issue candidate.

---

## Stage 5: client and streaming (2026-11-23)

**Done**

- `op_stream`:
  - `WorldIndex`: the world manifest and all tile indexes, plus mesh
    decoding.
  - `Streamer`: a pool of 4 loader threads filling an LRU cache of decoded
    meshes (768 MB budget in the client).
  - `StreamSource`: the simulation's view of the streamer. It answers
    `Pending` while a mesh loads, so the simulation stalls instead of
    guessing.
  - Rendering and physics read the same cache, so they share one loading
    path.
- `op_client` (Bevy 0.20.1):
  - **Simulation:** `op_sim` runs in `FixedUpdate` at exactly 60 Hz. Input
    is sampled every frame and latched for the next tick; a jump pressed
    between ticks is kept until a tick uses it.
  - **Camera:** it interpolates between the last two simulated positions.
    Mouse look is applied every frame at full precision; the simulation
    gets the heading quantised to 1/65536 turn.
  - **Streaming:** meshes within 600 m of the camera are loaded nearest
    first and unloaded beyond 700 m.
  - **Look:** palette materials (drawn double-sided), an afternoon sun
    with cascaded shadows, ambient light, sky colour and distance fog.
  - **HUD:**
    - the world's credits, always shown ("© OpenStreetMap contributors ·
      © swisstopo"), in a system font with those glyphs;
    - status and controls help;
    - "Loading…" while the simulation is stalled.
  - **Controls:** click to capture the mouse, WASD, Shift sprint, Space
    jump; F9 saves a replay of the session; R restarts at the spawn point;
    Esc releases the mouse.
  - **Options:**
    - `--world`, `--seed`;
    - `--replay <file>` plays a replay back in the client and checks every
      checkpoint;
    - `--exit-when-done` and `--screenshot <png> [--screenshot-tick <n>]`
      for scripted runs.
- README: how to walk the district and check or watch replays.

**Checks** (local, Windows 11 x86_64, RTX 3070)

| Check | Result |
|---|---|
| fmt, clippy `-D warnings` (Bevy client included) | pass |
| `cargo test --workspace --locked` | 59 passed (3 new) |
| `python -m pytest` | 80 passed |
| Sample replay through `StreamSource` (2 loader threads, 64 KB cache forcing evictions, stalls) | same 20 checkpoints as recorded |
| A live client session (streaming, stalls, latched jumps, turning, sprint) saved the F9 way, then `op_replay check` | match |
| `op_client --replay fixtures/replays/sample.oprep --exit-when-done` | real-time playback in the window, every checkpoint matches, exit code 0 |
| Same with `--release` (4.5 min build) | every checkpoint matches, exit code 0 |
| `op_client` on `data/world/zurich-altstadt` | Lindenhof view east over the old town; 1,355 meshes streamed within 600 m, 296 colliders; credits rendered with © |
| `op_replay check fixtures/replays/sample.oprep` (twice) | match |
| `python tools/ci/check_deps.py` | pass |
| GitHub Actions CI on `main` (commit `27a96c5`) | all 10 jobs green, incl. the client's tests on Linux ([run 45157493381](https://github.com/AndromedaSLS/StreetWide/actions/runs/45157493381)) |

**Changes from the plan**

- **F9 saves rather than toggles.** It saves the session from its start to
  now, instead of starting and stopping a recording. A replay must begin
  from a fresh simulation at a spawn point, and recording that way means
  you never lose what just happened. R starts a fresh session.
- **Credit font.** Bevy's built-in font has no `©`, so the client loads a
  common system font (Segoe UI, Arial, DejaVu Sans or Noto Sans). If none
  is found, it writes "(c)".

**Open items**

- **Known gaps** (stage 6 good-first-issue drafts):
  - no water surface;
  - you can walk off the edge of the built area and fall (R restarts);
  - no levels of detail beyond 600 m;
  - the corner jitter noted in ADR-0004.

---

## Stage 6: docs and review (2026-11-23)

**Done**

- **README: Getting started.** One path from a fresh clone to walking the
  district:
  - per-OS prerequisites (Git, Rust, C/C++ build tools, Python 3.11+, GPU,
    disk);
  - the determinism check;
  - building the district (bash and PowerShell commands);
  - walking it, then checking and watching replays;
  - other areas, development checks, and a troubleshooting table.
- **CONTRIBUTING:**
  - where to start;
  - the full check list, including `op_replay check`;
  - what to do when a change alters simulation results: bump the contract
    version, re-record the sample, never edit hashes.
- **ADR audit:** one record per foundational choice.
  - ADR-0001 to 0007 are *Accepted*.
  - New ADR-0008 (accepted with this checkpoint's go-ahead) covers the
    client architecture: the
    fixed-step simulation inside Bevy, input latching, one shared mesh
    cache for rendering and physics, F9 recording, the credit font.
  - The world format is RFC 0001 (still *Draft*).
- **`docs/issues/`:** eight good-first-issue drafts, each with why, what to
  do, done-when and pointers:
  - ingest progress output;
  - pedestrian squares;
  - OSM `building:part`;
  - fall recovery in the client;
  - replay playback controls;
  - a minimap;
  - cargo-deny;
  - a schema reference page.

  The index also lists larger known gaps (water, corner jitter, levels of
  detail, bridges at doors).
- **`tools/ci/check_deps.py`** explains a missing `cargo` instead of
  printing a traceback (found during the fresh-clone test).

**Checkpoint: a fresh clone builds and runs from the README alone**

Cloned the committed repository into `c:\tmp\streetwide-fresh`, with an
empty build directory and an empty data cache, then followed the README
literally (Windows 11; PowerShell commands for Python):

| README step | Result |
|---|---|
| 2. `cargo run -p op_replay -- check fixtures/replays/sample.oprep` | cold build 58 s; `ok: 1200 ticks, 20 checkpoints match, final state a1580200…` |
| 3. `py -m venv .venv`, `pip install -r tools\requirements.txt` | pass (Python 3.13) |
| 3. `.venv\Scripts\python -m tools.ingest --world zurich-altstadt` | 3.9 min including fresh downloads (258 + 16 MB from swisstopo, checksums verified; Overpass query); 30 tiles, 1,483 assets |
| 3. `cargo run -p op_world -- validate data/world/zurich-altstadt` | 0 errors |
| 4. `cargo run --release -p op_client` | release build 270 s; the district renders from the Lindenhof, credits correct |
| 5. Client playback of the sample replay | every checkpoint matches |
| Development checks | `cargo fmt --check` pass; clippy `-D warnings` pass; `cargo test --workspace` 59 passed; pytest 80 passed; `check_deps.py` pass |
| GitHub Actions CI on `main` (commit `d9fe307`) | all 10 jobs green ([run 45161462257](https://github.com/AndromedaSLS/StreetWide/actions/runs/45161462257)) |

The fresh clone's world id (`5cdaeaf0…`) differs from the working copy's
(`30c4e880…`) only because OpenStreetMap moved on between the two
Overpass queries (08:22 vs 10:39 UTC). The swisstopo files were
byte-identical. With the working copy's cached inputs, the fresh clone's
code reproduces `30c4e880…` exactly, so the ingest is reproducible across
checkouts.

**Not covered by this test**

- **rustup first install:** the pinned toolchain was already installed on
  this machine.
- **Linux and macOS:** CI builds, lints and tests there (including the
  determinism and client tests) but does not run the ingest downloads or
  open a window.
- **The published clone:** the clone came from the local repository
  because the stage 6 commits were not pushed yet.

**Open items before announcing the project**

- **RFC 0001:** *Draft*; the maintainer decides when to accept it.
- **Code of Conduct:** the contact address is still a placeholder.
- **Issue drafts:** file the ones in `docs/issues/` on GitHub if wanted.
