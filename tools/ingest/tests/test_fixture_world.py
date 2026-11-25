"""The committed ingest fixture (`fixtures/world/`), checked document by
document: canonical bytes, hashes, schemas, references and licences, and
the tile's trees and light points (RFC 0004, ADR-0015, ADR-0016).
(`op_world validate` checks the same store independently in Rust.)"""

import json
import struct
from pathlib import Path

import pytest

from tools.ingest import VERSION
from tools.ingest.assemble import (
    CITY_POINTS_LICENSE,
    OSM_CREDIT,
    OSM_ON_TERRAIN_LICENSE,
    SWISSTOPO_CREDIT,
    SWISSTOPO_LICENSE,
)
from tools.ingest.geo import Frame, parse_tile_key
from tools.ingest.stadt_zuerich import CREDIT as CITY_CREDIT
from tools.worldfmt import cjson, points, schemas
from tools.worldfmt.licences import licence_problems
from tools.worldfmt.store import Store

STORE = Store(Path(__file__).resolve().parents[3] / "fixtures" / "world")


def _json(cid):
    doc = cjson.loads(STORE.read_object(cid, "json"))  # verifies hash and canonical form
    assert schemas.errors(doc) == []
    return doc


@pytest.fixture(scope="module")
def world():
    world_id, world = STORE.read_world()
    assert schemas.errors(world) == []
    index = _json(world["tiles"][0]["tile_id"])
    manifests = {a["feature_id"] + "@" + a["layer"]: (a, _json(a["manifest_id"])) for a in index["assets"]}
    return world, index, manifests


def test_world_frame_and_origin(world):
    w, index, _ = world
    assert w["name"] == "grossmuenster" and len(w["tiles"]) == 1
    frame = Frame((2145, 1434))
    assert w["region"] == {"key": frame.key, "anchor_e9": frame.anchor_e9}
    assert index["origin_mm"] == frame.tile_origin_mm(*parse_tile_key(index["tile"]))
    assert w["attribution"] == sorted([CITY_CREDIT, OSM_CREDIT, SWISSTOPO_CREDIT], key=str.encode)


def test_assets_reference_existing_meshes_of_the_right_size(world):
    _, _, manifests = world
    for entry, m in manifests.values():
        kind = "json" if m["layer"] in points.POINT_LAYERS else "glb"
        data = STORE.read_object(m["content_id"], kind)
        assert len(data) == m["size_bytes"]
        assert m["provenance"]["generator"] == {"name": "streetwide-ingest", "version": VERSION}
        for k in ("content_id", "license", "bounds_mm", "layer", "feature_id"):
            assert entry[k] == (m["provenance"]["license"] if k == "license" else m[k])


def test_licences_follow_the_sources(world):
    _, _, manifests = world
    for entry, m in manifests.values():
        datasets = {s["dataset"] for s in m["provenance"]["sources"]}
        if "OpenStreetMap" in datasets:
            assert m["provenance"]["license"] == OSM_ON_TERRAIN_LICENSE
            assert m["provenance"]["attribution"] == sorted([OSM_CREDIT, SWISSTOPO_CREDIT])
        elif m["layer"] in points.POINT_LAYERS:
            assert m["provenance"]["license"] == CITY_POINTS_LICENSE
            assert m["provenance"]["attribution"] == [CITY_CREDIT, SWISSTOPO_CREDIT]
        else:
            assert m["provenance"]["license"] == SWISSTOPO_LICENSE
            assert m["provenance"]["attribution"] == [SWISSTOPO_CREDIT]
        # ADR-0017: every source's licence and credit are the asset's.
        assert licence_problems(m["provenance"]) == [], entry["feature_id"]
        assert m["fictional"] is False and m["parent_version_id"] is None
        # Visual only: street ribbons (one asset per tile), water (RFC 0003)
        # and points (RFC 0004).
        visual = m["layer"] in ("water", *points.POINT_LAYERS) or (m["layer"], m["feature_id"][:5]) == (
            "transport", "tile:")
        assert m["physics"]["collider"] == ("none" if visual else "trimesh")


