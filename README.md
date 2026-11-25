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
> tracked in [docs/progress.md](docs/progress.md); where we are going, and
> in which order, is in the [roadmap](docs/roadmap.md).

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
| `fixtures/` | Small committed test data: golden fixtures for the world format and contributions, one real tile, a replay ([fixtures/README.md](fixtures/README.md)) |
| `worlds/` | World definitions for the ingest (area, spawn point) |
| `tools/ingest` | Python ingest: builds a world from swisstopo and OpenStreetMap data ([ADR-0006](docs/adr/0006-ingest-pipeline-and-data-sources.md)). `tools.ingest.changes` turns a newer build into baseline refresh change sets for the canonical world, and checks them through the gate ([guide](docs/contributing/baseline-refresh.md)) |
| `tools/worldfmt` | Python implementation of the world format, byte-compatible with `op_world` |
| `tools/contrib` | Contribution tools: export a building to model on, turn a model into a contribution ([guide](docs/contributing/facades.md)) |
| `tools/ci` | CI helpers: dependency-direction, DCO and glTF checks |
| `docs/adr`, `rfcs/` | Architecture decision records and RFCs |
| `docs/issues` | Ready-to-file issue drafts for newcomers |
| `docs/progress.md` | What has been built, checkpoint by checkpoint |

Dependencies point inward: `op_sim` depends only on `op_kernel`, nothing
depends on `op_client`, and only `op_client` may use Bevy. CI enforces this
([ADR-0002](docs/adr/0002-crate-layout-and-dependency-direction.md)).

## Getting started

From a fresh clone to walking the Zurich old town takes five steps. The
first builds and downloads take a while (see the times below); later runs
are fast.

### 1. Install the prerequisites

