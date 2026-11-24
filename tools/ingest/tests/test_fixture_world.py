"""The committed ingest fixture (`fixtures/world/`), checked document by
document: canonical bytes, hashes, schemas, references and licences.
(`op_world validate` checks the same store independently in Rust.)"""

import json
import struct
from pathlib import Path

import pytest

from tools.ingest.assemble import OSM_CREDIT, OSM_ON_TERRAIN_LICENSE, SWISSTOPO_CREDIT, SWISSTOPO_LICENSE
from tools.ingest.geo import Frame, parse_tile_key
from tools.worldfmt import cjson, schemas
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
    assert w["attribution"] == sorted([OSM_CREDIT, SWISSTOPO_CREDIT])


def test_assets_reference_existing_meshes_of_the_right_size(world):
    _, _, manifests = world
    for entry, m in manifests.values():
        data = STORE.read_object(m["content_id"], "glb")
        assert len(data) == m["size_bytes"]
        for k in ("content_id", "license", "bounds_mm", "layer", "feature_id"):
            assert entry[k] == (m["provenance"]["license"] if k == "license" else m[k])


def test_licences_follow_the_sources(world):
    _, _, manifests = world
    for entry, m in manifests.values():
        datasets = {s["dataset"] for s in m["provenance"]["sources"]}
        if "OpenStreetMap" in datasets:
            assert m["provenance"]["license"] == OSM_ON_TERRAIN_LICENSE
            assert m["provenance"]["attribution"] == sorted([OSM_CREDIT, SWISSTOPO_CREDIT])
        else:
            assert m["provenance"]["license"] == SWISSTOPO_LICENSE
            assert m["provenance"]["attribution"] == [SWISSTOPO_CREDIT]
        assert m["fictional"] is False and m["parent_version_id"] is None
        # Visual only: street ribbons (one asset per tile) and water (RFC 0003).
        visual = m["layer"] == "water" or (m["layer"], m["feature_id"][:5]) == ("transport", "tile:")
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
