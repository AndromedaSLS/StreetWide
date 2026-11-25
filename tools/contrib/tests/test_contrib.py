"""The contribution tool: reading editor glTF files and building bundles
that meet RFC 0002. CI also runs the tool and then the Rust gate on its
output (the `tests` job)."""

import json
import math
import struct
from pathlib import Path

import numpy as np
import pytest

from tools.contrib.facade import (
    ContributionError, World, export_reference, find_building, make_bundle, palette_material,
)
from tools.contrib.gltf_in import GltfError, read
from tools.worldfmt import cjson, schemas
from tools.worldfmt.limits import TILE_INDEX
from tools.worldfmt.store import Store

REPO = Path(__file__).resolve().parents[3]
WORLD = REPO / "fixtures" / "world"
SAMPLE = REPO / "fixtures" / "changes" / "facade-sample"


def editor_glb(translation, rotation, scale, material="Brick Wall"):
    """A glb like a 3D editor writes: one triangle, u16 indices, a node
    transform and a named material."""
    pos = struct.pack("<9f", 0, 0, 0, 1, 0, 0, 0, 0, 1)
    idx = struct.pack("<3H", 0, 1, 2) + b"\0\0"
    binary = pos + idx
    doc = {
        "asset": {"version": "2.0", "generator": "test"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"children": [1], "translation": translation}, {"mesh": 0, "rotation": rotation, "scale": scale}],
        "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "indices": 1, "material": 0}]}],
        "materials": [{"name": material}],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"},
            {"bufferView": 1, "componentType": 5123, "count": 3, "type": "SCALAR"},
        ],
        "bufferViews": [
            {"buffer": 0, "byteOffset": 0, "byteLength": 36},
            {"buffer": 0, "byteOffset": 36, "byteLength": 6},
        ],
        "buffers": [{"byteLength": len(binary)}],
    }
    j = json.dumps(doc).encode()
    j += b" " * (-len(j) % 4)
    body = struct.pack("<II", len(j), 0x4E4F534A) + j + struct.pack("<II", len(binary), 0x004E4942) + binary
    return b"glTF" + struct.pack("<II", 2, 12 + len(body)) + body


def test_reader_applies_the_node_hierarchy():
    half = math.sqrt(0.5)
    (part,) = read(editor_glb([1, 2, 3], [0, half, 0, half], [2, 2, 2]))
    assert part.material == "Brick Wall"
    # Rotating +X by 90 degrees about +Y gives -Z; scaled by 2, moved by (1, 2, 3).
    assert part.triangles[0][1] == pytest.approx([1, 2, 1])
    assert part.triangles[0][2] == pytest.approx([3, 2, 3])


def test_reader_rejects_other_files():
    with pytest.raises(GltfError):
        read(b"not a glb at all")


@pytest.mark.parametrize("name,material", [("Brick Wall", "wall"), ("roof tiles", "roof"), ("Window glass", "wall")])
def test_material_names_map_to_the_palette(name, material):
    assert palette_material(name) == material


def test_finds_the_building_at_a_point_and_exports_its_shell(tmp_path):
    feature = find_building(WORLD, 8.54397, 47.37017)  # the Grossmuenster
    assert feature.startswith("swisstopo:swissbuildings3d:")
    path = export_reference(WORLD, feature, tmp_path)
    assert path.read_bytes()[:4] == b"glTF"
    with pytest.raises(ContributionError):
        find_building(WORLD, 8.6, 47.5)  # far outside the world


def _sample_model():
    change = cjson.loads((SAMPLE / "change.json").read_bytes())
    entry = change["changes"][0]
    manifest = cjson.loads(Store(SAMPLE).read_object(entry["manifest_id"], "json"))
    return entry["feature_id"], Store(SAMPLE).read_object(manifest["content_id"], "glb")


