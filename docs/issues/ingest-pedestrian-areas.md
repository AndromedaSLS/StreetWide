# Draw pedestrian squares mapped as areas

Labels: `good first issue`, `area: ingest`

## Why

In OpenStreetMap, squares such as the Münsterhof or the Lindenhof are
often mapped as closed ways with `highway=pedestrian` + `area=yes`, or as
`area:highway=*`. The ingest skips them (35 ways in the old town), so
squares show bare terrain while the streets around them have paving.

## What to do

In `tools/ingest/transport.py`, turn these areas into draped polygons in
the tile's transport asset:

1. Collect closed highway ways with `area=yes`, and ways tagged
   `area:highway`. Build shapely polygons in LV95, like the building
   footprints in `osm.py`.
2. Cut each polygon to the tile and the area (`tile_ribbons` shows how).
3. Triangulate it so it follows the terrain. Earcut alone gives long thin
   triangles that cut through slopes. Intersect the polygon with a grid of
   about 2 m squares and triangulate each piece, then sample heights with
   `dem.height` plus `LIFT_M["path"]`.
4. Use the `path` material, with `collider: none` like the ribbons.

## Done when

- The Münsterhof and the Lindenhof show paving in the client.
- The squares sit on the terrain, with no gaps or floating parts on the
  slopes.
- A unit test with a synthetic square on a sloped `PlaneDem` checks the
  heights (see `tools/ingest/tests/test_osm_transport.py`).
- The world still validates: `cargo run -p op_world -- validate
  data/world/zurich-altstadt`.

## Pointers

- `tools/ingest/transport.py` (`exclusion`, `tile_ribbons`),
  `tools/ingest/osm.py`, `tools/ingest/mesh.py`
- ADR-0006 section 6
