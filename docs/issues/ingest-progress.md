# Show progress while the ingest runs

Labels: `good first issue`, `area: ingest`

## Why

`python -m tools.ingest --world zurich-altstadt` takes several minutes on
a first run. It prints one line per phase and then goes quiet. In
particular, the 258 MB download and the 100-second CityGML parse look
like a hang.

## What to do

- **Downloads** (`tools/ingest/http.py`, `fetch_file`): print progress
  about once a second, e.g. `  swissbuildings3d_…zip  120.4 / 258.0 MB`.
  Use `\r` when stdout is a terminal and plain lines otherwise
  (`sys.stdout.isatty()`).
- **CityGML parse** (`tools/ingest/citygml.py`, `read_buildings`): report
  buildings read so far every few thousand buildings.
- **Phase times** (`tools/ingest/pipeline.py`): time each phase (terrain,
  buildings, OpenStreetMap, writing). Add the timings to the report under
  `data/reports/`.

## Done when

- A first run shows visible progress at least every few seconds.
- Output written to a file or CI log stays readable: no `\r` there.
- The world id does not change. Progress output must not affect the
  result. Check by running twice from cache with `--offline`.
- `python -m pytest` passes.

## Pointers

- `tools/ingest/http.py`, `citygml.py`, `pipeline.py`
- ADR-0006 (ingest decisions)