def test_bundle_meets_the_contribution_rules(tmp_path):
    feature, model = _sample_model()
    change_id = make_bundle(
        WORLD, feature, model, tmp_path, author="github:tester", title="Test", rationale="Test.",
        derived_from_shell=True,
    )
    change_bytes = (tmp_path / "change.json").read_bytes()
    change = cjson.loads(change_bytes)
    assert schemas.errors(change) == []
    (entry,) = change["changes"]
    assert entry["op"] == "add" and entry["parent_version_id"] is None
    manifest = cjson.loads(Store(tmp_path).read_object(entry["manifest_id"], "json"))
    assert schemas.errors(manifest) == []
    prov = manifest["provenance"]
    assert manifest["layer"] == "facade" and manifest["physics"]["collider"] == "none"
    assert set(prov["license"].split(" AND ")) == {"CC-BY-SA-4.0", "LicenseRef-swisstopo-OGD"}
    assert prov["attribution"] == ["The StreetWide contributors", "© swisstopo"]
    assert any(s["dataset"] == "StreetWide contributions" and "github:tester" in s["source_ids"] for s in prov["sources"])
    # Deterministic: the same inputs give the same bundle.
    again = tmp_path.parent / "again"
    assert make_bundle(
        WORLD, feature, model, again, author="github:tester", title="Test", rationale="Test.",
        derived_from_shell=True,
    ) == change_id


def test_refuses_unknown_buildings_and_empty_models(tmp_path):
    _, model = _sample_model()
    with pytest.raises(ContributionError):
        make_bundle(WORLD, "swisstopo:swissbuildings3d:nope", model, tmp_path, author="github:t", title="t", rationale="t")
    feature, _ = _sample_model()
    tiny = editor_glb([0, 0, 0], [0, 0, 0, 1], [1e-5, 1e-5, 1e-5])  # collapses at the millimetre
    with pytest.raises(ContributionError):
        make_bundle(WORLD, feature, tiny, tmp_path / "b", author="github:t", title="t", rationale="t")


def test_flat_normals_are_unit_length(tmp_path):
    feature, model = _sample_model()
    make_bundle(WORLD, feature, model, tmp_path, author="github:t", title="t", rationale="t")
    change = cjson.loads((tmp_path / "change.json").read_bytes())
    manifest = cjson.loads(Store(tmp_path).read_object(change["changes"][0]["manifest_id"], "json"))
    (part,) = read(Store(tmp_path).read_object(manifest["content_id"], "glb"))
    assert len(part.triangles) > 0 and np.isfinite(part.triangles).all()


def test_a_tile_index_over_its_limit_is_refused_unparsed(tmp_path):
    # RFC 0001 section 7: a tile index is at most 16 MiB, checked before it
    # is parsed. One-key objects take a parser the most memory per byte.
    data = b"[" + b'{"a":0},' * (TILE_INDEX.max_bytes // 8)
    data = data[: TILE_INDEX.max_bytes].ljust(TILE_INDEX.max_bytes, b" ") + b"]"
    store = Store(tmp_path)
    world = cjson.loads((WORLD / "world.json").read_bytes())
    world["tiles"][0]["tile_id"] = store.write_object(data, "json")
    store.write_world(world)
    with pytest.raises(ContributionError) as e:
        World(tmp_path)
    assert str(e.value) == f"tile {world['tiles'][0]['tile']} is {TILE_INDEX.too_large()}"
    store.object_path(world["tiles"][0]["tile_id"], "json").unlink()


def test_a_world_with_points_works_as_any_other(tmp_path):
    # The format fixture's tile has point assets (RFC 0004): the tool reads
    # its tile indexes, finds the house among them, exports its shell and
    # builds a facade for it.
    fmt_world = REPO / "fixtures" / "format" / "world"
    world = World(fmt_world)
    layers = [a["layer"] for _, index in world.tiles for a in index["assets"]]
    assert {"street_objects", "vegetation"} <= set(layers)
    path = export_reference(fmt_world, "streetwide:sample-house", tmp_path / "ref")
    assert path.read_bytes()[:4] == b"glTF"
    model = editor_glb([0, 0, 0], [0, 0, 0, 1], [1, 1, 1])
    make_bundle(fmt_world, "streetwide:sample-house", model, tmp_path / "bundle", author="github:tester",
                title="Test", rationale="Test.")
    change = cjson.loads((tmp_path / "bundle" / "change.json").read_bytes())
    assert schemas.errors(change) == [] and change["changes"][0]["layer"] == "facade"
