"""StreetWide ingest: builds a world from open data (stage 3).

    python -m tools.ingest --bbox <minlon,minlat,maxlon,maxlat>
    python -m tools.ingest --world zurich-altstadt

Downloads swissBUILDINGS3D 3.0 Beta, swissALTI3D and OpenStreetMap data for
the area, and inside the City of Zurich its trees and light points
(ADR-0015) with tree heights from swissSURFACE3D (ADR-0016), into
`data/cache/` (never committed), converts it to the world format of RFC 0001
and RFC 0004 and writes a content-addressed world store to
`data/world/<name>/`, with its licence notices beside `world.json`
(ADR-0019, `tools.ingest.notices`). See
docs/adr/0006-ingest-pipeline-and-data-sources.md.

The ingest always builds a whole world and replaces its output directory;
`--refresh` only makes it query STAC and Overpass again. Newer data enters
a world that has contributions as a baseline refresh (RFC 0002 section 7):
build into a directory of its own, then

    python -m tools.ingest.changes plan --current <canonical> --new <build> --log <log> --out <plan>
    python -m tools.ingest.changes verify --current <canonical> --plan <plan> --scratch <dir>

`tools.ingest.changes` compares the two stores and writes change sets
authored by `streetwide:ingest`, and `verify` runs them through the real
gate. See docs/contributing/baseline-refresh.md and
docs/adr/0020-baseline-refreshes.md (Proposed).
"""

VERSION = "0.2.0"
"""Bumped whenever the ingest's output changes (ADR-0020 section 7). 0.2.0:
trees and light points (RFC 0004, ADR-0015, ADR-0016)."""
GENERATOR = "streetwide-ingest"
USER_AGENT = f"{GENERATOR}/{VERSION}"
