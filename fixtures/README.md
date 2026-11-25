# Test fixtures

Small, committed test data. Everything else is generated locally into
`data/` (gitignored). Files under `*/world/` are content-addressed and must
stay byte-exact (`.gitattributes` turns off line-ending conversion there).

| Fixture | What | Checked by | Regenerate with |
|---|---|---|---|
| `format/` | The world-format golden fixture: a tiny fictional world (one house, one ground plate, and two trees and two light points as points documents, [RFC 0004](../rfcs/0004-point-features.md)), built from `meshes.json` and `points.json`, with the world id and each points document's id pinned in `expected.json` | `op_world` tests, `tools/worldfmt` tests, CI on Linux, Windows and macOS | `cargo run -p op_world --example make_format_fixtures -- fixtures/format` |
| `replays/sample.oprep` | A 20 s scripted walk in `world/`: lands, runs into a wall, jumps, climbs a lane. Its hash trail must reproduce on every platform | `op_replay` tests, `op_replay check` in CI on Linux, Windows and macOS (debug and release) | `cargo run -p op_replay -- synth` |
| `changes/facade-sample/` | A contribution bundle (RFC 0002): a change set adding a test facade to one building in `world/`, and the gate's report on it (`report.json`), which must come out byte-identical on every platform | `op_world` tests `gate` and `contrib_schemas`, `tools/worldfmt/tests/test_contrib.py`, `op_world check-change` in CI | `cargo run -p op_world --example make_change_fixtures -- fixtures` |
| `world/` | The ingest fixture: tile 17/68646/45903 of the Zurich old town (Grossmünster, Münsterbrücke, the Limmat), cut to the `zurich-altstadt` bbox. It holds 47 assets: terrain, 33 building shells, the ribbons of the tile, 9 bridge decks, the tile's water (the Limmat, [RFC 0003](../rfcs/0003-water-layer.md)), and its 33 trees and 135 light points from the City of Zurich as two points documents ([RFC 0004](../rfcs/0004-point-features.md), [ADR-0015](../docs/adr/0015-zurich-trees-and-lights.md)), with tree heights from swissSURFACE3D ([ADR-0016](../docs/adr/0016-swisssurface3d-heights.md)) | `op_world validate` and the `ingest_fixture` test (Rust), `tools/ingest/tests/test_fixture_world.py`, the Khronos glTF validator | `python -m tools.ingest --world fixture-grossmuenster --out fixtures/world` |

Regenerate only on purpose, and say why in the commit. The ingest fixture
also changes when the OpenStreetMap or swisstopo data it was built from
changes. Re-running from the same cache (`--offline`) reproduces it byte
for byte on the same platform. The cache (`data/cache/`) holds two
Overpass answers per area in `osm/`: `overpass-<key>.json` for roads,
paths and building footprints, and `overpass-water-<key>.json` for water
areas ([ADR-0014](../docs/adr/0014-water-areas-from-openstreetmap.md));
`--refresh` queries both again. It also holds the City of Zurich's two
WFS answers for LV95 square 2683-1247 in `stadt-zuerich/`, each with a
sidecar file, and the swissSURFACE3D Raster tile 2683-1247 (2024) in
`swisstopo/`.

**The city's answers** (ADR-0015, "Archive"). The city serves only its
current data, so the fixture's trees and light points can be rebuilt byte
for byte only from the answers it was built from:

| Answer (in `data/cache/stadt-zuerich/`) | Publisher's date | Features | Size | SHA-256 |
|---|---|---|---|---|
| `baumkataster_baumstandorte/2683-1247.json` (trees) | 2026-11-22 | 1,027 | 587,593 bytes | `d11bec1121b65be023f21c3fadd0539922b2de77fe4597a5063aaaa4d17e68fd` |
| `ewz_brennstelle_p/2683-1247.json` (light points) | 2026-11-22 | 2,023 | 528,038 bytes | `d4aa51b443ec092d12ca08438aad9cadece5210bbbad890494d314866f7fe516` |

They are to be attached to a GitHub release of this repository, as
ADR-0015 decided, and are not in Git. A rebuild needs them in the cache,
each with its sidecar (`<square>.sidecar.json`, written by the ingest when
it fetches an answer); the SHA-256 pins the bytes. A rebuild after the
city's next weekly update without them fetches newer data, and the
fixture's points (and so its id) change.

Regenerating `world/` changes its id, and two fixtures name that id.
Regenerate them after it, in this order:
1. `replays/sample.oprep`. When only visual-only assets changed (as when
   water, then the trees and light points, were added), only the world id
   in its header changes: move it with `cargo run -p op_replay --
   rerecord fixtures/replays/sample.oprep --world fixtures/world --out
   fixtures/replays/sample.oprep`, which keeps the walk's inputs and
   reports how many checkpoints are unchanged (all 20 must be);
   `cargo run -p op_replay -- synth` records the walk anew. If a
   checkpoint changes, a collider or the simulation changed: find out
   which before committing (see [CONTRIBUTING.md](../CONTRIBUTING.md)).
2. `changes/facade-sample/`, with
   `cargo run -p op_world --example make_change_fixtures -- fixtures`.

**Licences.** `format/` is fictional content under CC BY-SA 4.0. `world/`
is derived from swisstopo data, OpenStreetMap and the City of Zurich's open
data. Each asset's manifest states its licence and credits: swisstopo
terms (credit "© swisstopo") for terrain and swissBUILDINGS3D shells; ODbL
1.0 together with the swisstopo terms for roads, paths, bridges, water and
the one OSM fallback building (credit "© OpenStreetMap contributors"); and
CC0 1.0 together with the swisstopo terms for the tile's trees and light
points (credits "Quelle: Stadt Zürich" and "© swisstopo").
See [ATTRIBUTION.md](../ATTRIBUTION.md).

The ingest also writes the licence notices beside `world.json`
(`NOTICE.txt`, `CREDITS.txt` and the licence texts,
[ADR-0019](../docs/adr/0019-odbl-and-the-published-world.md)), so
regenerating `world/` writes them there too. They change no id. The
committed fixture does not carry them: ADR-0019 asks for them beside the
published world, and in this repository `LICENSES/` and ATTRIBUTION.md
travel with the fixture.
