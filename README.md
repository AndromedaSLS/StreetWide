# StreetWide

**An open-source, planet-scale open world, built and improved by its players.**

An AI world compiler turns open map data into a street-level baseline of the
Earth. Players then refine it: better facades, interiors, furniture, things
to do. A contribution is merged into the canonical world only if it passes
the checks and beats what is already there, so the world only ever gets
better.

> **Status: pre-alpha.** We are building the first walkable slice: about
> 1 km² of Zurich's old town, built only from open data (swisstopo and
> OpenStreetMap). There is no ML, networking or economy yet. Progress is
> tracked in [docs/progress.md](docs/progress.md).

## Principles

- **The canonical world only moves upward.** Every merge must pass the checks
  and beat the current version.
- **The simulation is deterministic.** The same inputs give the same world,
  bit for bit, on every platform. Rendering may differ.
- **Contracts before features.** The world format, module interfaces and
  tests come first.
- **Every asset carries provenance and licence** in its manifest.
- **Private by design.** Interiors are fictional, never replicas of real
  homes.

## Repository layout

| Path | What it is |
|---|---|
| `crates/op_kernel` | Module API, fixed 60 Hz schedule, deterministic math, seeded RNG, state hashing |
| `crates/op_sim` | Deterministic simulation (Rapier physics, player capsule) |
| `crates/op_replay` | Replay format and the `op_replay` CLI |
| `crates/op_world` | World format: manifests, content ids (BLAKE3), glTF validation |
| `crates/op_stream` | Tile store and streaming |
| `crates/op_client` | Desktop client (Bevy) |
| `schemas/` | JSON Schemas for the world format *(stage 2)* |
| `tools/ingest` | Python ingest from open data *(stage 3)* |
| `tools/ci` | CI helpers: dependency-direction and DCO checks |
| `docs/adr`, `rfcs/` | Architecture decision records and RFCs |

Dependencies point inward: `op_sim` depends only on `op_kernel`, nothing
depends on `op_client`, and only `op_client` may use Bevy. CI enforces this
([ADR-0002](docs/adr/0002-crate-layout-and-dependency-direction.md)).

## Building

You need:

- Rust via [rustup](https://rustup.rs). The pinned toolchain in
  `rust-toolchain.toml` installs automatically on the first `cargo` command.
- On Windows: Visual Studio Build Tools with the "Desktop development with
  C++" workload.
- On Linux: Bevy's system packages
  ([list](https://github.com/bevyengine/bevy/blob/main/docs/linux_dependencies.md)),
  e.g. on Debian/Ubuntu
  `sudo apt-get install g++ pkg-config libx11-dev libasound2-dev libudev-dev libxkbcommon-x11-0 libwayland-dev libxkbcommon-dev`.
- Python 3.11+ for the tools.

```sh
cargo run -p op_client                      # the client (an empty window for now)
cargo test --workspace                      # tests
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
```

Complete instructions for downloading the district and walking it will be
added as the stages land.

## Contributing

Contributions are welcome. Please read [CONTRIBUTING.md](CONTRIBUTING.md).
Every commit must be signed off under the
[Developer Certificate of Origin](https://developercertificate.org)
(`git commit -s`). We follow a [Code of Conduct](CODE_OF_CONDUCT.md).
Governance is described in [GOVERNANCE.md](GOVERNANCE.md), and security
reports go through [SECURITY.md](SECURITY.md).

## Licensing

| What | Licence |
|---|---|
| Code | [MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE), at your option |
| Contributed world content (models, textures, manifests) | [CC BY-SA 4.0](LICENSES/CC-BY-SA-4.0.txt) |
| Layers derived from OpenStreetMap | [ODbL 1.0](LICENSES/ODbL-1.0.txt) |
| Layers derived from swisstopo data | swisstopo terms of use (free use, attribution required) |

Data sources and the required credits are listed in
[ATTRIBUTION.md](ATTRIBUTION.md). The rationale is in
[ADR-0001](docs/adr/0001-licensing.md).

Unless you explicitly state otherwise, any contribution of code you
intentionally submit for inclusion in this project, as defined in the
Apache-2.0 licence, is dual-licensed as above, without any additional terms
or conditions.
