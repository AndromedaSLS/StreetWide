"""StreetWide ingest: builds a world from open data (stage 3).

    python -m tools.ingest --bbox <minlon,minlat,maxlon,maxlat>
    python -m tools.ingest --world zurich-altstadt

Downloads swissBUILDINGS3D 3.0 Beta, swissALTI3D and OpenStreetMap data for
the area into `data/cache/` (never committed), converts it to the world
format of RFC 0001 and writes a content-addressed world store to
`data/world/<name>/`. See docs/adr/0006-ingest-pipeline-and-data-sources.md.
"""

VERSION = "0.1.0"
GENERATOR = "streetwide-ingest"
USER_AGENT = f"{GENERATOR}/{VERSION}"
