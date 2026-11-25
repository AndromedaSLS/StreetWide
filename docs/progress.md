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
- **Linux and macOS:** CI builds, lints and tests every crate on Linux,
  but does not run the ingest downloads or open a window. *(Corrected
  2026-11-24: on Windows and macOS, CI ran only the determinism job, so
  the client was never compiled there. A `client` job now builds and tests
  it on both.)*
- **The published clone:** the clone came from the local repository
  because the stage 6 commits were not pushed yet.

**Open items before announcing the project**

- **RFC 0001:** *Draft*; the maintainer decides when to accept it.
- **Code of Conduct:** the contact address is still a placeholder.
- **Issue drafts:** file the ones in `docs/issues/` on GitHub if wanted.

---

## Next: contributions (started 2026-11-23)

- [RFC 0002](../rfcs/0002-contributions-and-merge-rules.md) (*Draft*):
  - change sets;
  - the deterministic gate (format, lineage, layer policy, licence,
    provenance, privacy, budgets, placement, walkability, replays);
  - "beats what is there": no measured regression, plus a measured or
    reviewed improvement;
  - a hash-chained world log, baseline refreshes, takedowns;
  - facades as the first, visual-only contributable layer;
  - OSM-derived layers fixed upstream.

  Open for comment until at least 2026-11-30.

### Contribution gate, part 1 (2026-11-23)

