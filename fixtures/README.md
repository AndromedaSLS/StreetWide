# Test fixtures

Small, committed test data. Everything else is generated locally into
`data/` (gitignored). Files under `*/world/` are content-addressed and must
stay byte-exact (`.gitattributes` turns off line-ending conversion there).

| Fixture | What | Checked by | Regenerate with |
|---|---|---|---|
| `format/` | The world-format golden fixture: a tiny fictional world (one house, one ground plate) with pinned ids in `expected.json` | `op_world` tests, `tools/worldfmt` tests, CI on Linux, Windows and macOS | `cargo run -p op_world --example make_format_fixtures -- fixtures/format` |
| `world/` | The ingest fixture: tile 17/68646/45903 of the Zurich old town (Grossmünster, Münsterbrücke, the Limmat), cut to the `zurich-altstadt` bbox. It holds 44 assets: terrain, 33 building shells, the ribbons of the tile and 9 bridge decks | `op_world validate` and the `ingest_fixture` test (Rust), `tools/ingest/tests/test_fixture_world.py`, the Khronos glTF validator | `python -m tools.ingest --world fixture-grossmuenster --out fixtures/world` |

Regenerate only on purpose, and say why in the commit. The ingest fixture
also changes when the OpenStreetMap or swisstopo data it was built from
changes. Re-running from the same cache (`--offline`) reproduces it byte
for byte on the same platform.

**Licences.** `format/` is fictional content under CC BY-SA 4.0. `world/`
is derived from swisstopo data and OpenStreetMap. Each asset's manifest
states its licence and credits: swisstopo terms (credit "© swisstopo") for
terrain and building shells, and ODbL 1.0 together with the swisstopo terms
for roads, paths and bridges (credit "© OpenStreetMap contributors").
See [ATTRIBUTION.md](../ATTRIBUTION.md).
