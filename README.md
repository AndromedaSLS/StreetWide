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
| `schemas/` | JSON Schemas for the world format ([RFC 0001](rfcs/0001-world-format.md)) |
| `fixtures/` | Small committed test data: the world-format golden fixture and one real tile ([fixtures/README.md](fixtures/README.md)) |
| `worlds/` | World definitions for the ingest (area, spawn point) |
| `tools/ingest` | Python ingest: builds a world from swisstopo and OpenStreetMap data ([ADR-0006](docs/adr/0006-ingest-pipeline-and-data-sources.md)) |
| `tools/worldfmt` | Python implementation of the world format, byte-compatible with `op_world` |
| `tools/ci` | CI helpers: dependency-direction, DCO and glTF checks |
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
cargo run -p op_client                      # walk the world (see below)
cargo test --workspace                      # tests
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
cargo run -p op_replay -- check fixtures/replays/sample.oprep   # determinism check
```

`op_replay check` replays a recorded walk through the Grossmünster tile
and verifies that the simulation reproduces every recorded state hash
([ADR-0003](docs/adr/0003-determinism-strategy.md),
[ADR-0007](docs/adr/0007-replay-file-format.md)). `op_replay info`,
`trace` and `run` show a replay's header, the player's path and the final
state hash.

### Building the Zurich district from open data

The ingest downloads about 275 MB (one swissBUILDINGS3D sheet, one
swissALTI3D tile and one OpenStreetMap query) into `data/cache/`, then
writes the world to `data/world/zurich-altstadt/` (about 50 MB). Nothing
there is committed. A first run takes about 4 minutes; later runs reuse the
cache.

```sh
python -m venv .venv
.venv/bin/pip install -r tools/requirements.txt       # Windows: .venv\Scripts\pip
.venv/bin/python -m tools.ingest --world zurich-altstadt
cargo run -p op_world -- validate data/world/zurich-altstadt
```

Any other area works too, as long as it lies in one z12 region in
Switzerland: `python -m tools.ingest --bbox minlon,minlat,maxlon,maxlat --name my-area`.
Run `python -m tools.ingest --help` for the options (`--offline` uses only
cached data; `STREETWIDE_CACHE` moves the cache).

### Walking the district

```sh
cargo run --release -p op_client
```

The client opens `data/world/zurich-altstadt` if you have built it, or the
one-tile fixture otherwise (`--world <dir>` picks another world). In the
district you start on the Lindenhof, looking east over the Limmat; in the
fixture, on the Grossmünsterplatz.

| Key | Action |
|---|---|
| Click | Capture the mouse (Esc releases it) |
| Mouse | Look around |
| W A S D | Walk |
| Shift | Sprint |
| Space | Jump |
| F9 | Save a replay of the session so far to `replays/` |
| R | Restart at the spawn point |

The simulation runs at a fixed 60 Hz, independent of the frame rate. A
saved replay can be checked and watched:

```sh
cargo run -p op_replay -- check replays/<file>.oprep     # same hashes as when you played
cargo run --release -p op_client -- --replay replays/<file>.oprep
```

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