RFC 0002 was merged into `main` (PR #1). Its status stays *Draft* until it
is accepted after the comment period. Implementing it, as RFC 0001 was
while it was a draft:

- **Schemas** for change sets, check reports and merge records
  (`schemas/change-set`, `check-report`, `merge-record`), with Rust and
  Python agreement tests.
- **`op_world`:**
  - `contrib`: the three document types with strict validation, and
    world-log hash-chain verification;
  - `gate`: builds the candidate world (new tile indexes over the bundle
    and the canonical store, via layered stores, without copying) and runs
    G1–G8;
  - `op_world check-change <world> <bundle>` prints the report and exits
    1 unless the change set passes.
- **G9 and G10** (walkability, replays) need the simulation and are not
  implemented yet. Change sets that touch colliders report them as
  `not_run` and cannot pass. Facades have no collider and are not
  affected.
- **Golden fixture** `fixtures/changes/facade-sample`: a test facade (a
  3 cm skin over one building's walls, swisstopo-derived, so it carries
  the combined licence). It passes the gate with G6 left to review. Its
  report bytes are pinned and checked on Linux, Windows and macOS.
- **Gate tests:** for each check, a deliberately broken contribution that
  it must catch. They cover a stale parent, a facade with a collider, a
  closed layer, ODbL content, a missing licence, a proprietary source, a
  wrong author, a missing credit, an over-budget mesh, a facade 2 m from
  its wall, a facade without a building, and a collider change.
- **RFC 0002 clarified** (still *Draft*): the bundle layout, the five check
  outcomes, the cases left to review, the 10 mm roof tolerance, and the
  null fields of the baseline record.

### Contribution gate, part 2 (2026-11-23)

- **`op_world baseline` and `op_world merge`:**
  - write the first world-log record, and merge (or take down) a change
    set into a world store with its merge record and check report;
  - a merge reruns the gate, needs a review judging the change better and
    none blocking, and checks that the log ends at the store's world;
  - nothing is written unless all of that holds;
  - new objects are copied in, and `world.json` is written last.
- **Tests:**
  - baseline, then merge of the sample facade, then takedown, form a log
    that verifies;
  - the takedown restores the original world id exactly;
  - refused merges leave the store untouched.
- **Two decisions drafted** (*Proposed*, for the maintainer):
  - [ADR-0009](adr/0009-simulation-based-gate-checks.md): run G9
    (walkability) and G10 (replays) in `op_replay`, with no new crate;
  - [ADR-0010](adr/0010-hosting-the-canonical-world.md): the world log in
    its own Git repository, objects in an S3-compatible bucket. The
    provider is the maintainer's choice.

### Contribution gate, part 3: the simulation checks (2026-11-23)

ADR-0009 was accepted (option A, `op_replay`).

- **`op_sim::probe::ProbeWorld`:** the player capsule against static
  colliders. A point is blocked if the capsule overlaps something, or if
  it is enclosed (rays in four horizontal directions and up all hit within
  30 m).
- **`op_replay check-change <world> <bundle> --replay <file>...`:** the
  complete gate.
  - **G9:** probe points on the transport ribbons within 2 m of the change
    must not become blocked.
  - **G10:** registered replays within 160 m of the change must still
    reach every waypoint (one per second) within 2 m.
  - With these, shell and terrain contributions can pass the gate.
- **Tests on the fixture tile:**
  - the sample facade gets exactly the committed report (G9 and G10 do not
    apply to facades);
  - removing the house the sample walk runs into fails G10 (5.6 m off the
    path at tick 300);
  - removing a building away from the path keeps every waypoint;
  - a 3 m block placed on a street fails G9 (6 of 13 probe points newly
    blocked).

### Facade contributions: tool and guide (2026-11-23)

- **`tools/contrib`** (Python):
  - `reference` finds the building at a longitude and latitude and exports
    its shell as a standard `.glb` to model on, in the building's own
    frame;
  - `facade` turns a model exported from a 3D editor into a contribution
    bundle. It applies node transforms, snaps to millimetres, maps
    materials to the palette and writes the manifest. The licence is
    CC BY-SA 4.0, plus swisstopo's terms with `--derived-from-shell`. The
    tool refuses facades derived from OpenStreetMap shells.
  - It is deterministic: the same model gives the same bundle.
- **[docs/contributing/facades.md](contributing/facades.md):** the rules,
  then picking a building, modelling in Blender, exporting, converting,
  checking with the gate, and previewing by merging into a private copy of
  the world. Submission opens with ADR-0010.
- **Checks:**
  - 9 Python tests (glTF node transforms, palette mapping, bundle rules,
    determinism, refusals);
  - CI's `tests` job now runs the tool on the sample model and the full
    Rust gate on its output;
  - locally, a bundle made by the tool for the Grossmünster tile passes
    the gate.

### Corner oscillation fixed, simulation contract 0.2.0 (2026-11-23)

- **The problem** (ADR-0004's known limitation): where a wall meets
  ground too steep to climb, Rapier's character controller slid along one
  surface, then the other. Sprinting into the corner at the top of the
  lane east of the Grossmünster moved the player about 2 cm back and forth
  in 174 of 200 ticks.
- **The rule** ([ADR-0011](adr/0011-stop-corner-oscillation.md),
  *Proposed*):
  - a tick whose horizontal move points against the input is resolved
    again with only the vertical part of the input, so the player stays
    put horizontally but still falls and collides;
  - a descending slide down too-steep ground is exempt.
  - The first version simply zeroed the horizontal move and kept the
    vertical one. That could apply a vertical move the controller had
    computed for a different path, so it was replaced before commit.
- **`SIM_CONTRACT_VERSION` 0.2.0.**
  - `fixtures/replays/sample.oprep` is re-recorded. It still lands, hits
    the wall (86 ticks blocked), jumps (42 ticks airborne) and climbs
    2.47 m. The final state is now
    `212bd353e656f51dbff126640d3e6333ec1e1cd19c57e76bbd9cb5aa0c85ad3b`.
  - Replays recorded with 0.1.0, including walks recorded in the client,
    are refused: "recorded with simulation contract 0.1.0, this build
    implements 0.2.0".
- **Tests:**
  - `op_replay` `corners`: pushing into the fixture corner for 10 s gives
    0 reversals in the last 200 ticks (174 before), and the player then
    walks more than 2 m out of the corner.
  - `op_sim`:
    - a wall beside a 60° slope (6 reversals without the rule, 0 with it);
    - a wedge of two walls;
    - sliding along a single wall at 45°, which must keep its speed.

| Check | Result |
|---|---|
| `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` | pass |
| `cargo test --workspace` | 84 passed |
| `python -m pytest` | 91 passed |
| `op_replay check fixtures/replays/sample.oprep`, debug twice and release once | 20/20 checkpoints, final `212bd353…` |
| Rapier canary (release) | pass, unchanged |
| Sample facade, `op_world check-change` | passes, same report `2fc732d4…` |
| Tool-made facade, `op_replay check-change --replay sample.oprep` | passes the gate |
| `python tools/ci/check_deps.py` | pass |
| The 4 new tests with the rule disabled | the slope test and `corners` fail; the wedge and slide tests pass (they guard behaviour the rule must keep) |

### Client: realism and fast travel (2026-11-24)

**Realism**
([ADR-0012](adr/0012-client-realism-from-procedural-detail.md),
*Proposed*). Everything is drawn by the client; the world data and the
simulation are unchanged.
- **Light:**
  - Bevy's physically based atmosphere: sky, sun disc, haze with distance,
    sky light;
  - the sun at its real position over Zurich for the date and Swiss time
    (NOAA algorithm, tested against the solstices);
  - exposure that follows the sun;
  - ACES tonemapping, picked over TonyMcMapface and AgX on screenshots of
    the same street;
  - 4096-texel cascaded shadows, ambient occlusion and temporal
    anti-aliasing.
- **Surfaces:** 15 procedural textures generated at start-up (0.9 s), each
  with mipmaps, a normal map and a roughness map; no image files.
  - **Walls:** three window styles and three matching ground floors (shop
    fronts, doors and windows, rusticated stone). Each wall gets a whole
    number of bays, and storeys count from the terrain in front of the
    wall, so ground floors sit at street level on slopes. Buildings with a
    contributed facade get plain plaster.
  - **Roofs and ground:**
    - clay tiles in rows down the slope;
    - granite setts on flat open ground, grass on slopes;
    - cobbles on paths, asphalt on roads, stone on bridges.
  - **Variety:** eight plaster colours and four roof variations, chosen
    per building from its id.
- **F2** shows the plain palette colours of the data; **F4** switches to
  lighter effects for slower GPUs.

**Fast travel**
([ADR-0013](adr/0013-travel-by-starting-new-sessions.md), *Proposed*):
- **V** flies. Speed is set with the mouse wheel; Enter lands below.
- **M** shows the whole district from above; a click goes there.
- **1 to 9** go to named places from `worlds/<name>.json`: nine squares and
  streets of the old town, and three spots in the fixture, including the
  lane to the ADR-0011 corner. Their positions come from OpenStreetMap
  and are credited in ATTRIBUTION.md.
- **Where you land:** places and map clicks land on open ground, up to
  40 m from the point if it is inside a building, then up to 4 m clear of
  the wall.
- **Sessions:** each arrival starts a new simulation session there, so
  replays stay valid.
- **T, Shift+T** move the time of day; **F1** opens the help; **F3** shows
  frame rate, position and sun.
- **Command line:** `--place`, `--at`, `--time`, `--date`, `--view` and
  `--details` start anywhere, and make scripted screenshots possible.

| Check | Result |
|---|---|
| `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` | pass |
| `cargo test --workspace` | 106 passed (22 new in `op_client`, e.g. the frame matches the ingest's PROJ result at the fixture spawn; ground floors at street level on a slope; a replay saved after travelling checks with `op_replay`) |
| `python -m pytest` | 91 passed |
| `op_replay check fixtures/replays/sample.oprep`, debug twice and release once | final `212bd353…`, unchanged |
| `python tools/ci/check_deps.py` | pass |
| Screenshots with `--screenshot` at Hirschenplatz, Grossmünsterplatz, Lindenhof (14:00 and 20:45 in June), the map and flight | as intended; Hirschenplatz at 169 fps (1280 × 720, RTX 3070, high) |

**Known limits** (ADR-0012):
- churches get house windows, because the shells have no building type;
- the Limmat is terrain in the data and now looks paved; water needs
  data and a world-format change.

### Roadmap and client CI (2026-11-24)

- **[docs/roadmap.md](roadmap.md):** where StreetWide is going (AAA
  open-world quality, at planet scale, from open data), an honest current
  position, the size of the challenge, and milestones M0 to M7 with what
  each opens with, needs, and exit criteria we can check.
  - **Facts:** gathered by six research tracks with sources: a repository
    audit, open building data, terrain, imagery, water and trees,
    planet-scale engineering, the systems of AAA open worlds, and
    open-source precedents.
  - **Review:** four independent reviewers checked the draft for
    repository accuracy, external facts and links, clarity, and strategic
    completeness. They raised 49 issues, all addressed. The most important
    ones:
    - opening to contributions (M1) now comes with the baseline-refresh
      tooling that protects them;
    - the licence question is stated as how the published world meets the
      ODbL, building on ADR-0001;
    - lines that read as decisions are now options;
    - every exit criterion is checkable.
- **CI:** a `client` job builds and tests the client on Windows and macOS.
  Until now it was compiled only on Linux (see the correction in stage 6).
  CI now has 12 jobs.

**Next:** ADR-0010 (hosting) is waiting for the maintainer's choice of
provider. After it come the world repository, the merge bot, a CI workflow
running the gate on contribution pull requests, and an HTTP-backed store
for the client. For realism, the next step is water: options in ADR-0012.

### Water for the Limmat (2026-11-24, branch water, RFC 0003 Draft)

[RFC 0003](../rfcs/0003-water-layer.md) (*Draft*) adds a `water` layer,
and [ADR-0014](adr/0014-water-areas-from-openstreetmap.md) (*Proposed*)
takes water areas from OpenStreetMap and places them as level surfaces.
Both are implemented on the `water` branch while they are open for
comment, as RFC 0001 and RFC 0002 were. The simulation is unchanged.

- **Format:**
  - `water` is the eighth layer: one asset per tile (`tile:17/x/y`), palette
    material `water`, sorted after `transport`;
  - a water asset must have `collider: none`. This is checked in the
    asset-manifest and tile-index schemas, in `op_world`'s manifest and
    tile-index checks, and in the Python tools;
  - the gate (now 0.1.1) fails contributed water with "fix it in the
    source (OpenStreetMap)", and sends a baseline refresh by
    `streetwide:ingest` to review;
  - `contract_version` stays 0.1.0; RFC 0003 leaves a bump to 0.2.0 to the
    maintainer.
- **Ingest:**
  - a second Overpass query, for water areas, with its own cache file. The
    existing query and its cached answers are unchanged;
  - fountains, drinking water, reflecting pools, basins, and covered or
    underground water are left out;
  - multipolygon relations become polygons with holes, nested as deep as
    they are mapped (a pond on an island in a lake is water). A relation
    that reaches the area but gives no area stops the ingest, naming it:
    rings that do not close, a member way without geometry or with a role
    other than outer or inner, or no area at all;
  - outlines are buffered by 1 m. Each water body gets one level per
    swissALTI3D tile: the median terrain height at 2 m grid points at least
    3 m inside the outline, plus 3 cm. Water with no such grid point in a
    terrain tile gets no level there and is not drawn; the report lists it
    under `not_drawn`;
  - the report gives the water area per tile, each level with its spread,
    and the area hidden under terrain: the share of grid points where the
    terrain is more than 10 cm above the water, times the area.
- **Client:**
  - an opaque water material: dark green-blue, water's reflectance (F0
    0.02), low roughness, and a procedural ripple normal map that drifts
    slowly;
  - flat terrain under the water as mapped is drawn 10 cm below the water:
    only at least 1 m inside the water surface, so not on the 1 m strip the
    ingest adds round the shore (across a tile's edge the next tile's water
    carries on). Banks stay as they are, and so do surfaces more than 1.6 m
    above the water (bridges and quays);
  - landings at places and from the map avoid water. A water mesh that
    cannot be loaded counts as dry, as the scene then draws the terrain as
    it is;
  - the help panel shows the OpenStreetMap credit and the ODbL link.
- **Fixtures regenerated** (see [fixtures/README.md](../fixtures/README.md)):
  - `fixtures/world`: 45 assets. The new one is the Limmat (285
    triangles, level 405.743 m). Apart from the tile index, all 88 objects
    it had before are byte-identical: `world.json` and the tile index
    changed, and the water's mesh and manifest were added.
  - `fixtures/replays/sample.oprep`: recorded again. Its 1,200 inputs and
    20 checkpoints are byte-identical; only the world id in its header
    changed.
  - `fixtures/changes/facade-sample`: the facade mesh and manifest are
    byte-identical. `base_world_id`, the change id and the report (ids,
    gate version 0.1.1) changed; every check outcome is the same.
- **The district** (`--world zurich-altstadt`, plus one 0.2 MB Overpass
  answer): 8 water assets, 69,533 m² of water as mapped.
  - **The Limmat:** 405.743 m in swissALTI3D tile 2683-1247, as in the
    fixture. Of the 16,358 terrain samples under it, the 10th percentile
    is 405.692 m; the 90th, 406.620 m, shows the false ramp.
  - **A 15 m² pond** (way 1132023270, `natural=water`) is not drawn: none
    of its grid points lies 3 m inside it.
  - **Hidden water:** 18,584 m² lies under terrain more than 10 cm above
    the water. 12,474 m² of it is in tile 17/68646/45901, all of that
    tile's water: the false ramp, which the client's lowered terrain
    shows.
  - **Lowered terrain:** the client draws 25,009 terrain vertices below the
    water. The highest lies 1.33 m above it, on the false ramp. No flat
    terrain under the water as mapped lies between that and 2.68 m (the
    Rathausbrücke). Bank tops on the 1 m strip, such as the Schipfe
    walkway 1.7 m above the water, stay.
  - 2,988 of the 2,996 objects of the district built before water are
    byte-identical; the other 8 are tile indexes.

| Check | Result |
|---|---|
| `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings` | pass |
| `cargo test --workspace --locked` | 126 passed (20 new: water in the schemas, the gate and the fixture; the client's water surface, lowered terrain, ripples and landings) |
| `python -m pytest` | 153 passed (62 new: water selection, assembly, levels, report and meshes on synthetic data; the Overpass water query and its cache; the schema rules; the fixture's water asset) |
| Mutation checks after review | each of 13 mutations of the kinds reviewers found (a mean for the median, a kept `water` value dropped, member ways skipped, no repair, no nesting, hidden area per grid point, the old level fallback, the water query and its cache) fails a Python test; undoing either client fix fails its new test |
| Fixture regenerated from cache after review | byte-identical, same world id; the sample replay and the facade sample regenerate byte for byte |
| `op_replay check fixtures/replays/sample.oprep`, debug twice and release once | 20/20 checkpoints, final `212bd353…`, unchanged |
| The old sample replay in a copy of the old fixture world, with this build | 20/20 checkpoints |
| Rapier canary (release) | pass, unchanged |
| `op_world validate fixtures/world` | 1 tile, 45 assets, 42,512 triangles, 0 errors |
| Sample facade, `op_world check-change` | passes, report `857c786a…` (G6 left to review, as before) |
| Tool-made facade, `op_replay check-change --replay sample.oprep` | passes the gate |
| Khronos glTF validator | 48/48 glb files valid; the water mesh has 0 errors and 0 warnings |
| `python tools/ci/check_deps.py` | pass |
| `op_world validate data/world/zurich-altstadt` | 30 tiles, 1,491 assets, 1,078,342 triangles, 0 errors |

**Known limits** (ADR-0014):
- players still walk on the river, because the terrain is the collision
  surface;
- along the false ramp the water is drawn correctly, but collision follows
  the ramp, up to 1.3 m above the visible water;
- reflections show the sky but not the buildings;
- the ripples drift in one direction everywhere.

**Open:**
- RFC 0003 (comment period until at least 2026-12-01) and ADR-0014 wait
  for the maintainer, including the choice between keeping contract
  0.1.0 and bumping to 0.2.0.
- Water with no grid point 3 m inside it in a terrain tile is left out, as
  ADR-0014 now says. A fallback level for such narrow water (ponds under
  about 6 m across, slivers at terrain tile edges) would need an amendment,
  for the maintainer.
- Walks recorded in a local zurich-altstadt world built before water no
  longer match the rebuilt one; check them against a copy of the old
  world.

### M2 decisions drafted (2026-11-24, branch m2-decisions, drafts only)

A design for M2's trees and street lights found decisions that must come
first: how the published world meets the ODbL, which licences new sources
may use, where the lighting data's objects stand, how night is exposed,
and what the client may cost. They are drafted as options, each with a
recommendation. **Nothing is decided;** the maintainer decides each one.

| Draft | Question | Recommended |
|---|---|---|
| [RFC 0004](../rfcs/0004-point-features.md) (*Draft*) | how the world format stores trees and lights | `vegetation` and `street_objects` layers of canonical-JSON points documents, one per tile and source; collider `none`; closed to contributions; gate 0.1.3 *(drafted as 0.1.2, which reader fuzzing took)*; the contract version chosen once with RFC 0003; the pedestrian ADR decided first |
| [ADR-0015](adr/0015-zurich-trees-and-lights.md) | where Zurich's trees and lights come from | the city's CC0 tree register and ewz's lighting data; no OSM trees or lamps anywhere in Switzerland; anchors on swisstopo terrain only; a status allowlist for private ground |
| [ADR-0016](adr/0016-swisssurface3d-heights.md) | tree heights | swissSURFACE3D minus swissALTI3D within 1.5 m of the trunk, measured from the anchor; the measured height or null, never a formula |
| [ADR-0017](adr/0017-licence-allowlist.md) | which licences world data and client assets may use | a machine-readable allowlist enforced by the gate, validation and the ingest; NC, ND and redistribution-barred terms refused |
| [ADR-0018](adr/0018-performance-tier.md) | the performance tier | an 8 GB RTX 3060 / RX 6600-class card at 1080p, 99th percentile frame time at most 16.7 ms on high, a `--benchmark` route of three places, and a lower night exposure |
| [ADR-0019](adr/0019-odbl-and-the-published-world.md) | how the published world meets the ODbL | offer the whole world under the ODbL, each manifest keeping its own sources' licences; five questions for the OSMF Licensing Working Group |

- **Facts** gathered on 2026-11-24: the city's WFS services (fetched and
  counted per LV95 kilometre square), swissSURFACE3D heights measured at
  every register tree in the district, the publishers' licence pages, the
  OSMF guidelines, the Steam hardware survey, and the client's code.
- **Review:** two independent reviewers, one for consistency and accuracy
  and one for process and governance, raised 31 findings; 30 were fixed
  and 1 (length) partly. The most important: the drafts left render-time
  uses of OSM to ADR-0019, which did not answer them (now its option B
  reading and LWG question 5); the source archive had been put in the
  world store (now a separate area, RFC 0001 unchanged); changing the
  allowlist would change a merge criterion without an RFC (now two
  drafted ways, delegation recommended); and the reference card had no
  recommendation.
- **Checked by hand** against the code and the documents before commit:
  what the simulation hashes per collider, the serde name the new layer
  would get, the fixture tile, the exposure floor, the shadow cascades
  and cut-off, the STAC item-id pattern, the help panel under
  `--screenshot`, and every cited section and comment period. One figure
  in ADR-0015 was corrected (the bridge rule counts 200 bridge lights,
  not 191). Every relative link and anchor resolves.
- **Also changed:** the ADR and RFC indexes, the roadmap's section 8, a
  pointer in RFC 0003 to the joint contract-version decision, and a note
  in ADR-0010 about the proposed source archive.
- **No code, schema, fixture or replay changes.**

**Waiting for the maintainer, in the recommended order:**
1. ADR-0019, which M1 opens with, and its question to the LWG.
2. ADR-0017, with a short RFC for the G4 and validation rules it needs.
3. The pedestrian ADR (to be drafted), before RFC 0004.
4. RFC 0004 with RFC 0003 (one contract-version choice), then ADR-0015
   and ADR-0016.
5. ADR-0018, including who buys or lends the reference card.

### Water and the M2 drafts merged into main (2026-11-24)

At the maintainer's request, the two open pull requests were merged into
`main` with merge commits, as PR #1 was:
- **#2** (`water`): RFC 0003 (a water layer) and ADR-0014 (water areas
  from OpenStreetMap), with their implementation;
- **#3** (`m2-decisions`): RFC 0004 and ADR-0015 to ADR-0019, retargeted
  to `main` after #2.

Merging is not accepting: RFC 0003 and RFC 0004 stay *Draft*, and ADR-0014
to ADR-0019 stay *Proposed*, until the maintainer decides. Both branches
already contained `main`, so `main` now holds exactly the commit that the
pull requests' CI tested (`0a9de73`, 12/12 jobs). The roadmap's "Now" and
section 8 no longer point at branches.

**For local worlds:** `main` now reads worlds with water. A
`zurich-altstadt` world built before water still opens; walks recorded in
it check against that old world, not a rebuilt one.

### Client: inspect mode (2026-11-24, roadmap M1, newcomers)

The client now shows what you are looking at and whether you can improve
it. Press I, or start with `--inspect`:
- **What it shows:** a crosshair, and what it points at is highlighted as a
  whole: a building (shell and facade together), or one asset (a tile's
  terrain, its streets and paths, its water, or a bridge deck).
  - Amber: a building with no contribution yet, so it is improvable.
  - Blue: a building or asset with a contribution.
- **The panel** names the feature and tile, baseline or contribution and
  by whom, and the licence and credits. Then it says what to do next:
  - for a building, the facade guide and the exact command that exports
    its shell (step 1 of the guide), quoted for PowerShell or Git Bash;
  - for streets, bridges and water, "fix it in OpenStreetMap", with the
    way or relation link;
  - for terrain, that it needs evidence and has no tool yet.
- **A click** (with the mouse captured) prints the same to the terminal,
  with the command on its own line.
- **Where it works:** in Walk and Fly views and during replays; it pauses
  on the map, where the click travels.
- **Baseline or contribution:** read from the manifest the way the gate
  reads it. A `StreetWide contributions` source makes a contribution (G5);
  otherwise generator `streetwide-ingest` means the baseline.
- **Picking:** the client's own ray test against the meshes it draws
  (Bevy's mesh picking cannot see them once they are on the GPU). With the
  whole district loaded (1,491 assets, 1.08 million triangles), a pick
  takes 20 to 30 µs (median) and at most 0.35 ms. It runs only when the
  camera or the scene changes. The grid code that heights, water and
  picking share is now one module.
- **Also:** `--pitch <deg>` sets where the camera looks at start (used for
  scripted screenshots), `/contrib/` is ignored by Git (where the printed
  command writes), and the README and the facade guide describe the key.
- **Nothing in the simulation changed.** Inspect only reads; the sample
  replay still matches every checkpoint, also when played with `--inspect`.

**Reviews:** three reviews (correctness, architecture, UX and docs) found
14 minor issues, all fixed. The final check found the panel's box one or
two lines too short when the command wraps; it now has a definite width.
Screenshots were taken of every case (district buildings, bridge, street,
water, terrain, the map, a replay, the sample facade merged into a copy of
the fixture world).

**Checks:** `cargo fmt --check`; `cargo clippy --workspace --all-targets
-- -D warnings`; `cargo test --workspace` (155 passed, 1 ignored: the
district pick benchmark); `op_replay check` on the sample replay; pytest;
the dependency check. The printed commands were run as printed in
PowerShell and Git Bash.

**For the maintainer:**
- **Buildings outlined from OpenStreetMap** (10 in the district) are amber
  and get the command, with a note to model the facade from your own
  observations and use the ODbL shell only to place it (never
  `--derived-from-shell`). The alternative is a link to OpenStreetMap only.
  This touches ADR-0019.
- **A manifest that is both** (generator `streetwide-ingest` and a
  contributions source) shows as a contribution. The gate does not refuse
  it; the baseline refresh design treats it as a conflict.

### Reader fuzzing (2026-11-24)

Roadmap M1 asks for fuzzing of the strict file readers before announcing.
Every reader of untrusted bytes is now fuzzed in CI, and what the fuzzing
and two adversarial reviews found is fixed. Valid input is unchanged.

- **What is fuzzed:**
  - Rust (`op_world`, `op_replay`): canonical JSON and every document
    type, glb meshes, ids, tile keys and store paths, store reads,
    validation of a whole world, the gate on contribution bundles, the
    world log and replays;
  - Python (`tools/worldfmt`, `tools/contrib`, `tools/ingest`): canonical
    JSON, schemas, store reads, the glTF importer and the facade tool on
    crafted worlds, Overpass answers for roads and water, OSM and water
    parsing, STAC searches, swissALTI3D GeoTIFFs, swissBUILDINGS3D CityGML
    and the derived building cache.
- **How:** one small mutation fuzzer per language, seeded and
  deterministic. Each input follows from the seed, the test and the case
  number, so every failure reproduces.
  - No new dependencies: stable Rust without a new crate, and Python's
    standard library.
  - A failing input is shrunk and kept as a regression test.
  - Memory: on Linux, each Rust target runs alone in a child process that
    checks what every input takes. Python measures with tracemalloc, and
    on Linux with the process's peak memory for the readers that work in
    GEOS.
  - CI runs them in `cargo test --workspace` and `pytest`.
    [CONTRIBUTING.md](../CONTRIBUTING.md#fuzzing) gives the settings and
    how to run a long campaign.

**Bugs fixed: 60**, 14 in Rust and 46 in Python:

| Reader | Fixed | The most serious |
|---|---|---|
| Canonical JSON | 4 | Python accepted documents nested deeper than `op_world` accepts |
| Schemas and ids | 4 | a list of 60,000 mixed items took over 30 s to validate |
| Stores and download caches | 5 | a pipe, or a link to `/dev/zero`, in a store blocked a reader or made it read forever |
| glb (`op_world`) | 2 | a 37 KB file made the reader copy 786 MB; a 66 MB file with one long JSON chunk took 1.1 GB |
| glTF importer | 6 | 60 nodes sharing their children gave 2^60 paths to walk |
| Facade tool | 6 | **path traversal:** a crafted world's feature id put the exported shell outside the output directory |
| Validation | 1 | a mesh listed 4,000 times was read 4,000 times (16.6 s) |
| Gate | 6 | **the G8 hang:** a contributed shell triangle 2,000 km across meant 10^12 grid cells |
| Replays | 1 | a 349-byte file reserved 41 MB |
| Overpass, OSM, water and bridges | 9 | GEOS took 2.5 GB on a way that retraces its outline; one bridge solve took 7.2 GB |
| STAC | 5 | **path traversal:** a crafted asset name put a download outside the cache; a 3,000-ring footprint took 3 GB in GEOS |
| GeoTIFF | 3 | two tiles 1,000 km apart asked for 29.1 TiB |
| CityGML and its cache | 8 | one roof ring retraced 6,000 times kept the triangulation busy for over 40 s |

Four of the 60 were mistakes in earlier fixes, caught by the reviews: G8's
first step limit refused valid facades on dense roofs, G8 skipped
canonical shells over the shell budget, the traversal fix renamed the
export of some valid feature ids, and refusing links in stores also
refused links in the download cache.

- **Limits now in the Draft RFCs**, to be confirmed with them:
  - RFC 0001 section 5: canonical JSON nests at most 127 levels, the
    limit of serde_json, now checked explicitly by both implementations;
  - section 6: a glb is at most 64 MiB, and its JSON chunk at most 1 MiB;
  - section 7: readers take `world.json` and objects only as regular
    files of at most 64 MiB, never links, devices or pipes;
  - RFC 0002 G8: a facade over its G7 budget, or on a shell that the same
    change set proposes over the shell budget, is `not_run`. Measuring
    stops at 20,000,000 grid steps or 100,000,000 vertex-to-triangle
    distances, and the facade then fails as too complex to check.
- **Other limits**, in the code and far above real data: OSM ways of
  2,000 nodes (OpenStreetMap's own limit), CityGML rings of 100 points and
  buildings of 16 MiB of XML, STAC footprints of 1,000 points and 100
  result pages, DEM mosaics of 400 million pixels, and work budgets for
  the glTF importer.
- **Gate 0.1.2.** The gate's output changes for some change sets, so its
  version changes. G8 is `not_run` for meshes that G7 refuses, or fails
  as too complex beyond its step limits. Links, devices, pipes and files
  over 64 MiB are refused before reading. glb JSON chunks over 1 MiB and
  JSON nested deeper than 127 levels get new errors. A detail that breaks
  the text rule is repaired, where before no report could be written.
  - The sample report changes only in `gate_version`: report `e5dee148…`.
  - RFC 0004's gate version moves to 0.1.3.
- **Valid input is unchanged:**
  - a re-ingest of the whole district with the final code, offline from a
    fresh copy of the cache, is byte-identical to the stored world:
    `blake3:4689c972…`, 3,013 files. The rebuilt CityGML cache (version 2)
    is byte-identical to version 1;
  - the sample report is byte-identical apart from `gate_version`;
  - the glTF importer reads all 1,491 district meshes as before;
  - G8 decides exactly as before (57d7563) whenever it stays within its
    limits. 80,000 random shells shaped like buildings (several nodes,
    roof first, rotated or on cell lines, some degenerate triangles)
    with 14.5 million facade vertices, many exactly 0.5 m from the shell,
    10 mm above its top or on cell lines, give the same counts and worst
    distances, bit for bit.

**G8 under attack** (the final check):
- **Real shells stay below the limits.** A facade at the G7 maximum (two
  kinds of 49,800-triangle skin) on the sample building and two of the
  district's largest shells, with roofs or walls divided into up to
  87,348 triangles, takes at most 722,044 grid steps and 2.3 million
  distances, in under half a second. Reordering its vertices to defeat
  G8's cache gave at most 6.6 million distances. Even with no cache hit
  at all, none would go over 91 million.
- **The review's dense roofs now pass.** The skin on the sample building
  with its roof in 19,320 triangles, refused before, takes 120,378 grid
  steps and 289,058 distances; the gate passes it in 0.7 s (debug). The
  four variants of shell `ac8fa799` that took 7.3 to 211 million steps
  take 0.26 to 1.4 million (grid steps and distances together).
- **Crafted shells can still reach the limits**, as intended. One 100 m
  wall listed 20,000 times (40,001 triangles) under a skin facade fails
  as too complex after 0.3 s, where 57d7563 would pass it after about two
  minutes. 99,000 stacked triangles in front of nine anchors refuse a
  facade of 1,800 vertices that 57d7563 passes in 1.9 s.
- **The most one facade can cost:** about 2 s and 120 MB at both limits.
  A change set may hold 50 changes, so a crafted one can keep the gate
  busy for a minute or two.

**Reviews:** two adversarial rounds, then a final check.
- **First round** (fixes and behaviour, harness quality): 18 findings, 17
  fixed as proposed. G8 got its column index, its step limit and the
  `not_run` rule, after one bundle took 2 min 37 s. Rust reads got the
  file rules and the glb JSON cap, validation reads a repeated object at
  most twice, and the branch moved onto `main`, so the water reader was
  hardened and fuzzed too. A counting allocator would need `unsafe`, so
  the Rust memory check runs in child processes instead.
- **Second round** (Rust gate and readers, Python on the whole district,
  harness and CI): 23 findings, 1 blocking. 20 were fixed, 2 are drafted
  below, and the documentation is this entry and CONTRIBUTING.md. G8 now
  searches 2 m height buckets with a small cache: 28 to 150 times fewer
  steps on the review's dense roofs. The Python memory check sees GEOS on
  Linux, a hang's stacks reach the CI log, and CityGML buildings over
  16 MiB are skipped and counted instead of failing the sheet.
- **Final check:** every check below, the district re-ingest, and the
  attack on G8 above. Nothing new to fix.

| Check | Result |
|---|---|
| `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings` | pass |
| `cargo test --workspace --locked` | 202 passed, 1 ignored (the district pick benchmark) |
| `python -m pytest` | 332 passed, 5 skipped on Windows (links, pipes, Linux memory readings), 70 s; 337 passed on Linux (Python 3.13, Docker) |
| `python tools/ci/check_deps.py` | pass |
| `op_replay check fixtures/replays/sample.oprep` | 20/20 checkpoints, final `212bd353…`, unchanged |
| `op_world validate`, both fixture worlds | 0 errors, `fd6446c2…` and `f2d6f869…` as before |
| Sample facade, `op_world check-change` | passes; the report is byte-identical to the committed one |
| Tool-made facade, `op_replay check-change --replay sample.oprep` | passes the gate |
| `op_world validate data/world/zurich-altstadt` | 30 tiles, 1,491 assets, 1,078,342 triangles, 0 errors |
| District re-ingest, offline | byte-identical, 3,013 files, 295 s |
| G8 against 57d7563 | 80,000 random cases, 0 differences |
| Long campaigns, final code | Rust at 20 times the CI budget (`STREETWIDE_FUZZ_ITERS=40000`) on two seeds, 2.1 million inputs each; Python at 10 times on Windows and Linux (20 times after the first review); no failure |
| `git diff --check`, CR scan | clean |

**Linux-only Rust tests** (the child process's memory check, its
self-test, and the link tests) cannot run on Windows. They passed in the
branch's first Linux CI run, with all 12 jobs green.

**For the maintainer:**
1. **JSON size limits** ([RFC 0001](../rfcs/0001-world-format.md),
   unresolved questions): any document up to 64 MiB is parsed whole, and
   a crafted one can take about 7 GB. Recommended: limits per kind of
   document, checked before parsing.
2. **Repairing invalid water rings**
   ([ADR-0014](adr/0014-water-areas-from-openstreetmap.md), open question
   1): `make_valid` is unbounded; a crafted 200-point ring takes 103 s.
   Recommended: count where a ring touches itself and stop at 1,000
   pairs. OpenStreetMap building footprints have the same question.
3. **The Draft RFC text** the limits added: RFC 0001 sections 5 to 7, and
   RFC 0002's G8 row and its question 8 (no triangle budget for baseline
   shells; recommended: leave it until real data comes near the budget).
4. **Gate time per change set** (RFC 0002, question 2): G8's limits are
   per facade, so a crafted change set of 50 facades can keep the gate
   busy for a minute or two. Recommended: leave it until the gate runs as
   a service; a budget shared by the change set would make one facade's
   outcome depend on the others.

### Baseline refresh tool (2026-11-25, branch refresh, ADR-0020 Proposed)

Roadmap M1's baseline refresh: newer open data can now enter the
canonical world as change sets authored by `streetwide:ingest`, planned
from a fresh ingest and checked through the real gate
([RFC 0002 section 7](../rfcs/0002-contributions-and-merge-rules.md#7-baseline-refreshes-and-takedowns)).
[ADR-0020](adr/0020-baseline-refreshes.md) (*Proposed*) records every
choice with its options. **Nothing is decided:** where a choice is open,
the tools apply the ADR's recommended option as a default. Maintainers
follow [docs/contributing/baseline-refresh.md](contributing/baseline-refresh.md).

- **Drafts:** ADR-0020; the refresh rules of RFC 0002 section 7, as a
  clarification proposed with the ADR (the RFC stays a Draft); RFC 0002's
  unresolved questions 9 to 16, which are amendments A1 to A8; and a
  note in ADR-0010 (*Proposed*) on the local log layout, with `<hex>`
  paths and `worlds/`.
- **`tools/worldfmt`:** `changes.py`, the change-set rules and `apply()`,
  which builds the world a change set produces, byte for byte as the
  gate does; `log.py`, the world log and its rules; a strict reader for
  the glb profile.
- **Rust:** `op_world::merge::merge_with` takes the gate as a parameter.
  `op_replay` gains `merge` (the full gate, so that sets touching
  colliders can be merged), `rerecord` and `compare-worlds`. Both merge
  commands gain `--log <dir>`, which keeps the log complete, and an
  opt-in `--require-base`. `op_world baseline` gains `--log`.
- **`python -m tools.ingest.changes plan`:** compares a fresh ingest
  with the canonical store and its log. It writes change sets of at most
  50 changes, ordered for merging, and a refresh report in canonical
  JSON (with its own schema) and Markdown, with the commands for each
  set and an overrides template. The hold rules H1 to H14 list every
  conflict and held-back change with all of its reasons. An overrides
  file gives holds, scoped accepts and removal allowances, each with a
  reason. An accepted change to a contribution goes into a decision set
  of its own.
- **`verify`:** runs every set through `op_replay check-change`, in
  order, on a work store of hard links. It checks each gate report
  (base world, predicted world, passed, G3 `review`, G4 and G5
  `not_applicable`), applies the set, re-records the replays, and checks
  every id. At the end it runs `op_world validate` and compares the
  world before the refresh with the final one, for review. `--replan`
  plans again and compares every file; `--report` writes a summary in
  canonical JSON.
- **CI:** the determinism matrix checks the pinned ids of the fixture
  plan and of the pre-water fixture world on Linux, Windows and macOS.
  The `tests` job runs the gate tests with `STREETWIDE_GATE=1`, with the
  opt-in comparisons of `tools/worldfmt` against `op_world`.
- **Docs:** the maintainer guide, a section in the facade guide ("When
  the building changes upstream"), and notes in the README,
  CONTRIBUTING.md and `tools/ingest`'s docstring.

**Defaults the tools apply until the maintainer decides** (ADR-0020,
sections 1 to 9): rule C for what changed (Q1); the generator, the
contributions source and the log together for who made an asset (Q2);
adds and removals with their holds (Q3, Q4); the log for removed keys
(Q5); the shell under a facade held, with no override (Q6); a move as a
removal and an add in one set (Q7); overlapping changes as one unit
(Q8); terrain whose edges move held (Q9); conflicts only when upstream
changed (Q10), listed in the report (Q11); draped assets over
contributed terrain held (Q17); per-set judging plus the whole-refresh
comparison (Q19); and the planner approving while there is one
maintainer (Q22). For Q18 the tools apply (a), stricter than the
recommended (b): nothing lifts a takedown.

**Pending:** ADR-0020 itself and the section 7 clarification; the
amendments A1 to A8 (RFC 0002 unresolved questions 9 to 16), including
A7, which would make `--require-base` mandatory for refreshes; the
ADR-0010 note; the two rules ADR-0020 proposes for the ingest (bump its
`VERSION` when its output changes; refresh on the platform of the last
baseline); and the ingest options Q20, Q21 and refreshing one source at
a time.

| Check | Result |
|---|---|
| `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings` | pass |
| `cargo test --workspace --locked` | 247 passed, 1 ignored, with main's reader fuzzing merged in: among them the log and its locks, `--require-base`, `op_replay merge`, `rerecord`, `compare-worlds`, and the refresh's fuzz targets and regression tests |
| `python -m pytest` | 763 passed, 95 skipped (the opt-in tests below, links and FIFOs where Windows cannot make them, and one that needs a fresh district ingest) |
| `STREETWIDE_GATE=1`, CI's gate step (`test_changes_gate.py`, the fixture plan, and `tools/worldfmt`'s `test_changes.py`, `test_log.py` and `test_glb_reader.py`) | 303 passed in 61 s: the refresh sets pass the full gate with the predicted ids, and the Python change-set, manifest, log and glb rules and the terrain-edge count give op_world's verdicts and messages |
| Fuzz campaigns, `STREETWIDE_FUZZ_ITERS=20000`, `STREETWIDE_FUZZ_SEED=0xBADC0DE` (Windows) | Rust, the four refresh targets: no failures (merge-args 200,000 cases, require-base 20,000, log-open 15,000, log-merge 4,000; 206 s). Python: `test_fuzz_refresh_readers.py` 39 passed (294 s), `test_fuzz_changes_readers.py` 23 passed, 1 skipped (253 s) |
| CI on the branch (Linux, Windows, macOS) | all 12 jobs green. On Linux, with the tests that need links and FIFOs: `cargo test` 249 passed, 1 ignored (`fuzz_readers` 34); `pytest` 788 passed, 70 skipped; the gate step 302 passed, 1 skipped (a Windows junction case) |
| Fixture water refresh, pinned | change `49650252…`, report `08f8cb7d…`; from the pre-water world `6af9f186…` (rebuilt from `fixtures/world`, as committed at `c16572a`) to `fd6446c2…`, which is `fixtures/world`; `verify` passes, `--replan` byte-identical |
| Synthetic refresh over two tiles | a ribbon, a shell replace, a move to the other tile, a 1-to-2 split and a removal give 2 sets; both pass the full gate (G9 with 3 probe points, G10 with a recorded walk; the whole-refresh comparison probes 22) and every id matches; two runs give the same summary bytes |
| Terrain-edge mirror | a held terrain change forced into a bundle fails G8 with 3 edge vertices, as the planner counted |
| A contribution's removal | after the far-building removal of `full_gate.rs` is merged with `op_replay merge --log`, planning gives a `removed-by-contribution` conflict, not an add |
| District, water added and lost (local data) | 1 set of 8 adds, ending at the district's own id `4689c972…`, passes `op_world`'s gate; the reverse stops with an error, and with an allowance gives 8 removals: `verify` passes (3,012 objects linked, final `30c4e880…`, 30 tiles, 1,483 assets, 0 errors; 14 s with `--replan`) |
| District, fresh OpenStreetMap snapshot | nothing to refresh: 0 changes, 76 provenance-only differences kept |
| The guide's commands, on the fixture | baseline with `--log`, plan, verify, check, merge with `--require-base --log`, re-record in place; planning again gives "nothing to refresh", and merging the old set again is refused |
| `op_replay check fixtures/replays/sample.oprep` | 20/20 checkpoints, final `212bd353…`, unchanged |
| `python tools/ci/check_deps.py` | pass |

**Found on the way:**
- **Adding back the far building fails G9.** Removing it passes the full
  gate, but an accepted add of the same building fails: 3 of 22 probe
  points of a mapped way are newly blocked. This is amendment A4's case,
  a building over a way that OpenStreetMap still maps. A test now
  records it.
- **Windows paths.** A plan's object paths are about 100 characters
  long, and Windows refuses paths over 260 characters unless long paths
  are enabled. The guide says to keep plan and scratch directories
  short.

**After three reviews** (2026-11-25), fixed with a test each:
- **A tile emptied between sets.** Sets are merged one after another, so
  `would-empty-tile` now also looks at each set in merge order. Before,
  a set that removed a tile's last asset, with the new one coming in a
  later set, stopped the planner with an internal error.
- **Who dates the ingest.** Only assets made by the ingest (manifest and
  log agree) set the version a new store must reach. A contribution that
  names the ingest's generator no longer stops every refresh, and a
  version with a digit that is not ASCII no longer crashes.
- **Draped changes over a contributed terrain that stays** are held
  (Q17), also when upstream leaves that terrain as it was.
- **One merge at a time.** Both merge commands hold `merge.lock` in the
  store, and with `--log` `log.lock` in the log. A merge reads the
  store's world again before it writes, and refuses if it moved.
  `WorldLog::open` checks every change set, as the planner does.
  `op_replay merge` refuses a replay recorded in another world, which
  G10 would skip.
- **Rules made stricter:** `possible-duplicate` also holds a replace that
  grows over a removed or contributed feature; `spawn` looks at the
  player's whole capsule, and at a collider switched on in a manifest
  alone; `mass-removal` counts the removals that a unit's adds, moves and
  replaces do not account for, so 49 buildings replaced by one count 48.
- **Robustness:** manifests are read with op_world's rules beyond the
  schema, and a new mesh must match its manifest's size and bounds; the
  overrides file is identified by its document in canonical JSON, so
  line endings do not change a plan; numbers above 2^53 - 1 are refused
  as input errors; a plan that cannot be written leaves nothing behind;
  output that the console cannot encode is escaped; `verify --report`
  never writes through a hard link; an `op` of the wrong JSON type is
  reported instead of crashing; the baseline hint names `--log`.
- **Docs:** the guide asks for a `CANONICAL` marker in the store; RFC
  0002 section 7 says how a refresh is split and lists every conflict;
  code and CI cite ADR-0020 instead of the design notes outside the
  repository.

**Merged with main's fuzzing** (2026-11-25): the refresh's readers now
meet main's standard for files from elsewhere (CONTRIBUTING.md,
"Fuzzing"). Three more reviews followed; each finding fixed has a test
that fails without its fix.
- **Bounded reads:** every file the refresh tools read is a regular file
  itself, of at most 64 MiB (replays: op_replay's own limit), read with
  `read_file` in Rust and `store.read_file` in Python. Links, FIFOs and
  devices are refused unopened. Nothing inside a world log may be a link
  (Windows junctions included), and neither may a plan's `sets/<k>` or
  its `objects/`. Writes go through a new temporary file, never through
  a link, and leave none behind when they fail.
- **The glb reader:** Python refuses what `op_world` refuses, in its
  order and words: the JSON chunk's length and nesting, node i showing
  mesh i, accessors that read the BIN chunk twice, numbers no float
  holds, and node names quoted as Rust quotes them. With
  `STREETWIDE_GATE=1`, 42 crafted meshes go through `op_world
  check-change` and the Python reader: the same verdicts, and the same
  messages for all but the 7 that are invalid JSON.
- **New fuzz targets:** in Rust, world-log directories, merges with
  `--log` on crafted logs, `--require-base` on crafted bundles and the
  merge arguments; in Python, the glb reader, change sets, the world log,
  objects read from a directory, `apply`, and the planner's overrides,
  plans, stores and gate reports. Besides the earlier campaigns (default
  seed and `0xC0FFEE`, Linux and Windows), each ran once more with
  `STREETWIDE_FUZZ_ITERS=20000` and `STREETWIDE_FUZZ_SEED=0xBADC0DE`.
- **Merging:** an empty path (`--log ""`) skipped the locks, so two
  merges from one store both went through; it is now refused, and the
  library takes the lock in the current directory. The locks are held
  until the record is appended. A log that cannot take the record (a
  link inside it, a file with other bytes) is found before the store is
  written. A merge that stopped between the store and the log is named
  by the next one, with how to put the store back, before
  `--require-base` calls the plan stale. The candidate world is checked
  against the gate's before anything is written, in a scratch directory
  the merge makes itself. `op_world id` and the store's writes follow no
  link.
- **Planning:** what is draped over a contributed terrain waits for that
  terrain's decision set, which merges last and only if judged better
  (Q17); `would-empty-tile` holds one set's removals at a time, then
  packs again; `mass-removal` counts per-tile removals too (Q4); `spawn`
  measures the ground under the feet, tests meshes triangle by triangle
  (a building enclosing the spawn counts, a courtyard around it does
  not) and holds a removal of the ground under the spawn. The world log
  reads each change set once: 400 records naming one 55 KB change set
  took 45 s, and now take 2.6 s.
- **Left to the maintainer** (ADR-0020, "Left open by this ADR"): an
  exit code of its own for a merge that wrote the store but not the log;
  flushing merges to disk against power loss; a size limit for JSON
  documents and the reviews of a record. The report's
  `spawn_ground_gap_mm` still measures the highest vertex near the
  spawn, since measuring under the feet changes the fixture report's
  pinned id.
- **Tests clean up after themselves:** `op_world`'s test helper and
  `op_replay`'s `full_gate.rs` left their temporary directories in the
  system's temporary folder, about 100 to 300 MB a run (674 MB had
  built up here since 2026-11-23). A guard now deletes each one at the
  end of its test.

**Out of scope:** no gate rule, `GATE_VERSION`, schema,
contract version or merge rule changed, and A1 to A8 are only drafted;
`--require-base` stays opt-in. The 50-change cap stays. No ingest change
(Q20, Q21, refreshing one source at a time), and no edit to an accepted
ADR: ADR-0006 is unchanged. Hosting, submissions and the merge bot
(ADR-0010), scheduled refreshes (M3) and the migration tool to a new
world-format version are not started.

### Pedestrian ADR drafted (2026-11-25)

[ADR-0021](adr/0021-pedestrians.md) drafts the pedestrian decision that
M2 opens with and that RFC 0004 asks to settle first. **Nothing is
decided;** the maintainer decides. It is numbered 0021 because ADR-0020
(baseline refreshes) is drafted on branch `refresh`.

| Option | Recommended |
|---|---|
| A. In the client only, not colliding | no; seeded, the fallback if C2's prototype fails |
| B. Deterministic agents in `op_sim`, recorded in replays | no |
| C1. A near population in `op_sim`, ambient people in the client | left to M4 and M5 |
| C2. People in the client from a shared seed | **yes, for M2** |
| D. Not in M2 | no |
| E. Pedestrians as Rapier bodies | rejected by measurement |

- **The recommendation:** the client draws people from a population
  that is a pure function of the world's lanes, places and population
  profiles, the session seed, the crowd's clock (a full date and time,
  plus ticks), the population's version and the character-asset
  catalogue. People stay outside the simulation's state, schedule and
  hash and do not collide, so trunks and poles stay visual (RFC 0004 as
  drafted), and navigation data becomes a world layer that the client
  reads, shaped to become simulation input later. At a pinned crowd
  tick, screenshots and benchmark stops show the same people on every
  machine; in M5, every player sees the same people if the sessions
  agree on the crowd's inputs. A seeded A gives the same on one machine,
  so C2's larger benefits arrive in M4 and M5, and its risk, an
  unproven look, in M2.
- **What the crowd must do:** trips that start and end only at places
  in the world data, never at the edge of the area the client works
  out; people who avoid each other, sit and stand; speeds by trip
  purpose, group and steps; a month factor in the profiles; levels of
  detail; and pass-or-fail checks for a client prototype, which comes
  first.
- **Evidence,** gathered on 2026-11-25 in three research notes and a
  prototype, none of them in the repository; the ADR cites the sources:
  - **open-world games:** tiers everywhere (Assassin's Creed Unity: 40
    full AIs and 10,000 crowd NPCs on screen, the far ones on fixed
    paths); crowds online owned by a client or a server; For Honor's
    hundred soldiers in a deterministic simulation with rollback; no
    published game with rollback networking and hundreds of ambient
    pedestrians;
  - **crowd models:** following a graph with separation needs only the
    simulation's basic arithmetic, but lets bodies overlap in dense
    head-on flows; social force diverged at 60 Hz; no crowd crate fits
    `op_sim` as it is; `f32::min` and `max` do not fix which zero they
    return, which ADR-0003 does not cover;
  - **open data:** OSM's paths are nearly complete in the district, but
    the ingest keeps no walking graph; hourly counts exist for
    Bahnhofstrasse (licence version unclear) and, under CC0, for the
    Limmatquai until 2024; VBZ publishes boardings per stop and trip,
    as computed averages; Bahnhofstrasse had 1.4 to 2.6 times as many
    people at 18:00 in December as in June.

**The prototype:** pedestrians in a module written like `op_sim`'s, on
the district's walk graph from the ingest's OSM cache (59.98 km): graph
and separation (a), graph and ORCA (b), and capsules moved by `op_sim`'s
character controller against the real shells of 4 tiles (c). AMD Ryzen
9 3900 shared with other work, Windows 11, Rust 1.100.0, release
build, one thread, 3,600 measured ticks, every configuration run twice,
each tick hashed the way `op_sim` hashes the player. The worst-case
rollback re-simulates 8 ticks on every frame.

| Variant | Agents | Per tick, median | Worst rollback frame, median |
|---|---|---|---|
| a | 500 | 0.12–0.13 ms | 1.15–1.16 ms (6.9% of 16.7 ms) |
| a | 2,000 | 0.52–0.53 ms | 4.9–5.2 ms (29–31%) |
| a | 10,000 | 3.04 ms | 28.6–29.5 ms |
| b | 2,000 | 0.62 ms | 6.4–6.6 ms |
| c, 4 tiles | 100 | 3.6–3.9 ms | 31.6 ms; 423 of 28,800 re-simulated ticks gave another hash |

At the 99th percentile, which ADR-0018 budgets, the worst rollback
frames cost 1.5 to 1.8 times these medians (7.8–8.0 ms, 47–48%, for
2,000 agents of a).

- **Hashing** was about two thirds to four fifths of a's tick (64 to
  79%). A hasher that buffers the same bytes would keep every hash and
  cost about an eighth as much from 2,000 agents up (a quarter at 100).
  ADR-0003's rule 9 also hashes every re-simulated tick, so the ADR
  gives its rollback estimates with and without that.
- **Determinism:** every configuration gave the same hash chain in both
  runs, and a Linux build matched Windows. Only c broke exact rollback,
  which the prototype traces to Rapier's broad phase. arm64 and macOS
  were not tested.
- **Paths and shells:** in the 4 tiles, 269 of 1,120 graph edges have
  at least one point where the capsule overlaps a shell. Part of it is
  by design (paths that end at doors, building passages), so lanes must
  be matched to the shells, keeping those, before anyone walks on them.
- **Review:** three reviews of the draft (architecture, a player's
  view, sources) were applied on 2026-11-25. Among the changes: the
  rollback costs under rule 9 and at the 99th percentile; the crowd's
  clock; where the code lives (`op_sim`, a crate of its own or the
  client); the hand-back from `op_sim`; players blocked by people (an
  ADR-0004 change); the client's CPU cost for ADR-0018; the rule for
  fictional people stated as new; and an open question on the lane and
  profile licences.
- **Also changed:** the ADR index, the roadmap's "Now" and section 8,
  and two links to ADR-0021 in RFC 0004.
- **No code, schema, fixture or replay changes.** The prototype's code
  is not in the repository; the ADR asks for it to be kept before M4 or
  M5 relies on its numbers.

**Waiting for the maintainer:**
1. ADR-0021: C2 or another option, a seeded A among them. The answer
   unblocks RFC 0004 (with RFC 0003, one contract-version choice) and
   then the navigation-data RFC.
2. hystreet's licence version, before its counts shape the crowd
   (ADR-0021, open question 2).
3. The lane and profile licences, with ADR-0019 (ADR-0021, open
   question 6).
4. Whether "Private by design" extends to people (ADR-0021, Privacy).
5. Whether replays should record the date and time for playback
   (ADR-0021, open question 1).
6. Where to keep the prototype: a branch, or a folder with a README
   (ADR-0021, Consequences). It is kept outside the repository for now.
   Its ORCA solver (variant b) follows the RVO2 library's
   `computeNewVelocity`, which is Apache-2.0, so in the repository that
   file would need RVO2's notice and stay Apache-2.0 only, or variant b
   could be kept as results without its code.

### Drawing ADR drafted (2026-11-25)

[ADR-0022](adr/0022-drawing-trees-lamps-and-windows.md) drafts how the
client draws trees, street lamps and lit windows, the decision that RFC
0004 asks for before that client work. It extends ADR-0012. **Nothing
is decided;** the maintainer decides.

| Part | Options | Recommended |
|---|---|---|
| Trees | T0 proxies; T1 models generated per genus group, with levels of detail; T2 T1 plus impostors; T3 one model per tree; T4 a crate or virtual geometry | **T1**, sun shadows from the nearest level only, shadow-only crowns tried before merging; no wind or impostors in M2 |
| Street lamps | L0 glowing heads only; L1 painted pools of light; L2 heads plus a pool of real lights near the camera; L3 plus a few shadowed lamps; L4 every lamp a light, or ray tracing | **L2**: 64 shadowless point lights of 20 m range; mounts inferred in the client |
| Lit windows | W0 a glow per building; W1 a seeded share of the procedural windows; W2, W3 fictional rooms behind them | **W1**, one share curve for all buildings |
| Night exposure and sky | N1 a lower floor only; N2 a floor plus moonlight and sky glow; N3 automatic exposure | **N2**, under ADR-0018's floor, EV100 3 as the starting value (3 or 4, ADR-0022 open question 2) |
| Floodlit landmarks | F0 none; F1 by building type; F2 a source decision or contributions | **F0** in M2 |

- **The recommendation:** spend where the look is. Lights and windows
  cost a few tenths of a millisecond; trees in leaf cost 2.5 ms at
  Lindenhof, mostly in shadows and overdraw near the camera. Everything
  invented (models, leaf dates, mounts, heights, colours, which window
  is lit) stays client code. RFC 0004 needs no new attribute for M2's
  drawing, and leaf habit comes from a client table, since RFC 0004
  does not let the world estimate it.
- **The night is part of the decision.** At EV100 3, lamps alone gave
  a black sky and black ground between lamps, so the draft adds
  moonlight without shadows and a sky glow, and asks for the floor, the
  fill and the window and shop luminance to be chosen by comparing
  screenshots with two 2025 photos of the old town at night (exposed at
  EV100 3.0 and 4.0).
- **Evidence,** gathered on 2026-11-25 in three research notes and a
  prototype. The notes and measurements are outside the repository; the
  prototype's code is on branch `drawing-prototype`. The ADR cites the
  sources:
  - **the engine:** Bevy 0.20.1 clusters any number of lights on the
    GPU, but a shadowed spot light costs a 64 MiB layer and a full
    shadow pass in this client; alpha-tested leaves are drawn in the
    prepass, the main pass and every cascade; an RX 6600 is about 1.65
    times slower than the RTX 3070 (1.33 to 1.93, press), so ADR-0018's
    16.7 ms is about 8.8 to 10.1 ms of GPU time on the RTX 3070;
  - **games and Zurich:** games split a lamp into near light and far
    glow, ration lamp shadows and retarget night exposure (Far Cry 5);
    lit flats average 6 cd/m² and shop windows 50 cd/m² (CIE 2025);
    the Plan Lumière's open concept plan has no landmark in the
    district;
  - **the data:** 1,005 trees, 94.3% deciduous by genus, and 1,959
    light records at 1,717 positions; no mount, height or colour for
    any light; an unvalidated mount heuristic from the building shells.

**The prototype:** branch `drawing-prototype` (commit 68b149f), about
3,650 lines in `op_client`, not for merging. Without its options
(`--proto-*`, `--bench`, `--preset`), the client behaves as before. It
draws procedural trees per genus group with three levels of detail and
leaves by date (MeteoSwiss's medians where they exist; lime's leaf fall
and other genera assumed); lamp models per mount with glowing heads and
a pool of real shadowless lights; and a seeded share of the procedural
windows lit at night, from one repeating mask per facade style. It has
a benchmark mode. Its input is a scratch file converted from the city's
data, not world data and not committed. RTX 3070 on Vulkan, Ryzen 9
3900, Windows 11, 1920 x 1080 without vsync, one camera turn over 600
frames per run (2.5 to 4.7 s, against ADR-0018's 20 s), night exposure
EV100 3. 88 runs: a matrix of 80, then 8 on the committed build; the
matrix's first 70 runs came from an earlier build, which narrowed 100
more tree crowns at facades and picked levels differently. One run per
configuration, on a machine that may have been in use.

In the default configuration (two tree levels casting shadows, 32 spot
lights), earlier build:

| Stop, 18:00 | GPU median, today | With trees, lamps (32 lights) and windows | GPU p99 | Frame p99 |
|---|---|---|---|---|
| Lindenhof, 21 June | 4.18 ms | 6.79 ms | 7.80 ms | 9.29 ms |
| Hirschenplatz, 21 June | 4.71 ms | 5.30 ms | 5.56 ms | 9.09 ms |
| Rathausbrücke, 21 June | 4.62 ms | 4.79 ms | 5.17 ms | 9.93 ms |
| Lindenhof, 21 December | 2.77 ms | 3.32 ms | 3.60 ms | 6.82 ms |
| Hirschenplatz, 21 December | 2.97 ms | 3.31 ms | 3.44 ms | 6.59 ms |
| Rathausbrücke, 21 December | 2.89 ms | 3.11 ms | 3.28 ms | 8.12 ms |

In the recommended configuration (shadows from the nearest tree level,
64 point lights, trees and windows), committed build:

| Stop and time | GPU median | GPU p99 | Frame p99 |
|---|---|---|---|
| Lindenhof, 21 June 18:00 | 6.69 ms | 7.77 ms | 9.25 ms |
| Lindenhof, 21 December 18:00 | 3.47 ms | 3.76 ms | 8.00 ms |
| Hirschenplatz, 21 December 18:00 | 3.44 ms | 3.60 ms | 6.32 ms |
| Lindenhof, 21 June 22:30 | 3.87 ms | 4.36 ms | 7.06 ms |

- **Lights:** 0 to 256 shadowless spot lights added 0.08 to 0.20 ms of
  GPU median; point lights cost 0.03 to 0.28 ms more than spot lights;
  64 point lights cost 0.19 to 0.31 ms over none in December.
- **Windows:** 0.04 to 0.06 ms at night.
- **Trees:** in leaf at Lindenhof, +2.49 ms (+2.50 on the committed
  build): the sun's shadow pass went from 1.28 to 2.89 ms, and
  main-pass fragments per pixel from 0.59 to 2.67. Bare December trees
  added 0.04 to 0.40 ms. Tree shadows cost 1.5 to 1.7 ms there, about
  85% of it from the nearest level, so shadows from that level only
  save little against two levels. On one build, trees drawn to 300 m
  were not cheaper than to 600 m.
- **The worst case,** Lindenhof in June in the recommended
  configuration (GPU p99 7.77 ms), would take about 12.8 to 14.8 ms of
  GPU time on the reference class (estimate), against 16.7 ms for the
  whole frame, leaving little for the rest of M2. The default
  configuration measured the same (7.80 ms).
- **CPU:** about 9,500 more entities; frame median 0.58 to 0.74 ms
  higher at the December stops, where the frame is not GPU-bound.
- **The look:** June at Lindenhof (limes and dappled shade) looks
  right, and December at Hirschenplatz reads as an old town at night.
  But shop windows blow out to white, the sky is pitch black with no
  city glow or moon, the ground away from lamps is nearly black, and
  spot lights aimed down leave the facades and crowns above the
  lanterns dark, where point lights show them.
- **Determinism:** on the prototype's build, the client played the
  sample replay matching every checkpoint, and `op_replay check` passed
  (20 checkpoints).
- **Not measured:** an AMD card, DX12, video memory, popping and
  shimmer in motion, shadow-only crowns, occlusion culling, wind.
- **Also changed:** the ADR index, the roadmap's "Now" and section 8,
  RFC 0004's lines on its follow-ups, a pointer in ADR-0018, and
  ADR-0012's header ("extended by ADR-0022").
- **No code, schema, fixture or replay changes** on this branch. The
  prototype's code is on its own branch; its input file and
  measurements are kept outside the repository.

**Waiting for the maintainer:**
1. ADR-0022: an option per part (trees, lamps, lit windows, night,
   landmarks). It builds on RFC 0004 and ADR-0018, and can be decided
   with them.
2. A share of the budget for trees, lamps and windows: none, or 2 ms of
   GPU time at the 99th percentile on the RTX 3070 as a starting value
   (ADR-0022, open question 1), with ADR-0018.
3. The night's values: the exposure floor, moonlight, sky glow, and
   window and shop luminance, chosen by screenshots against the photos
   (ADR-0022, open question 2), with ADR-0018's night exposure.
4. MeteoSwiss's leaf dates: their licence ("CC-BY" with no version) and
   the credit "Source: MeteoSwiss" (ADR-0022, open question 6), with
   ADR-0017.
5. Lamps stood on bridge decks or quays, and the inputs of the mount
   inference (ADR-0022, Consequences), with ADR-0019's question 5.
6. A card of the RX 6600 class to run the prototype's matrix on, two
   runs per configuration (ADR-0018's open question on who buys or
   lends it).
7. Where to keep the prototype: on branch `drawing-prototype` until the
   reference card's run, as the ADR proposes, then reused in pieces once
   RFC 0004's points load. Its input conversion and raw reports are
   kept outside the repository for now.

### ADR-0020, ADR-0021 and ADR-0022 accepted (2026-11-25)

The maintainer merged pull requests #4, #5 and #6 and accepted the three
decision records with their recommended options:
- **[ADR-0020](adr/0020-baseline-refreshes.md), baseline refreshes:**
  the recommended option of each of its sections 1 to 9, and the
  clarification of RFC 0002 section 7 (the RFC as a whole stays a
  Draft). For Q18 that is (b): a revert by a takedown may be lifted by a
  written accept in a decision set; the tool still applies the stricter
  (a) until it implements (b). The amendments A1 to A8 (RFC 0002,
  questions 9 to 16), the ingest options of section 10 and the
  questions left open by the reviews stay open.
- **[ADR-0021](adr/0021-pedestrians.md), pedestrians:** C2 in M2,
  people drawn by the client from a shared seed, outside the
  simulation; a seeded A is the fallback if the client prototype fails
  its checks; C1 is left to M4 and M5.
- **[ADR-0022](adr/0022-drawing-trees-lamps-and-windows.md), drawing
  trees, street lamps and lit windows:** T1 trees with sun shadows from
  the nearest level, L2 lamps, W1 lit windows, N2 for the night, and no
  floodlit landmarks in M2.

The open questions inside each record stay open. RFC 0001 to RFC 0004
and the other proposed ADRs are unchanged.

**Next:** lit windows and the night's look (ADR-0022, W1 and N2) need no
world data, so they go into the client first. Trees and lamps wait for
RFC 0004's point features. ADR-0020's Q18 (b) goes into the refresh
planner. ADR-0021 starts with its client prototype and the
navigation-data RFC.

### Refresh takedowns: ADR-0020's Q18 (b) (2026-11-25)

The planner now applies the accepted Q18 (b). A key whose last change
was a takedown that removed it keeps the hold code `takedown`, which
nothing lifts. A key whose last change was a takedown that reverted it
gets the new code `takedown-revert`: an accept with a reason lifts it,
and the change goes into a decision set of its own, titled "... that a
takedown reverted (maintainer decision)". The report schema lists the
new code, and the maintainer guide and ADR-0020's code table describe
it. The fixture plan's pinned ids are unchanged; `tools/ingest` and
`tools/worldfmt` tests: 691 passed, 98 skipped.

### Lit windows and the night (2026-11-25, ADR-0022 W1 and N2)

The client now draws the night as
[ADR-0022](adr/0022-drawing-trees-lamps-and-windows.md) decided for lit
windows (W1) and the night (N2), on by default. Trees and street lamps
wait for RFC 0004's point features. Only `op_client` changed; the
simulation, the world format and the replays did not.

**Lit windows (W1):**
- **Which window is lit** follows only from the building id, the wall,
  the window's bay and storey, and the night: a hash of them, compared
  with the share of windows lit at that hour. Nothing comes from
  data about real homes, and nothing is drawn behind the glass. Each
  window has its own hash, so no pattern repeats: the prototype's masks
  repeated every 16 bays and 8 storeys, and every wall of a style showed
  the same pattern. As the share rises through an evening windows only
  switch on; a night runs from noon to noon, and another night lights
  other windows.
- **How:** windowed walls use a small shader extension of the
  StandardMaterial (`windows.wesl`), which adds the glow after lighting,
  in cd/m² times the exposure. It is bindless like the StandardMaterial:
  a first, non-bindless version cost 3 to 7 ms of frame time (our
  reading: each of its 48 materials, three styles by two floors by eight
  tints, broke the batches). A wall's seed (from the building id and the
  wall's plane) rides in the wall's second texture coordinates; the first
  already count bays and storeys. A thin sliver triangle whose normal is
  imprecise keeps its wall's seed, so no window is split between two
  choices (an early screenshot showed a half-lit shop window).
  The glass's shape is a mask per style made from the same rectangles as
  the facade textures, a little brighter at the top. Windows too small
  on screen to tell apart glow with the average instead of shimmering.
- **Shares, set by hand** (no Swiss data on windows lit by hour was
  found): homes and offices on upper floors and on ground floors with
  doors and windows, one curve for weekdays and one for weekends, at most
  45% at 20:00 to 21:00 (CIE 2025 found at most 50%); shop windows, one
  curve each for weekdays, Saturdays and Sundays (shut, some displays
  lit). Windows switch on as the sun goes from 3° above to 3° below the
  horizon, so by day they add nothing. At 18:00 on Monday 21 December:
  40% of homes and 88% of shop windows; at 22:30 on Sunday 21 June: 33%
  and 20%.
- **Brightness:** a lit home's window averages 6 cd/m² (CIE 2025); a shop
  window 10 cd/m², a fifth of CIE's 50, as a look choice: at EV100 3,
  50 cd/m² is white, as the prototype's 25 already was. Colours: homes
  65% at 2800 K, 27% at 3500 K and 8% at 5000 K; shops 70% at 3000 K and
  30% at 3600 K; each lit window from half to one and a half times the
  average.
- **The plain look (F2)** has no lit windows; inspect mode's highlight
  keeps them.

**The night (N2):**
- **Exposure floor EV100 3,** reached once the sun is 8° down. Down to 1°
  below the horizon the exposure is the curve the client always had; it
  then falls to the floor instead of stopping at 6.5. With the sun 7.5°
  down, as in the two 2025 photos from the Lindenhof (EV100 4.0 and 3.0),
  it is 3.25.
- **Moonlight:** a bluish directional light from the moon's position for
  the date and time, without shadows and without a disc. The position
  comes from the Astronomical Almanac's low-precision formulae, with
  parallax; against JPL Horizons it is within 0.25° on four dates, and
  the F3 panel shows it. Its strength follows the phase (Krisciunas and
  Schaefer, 1991) and the air it crosses (Kasten and Young's airmass),
  times 16: the real moon (0.31 lx when full, overhead) barely shows at
  EV100 3, so the gain is a look choice, as Far Cry 5 raised its moon. At
  18:00 on 21 December (92% lit, 37° up in the east) that is about
  1.6 lx; at 22:30 on 21 June (49% lit, 25° up in the south-west) about
  0.3 lx. A new moon or a moon below the horizon gives nothing; the
  city's glow keeps those nights from going black, so the moon has no
  floor of its own. Without light (by day, below the horizon, or in the
  plain look) the moon's light is hidden, so it costs nothing.
- **The city's glow:** a dome around the camera added to the sky,
  1.3 cd/m² at the horizon fading to 0.5 cd/m² overhead, orange-brown
  (2200 K, partly greyed), for an overcast sky lit by the city as in the
  2025 photos; and a warm ambient fill of 1.8 cd/m² (3000 K, half
  greyed), about 5.7 lx on the ground, the low end of an old-town lane's
  average in Lucerne's guideline, but even instead of in pools. Both are
  invented light, set by eye. The fill stands in for lamps and should go
  down when lamps are drawn.
- **All of it fades in** as the sun goes from 1° to 8° below the horizon,
  so daytime is unchanged. The plain look keeps only the sun and the
  exposure, as before, so at night it is as dark as it always was.

**Starting values** (ADR-0022, open question 2; the maintainer sets the
final ones):

| Value | Starting value | Why |
|---|---|---|
| Exposure floor | EV100 3, from the sun 8° down | ADR-0022's starting value; the photos' 3.0 and 4.0 |
| Lit home window | 6 cd/m² | CIE 2025's average |
| Lit shop window | 10 cd/m² | look choice: CIE's 50 is white at EV100 3 |
| Homes lit at peak | 45% | under CIE 2025's 50% |
| Moonlight | 16 times the real moon, bluish, no shadows | look choice; real moonlight barely shows at EV100 3 |
| Sky glow | 1.3 cd/m² at the horizon, 0.5 overhead, 2200 K | look choice, against the photos' orange-brown sky |
| Ambient fill | 1.8 cd/m², 3000 K | look choice: ground and facades readable without lamps |

**What the screenshots show** (1280 × 720, high, release build, at
Lindenhof, Hirschenplatz and Rathausbrücke; kept outside the repository):
- **18:00 on 21 December:** reads as the old town on a winter evening. A
  scattered share of upper windows glows warm, a few whiter; no grid and
  no repeat along or between walls. Shop fronts glow peach to cream with
  a brighter top, none clipped to white. Facades and setts are dim but
  readable, facades facing the moon a little lighter; the sky is
  orange-brown, brighter towards the horizon, where the far city shows
  as dots of lit windows.
- **22:30 on 21 June (a Sunday):** fewer windows lit, shops mostly dark;
  the same glow and fill, little moonlight.
- **12:00 on 21 December and 18:00 on 21 June:** unchanged. Against
  screenshots of the client before this change, the mean difference per
  pixel was 0.16 to 0.41 (of 255), below the 0.43 to 0.69 between two
  runs of the same build (TAA and timing).
- **Next to the photos** the scene is darker: there, street lamps and
  floodlights light the facades and quays. The sky's colour and the
  windows' warmth are close.
- **Known look issues:** on some near windows the glass reflects the sky
  with a diagonal seam, which is also there by day and was only hidden by
  the black night (our reading: the shells' triangles have slightly
  different normals); the moon has no disc; the plain look at night is
  dark.

**Cost** (RTX 3070, Vulkan, 1920 × 1080 without vsync, high). The client
has no benchmark mode on main, so a temporary patch (not committed) logged
Bevy's GPU time per pass and the frame time; each figure is the median of
the last six one-second averages of a 12-second run, and main's client
and this one ran alternately, three times each, at Lindenhof (18:00 in
June), Hirschenplatz (18:00 in December) and Rathausbrücke (22:30 in
June):
- **By day** nothing measurable: frame 5.4 to 6.0 ms with both, GPU time
  4.0 to 4.2 ms against 4.0 ms.
- **At night** GPU time rises by 0.1 to 0.3 ms a frame: about 0.11 ms for
  the moon in the atmosphere's scattering, 0.05 to 0.09 ms for the main
  pass with lit windows and moonlight, 0.01 to 0.04 ms for the glow dome.
  Frame time stays within run-to-run noise (0 to 0.4 ms more). The lit
  windows alone match the prototype's 0.04 to 0.06 ms.
- **Found on the way:** the first version's non-bindless extension cost 3
  to 7 ms of frame time and 0.2 to 0.6 ms of GPU time (in Bevy's uniform
  allocation and bin unpacking passes); and a moon light with no light
  still cost 0.11 ms in the atmosphere until it was hidden.

**Checks:**
- `cargo fmt --all --check` and `cargo clippy --workspace --all-targets
  --locked -- -D warnings`: pass.
- `cargo test --workspace --locked`: 263 passed, 1 ignored. New in
  `op_client`: the window choice is stable (with known hash values);
  over 16,000 windows the share lit matches the curve within 2 points at
  four times of 21 December, and windows only switch on as the share
  rises; lit windows average their kind's luminance; no repeat (two walls
  of one style, two buildings, bays 16 apart and storeys 8 apart, and two
  nights each agree about half the time at a share of one half, as
  independent windows do); the shader holds the same constants; a sliver
  keeps its wall's seed; the glow lies exactly on the textures' glass; the
  shares by hour and day type; the moon within a quarter of a degree of
  JPL Horizons on four dates, its phase within 3 points; the exposure
  unchanged down to 1° below the horizon and EV100 3 at both night stops;
  moonlight by phase and height; the glow brightest at the horizon.
- `op_replay check fixtures/replays/sample.oprep`: ok, 20 checkpoints,
  final state `212bd353…`, unchanged; the client plays the replay and
  matches every checkpoint.

**Still open:**
- **ADR-0022's open question 2:** the final night values, by screenshots
  at EV100 3 and 4 against the 2025 photos; this entry's values are
  starting values.
- **Trees and lamps** wait for RFC 0004 (Draft). When lamps come, the
  ambient fill should drop so that their pools show.
- **Open question 8:** one curve for all buildings; classes after
  ADR-0019.
- **The plain look at night** is dark, as before. A neutral fill in F2
  would make it readable, but it would be light the data does not hold.
- **A moon disc** with its phase, if wanted.

### Navigation-data RFC drafted (2026-11-25)

[RFC 0005](../rfcs/0005-navigation-data.md) drafts the world layer for
walking that [ADR-0021](adr/0021-pedestrians.md) asks for under "What
this means for other decisions": read by the client in M2, and shaped
to become simulation input later without a format change. **Nothing is
decided;** comments run until at least 2026-12-09, then the maintainer
decides. Drafting it now departs from ADR-0021's order of work, which
puts the client prototype first; the RFC is to be revised after the
prototype.

| Choice | Options | Recommended |
|---|---|---|
| The content | a graph document per tile; a navigation mesh; glb meshes; the ribbons; one asset per lane; no world data | **a canonical-JSON graph document per tile and source** |
| How lanes are stored | L1 one graph per tile, cut at tile edges; L2 lanes owned by one tile, uncut; L3 polylines without topology | **L1**, with nodes shared at the cuts and `world` nodes where the world ends |
| Where the profiles live (ADR-0021) | P1 in the navigation document; P2 curves in an asset of their own; P3 a `population` layer; P4 curves in the client | **P1**: one asset per tile, so the refresh planner, which splits layers into separate sets, never leaves numbers naming missing lanes |
| Matching lanes to shells (ADR-0021, open question 6) | K1 move lanes and snap doors by at most 1 m, narrow, choose connectors, mark the rest; K2 mark only; K3 none in the data; K4 offsets in an asset of their own; K5 doors on swisstopo walls at any distance | **K1**, with an amendment of RFC 0001 section 2; never against contributed shells |
| Where the lane check lives | a new G11; inside G9 | **G11**, so G9 and G10 stay as ADR-0021 says |
| The lane check during a refresh | (a) strict per set; (b) collider sets first; (c) a tile's shells and lanes in one unit; (d) `review` per set and the absolute check on the final world; (e) `review` through an A4-style rule | **(d)**: ADR-0020's accepted order stays; (b) and (c) would need their own decision |
| Passages | nobody walks them in M2; fade at the mouths; walk through the wall; the client opens the wall | **nobody walks them in M2**, a narrowing of ADR-0021; the client opens walls once ADR-0019's question 5 is answered |
| Smaller choices | kinds, area kinds, places, entrances, benches, rule ids, spacing, months, holidays, asset ids, trees and poles | `road`, `area` and `connector` kinds; `square` and `street` areas; places joined by a node; estimated doors; OSM benches in M2; a rule id per estimated value |

- **The data model:** one `navigation` document per tile and source,
  `collider: none`:
  - nodes, and lanes between them as polylines in integer millimetres
    (vertices at most 1.5 m apart, heights on the terrain mesh or a
    deck's top) with a kind, a width, `moved_mm` and ranges marked
    `entrance`, `passage` or `blocked`;
  - areas (`square` or `street`); places (entrances, stops, squares,
    benches, terraces, viewpoints, fountains) joined to a node, with
    opening hours; `bridge` flags on lanes, areas and places;
  - the population: curves per day type (a share, 24 hourly shares and
    12 monthly factors), a week, a holiday list and a time zone; people
    per day on every lane, trips or occupancy at every place;
  - every estimated value names its rule; ids come from OSM ids and are
    unique across the world; the layer is closed to contributions.
- **The gate:** a new G11 keeps any change from newly blocking a lane;
  G1, G3, G7 and G8 gain rules; G9 and G10 stay. The gate takes the next
  free version (`GATE_VERSION` is 0.1.2 today; 0.1.4 only if RFC 0004's
  0.1.3 lands first). The world format version joins the choice for RFC
  0003 and RFC 0004; the contribution contract is amended in place or
  bumped, a choice.
- **Evidence:** ADR-0021's data and prototype, and its research notes,
  which stay outside the repository: about 46 km of walkable OSM ways
  in the district; 208 entrances, 353 benches, 13 stops; 269 of 1,120
  prototype edges overlapping shells in 4 tiles. Counted in the cache
  for this RFC: 39 ways tagged `tunnel=building_passage`, and
  Bahnhofstrasse mapped as six pedestrian areas, not a line. Sizes
  (about 2.5 to 3 MB for the district) are estimates. Why most lanes
  overlap shells was not examined, so the 1 m bound is a starting
  value.
- **Review:** an adversarial review of the draft was applied the same
  day. Among the changes: the population moved into the navigation
  document (P3 to P1); the lane check became G11; ADR-0020's order of
  sets is kept, with a final-world check; doors are snapped onto walls
  within K1's 1 m; heights follow the terrain mesh; areas gained a
  `street` kind and places and areas a `bridge` flag.
- **Also changed:** the RFC index, the roadmap's "Now", the water and
  navigation rows of section 8, and a link to RFC 0005 in RFC 0004.
- **No code, schema, fixture or replay changes.**

**Waiting for the maintainer:**
1. The client prototype that ADR-0021 puts first; the RFC is revised
   after it and accepted only then.
2. ADR-0021's open question 6, settled with this RFC and ADR-0019: K1
   (an RFC 0001 amendment covering lanes, doors, terraces and
   connectors), and counts inside ODbL navigation assets. The answer is
   recorded in ADR-0021.
3. The lane check during a refresh: option (d) keeps ADR-0020's
   accepted order. Options (b) and (c) would need an ADR superseding a
   row of ADR-0020 section 5, or an amendment beside A1 to A8.
4. A new hold in the refresh planner for refreshed lanes that a
   contributed shell blocks, with ADR-0020's holds.
5. One world-format version choice for RFC 0003, RFC 0004 and RFC 0005,
   recommended after 2026-12-09; and whether the contribution contract
   is amended in place or bumped.
6. Passages: nobody walks them in M2, a narrowing of ADR-0021.
7. hystreet's licence version (ADR-0021, open question 2), before its
   counts shape a curve, and a reviewed source for the holiday list.
8. The navigation ingest's ADR, to be drafted: tags, the new query,
   the matching, and the rules for estimated values.

### Crowd prototype (2026-11-25, ADR-0021 C2)

Branch `crowd-prototype`, throwaway. ADR-0021's order of work in M2 starts
with "a throwaway client prototype of C2 on the pedestrian prototype's
graph, at the benchmark route's three places, judged by the checks above".

**Thresholds, set before the prototype ran** (written down here before the
first client run; the brief that asked for the prototype gave the numbers):

Each check is pass or fail per stop and time, over a 20-second camera turn
(1,200 ticks) at eye height at each of ADR-0018's three stops (Lindenhof,
Hirschenplatz, Rathausbrücke), on 2026-12-21 at 17:00 (a Monday) and on
2026-06-21 at 18:00 (a Sunday). Every case runs twice, in separate
processes, with seed 0, a drawing radius of 160 m whose last 25 m fade, and
both kinds of avoidance on (the configuration that is drawn). Everything is
counted at whole ticks, with the camera's angle a function of the tick, so
two runs count the same.

1. **Nobody appears or vanishes in view inside the fade distance:** 0
   events. An event is a person drawn at a tick but not at the one before,
   or the reverse, while inside the view frustum (70 degrees vertical, 16:9,
   buildings not hiding anyone) and nearer than the fade's start (135 m).
2. **Overlaps between people in view:** fewer than 10 a minute. An overlap
   starts when two people drawn, in view and within 40 m of the camera come
   closer than 0.4 m between centres; a minute is three turns. Counted for
   the drawn configuration, which is judged, and for each kind of
   avoidance alone and for none, as the ADR asks.
3. **Nobody stands inside a building shell:** 0 people with their feet more
   than 0.1 m inside a shell's footprint (the shell cut 0.5 m above the
   feet), outside marked entrance or passage ranges (the edges from a lane
   to a door or a spot, building passages, the last 3 m of paths that end
   at a door). Everyone drawn within the radius, at every tick.
4. **The number of people within the drawing radius stays within ±25% of the
   profile's target:** the people the function places within 160 m, at
   every 60th tick (21 samples), against lane length inside the radius times
   density per lane kind times hourly shape times month factor, plus spots
   times occupancy times mean party. Every sample within the band.
5. **The crowd's cost: at most 1.5 ms of frame time** (CPU main update plus
   GPU) on the RTX 3070 at 1920 x 1080, high preset, at the busiest stop and
   time (the case with the largest target), measured without the checks:
   the difference in the median main-update CPU time (from the start of
   Bevy's `First` to the end of `Last`) plus the difference in the median
   GPU time per frame, with the crowd against without it, two runs each,
   alternated. The difference in frame time is reported beside it.
6. **The same people at a pinned tick on two runs:** a hash of the
   positions of everyone the function places within the radius, at ticks
   0, 600 and 1,200, equal on both runs.

**What was built** (`crates/op_client/src/crowd_proto/`; nothing runs
unless `--proto-crowd <file>` or `--proto-crowd-bench <out.json>` is
given, and without them the plugin adds no system):
- **The population function** (`population.rs`): who is where is a pure
  function of the lanes, places and profiles, the session seed, the
  crowd's clock (date, time and ticks), the population's version
  (`crowd-proto-0.1`) and the catalogue's size. No Bevy and no floats:
  integer millimetres, ticks and per-mille factors, and a SplitMix64 hash
  of the seed for randomness (a test checks that the file names no float
  type, unordered map or clock), so it could move to `op_sim`. The camera
  only chooses which part is worked out; a cache keeps trips already
  worked out, and a test checks that asking other places and times first
  changes nothing.
  - **Trips** start only at places (doors of shops and public buildings,
    stops, squares, the world's edge), in 2-second slots at a rate that
    follows the profiles. Each group walks a seeded walk over the lanes for
    150 to 700 m, then on to the nearest place ahead, where the trip ends.
  - **Walk weights** are calibrated once at load by sampling 8 rounds of
    8,000 trips, so that each lane gets its share of the people. Weighted
    by density alone, the walk gave pedestrian streets half their share and
    steps three times theirs, because flows are decided at junctions. A
    factor per time brings the expected total to the profiles' total.
  - **Visits:** groups walk to spots in squares and at stops, stand in a
    ring for 1 to 10 minutes, and walk on to a place.
  - **Groups and speeds:** singles 60%, pairs 28%, threes 8% and fours 4%
    of the groups, so 60 to 67% of the people seen walk in groups.
    Weidmann's 1.34 m/s (sd 0.26) per member; a group walks at its
    members' mean less 0.06 m/s per extra member, times 0.9 from shops and
    squares and 1.05 from stops, at half speed on steps. Abreast where the
    lane is wide enough, staggered as it narrows.
  - **Avoidance built in** (`lanes`): keep right, slower walkers further
    out, and anyone closer than 0.8 m to someone of another group steps
    sideways by half the shortfall, worked out in closed form from where
    both are at that tick, every term continuous so that nobody jumps.
- **Client offsets** (`offsets`): within 40 m of the camera, people step
  aside from anyone closer than 0.8 m, as offsets from the function's path
  that fade back to it (about a third of a second), at most 5 cm a tick
  and 0.6 m in all, within the room beside the lane.
- **The camera may delay drawing, never move anyone:** someone new in
  view nearer than the fade (a trip starting at a door) is drawn once out
  of view or beyond the fade; someone whose trip ends in view stays drawn,
  standing where it ended, until out of view. Positions never depend on
  the camera.
- **Lanes** (`lanes.rs`, `walls.rs`): read from a scratch file that a
  conversion script writes from the ingest's OSM cache, then cleared of
  the building shells at load, as the ingest would. Shells cut 0.5 m above
  the ground give walls; a lane point closer than 0.35 m to a wall moves
  sideways by at most 1 m, or the lane is dropped, except the last 3 m of
  paths that end at a door and building passages, which are marked. The
  room beside each lane is measured every 0.5 m, and each node's clearance
  from walls; people keep 0.3 m from walls, and turns keep within the
  narrower lane and the node's clearance.
- **Characters** (`figures.rs`): generated in code, no asset files: box
  figures (legs with knees and shoes, hips, a torso or long coat, arms and
  hands, a head with hair, sometimes a hat or a bag), 48 looks per season
  from seeded palettes (coats and hats in winter, shorts and short sleeves
  in summer), 1.50 to 1.98 m tall. A flipbook step cycle of 32 poses
  within 30 m and 8 beyond, its phase following the distance walked (one
  cycle per 1.38 m at 1.75 m, the figures' own stride) from a seeded
  start, so planted feet stay put and nobody steps in time; standing and
  sitting poses. 2,112 meshes, 0.83 million vertices, built in 45 ms.
- **Drawing:** one entity per person; people move every frame within 60 m
  and at each tick beyond; a radius of 160 m whose last 25 m fade in four
  steps; hidden in F2's plain look.
- **The crowd's clock:** the client's date and time plus the simulation's
  ticks; the benchmark counts from the end of loading, and
  `--proto-crowd-tick` pins it. People are worked out at whole ticks.
- **The benchmark** (`bench.rs`): ADR-0018's method (1920 x 1080, vsync
  off, high preset, a 20-second turn at eye height), with the camera's
  angle a function of the tick and every check counted at whole ticks, for
  all four ways of avoiding each other. `--proto-crowd-census` prints
  people against the target by lane kind.

**Inputs** (local, not in the repository; OSM data of 2026-11-23):
- **Lanes:** 7,042 edges: 74.5 km of lanes (28.5 km of them across 34
  squares and pedestrian areas) and 1.7 km of edges to doors and spots.
  Against 1,387 shells, 128 lanes came closer than 0.35 m to a wall and 37
  had points inside a shell. 24 nodes moved, 38 lanes moved sideways (at
  most 0.98 m) and 98 lanes (2.4 km) were dropped: 31 ran into a wall, 31
  needed a shift at an end, 22 more than 1 m, 11 ended at a node that
  could not be cleared and 3 had walls on both sides. No lane point was
  left inside a shell. 351 lanes end at a door; 28 pass through buildings.
- **Places:** 470: 205 doors of shops (paths that end at the building, or
  a door placed on the outline nearest a lane), 123 of public buildings,
  2 stops (the only platforms in range), 35 squares and 108 points where a
  lane leaves the district. Never a door of a building tagged as homes or
  with no use: 67 paths to such doors were dropped. The cache has no
  benches, terraces, entrances or tram stops (they are nodes), so nobody
  sits.
- **Spots:** 66 in squares, at least 1.8 m from any lane, and 4 at stops.
- **Profiles:** hourly shapes per day type from Bahnhofstrasse (Mitte,
  2025) for city lanes and from the Limmatquai counter (2024) for lanes
  within 15 m of a way named as a quay or bridge, their mean for squares.
  Month factors from the same series: Bahnhofstrasse weekdays 853 to 1,277
  per mille (December the highest), the Limmatquai 672 to 1,325 (December
  estimated as the mean of November and January). Densities per lane kind
  set by hand, people per km at the weekday peak in an average month: 350
  on pedestrian streets, 120 on living streets, 100 on platforms, 80 on
  sidewalks, 60 on crossings, 50 on footways and on lanes without
  sidewalks, 30 on steps; 200 people per hectare of square; 40% of square
  spots and 50% of stop spots taken at the reference.

**Results** (12 runs, each case twice; seed 0; the drawn configuration
has both kinds of avoidance):

| Check | Lindenhof, Dec | Lindenhof, Jun | Hirschenplatz, Dec | Hirschenplatz, Jun | Rathausbrücke, Dec | Rathausbrücke, Jun |
|---|---|---|---|---|---|---|
| 1. Appear or vanish in view (0) | pass, 0 | pass, 0 | pass, 0 | pass, 0 | pass, 0 | pass, 0 |
| 2. Overlaps a minute (under 10), both | pass, 9.0 | pass, 3.0 | pass, 0.0 | pass, 3.0 | pass, 6.0 | pass, 3.0 |
| 2. ... built into the function alone | fail, 93.0 | fail, 27.0 | pass, 9.0 | fail, 33.0 | fail, 63.0 | fail, 30.0 |
| 2. ... client offsets alone | fail, 45.0 | fail, 27.0 | pass, 6.0 | fail, 21.0 | fail, 63.0 | fail, 24.0 |
| 2. ... no avoidance | fail, 276.0 | fail, 120.0 | fail, 60.0 | fail, 54.0 | fail, 195.0 | fail, 105.0 |
| 3. People inside shells (0) | pass, 0 | pass, 0 | pass, 0 | pass, 0 | pass, 0 | pass, 0 |
| 4. People within 160 m against the target (±25%) | pass, 1,044 to 1,068 against 1,013 (+3 to +5%) | pass, 609 to 622 against 617 (-1 to +1%) | pass, 822 to 833 against 879 (-6 to -5%) | pass, 494 to 506 against 419 (+18 to +21%) | pass, 1,113 to 1,155 against 1,036 (+7 to +11%) | pass, 614 to 633 against 603 (+2 to +5%) |
| 6. Same people on two runs | pass | pass | pass | pass | pass | pass |

- **Check 3** gave 0 for all four ways, not only the drawn one.
- **Check 6:** the hashes at ticks 0, 600 and 1,200 were equal on both
  runs at every stop, for both variants of the function, and so was every
  count of every check: the checks themselves are reproducible.
- **Seen:** 103 to 243 people in view on average (14 to 40 within 40 m),
  60 to 67% of them in groups. Trips starting in view and held until out
  of view: 3 to 18 a run; trips ending in view and held standing: 4 to 8 a
  run, about one in view at a time.
- **Overlaps left** with both kinds of avoidance are mostly grazes on
  squares (0.38 to 0.40 m), and twice two members of one group in a turn.
  Lindenhof in December is the closest to the threshold (3 onsets in the
  turn).
- **Hirschenplatz in June** is the case furthest from its target (+21%).
  The census tool gives +1 to +5% there with seeds 1 to 3: between seeds,
  the count at a stop varies by about 10%.
- **Not a check:** one person in a group of four moved more than 0.25 m
  in a tick, in view, once, at Lindenhof in June. Two sources of such
  jumps were found and fixed during development (a group's stagger that
  changed abruptly when the lane narrowed by 2 cm, and closed-form steps
  whose direction swung when two paths crossed); this one was not traced.

**5. Cost** at the busiest case by target (Rathausbrücke, 21 December at
17:00: 1,036 people expected within 160 m, about 1,135 drawn), without
the checks, runs alternated (ms):

| | Without, run 1 | With, run 1 | Without, run 2 | With, run 2 |
|---|---|---|---|---|
| Frame, median | 4.86 | 5.25 | 4.73 | 5.25 |
| Frame, mean | 5.00 | 5.90 | 4.84 | 5.90 |
| Frame, 99th percentile | 7.16 | 9.12 | 6.59 | 8.67 |
| Main update, median | 3.66 | 3.81 | 3.57 | 3.80 |
| Main update, mean | 3.74 | 4.59 | 3.66 | 4.59 |
| GPU, median | 3.20 | 3.18 | 3.18 | 3.20 |

- **By the measure set before the runs: pass, 0.19 ms** (0.15 - 0.02 =
  0.12 ms for the first pair, 0.23 + 0.02 = 0.25 ms for the second).
- **But the median hides the ticks.** The crowd's own CPU time is 0.36 ms
  a frame at the median (drawing), 0.85 ms on average and 2.4 to 2.5 ms
  at the 99th percentile: working out about 1,150 people at a tick takes
  about 2 ms, and a tick falls on about one frame in three at 170 to 200
  frames a second. The mean frame is 0.9 to 1.1 ms longer and the 99th
  percentile frame 2.0 to 2.1 ms longer, which would fail a 1.5 ms budget
  taken at the 99th percentile, as ADR-0018 takes its frame budget. The
  fixes are known: spread a tick's work over frames, work out people
  beyond 60 m every second or fourth tick, or work the function out on
  another thread (it keeps no state).
- **GPU:** no measurable difference at this dusk stop (no sun shadows).
  The shadows of people in June daylight were not measured for cost.

**The look** (screenshots at tick 0, facing each place's heading, kept
outside the repository):
- **18:00 on 21 June (a Sunday):** the streets read as alive. Lindenhof
  is full of people crossing alone and in groups, with bags, in summer
  colours, casting long shadows; Hirschenplatz is a quieter Sunday
  evening, with a few groups walking away; at the Rathausbrücke a group of
  four walks abreast into the old town and people fill the street beyond.
- **17:00 on 21 December:** almost black. The sun is 4 degrees below the
  horizon, where the client's exposure has not reached its night floor and
  the night fill has only begun (ADR-0022 fades it in from 1 to 8 degrees
  below): people show only as silhouettes against lit shop windows. This
  is the dusk look, not the crowd, but M2's December screenshots at this
  hour would show no one. Extra screenshots at 15:00 on 21 December show
  winter crowds in dark coats and hats, couples and groups: a busy
  afternoon.
- **What looks wrong:**
  - box figures with blank faces: placeholders until ADR-0017's models;
  - clones: in the group of four at the Rathausbrücke two members look the
    same (48 looks a season, drawn for each member independently), and the
    dark winter palette makes many people look alike;
  - lines of people: keeping right puts walkers in a column on one half of
    a wide pedestrian street (the Rathausbrücke at 15:00 in December), and
    on squares people follow the same straight lines from one entry point
    to another; real crowds spread over the width;
  - people walk through the camera: nobody gives way to the player (not
    built), and at Lindenhof and Hirschenplatz someone passes within an
    arm's length of the eye;
  - not visible in stills: stepping aside and a group's changing stagger
    move people sideways without steps (up to 5 cm a tick), and the near
    step cycle advances 4 cm at a time; otherwise planted feet stay put,
    since the cycle follows the distance walked;
  - squares are not empty, but only the larger ones have standing groups,
    nobody sits (no benches in the data), and nobody was seen inside a
    wall.

**What it means for RFC 0005** (the navigation-data RFC, a Draft in pull
request #7):
- **Per-lane numbers need a way to be met.** A walk from places, weighted
  by lane density alone, gave pedestrian streets half their share and
  steps three times theirs: flows are decided at junctions, and chains of
  lanes take what their junction gives them. The prototype met its
  targets only by calibrating a weight per lane at load (8 rounds of 8,000
  sampled trips, about a second for the district). RFC 0005 stores
  `people_per_day` per lane and `trips_per_day` per place; the two must
  agree (the trips from places make the people on lanes), so either the
  ingest makes them consistent, or places carry relative weights and the
  population function derives the trips, as the prototype did. Whether the
  calibrated weights are worked out by the function (and so belong to its
  version) or baked by the ingest is a choice for the RFC.
- **A centred `width_mm` is not enough.** Keeping people off walls took
  the free room on each side of a lane, measured against the shells every
  0.5 m (walls jut out between OSM vertices; rays every 1 to 2 m left
  people up to 0.35 m inside shells), and each junction's clearance from
  walls, because people turning there cut the corner. The RFC could store
  room left and right per point (or a width and an offset), and a
  clearance per node, both from the matching to shells, marked as
  estimated.
- **Areas need their own numbers.** Joining the points where lanes reach
  a square, through a centre, made 28.5 km of `area` lanes in 34 areas, 38%
  of all lane length, crossing each other; a density per lane would have
  filled squares many times over. The prototype gave each area a density
  per hectare (200 people at the reference) spread over its lanes. The
  RFC's areas could carry that, or `people_per_day` for `area` lanes could
  be defined from the area's.
- **Standing places must lie off the lanes.** With spots on the lines
  people walk, walkers passed through standing groups; spots at least
  1.8 m from any lane fixed it, at the cost of most spots in small squares
  (193 candidates down to 66). Anchors of `square`, `viewpoint` and
  `fountain` places want the same rule.
- **Ranges and matching hold up.** The prototype's marks match the RFC's
  `entrance` and `passage` ranges. Clearing dropped 98 lanes (2.4 km of
  77 km): 31 ran into a wall, 31 needed a shift at an end, 22 more than
  1 m, 11 ended at a node that could not be cleared, 3 had walls on both
  sides. RFC 0005's `blocked` range keeps such lanes in the graph instead
  of dropping them, which is better: dropping can cut a street in two.
  Walls must be cut at a height above the walking surface, and two walls
  equally far away must not decide a point's side between them (the
  prototype's first signed distance flipped sign there).
- **Doors from the cache alone are few and estimated:** 80 paths that end
  at a shop or public building, and 253 doors placed on outlines nearest a
  lane; benches, terraces, stops and entrances need the RFC's node query.
  Trips that end at a door in view were held standing there until out of
  view (about one person in view at a time); real doors drawn there would
  let them walk in, but walking in is a vanish in view.
- **Heights:** the highest non-building surface (terrain or a transport
  deck) worked for bridges and steps, as the RFC's `bridge` rule says.
- **Not exercised:** tile cuts and joining (the prototype used one graph
  for the district), `forward_permille`, opening hours, holidays.

**What it means for C2 against the fallback (a seeded A):**
- **C2's core holds in the district.** A pure function, keeping no state,
  gave the same people on two runs, started and ended every trip at a
  place, kept people on lanes and out of shells, followed the profiles
  within 21%, and nothing appeared or vanished in view, with the camera
  delaying both the start and the end of trips seen in view. All six
  checks pass at all three stops on both dates, as defined.
- **It needs both kinds of avoidance.** Built into the function alone, or
  client offsets alone, fails check 2 in five of six cases (21 to 93
  overlaps a minute); together they pass everywhere (0 to 9). So C2 comes
  with a near tier in the client after all: offsets near the camera that
  fade back to the function's path. They decide nothing about who is
  where, but they are the part of A that C2 needs.
- **The fallback is not needed by these checks.** What a seeded A would
  add (any steering, people that react) is partly here already in the
  offsets; what it would lose (the same people across machines and camera
  paths) is shown working. Recommended: keep C2, with client offsets as
  its near tier, and fix the per-tick cost before the route is measured.
- **Limits of the evidence:** three stops, two times, one seed and a
  camera that turns in place; overlaps close to the threshold at
  Lindenhof in December; a cost that passes at the median but not at the
  99th percentile; and people held at doors until the player looks away,
  which a player who walks could notice.

**Checks:**
- `cargo fmt --all --check` and `cargo clippy --workspace --all-targets
  --locked -- -D warnings`: pass.
- `cargo test --workspace --locked`: 273 passed, 1 ignored. New in
  `op_client`: the population function gives the same people for the same
  inputs whatever was asked before, and other people for another seed;
  trips start and end at places, and their legs join; counts follow the
  profile (on a grid of streets, averaged over three seeds, within -20 to
  +25% by day and -40 to +50% at 3 a.m., and night far below the
  afternoon); more than half the people in groups, a mean speed of 1.1 to
  1.4 m/s and half speed on steps; nobody moves more than 12 cm in a
  tick, with or without
  avoidance; people stand and sit at spots; the function's file names no
  float type, unordered map or clock; the calendar's day types; the
  figures' stride matches the function's; looks vary and follow the
  season.
- `op_replay check fixtures/replays/sample.oprep`: ok, 20 checkpoints,
  final state `212bd353…`, unchanged: the simulation is untouched.

**Where things are** (outside the repository, in the session's scratch
folder `crowd/`): `scripts/convert.py` (the conversion from the OSM
cache), `data/lanes.txt` (its output, 240 KB), `formal/` (the 12 check
reports and logs, the 4 cost reports, and the screenshots), and
`formal/extra/` (December at 15:00).

**Still open:**
- **The per-tick cost** at the 99th percentile (above).
- **The look:** the dusk exposure at 17:00 in December; lines of people
  on wide streets (spread walkers over the width by speed and group, not
  only to the right); clones within a group (draw members' looks without
  repeats); figures until ADR-0017's models.
- **Not built:** giving way to the player and glances; stair animations;
  sitting (the cache has no benches or terraces); trams at stops; a
  cross-fade when the clock moves (it cuts).
- **Not tested:** a walking camera, travel and the T key in a run; F2
  hides the crowd in code but was not checked in a run; the function on
  Linux and macOS, or in a debug build (the golden test ADR-0021 asks
  for).
- **One jump** at Lindenhof in June, not traced.
- **RFC 0005:** the points above.

### RFC 0005 revised after the crowd prototype (2026-11-25)

[RFC 0005](../rfcs/0005-navigation-data.md) said it would be revised
after ADR-0021's client prototype; this is that revision, on branch
`nav-rfc-prototype`. It stays a Draft, with comments until at least
2026-12-09. **Nothing is decided.**

| What the prototype found | What changed in the RFC |
|---|---|
| Per-lane numbers were met only by calibrating walk weights at load, with a factor per time | The numbers are targets. New choice N1 to N3 (who calibrates): **N2**, the population function calibrates within its version, over a unit the data fixes, never over what is loaded; place trips become weights that the function scales, and lanes and areas win. Agreement is checked by G1's listing of nodes no walk can meet and by the function's census |
| One centred width was not enough | New `room_mm` per segment (room free of walls on each side, measured every 0.5 m against shells cut 0.5 m up) and `clearance_mm` per node, from the matching. New choice R1 to R4: **R1**, stored; G11 checks the room claimed. Width narrowing near walls is gone |
| Area lanes were 38% of lane length | Areas carry `curve` and `people_per_ha`; `area` lanes carry no number of their own |
| Standing spots on lanes put walkers through groups | New `spots_mm` on stops, squares, viewpoints and fountains, at least 1.8 m from every lane; seats outside every lane's band; G1 checks both |
| 98 lanes (2.4 km) were dropped | The ingest never drops a lane: it marks `blocked`. Probes every 0.5 m, not 1 m, in the ingest and G11 |
| No benches, terraces, entrances or tram stops in the cache | Evidence added for the node query |
| Both kinds of avoidance were needed | The client section names the near tier and what it needs from the data (room on each side); sub-lanes by direction need no data of their own |
| About 2 ms a tick at the 99th percentile | Named as the client's cost; precomputed weights (N3) would save load time, not this |

- **Also:** the reference for numbers of people present (the busiest
  weekday hour at a month factor of 1,000, as the prototype set its
  densities); the prototype's starting values (densities per lane kind,
  200 people per hectare of square, occupancy shares, month factors)
  in the ingest section; K1's amendment now covers measuring room and
  choosing spots; the size estimate rises to about 3 to 3.5 MB for the
  district.
- **Unresolved, updated:** what the prototype did not exercise (tile
  cuts and joining, `forward_permille`, opening hours, holidays,
  sitting, a walking camera, more than one seed); calibration at a
  city's scale; how area lanes are laid and walkers spread over squares;
  doors in view.
- **No code, schema, fixture or replay changes.**

**Waiting for the maintainer,** besides the list in the RFC's first
entry: N2 against N3 (who calibrates), and R1 (room stored in the data)
against computing it in the client.

### Dusk (2026-11-25, ADR-0022 N2)

At 17:00 on 21 December (sun 4° down, civil twilight) the client showed
an almost black picture, lit windows only, although real civil twilight
is bright: hundreds of lux at sunset, tens with the sun 3° down, a few at
6° down. Only `op_client` changed (`sky.rs`, and the F3 panel in
`hud.rs`); the simulation, the world format and the replays did not. No
ADR changed: N2's starting values at night are kept.

**The cause** (checked before changing anything):
- The client faded the sun's light out as it went from 2° up to 3° down.
  The sun is also what lights Bevy's atmosphere, so the sky and the light
  it gives the scene (the atmosphere's environment light) went out with
  it: from 3° down the sky was black and only the night fill, still
  fading in, lit the street.
- The exposure was still close to daylight: the old curve, only partly
  pulled down by the night factor (0 at 1° down, 1 at 8° down). The F3
  panel read EV100 7.6 at 16:50 (2.6° down), 6.2 at 17:00 (4.0° down)
  and 4.7 at 17:10 (5.5° down).
- Together, a grey surface showed about a twentieth of its night
  brightness at 4° down. Main's screenshots at Hirschenplatz: readable at
  16:40 (1.2° down), dark at 16:50, black at 17:00 and 17:10, readable
  again from 17:30 (8.5° down). Lindenhof and Rathaus were black at 17:00
  too, and Hirschenplatz at 21:50 on 21 June (3.9° down).

**What Bevy does with a sun below the horizon** (measured for this fix):
- Its lighting already hides a directional light that has set (the
  atmosphere's transmittance and the share of the sun's disc above the
  horizon, in `pbr_lighting.wesl`), so a sun kept on below the horizon
  only lights the sky and the light the sky gives.
- But its atmosphere's twilight falls far more slowly than the real one.
  With the sun at full strength, its light on level ground was 2,900 lx
  with the sun on the horizon, 430 lx at 4° down and 4 lx at 10° down, against 980, 24 and
  0.04 lx in the USNO model below: three, eighteen and a hundred times too
  bright. Measured with the release client at Hirschenplatz on 21
  December, every five minutes (about 0.7°) from 2.2° up to 11.6° down,
  from screenshots without tonemapping at a known exposure, against a
  known ambient light (a temporary patch, not committed). Lindenhof
  agreed within 14% at three elevations, and a repeat within 0.1%.

**The fix (detailed look only):**
- **Twilight from the sky:** below the horizon the sun keeps the strength
  that makes the atmosphere's light on level ground the real twilight's
  (the USNO table over the measured one), and at least the old fade, down
  to 10° below; below that nothing, as before. Measured again with the
  fix: 100, 25 and 5.5 lx at 2.6°, 4.0° and 5.5° down, against the
  model's 99, 24 and 5.2. The sky and the light it gives stay one thing,
  coloured and directional by the atmosphere (lavender overhead, rose
  towards the sunset), cooler than the warm fill that takes over.
- **Exposure from the light in the scene:** below 2° up, EV100 = 3 + 0.9
  × log2(light / 10.6 lx), never above the daylight curve and never below
  the floor of 3. The light is the atmosphere's (as above), the moon's
  and the fill's, as they fade in. 10.6 lx is the fill's 5.7 lx plus a
  full moon's 5.0 lx before the air dims it, so every night stays on the
  floor; the slope 0.9 (a look choice) lets dusk darken a little where 1
  would keep surfaces as bright on screen as the light falls.
- **Why not an ambient twilight term,** as suggested: it would have lit
  the street from the same table, but the atmosphere stays black from 3°
  down, so the sky would have needed an invented dome of its own. Scaling
  the sun keeps the sky and its light in agreement, with the atmosphere's
  colour and direction, and adds no mesh or light. The price is a
  measured table, to measure again if Bevy or the atmosphere's settings
  change.
- **Unchanged:** everything with the sun 2° up or higher, and with it 10°
  or more down (no sun, the floor); ADR-0022's night values (EV100 3, the
  moon, the glow, the fill); and the plain look (F2), which keeps the old
  fade and exposure, and so is still dark at dusk. Shadows still end with
  the old fade at 3° down: nothing on the ground sees the sun by then.
  The floor is now reached between 5.2° and 6.2° down (6.2° under the
  brightest moon) instead of at 8°.
- **The F3 panel** shows the camera's exposure instead of the old curve.

| Value | Value | Source |
|---|---|---|
| Twilight on level ground, clear sky | 1950 lx at 2° up, 984 at 0°, 387 at 1° down, 169 at 2°, 66 at 3°, 24 at 4°, 8.7 at 5°, 3.0 at 6°, 0.35 at 8°, 0.04 at 10°, by whole degrees | U.S. Naval Observatory: Janiczek and DeYoung, "Computer Programs for Sun and Moon Illuminance With Contingent Tables and Diagrams", USNO Circular 171, 1987 (its REFR and ATMOS, 133,775 lx times ATMOS's factor) |
| Bevy's atmosphere, sun at full strength | 5070 lx at 2° up, 2910 at 0°, 432 at 4° down, 4.2 at 10° down | measured, above |
| Dusk exposure | 3 + 0.9 × log2(light / 10.6 lx), between 3 and the daylight curve | 0.9 a look choice |

**Exposure** at Hirschenplatz on 21 December, before and after: 16:20
(1.6° up) 9.7 and 9.7; 16:40 (1.2° down) 8.6 and 8.4; 16:50 (2.6° down)
7.6 and 5.9; 17:00 (4.0° down) 6.2 and 4.2; 17:10 (5.5° down) 4.7 and
3.0; from 17:20 (7.0° down) 3.0 (before: 3.5, then 3.1 at 17:30). On 21
June, after: 21:10 (1.4° up) 9.7; 21:30 (1.3° down) 8.1; 21:50 (3.9°
down) 4.3; 22:30 (8.7° down) 3.0 (before 3.06).

**What the screenshots show** (1280 × 720, high, release build; kept
outside the repository):
- **Hirschenplatz every ten minutes from 16:20 to 18:10 on 21
  December:** the picture darkens smoothly and never goes black. The sky
  goes from pale blue through lavender (16:40 to 17:10) to the night's
  orange-brown (from 17:20); facades and setts stay readable at every
  step, and lit windows appear from 16:50 and brighten as the exposure
  falls. Mean pixel level of the setts, of 255: 73, 57, 35, 30, 26, 22,
  16, 15, then 15 to the end (before: 73, 57, 31, 5, 0, 1, 8, 13, 15).
- **Lindenhof and Rathaus at 16:40, 17:00 and 17:20:** a rose sunset sky
  over the city at 16:40, lavender at 17:00 with the far city's windows
  coming on, the night's sky at 17:20; before, both were black at 17:00.
- **21 June at 21:10, 21:30, 21:50 and 22:30:** the same; 21:50 (3.9°
  down) was black before. 22:30 is 8% brighter on average than before
  (the floor reached, and the last of the twilight).
- **12:00 and 18:00 on 21 December at the three stops:** unchanged.
  Against main, the mean difference per pixel was 0.19 to 0.28 (of 255)
  at noon and 0.06 to 0.08 at 18:00; between two runs of the same build,
  0.15 to 0.31 at noon and 0.03 to 0.08 at 18:00 (seven runs at
  Lindenhof at 18:00: 0.04 to 0.08 among main's, 0.05 to 0.08 between
  main and the fix).
- **Found on the way:** the moon is drawn as a small bright disc wherever
  it is in view, at dusk as at night, on main too: the moon's light has
  no `SunDisk`, and Bevy then gives it the sun's.

**Checks:**
- `cargo fmt --all --check` and `cargo clippy --workspace --all-targets
  --locked -- -D warnings`: pass.
- `cargo test --workspace --locked`: 266 passed, 1 ignored. New or
  changed in `op_client`: the twilight table matches the USNO model
  (within 0.5%), with hundreds of lux at sunset, tens at 3° down and a
  few at 6° down; the sun's strength is the old fade in the plain look
  and from 2° up, nothing below 10° down, and from 3° down gives the
  ground exactly the table's light; the exposure is the daylight curve
  from 2° up, the floor from 7° down whatever the moon, and never rises
  as the sun goes down; and a mid-grey surface under the scene's light,
  as exposed, stays between the night's brightness and four times it
  from sunset to night, darkening all the way (before the fix it fell
  below a tenth of the night's at 4° down). The plain look's exposure is
  the old one.
- `op_replay check fixtures/replays/sample.oprep`: ok, 20 checkpoints,
  final state `212bd353…`, unchanged.

**Still open:**
- **ADR-0022's open question 2:** the slope 0.9 joins the night's
  starting values; the maintainer sets the final ones.
- **The plain look at dusk** is still dark, as it was: it keeps the old
  fade and exposure on purpose.
- **Clear skies only:** the USNO figures are for a clear sky; weather is
  not in M2.
- **The atmosphere table** was measured at street level at one stop on
  one date and checked at a second; the map view and high flying were
  not measured, and the table must be measured again when Bevy or the
  atmosphere's settings change.
- **The moon's disc** (above): give the moon a `SunDisk` of its own size,
  or none.

### Window glass and the moon (2026-11-25)

Two look defects fixed in `op_client` only (`materials.rs`, `sky.rs`); the
world format, the meshes and their hashes, the simulation and the replays
did not change. No ADR changed.

**The diagonal seam on window glass.** Near windows were cut along the
diagonal between a wall's two triangles: the glass and frames on one side
sat elsewhere than on the other, so by day one triangle showed glass
reflecting the sky where the other showed plaster, and at night one half
of a window was lit and the other not.
- **The cause** (checked with a temporary test that ran the client's wall
  code over every windowed wall of the world and compared each corner
  shared by two nearly coplanar triangles): the shells' wall triangles
  differ a little in their normals, as suspected, but the normals were not
  what showed. Each triangle took its direction along the wall from its
  own normal, and the texture coordinate along the wall is measured from
  the frame's origin, 2 to 2.6 km away, so a hundredth of a degree moved
  one triangle's windows about 0.4 m against its neighbour's. Of the
  edges between triangles 0.001° to 0.01° apart, 31% had windows shifted
  by more than 2% of a bay; over all walls, 35,875 of 268,842 shared
  edges had a shift of 5% or more of a bay or storey (whole bays aside).
  Walls split into several plane keys also fitted their bays and storeys
  per key, and the keys move by metres for the same reason. The normals
  themselves differed by more than 0.5° at only 8,501 edges; a 0.1°
  difference turns the reflection by 0.2°, too little to see.
- **The fix:** triangles that lie in one plane (within 0.2 m of the
  plane and at most 10° off it, or within 2 cm whatever their normal, as
  slivers are) are drawn as one wall: one direction along the wall, one
  fit of bays and storeys, one normal (the area-weighted mean of the
  triangles'), and all wound to face the same way. The grouping measures
  nothing from the frame's origin. Lit windows keep the seed of the
  wall's largest triangle, so walls that had no seam keep their lit
  windows. Shared edges with a shift of 5% or more: 926 of 280,262. The
  cost is unchanged: the wall code for all 1,369 shells takes 402 ms in
  total against 470 ms before (one run each, test build).
- **Screenshots** (1280 × 720, high, release build; kept outside the
  repository): at Hirschenplatz and Rathaus at 12:00 and 18:00 on 21
  December and 22:30 on 21 June, windows cut along a diagonal (with a
  lit and an unlit half at night) and the jagged ground floor of the
  corner building at Rathaus are whole now.

**The moon drawn as a second sun.** The moon's light had no `SunDisk`, so
Bevy's atmosphere drew the sun's for it (0.533°), as bright as the light
spread over that disc: about 1,800 times the exposure's white at 18:00 on
21 December, a blown disc with a sun's bloom. Bevy 0.20.1 draws a light's
disc (`sample_sun_radiance` in the atmosphere's shaders) as a round,
evenly bright circle: a size, and an intensity times the light over the
disc; no phase and no texture.
- **The fix:** the moon's disc is 0.518° (its mean diameter), as bright
  as the real moon (about 4,800 cd/m² when full, dimmed by the phase) but
  at most 6 times the exposure's white, a look choice; it appears as the
  sun goes from 1° to 3° below the horizon (the moonlight starts at 1°
  down). Since the disc cannot show a phase, only a nearly full moon has
  one, fading in from 85% to 95% lit; other moons have none.
- **Screenshots:** at Lindenhof looking east 25° up on 21 December (92%
  lit), at 16:50, 17:00 and 18:00 the moon is a small white disc, about
  5 pixels, with little bloom (at 18:00, 28 pixels more than 60 levels
  above the sky and 69 more than 10, against 118 and 519 before); it is
  pale against the twilight at 16:50, as a real moon is. At 22:30 on 21
  June (50% lit) the half moon no longer shows as a full bright disc.

**The rest unchanged:** 12:00 and 18:00 on 21 December at the three stops
against main. The street, the sky and the roofs are within run-to-run
noise (the street: 0.28 to 0.45 of 255 against 0.44 to 0.62 between two
runs of main at noon, 0.06 to 0.10 against 0.05 to 0.07 at 18:00). Over
the whole picture the mean difference is 0.7 to 7.7 against 0.05 to 0.33,
all of it on facades: windows moved on walls made of several triangles,
which each had their own frame (1.2% of pixels changed by more than 20
levels at Lindenhof, 6.0% to 7.4% at Hirschenplatz and Rathaus, against
0.01% between two runs of main); at 18:00 some of those walls' windows
light differently.

**Checks:**
- `cargo fmt --all --check` and `cargo clippy --workspace --all-targets
  --locked -- -D warnings`: pass.
- `cargo test --workspace --locked`: 272 passed, 1 ignored. New in
  `op_client`: a bent wall far from the origin draws as one plane, with
  one normal between its halves' and the same texture coordinates at
  every shared corner, a sliver included; a triangle wound the other way
  is turned to face its wall's way; a narrow return keeps its own
  normal; the moon's disc is 0.518°, smaller than the sun's; it is drawn
  at the luminance asked for, white but at most 6 times white (1,800
  times before); only a nearly full moon has one, from 1° to 3° after
  sunset.
- `op_replay check fixtures/replays/sample.oprep`: ok, 20 checkpoints,
  final state `212bd353…`, unchanged.

**Still open:**
- Where a facade steps or bends by more than 0.2 m (two planes), each
  plane keeps its own fit of bays and storeys, so a window can still be
  cut along a vertical line there, as on the west front of the corner
  building at Rathausbrücke (a 0.45 m step).
- A moon with its phase needs a disc the client draws itself; Bevy's is
  round.
- The disc's 6 times white and the phase threshold are starting values
  for ADR-0022's open question 2.

### Every decision record accepted (2026-11-25)

The maintainer accepted every remaining record with its recommended
options ("all approved, make choices and merge"): ADR-0010 to ADR-0019,
RFC 0001 to RFC 0005, and the questions left open in ADR-0020 to
ADR-0022. Each record says so in its status and Decision section, and
each open question with a recommendation is marked "Decided
2026-11-25". Questions with no recommendation stay open, in the text.

**The RFCs were accepted before the end of their comment periods**
(GOVERNANCE.md asks for at least 7 days; RFC 0003 and 0004 named
2026-12-01, RFC 0005 2026-12-09), by the founding maintainer's decision:
no comments had been received and the project had not been announced.
Each RFC records this.

**Choices worth knowing:**
- **World format 0.1.0 is frozen:** RFC 0001 with the amendments of RFC
  0003, 0004 and 0005 (RFC 0004's contract-version option A). A later
  rule change to the format, such as ADR-0017's allowlist rules, needs
  0.2.0, which changes every id.
- **JSON size limits** (RFC 0001): 1 MiB for manifests and merge
  records, 8 MiB for change sets, 16 MiB for check reports and tile
  indexes; points and navigation documents keep 64 MiB until their own
  limits are set. ADR-0020's review question on JSON size follows it.
- **RFC 0002's amendments A1 to A8** are decided with their
  recommendations, the contract amended in place and `GATE_VERSION`
  raised as each is built: A7 now (refresh sets merge only on their base
  world); A1 and A8 before the first height-model refresh; A3 before
  M3; A2, A4 and A5 later.
- **Hosting** (ADR-0010) is option A, a world repository, a public
  bucket and a merge bot; the ADR recommends no provider.
- **The night's values** in use are the decided starting values
  (ADR-0022, question 2): the photo comparison is still to do.

**For the maintainer** (what code cannot do):
1. Choose a hosting provider, open the account, name who pays, create
   the world repository and the merge bot's credentials (ADR-0010).
2. Write to the OpenStreetMap Foundation's Licensing Working Group, and
   confirm swisstopo's terms (ADR-0019).
3. Write to ewz and Grün Stadt Zürich, and publish the fixture's two WFS
   answers as a release asset (ADR-0015, ADR-0022).
4. Ask the City of Zurich about hystreet's licence version (ADR-0021).
5. Ask CMU about its motion-capture licence (ADR-0017).
6. Obtain a card of the RX 6600 class (ADR-0018).
7. A Code of Conduct contact and branch protection (M1).

**Work that follows, in order:**
1. A7: refresh sets merge only on their base world.
2. RFC 0001's JSON size limits in `op_world` and `tools/worldfmt`, and
   ADR-0014's bound on repairing water rings in the ingest.
3. The benchmark route (ADR-0018), so each next step is measured.
4. RFC 0004's point features end to end: schemas, `op_world` and
   `tools/worldfmt`, the gate, the Zurich ingest (ADR-0015, ADR-0016),
   and the client's trees and lamps (ADR-0022), with fixtures and pinned
   ids updated.
5. RFC 0005's navigation layer and G11, then the crowd (ADR-0021).
6. The drafts the decisions call for: ADR-0017's RFC, the navigation
   ingest's ADR.
7. The ODbL notices in the client and the store (ADR-0019).

### JSON size limits (2026-11-25, RFC 0001)

RFC 0001's decided size limits (section 7, option 1) are enforced in
`op_world` and `tools/worldfmt` alike. Reader fuzzing found the problem:
serde_json takes 16 to 105 bytes of memory per byte of text, so a
crafted 64 MiB document took about 7 GB.

| Kind | Limit |
|---|---|
| asset manifest, merge record | 1 MiB |
| change set | 8 MiB |
| check report, tile index | 16 MiB |
| world manifest | 64 MiB, the limit of every file (none of its own) |

Points and navigation documents keep 64 MiB until their own limits are
set; nothing reads them yet.

- **Where:** the limits are defined once, in `op_world::limits`
  (`Document`) and `tools/worldfmt/limits.py`.
  - `from_bytes` of every document type refuses a longer document before
    parsing it, and `to_bytes` refuses to write one, so a merge never
    writes a record (too many reviews) that a later merge could not read.
  - Where the kind is known, a longer file is refused before it is read
    (`store::read_document`, `Store::read_document`): the gate, `validate`,
    merges, `--require-base`, `WorldLog::open`, `op_replay`'s complete
    gate and `op_stream`.
  - Python: `changes.parse`, `to_bytes`, `Objects` and `apply`; `log`'s
    records, change sets, reports and worlds; the planner's `verify`
    (the gate's reports) and the facade tool.
- **Messages** name the kind and the limit, the same in both languages:
  "larger than 1048576 bytes (1 MiB), the limit for an asset manifest".
- **Gate 0.1.3.** G1's output changes for oversized inputs, so its version
  changes: a change set over 8 MiB, a bundle manifest over 1 MiB or a
  candidate tile index over 16 MiB fail G1 with the new message. The
  sample report changes only in `gate_version`: report `f6e24f74…`.
  RFC 0004's gate change takes the next free version, 0.1.4.
- **Valid input is unchanged:** valid documents read up to exactly their
  limits, and every fixture and pinned id is unchanged but the sample
  report. The largest valid documents are far below the limits (Zurich:
  manifests 4,108 bytes, tile indexes 59,148).
- **Time:** the worst merge record Python can now be given (1 MiB of
  short reviews) takes about 2 s to check, where 64 MiB took minutes.
- **Tests:** a valid document of each kind at its limit reads and one
  byte more is refused, in Rust and Python; the worst case for the
  parser (one-key objects) one byte over each limit is refused with the
  memory of the input alone; files over their limit are refused by store
  reads, validation, the gate (G1), `--require-base`, the world log, the
  planner's `verify` and the facade tool (in Rust before they are read);
  writers refuse what readers would; Python's limits and messages are
  checked against `limits.rs`, and the gate's G1 against Python's
  messages (`STREETWIDE_GATE=1`).
