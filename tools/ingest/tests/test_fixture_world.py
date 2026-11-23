"""The committed ingest fixture (`fixtures/world/`), checked document by
document: canonical bytes, hashes, schemas, references and licences.
(`op_world validate` checks the same store independently in Rust.)"""

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
        expected_collider = "none" if (m["layer"], m["feature_id"][:5]) == ("transport", "tile:") else "trimesh"
        assert m["physics"]["collider"] == expected_collider