| | Windows | macOS | Linux |
|---|---|---|---|
| Git | [git-scm.com](https://git-scm.com) | `xcode-select --install` | your package manager |
| Rust | [rustup](https://rustup.rs) | [rustup](https://rustup.rs) | [rustup](https://rustup.rs) |
| C/C++ build tools | Visual Studio Build Tools, workload "Desktop development with C++" | `xcode-select --install` | `sudo apt-get install g++ pkg-config libx11-dev libasound2-dev libudev-dev libxkbcommon-x11-0 libwayland-dev libxkbcommon-dev` (Debian/Ubuntu; [other distributions](https://github.com/bevyengine/bevy/blob/main/docs/linux_dependencies.md)) |
| Python 3.11 or newer | [python.org](https://www.python.org) (gives the `py` launcher) | [python.org](https://www.python.org) or Homebrew | your package manager (`python3`, `python3-venv`) |

You also need:
- **A GPU** with Vulkan, DirectX 12 or Metal.
- **About 8 GB of free disk:** Rust build output plus about 330 MB of data.
- **An internet connection** for the first build and data download.

You don't need to pick a Rust version: `rust-toolchain.toml` pins it, and
`cargo` installs it on first use.

### 2. Get the code and run the determinism check

```sh
git clone https://github.com/AndromedaSLS/StreetWide.git
cd StreetWide
cargo run -p op_replay -- check fixtures/replays/sample.oprep
```

The first `cargo` command downloads the toolchain and dependencies and
compiles them, which takes a few minutes. It should end with
`ok: 1200 ticks, 20 checkpoints match, final state 212bd353…`: the
simulation reproduced a recorded 20-second walk bit for bit
([ADR-0003](docs/adr/0003-determinism-strategy.md)).

### 3. Build the Zurich district from open data

The ingest:
- downloads about 275 MB into `data/cache/`: one swissBUILDINGS3D sheet,
  one swissALTI3D tile and one OpenStreetMap query;
- builds the world in `data/world/zurich-altstadt/`, about 50 MB.

Nothing in `data/` is committed. It takes 4 to 5 minutes the first time.

Linux and macOS:

```sh
python3 -m venv .venv
.venv/bin/pip install -r tools/requirements.txt
.venv/bin/python -m tools.ingest --world zurich-altstadt
```

Windows (PowerShell):

```powershell
py -m venv .venv
.venv\Scripts\pip install -r tools\requirements.txt
.venv\Scripts\python -m tools.ingest --world zurich-altstadt
```

It ends by printing the new world's id. Optionally, check the result with
the Rust reference implementation:
`cargo run -p op_world -- validate data/world/zurich-altstadt`.

### 4. Walk the district

```sh
cargo run --release -p op_client
```

The first release build takes about 5 minutes. You start on the Lindenhof,
looking east over the Limmat. Without step 3, the client opens the one-tile
test fixture by the Grossmünster instead. `--world <dir>` picks any other
world. F1 shows and hides the controls in the client.

| Key | Action |
|---|---|
| Click | Capture the mouse (Esc releases it) |
| Mouse | Look around |
| W A S D (Z Q S D on AZERTY) | Walk |
| Shift | Sprint |
| Space | Jump |
| F9 | Save a replay of the session so far to `replays/` |
| R | Back to the start |
| **Getting around** | |
| V | Fly: W A S D, Space up, Ctrl down, Shift faster, mouse wheel for speed; Enter lands below you |
| M | Map: W A S D to pan, wheel to zoom, click to go there |
| 1 to 9 | Go to a named place (listed in the F1 help) |
| **Viewing** | |
| T, Shift+T | Time of day forward or back by 30 minutes |
| F2 | Detailed look, or the plain colours of the world data |
| F3 | Details: frame rate, position, sun |
| F4 | Graphics: high, or fast for slower GPUs |
| C | Credits: each source of the world's data, its licence and a link |
| **Contributing** | |
| I | Inspect: names what the crosshair is on (a building, street, bridge, water or terrain), who made it and its licence. Amber buildings are improvable, blue ones have a contribution. A click prints the details to the terminal, and for a building the facade tool's command. Paused on the map; works in replays |

The simulation runs at a fixed 60 Hz, whatever your frame rate. Buildings
and terrain stream in around you; "Loading…" means the simulation is waiting
for geometry it needs.

Going somewhere (a place, the map, landing from flight) starts a new session
there, so F9 saves the walk since you arrived
([ADR-0013](docs/adr/0013-travel-by-starting-new-sessions.md)). You can also
start somewhere: `--place Grossmünsterplatz` (or a number), `--at <lon>,<lat>`,
`--pitch -20` (looking down), `--time 19:30`, `--date 2026-06-21`,
`--view map`, `--inspect`; and `--preset fast` starts on the lighter
graphics (F4 switches).

The windows, doors, roof tiles and paving are drawn by the client, not
taken from the world data
([ADR-0012](docs/adr/0012-client-realism-from-procedural-detail.md)). So
is the night
([ADR-0022](docs/adr/0022-drawing-trees-lamps-and-windows.md)): lit
windows, a share by the hour, chosen from the building, wall, window and
clock (which ones is invented, never taken from data about real homes);
moonlight from the moon's real position and phase; and the city's glow on
the sky and the street. F2 shows the data alone, without any of these. Real facades come from contributions
([docs/contributing/facades.md](docs/contributing/facades.md)). Press I and
point at a building to see whether it has one, and to get the command that
starts yours.

To measure the frame rate, run the district's benchmark route
([ADR-0018](docs/adr/0018-performance-tier.md)):

```sh
cargo run --release -p op_client -- --benchmark worlds/zurich-altstadt.route.json --preset high
```

It visits Lindenhof, Hirschenplatz and Rathausbrücke, each on 21 June at
18:00 and 22:30 and on 21 December at 18:00, in a 1920 × 1080 window
without vsync. At each stop it waits until everything in range has
loaded, then turns the camera once round in 20 seconds and records every
frame. Keep the mouse and keyboard still and the window open for the 4
minutes it takes: a stop whose window is minimised or resized is not
measured. It
prints a summary and writes a JSON report to `benchmarks/` (or
`--out <file>`): the median, 95th and 99th percentile and worst frame
time per stop and overall, the GPU time per render pass (Vulkan and
DX12), and the GPU, driver, CPU, memory, system, commit, world id,
preset and resolution. `--shots <dir>` also saves each stop's
screenshot. The budget is a 99th percentile at or under 16.7 ms at every
stop on the high preset. A route file (`worlds/<world>.route.json`)
lists stops by place or position, each with a date (or a month, day and
day type) and a time; the format is described in
`crates/op_client/src/benchmark/route.rs`.

### 5. Check and watch your replay

```sh
cargo run -p op_replay -- check replays/<file>.oprep           # must say "ok"
cargo run --release -p op_client -- --replay replays/<file>.oprep
```

`op_replay info`, `trace` and `run` show a replay's header, the player's
path and the final state hash
([ADR-0007](docs/adr/0007-replay-file-format.md)).

For contributions and baseline refreshes, `op_replay` also has:
- `check-change`: the full contribution gate, G1 to G10;
- `merge`: `op_world merge` with the full gate, for change sets that
  touch colliders. It takes the same options plus `--replay`;
- `rerecord`: plays a replay's inputs again in a new version of its
  world;
- `compare-worlds`: compares two versions of a world for walkability and
  replay drift, for review.

See the [facade guide](docs/contributing/facades.md) and the
[baseline refresh guide](docs/contributing/baseline-refresh.md).

### Other areas

Any area inside one z12 map region in Switzerland works:

```sh
.venv/bin/python -m tools.ingest --bbox <minlon,minlat,maxlon,maxlat> --name <my-area>
cargo run --release -p op_client -- --world data/world/<my-area>
```

`python -m tools.ingest --help` lists the options:
- `--offline` uses only cached data;
- `--refresh` re-queries the data catalogues. It still builds a whole new
  world. It does not update a world that has contributions: that is a
  baseline refresh, planned from such a build with
  `python -m tools.ingest.changes`
  ([guide](docs/contributing/baseline-refresh.md));
- the `STREETWIDE_CACHE` environment variable moves the cache.

### Development checks

```sh
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
.venv/bin/python -m pytest          # Windows: .venv\Scripts\python -m pytest
```

[CONTRIBUTING.md](CONTRIBUTING.md) has the full list and the rules.

## Troubleshooting

| Problem | What to do |
|---|---|
| Windows: `linker 'link.exe' not found` | Install Visual Studio Build Tools with the "Desktop development with C++" workload, then open a new terminal. |
| Linux: build errors mentioning `alsa`, `libudev` or `pkg-config` | Install the system packages from step 1. |
| `python` is not found, or is older than 3.11 | Use `python3` (Linux, macOS) or `py` (Windows). Check with `python3 --version` or `py --version`. |
| Ingest: `Overpass error`, HTTP 429 or a timeout | The public OpenStreetMap server is busy. Wait a minute and run the same command again; finished downloads are cached and reused. |
| Ingest: `no ... data covers` | The area is outside Switzerland or outside the data's coverage. |
| Ingest: a checksum mismatch for a cached file | Delete that file under `data/cache/` and run again. |
| The client window stays black or closes at start | Update your GPU driver. Selecting a graphics backend explicitly can help, e.g. `WGPU_BACKEND=vulkan` (or `dx12`, `metal`). |
| You fell off the edge of the world | The built area ends at its bounding box. Press R. |
| `op_replay check`: `world ... not found` | The replay was recorded in a world that is not on this machine. Build it, or pass `--world <dir>`. |
| `op_replay check`: `recorded with simulation contract ...` | The replay comes from a different simulation version. Record a new one. |

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
| The published world, as a whole | [ODbL 1.0](LICENSES/ODbL-1.0.txt) |
| Inside it, contributed world content (models, textures, manifests) | [CC BY-SA 4.0](LICENSES/CC-BY-SA-4.0.txt) |
| Inside it, assets derived from OpenStreetMap | [ODbL 1.0](LICENSES/ODbL-1.0.txt) |
| Inside it, assets derived from swisstopo data | swisstopo terms of use (free use, attribution required) |

The world contains OpenStreetMap data, so it is offered as a whole under
the Open Database License, and each asset in it keeps the licence and
credits of its own sources, stated in its manifest
([ADR-0019](docs/adr/0019-odbl-and-the-published-world.md)). Every world
the ingest writes carries its notices beside `world.json`: `NOTICE.txt`,
`CREDITS.txt` and the licence texts. Data sources and the required
credits are listed in [ATTRIBUTION.md](ATTRIBUTION.md), and in the client
on the credits page (C). The rationale is in
[ADR-0001](docs/adr/0001-licensing.md) and ADR-0019.

Unless you explicitly state otherwise, any contribution of code you
intentionally submit for inclusion in this project, as defined in the
Apache-2.0 licence, is dual-licensed as above, without any additional terms
or conditions.
