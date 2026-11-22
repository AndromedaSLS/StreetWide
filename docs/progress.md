# Progress

The first milestone is a walkable, deterministic slice of Zurich's old town
(about 1 km²) built only from open data. Work is split into six stages, each
ending at a checkpoint where the checks are run and the result is reviewed.

| Stage | Scope | Status |
|---|---|---|
| 1 | Scaffold: workspace, licences, community docs, CI | ✅ checkpoint reached 2026-11-22 |
| 2 | World format: JSON Schemas, `op_world`, RFC 0001 | not started |
| 3 | Ingest: swisstopo + OSM → world tiles, one fixture tile | not started |
| 4 | Kernel, simulation, replay: 60 Hz, Rapier capsule, state hashes, replay CLI | not started |
| 5 | Client and streaming: Bevy first-person walking, replay recording | not started |
| 6 | Docs and review: ADRs, README run guide, good-first-issues | not started |

District bounding box (WGS84): `8.5377,47.3685,8.5507,47.3775`, the largest
lon/lat rectangle inside the LV95 km square E 2'683'000–2'684'000 /
N 1'247'000–1'248'000.

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
  direction, ADR-0005 toolchain and engine pinning (all *Proposed*); ADR and
  RFC templates.
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
| GitHub Actions CI on `main` | pending push |

**Open items**

- CODE_OF_CONDUCT contact address: placeholder until a private reporting
  address exists; must be filled before the project is announced.
- ADR-0001, 0002 and 0005 are *Proposed* until the checkpoint is approved.
