"""The Overpass queries (ADR-0014): the water query, its own cache file, and
the roads-and-buildings query, which adding water must not change."""

import json
import re
from pathlib import Path

import pytest

from tools.ingest import overpass
from tools.ingest.geo import Area, BBox, parse_tile_key

REPO = Path(__file__).resolve().parents[3]
DISTRICT = BBox(8.5377, 47.3687, 8.5507, 47.3774)


def _bbox(world: str) -> BBox:
    """The bbox the ingest queries for `--world <world>`."""
    spec = json.loads((REPO / "worlds" / f"{world}.json").read_text(encoding="utf-8"))
    if "tile" in spec:
        return Area.from_tile(*parse_tile_key(spec["tile"])).intersect(Area.from_bbox(BBox(*spec["bbox"]))).bbox()
    return Area.from_bbox(BBox(*spec["bbox"])).bbox()


def test_the_roads_and_buildings_query_is_unchanged():
    """ADR-0014: the existing query stays byte-identical, so its cached
    answers and the OSM assets built from them stay as they are."""
    assert overpass.query(DISTRICT) == (
        "[out:json][timeout:180][bbox:47.3687,8.5377,47.3774,8.5507];"
        '(way["highway"];way["building"];relation["building"]["type"="multipolygon"];);'
        "out body geom;"
    )


def test_the_water_query_bounds_each_statement_and_asks_for_geometry():
    """ADR-0014: `natural=water`, `waterway=riverbank` and `landuse=reservoir`
    ways and relations, each statement with its own bounding box and the
    query with none, so relations come back with all their members."""
    q = overpass.water_query(DISTRICT)
    swne = "47.3687,8.5377,47.3774,8.5507"
    statements = re.findall(r'(way|relation)\["(\w+)"="(\w+)"\]\(([^)]*)\);', q)
    assert sorted(statements) == sorted(
        (kind, key, value, swne)
        for key, value in (("natural", "water"), ("waterway", "riverbank"), ("landuse", "reservoir"))
        for kind in ("way", "relation")
    )
    assert "[bbox:" not in q
    assert q == (
        "[out:json][timeout:180];("
        f'way["natural"="water"]({swne});relation["natural"="water"]({swne});'
        f'way["waterway"="riverbank"]({swne});relation["waterway"="riverbank"]({swne});'
        f'way["landuse"="reservoir"]({swne});relation["landuse"="reservoir"]({swne});'
        ");out body geom;"
    )


@pytest.mark.parametrize("world,key,water_key", [
    ("fixture-grossmuenster", "4f5fa21e50160d37", "84b7261f56959038"),
    ("zurich-altstadt", "d279ef51243f68a4", "22f0c68c602e83ea"),
])
def test_the_committed_worlds_keep_their_cache_files(world, key, water_key):
    """The cache file names hash the queries: these are the answers the
    fixture world and the district were built from (`--offline`)."""
    bbox = _bbox(world)
    assert overpass._key(overpass.query(bbox)) == key
    assert overpass._key(overpass.water_query(bbox)) == water_key


def _answer(timestamp, *elements):
    return {"elements": list(elements), "osm3s": {"timestamp_osm_base": timestamp}}


def test_water_answers_have_their_own_cache_file(tmp_path):
    bbox = _bbox("fixture-grossmuenster")
    osm = tmp_path / "osm"
    osm.mkdir()
    roads = _answer("2026-11-23T08:22:35Z", {"type": "way", "id": 1, "tags": {"highway": "footway"}})
    (osm / "overpass-4f5fa21e50160d37.json").write_text(json.dumps(roads), encoding="utf-8")
    # The roads-and-buildings answer is never taken for the water answer.
    with pytest.raises(RuntimeError, match="the Overpass water response is not cached and --offline is set"):
        overpass.fetch_water(bbox, tmp_path, offline=True, refresh=False)
    water = _answer("2026-11-24T12:44:49Z", {"type": "relation", "id": 1598599, "tags": {"natural": "water"}})
    (osm / "overpass-water-84b7261f56959038.json").write_text(json.dumps(water), encoding="utf-8")
    assert overpass.fetch_water(bbox, tmp_path, offline=True, refresh=False) == water
    assert overpass.fetch(bbox, tmp_path, offline=True, refresh=False) == roads
    # Nor the other way round.
    (osm / "overpass-4f5fa21e50160d37.json").unlink()
    with pytest.raises(RuntimeError, match="the Overpass response is not cached and --offline is set"):
        overpass.fetch(bbox, tmp_path, offline=True, refresh=False)