def test_the_limmat_is_the_tiles_one_water_asset(world):
    """RFC 0003 and ADR-0014: one water asset per tile, visual only, from
    the Limmat's relation and placed by swissALTI3D heights."""
    _, index, manifests = world
    water = [(entry, m) for entry, m in manifests.values() if m["layer"] == "water"]
    assert len(water) == 1
    entry, m = water[0]
    assert entry["feature_id"] == m["feature_id"] == f"tile:{index['tile']}"
    # Water sorts after every other layer, so it comes last.
    assert index["assets"][-1] == entry
    assert entry["collider"] == m["physics"]["collider"] == "none"
    assert entry["license"] == m["provenance"]["license"] == OSM_ON_TERRAIN_LICENSE
    osm, terrain = m["provenance"]["sources"]
    assert (osm["dataset"], terrain["dataset"]) == ("OpenStreetMap", "swissALTI3D")
    assert "relation/1598599" in osm["source_ids"]
    # Every primitive uses the palette material `water`: the glb lists only
    # the materials it uses.
    data = STORE.read_object(m["content_id"], "glb")
    length, _ = struct.unpack_from("<II", data, 12)
    gltf = json.loads(data[20:20 + length])
    assert [mat["name"] for mat in gltf["materials"]] == ["water"]


def test_the_tiles_trees_and_light_points(world):
    """RFC 0004 and ADR-0015: one asset per layer and source, from the
    city's answers for square 2683-1247 (ADR-0015 counts 33 trees and 136
    light points in the tile; one light stands in a terrain cell that the
    zurich-altstadt bbox cuts, so it has no ground and is left out), with
    tree heights from swissSURFACE3D (ADR-0016)."""
    _, index, manifests = world
    key = index["tile"]
    found = {}
    for layer, source, count, sha in [
        ("vegetation", "stadt-zuerich:baumkataster", 33,
         "d11bec1121b65be023f21c3fadd0539922b2de77fe4597a5063aaaa4d17e68fd"),
        ("street_objects", "stadt-zuerich:beleuchtung", 135,
         "d4aa51b443ec092d12ca08438aad9cadece5210bbbad890494d314866f7fe516"),
    ]:
        entry, m = manifests[f"tile:{key}/{source}@{layer}"]
        assert entry["collider"] == "none" and entry["license"] == CITY_POINTS_LICENSE
        doc = points.parse(STORE.read_object(m["content_id"], "json"))
        assert len(doc["points"]) == count
        assert all(p["id"].startswith(source + ":") for p in doc["points"])
        assert m["bounds_mm"] == points.bounds(doc)
        city = m["provenance"]["sources"][0]
        assert city["version"] == f"2026-11-22 sha256:{sha}" and city["license"] == "CC0-1.0"
        assert city["source_ids"] == sorted(p["id"].rsplit(":", 1)[1] for p in doc["points"])
        datasets = [(s["dataset"], s["version"], s["source_ids"]) for s in m["provenance"]["sources"][1:]]
        assert datasets[0] == ("swissALTI3D", "2026", ["swissalti3d_2026_2683-1247"])
        found[layer] = (doc, datasets)
    trees, tree_sources = found["vegetation"]
    assert tree_sources[1:] == [("swissSURFACE3D Raster", "2024", ["swisssurface3d-raster_2024_2683-1247"])]
    assert all(p["estimated"] == [] for p in trees["points"]) and trees["rules"] == {}
    measured = [p["attributes"]["height_mm"] for p in trees["points"] if p["attributes"]["height_mm"] is not None]
    assert len(measured) == 33 and all(3_000 <= h <= 50_000 for h in measured)
    lights, light_sources = found["street_objects"]
    assert len(light_sources) == 1
    assert lights["rules"] == {"light": {"yaw": "adr-0015:yaw-from-orientierung:1"}}
    unknown = [p for p in lights["points"] if p["attributes"]["yaw"] is None]
    assert len(unknown) == 2 and all(p["estimated"] == [] for p in unknown)
    assert all(p["attributes"][k] is None for p in lights["points"]
               for k in ("count", "light_height_mm", "method", "mount", "purpose"))
