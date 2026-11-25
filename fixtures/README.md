# Test fixtures

Small, committed test data. Everything else is generated locally into
`data/` (gitignored). Files under `*/world/` are content-addressed and must
stay byte-exact (`.gitattributes` turns off line-ending conversion there).

| Fixture | What | Checked by | Regenerate with |
|---|---|---|---|
| `format/` | The world-format golden fixture: a tiny fictional world (one house, one ground plate, and two trees and two light points as points documents, [RFC 0004](../rfcs/0004-point-features.md)), built from `meshes.json` and `points.json`, with the world id and each points document's id pinned in `expected.json` | `op_world` tests, `tools/worldfmt` tests, CI on Linux, Windows and macOS | `cargo run -p op_world --example make_format_fixtures -- fixtures/format` |
| `replays/sample.oprep` | A 20 s scripted walk in `world/`: lands, runs into a wall, jumps, climbs a lane. Its hash trail must reproduce on every platform | `op_replay` tests, `op_replay check` in CI on Linux, Windows and macOS (debug and release) | `cargo run -p op_replay -- synth` |
| `changes/facade-sample/` | A contribution bundle (RFC 0002): a change set adding a test facade to one building in `world/`, and the gate's report on it (`report.json`), which must come out byte-identical on every platform | `op_world` tests `gate` and `contrib_schemas`, `tools/worldfmt/tests/test_contrib.py`, `op_world check-change` in CI | `cargo run -p op_world --example make_change_fixtures -- fixtures` |
| `world/` | The ingest fixture: tile 17/68646/45903 of the Zurich old town (Grossmünster, Münsterbrücke, the Limmat), cut to the `zurich-altstadt` bbox. It holds 45 assets: terrain, 33 building shells, the ribbons of the tile, 9 bridge decks and the tile's water (the Limmat, [RFC 0003](../rfcs/0003-water-layer.md)) | `op_world validate` and the `ingest_fixture` test (Rust), `tools/ingest/tests/test_fixture_world.py`, the Khronos glTF validator | `python -m tools.ingest --world fixture-grossmuenster --out fixtures/world` |

Regenerate only on purpose, and say why in the commit. The ingest fixture
also changes when the OpenStreetMap or swisstopo data it was built from
changes. Re-running from the same cache (`--offline`) reproduces it byte
for byte on the same platform. The cache (`data/cache/`) holds two
Overpass answers per area in `osm/`: `overpass-<key>.json` for roads,
paths and building footprints, and `overpass-water-<key>.json` for water
areas ([ADR-0014](../docs/adr/0014-water-areas-from-openstreetmap.md));
`--refresh` queries both again.

Regenerating `world/` changes its id, and two fixtures name that id.
Regenerate them after it, in this order:
1. `replays/sample.oprep`, with `cargo run -p op_replay -- synth`. When
   only visual-only assets changed (as when water was added), only the
   world id in its header changes. If a checkpoint changes, a collider or
   the simulation changed: find out which before committing (see
   [CONTRIBUTING.md](../CONTRIBUTING.md)).
2. `changes/facade-sample/`, with
   `cargo run -p op_world --example make_change_fixtures -- fixtures`.

**Licences.** `format/` is fictional content under CC BY-SA 4.0. `world/`
is derived from swisstopo data and OpenStreetMap. Each asset's manifest
states its licence and credits: swisstopo terms (credit "© swisstopo") for
terrain and swissBUILDINGS3D shells, and ODbL 1.0 together with the
swisstopo terms for roads, paths, bridges, water and the one OSM fallback
building (credit "© OpenStreetMap contributors").
See [ATTRIBUTION.md](../ATTRIBUTION.md).

The ingest also writes the licence notices beside `world.json`
(`NOTICE.txt`, `CREDITS.txt` and the licence texts,
[ADR-0019](../docs/adr/0019-odbl-and-the-published-world.md)), so
regenerating `world/` writes them there too. They change no id. The
committed fixture does not carry them: ADR-0019 asks for them beside the
published world, and in this repository `LICENSES/` and ATTRIBUTION.md
travel with the fixture.
