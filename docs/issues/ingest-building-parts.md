# Support OSM `building:part` in the fallback

Labels: `good first issue`, `area: ingest`

## Why

Where swissBUILDINGS3D has no building, the ingest extrudes the OSM
footprint to one height (ADR-0006). OSM often models buildings with
several heights as `building:part=*` ways with their own `height`,
`min_height` or `building:levels`, for example towers, wings or
canopies. The fallback ignores them, so such buildings come out as one
flat block.

## What to do

1. Add `way["building:part"]` (and relations) to the Overpass query in
   `tools/ingest/overpass.py`. This changes the query and so the cache key,
   so the first run downloads the OSM data again.
2. In `tools/ingest/buildings.py`, when a fallback building has parts
   inside it, extrude each part from `min_height` (or
   `building:min_level` × 3 m) to its `height` instead of the outline.
3. Keep one asset per building (`osm:way:<outline id>`), with every
   part's way in the provenance `source_ids`.

## Done when

- A synthetic test builds a footprint with two parts of different heights
  and checks the roof heights.
- Buildings without parts behave exactly as before. Their manifests may
  change only through the new OSM version.
- `python -m pytest` and `op_world validate` pass.

## Pointers

- `tools/ingest/buildings.py` (`fallback_shell`, `fallback_height`),
  `tools/ingest/osm.py`, `tools/ingest/tests/test_fallback.py`
- OSM wiki: Simple 3D Buildings
