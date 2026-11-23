# Progress

The first milestone is a walkable, deterministic slice of Zurich's old town
(about 1 km²) built only from open data. Work is split into six stages, each
ending at a checkpoint where the checks are run and the result is reviewed.

| Stage | Scope | Status |
|---|---|---|
| 1 | Scaffold: workspace, licences, community docs, CI | ✅ done 2026-11-22, CI green |
| 2 | World format: JSON Schemas, `op_world`, RFC 0001 | ✅ done 2026-11-23, CI green |
| 3 | Ingest: swisstopo + OSM → world tiles, one fixture tile | ✅ checkpoint reached 2026-11-23 |
| 4 | Kernel, simulation, replay: 60 Hz, Rapier capsule, state hashes, replay CLI | not started |
| 5 | Client and streaming: Bevy first-person walking, replay recording | not started |
| 6 | Docs and review: ADRs, README run guide, good-first-issues | not started |

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

**Open items**

- ADR-0006 is *Proposed*; it becomes *Accepted* with this checkpoint's
  go-ahead.
- Known gaps for good-first-issue drafts (stage 6):
  - no water surface (the Limmat is walkable terrain at water level);
  - squares mapped as areas are not drawn;
  - bridges that end at building doors are anchored to the ground;
  - no levels of detail.
- Rathausbrücke is tagged `highway=construction` in OSM (it is being
  rebuilt), so it is absent; the temporary bridge next to it is present.
- Ingest output is reproducible on one platform. Across platforms, libm,
  PROJ or GEOS differences could in rare cases move a vertex by 1 mm.
