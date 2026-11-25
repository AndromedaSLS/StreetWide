"""Baseline refreshes (RFC 0002 section 7): change sets from a fresh ingest.

    python -m tools.ingest.changes plan --current <canonical> --new <fresh ingest> \\
        --log <world log> --out <empty dir> [--overrides <file>] [--max-set-bytes <n>]
    python -m tools.ingest.changes verify --current <canonical> --plan <plan> \\
        --scratch <empty dir> [--replay <file>]... [--report <file>] [--no-compare]

`plan` compares a fresh ingest, built into a directory of its own, with
the canonical world store and its world log. It writes change sets
authored by `streetwide:ingest`, with at most 50 changes each, and a
refresh report that lists every conflict and every held-back change with
all of its reasons. It never runs the ingest and never writes to either
store or to the log. `verify` runs every set through the real gate
(`op_replay check-change`), in order, on a work copy of the canonical
store, and checks every world id the plan predicted. See
docs/adr/0020-baseline-refreshes.md (Proposed), the clarification of
RFC 0002 section 7 proposed with it, and
docs/contributing/baseline-refresh.md.

The planner imports only `tools.worldfmt` and the constants of
`tools/ingest/__init__.py`, so it needs neither numpy, shapely nor pyproj.

Modules:
- `stores`: reading and checking the two stores; the content key (rule C);
- `overrides`: the maintainer's holds, accepts and allowances;
- `classify`: matching by key, who made each asset, protected keys, the
  pending changes, units and height components;
- `holds`: the hold rules H1 to H14 and their fixed point;
- `sets`: bundles, packing into change sets and the chain of world ids;
- `report`: the refresh report, as canonical JSON and as Markdown;
- `checks`: the plan-time checks;
- `plan`: the `plan` subcommand;
- `layout`: the layout of the plan's output directory, for `verify`;
- `verify`: the `verify` subcommand.
"""

VERSION = "0.1.0"
"""Version of the refresh tool, recorded in every report and rationale."""
COMMAND = "tools.ingest.changes"
