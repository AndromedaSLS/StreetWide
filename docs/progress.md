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
