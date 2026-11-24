"""Fuzzing the contribution tool's readers (roadmap M1): the glb reader for
contributor models, the facade tool end to end, and world stores read by the
tool. Each must accept its input or raise its documented error (GltfError,
ContributionError). Regression tests replay the inputs that used to crash,
hang or write outside the output directory."""

from __future__ import annotations

import copy
import json
import shutil
import struct
from pathlib import Path

import numpy as np
import pytest

from tools.contrib import facade
from tools.contrib.__main__ import main
from tools.contrib.facade import ContributionError, export_reference, find_building, make_bundle
from tools.contrib.gltf_in import GltfError, read
from tools.worldfmt import cjson, schemas
from tools.worldfmt.store import Store
from tools.worldfmt.tests.fuzzing import INTERESTING_U32, MEMORY, Rng, mutate_bytes, mutate_json, reading, run_cases

REPO = Path(__file__).resolve().parents[3]
WORLD = REPO / "fixtures" / "world"
SAMPLE = REPO / "fixtures" / "changes" / "facade-sample"
GRASSMUENSTER = (8.54397, 47.37017)


def glb(doc: dict | bytes, binary: bytes | None = b"") -> bytes:
    """A glb container around a JSON chunk (a document or its exact bytes)
    and an optional binary chunk; only the 20- and 8-byte headers are added."""
    j = doc if isinstance(doc, bytes) else json.dumps(doc).encode()
    j += b" " * (-len(j) % 4)
    body = struct.pack("<II", len(j), 0x4E4F534A) + j
    if binary is not None:
        b = binary + b"\0" * (-len(binary) % 4)
        body += struct.pack("<II", len(b), 0x004E4942) + b
    return b"glTF" + struct.pack("<II", 2, 12 + len(body)) + body


def split(data: bytes) -> tuple[dict, bytes]:
    json_len = struct.unpack_from("<I", data, 12)[0]
    doc = json.loads(data[20 : 20 + json_len])
    bin_len = struct.unpack_from("<I", data, 20 + json_len)[0]
    return doc, data[28 + json_len : 28 + json_len + bin_len]


TRIANGLE = struct.pack("<9f", 0, 0, 0, 1, 0, 0, 0, 0, 1)


def editor_models() -> list[bytes]:
    """Hand-made models in the shapes editors write."""
    one = {
        "asset": {"version": "2.0"}, "scene": 0, "scenes": [{"nodes": [0]}],
        "nodes": [{"children": [1], "translation": [1, 2, 3]}, {"mesh": 0, "rotation": [0, 0.7071068, 0, 0.7071068], "scale": [2, 2, 2]}],
        "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "indices": 1, "material": 0}]}],
        "materials": [{"name": "Brick Wall"}],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"},
            {"bufferView": 1, "componentType": 5123, "count": 3, "type": "SCALAR"},
        ],
        "bufferViews": [{"buffer": 0, "byteOffset": 0, "byteLength": 36}, {"buffer": 0, "byteOffset": 36, "byteLength": 6}],
        "buffers": [{"byteLength": 44}],
    }
    # Interleaved positions and normals (stride 24), u8 indices, a matrix, two scenes.
    quad = struct.pack("<24f", 0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 1, 1, 1, 0, 0, 0, 1, 0, 1, 0, 0, 0, 1)
    two = {
        "asset": {"version": "2.0"}, "scene": 1, "scenes": [{"nodes": [0]}, {"nodes": [1, 0]}],
        "nodes": [{"mesh": 0, "matrix": [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 5, 0, 0, 1]}, {"mesh": 1}],
        "meshes": [
            {"primitives": [{"attributes": {"POSITION": 0, "NORMAL": 1}, "indices": 2, "material": 1}]},
            {"primitives": [{"attributes": {"POSITION": 0}, "mode": 1}, {"attributes": {"POSITION": 3}}]},
        ],
        "materials": [{"name": "glass"}, {"name": "roof tiles"}],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3"},
            {"bufferView": 0, "byteOffset": 12, "componentType": 5126, "count": 4, "type": "VEC3"},
            {"bufferView": 1, "componentType": 5121, "count": 6, "type": "SCALAR"},
            {"bufferView": 2, "componentType": 5122, "count": 3, "type": "VEC3"},
        ],
        "bufferViews": [
            {"buffer": 0, "byteOffset": 0, "byteLength": 96, "byteStride": 24},
            {"buffer": 0, "byteOffset": 96, "byteLength": 6},
            {"buffer": 0, "byteOffset": 104, "byteLength": 18},
        ],
        "buffers": [{"byteLength": 124}],
    }
    two_bin = quad + bytes([0, 1, 2, 0, 2, 3]) + b"\0\0" + struct.pack("<9h", 0, 0, 0, 100, 0, 0, 0, 100, 0) + b"\0\0"
    # No scene at all: every node that is nobody's child is drawn.
    loose = {
        "asset": {"version": "2.0"}, "nodes": [{"mesh": 0}, {"mesh": 0, "translation": [0, 3, 0]}],
        "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}],
        "accessors": [{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"}],
        "bufferViews": [{"buffer": 0, "byteLength": 36}],
    }
    return [glb(one, TRIANGLE + struct.pack("<3H", 0, 1, 2)), glb(two, two_bin), glb(loose, TRIANGLE)]


MODELS = [p.read_bytes() for p in sorted(REPO.glob("fixtures/**/*.glb")) if p.stat().st_size < 200_000] + editor_models()
DOCS = [split(m) for m in MODELS]


def test_editor_models_are_read():
    for data in editor_models():
        assert read(data)


def mutate_gltf(rng: Rng, doc: dict, binary: bytes) -> dict:
    """glTF-aware edits: indices, counts, offsets and strides set to edge
    values, node graphs with cycles or shared children, odd transforms,
    scenes and materials, and instancing."""
    d = copy.deepcopy(doc)
    edge = (*INTERESTING_U32, -1, len(binary), len(binary) + 4, 1.5, "1", None, True, [0])
    for _ in range(1 + rng.below(2)):
        op = rng.below(7)
        if op == 0 and d.get("accessors"):
            acc = rng.choice(d["accessors"])
            acc[rng.choice(("count", "byteOffset", "componentType", "type", "bufferView"))] = rng.choice(
                edge + (5120, 5121, 5122, 5123, 5125, 5126, "SCALAR", "VEC2", "VEC4", "MAT4"))
        elif op == 1 and d.get("bufferViews"):
            view = rng.choice(d["bufferViews"])
            view[rng.choice(("byteOffset", "byteStride", "byteLength", "buffer"))] = rng.choice(edge)
        elif op == 2 and d.get("nodes"):
            node = rng.choice(d["nodes"])
            n = len(d["nodes"])
            k = rng.below(5)
            if k == 0:
                node["children"] = [rng.below(n + 1) for _ in range(1 + rng.below(3))]
            elif k == 1:
                d["nodes"] = [{"children": [i + 1, i + 1]} for i in range(60)] + [{"mesh": 0}]
            elif k == 2:
                key, size = rng.choice((("matrix", 16), ("rotation", 4), ("scale", 3), ("translation", 3)))
                node[key] = [rng.choice((0, 1, 1e308, -1e308, 10**400, "x", None, 1e-320)) for _ in range(size + rng.below(3) - 1)]
            elif k == 3:
                node["mesh"] = rng.choice(edge)
            else:
                d["nodes"] = d["nodes"] * (1 + rng.below(8))
        elif op == 3:
            d["scene"] = rng.choice((0, 1, 5, -1, None, "0"))
            if rng.chance(50) and d.get("scenes"):
                d["scenes"][0]["nodes"] = [rng.below(len(d.get("nodes", [])) + 1) for _ in range(1 + rng.below(4))]
            elif rng.chance(30):
                d.pop("scenes", None)
        elif op == 4 and d.get("meshes"):
            prim = rng.choice(rng.choice(d["meshes"])["primitives"])
            key = rng.choice(("indices", "material", "mode"))
            prim[key] = rng.choice(edge)
            if rng.chance(30):
                prim["attributes"]["POSITION"] = rng.choice(edge)
        elif op == 5:
            d["materials"] = rng.choice(([{"name": 5}, {"name": "a"}], [{}], "x", [None], [{"name": ["x"]}]))
        elif op == 6 and d.get("meshes") and d.get("nodes"):
            # Instancing (test_instancing_is_bounded covers the budget).
            d["nodes"] = d["nodes"] + [{"mesh": 0} for _ in range(rng.choice((10, 300)))]
            if d.get("scenes"):
                d["scenes"][0]["nodes"] = list(range(len(d["nodes"])))
    return d


def mutated_model(rng: Rng) -> bytes:
    k = rng.below(10)
    if k < 4:
        return mutate_bytes(rng, rng.choice(MODELS), MODELS)
    doc, binary = rng.choice(DOCS)
    if k < 8:
        doc = mutate_gltf(rng, doc, binary)
    else:
        doc = mutate_json(rng, doc, [d for d, _ in DOCS])
    try:
        data = glb(json.dumps(doc).encode(), binary if rng.chance(90) else binary[: rng.below(len(binary) + 1)])
    except (ValueError, RecursionError):  # too deep for json.dumps
        data = glb(b"[]", binary)
    return mutate_bytes(rng, data, MODELS) if rng.chance(20) else data


def test_fuzz_glb_reader():
    """gltf_in.read returns finite triangles per material, or GltfError."""

    def case(rng: Rng) -> None:
        model = mutated_model(rng)
        try:
            with reading(len(model)):
                parts = read(model)
        except GltfError:
            return
        assert [p.material for p in parts] == sorted(p.material for p in parts)
        for p in parts:
            assert isinstance(p.material, str)
            assert p.triangles.dtype == np.float64 and p.triangles.shape[1:] == (3, 3)
            assert np.isfinite(p.triangles).all()

    # Instancing lets a small file ask for up to LIMITS["triangles"] (a
    # million triangles, about 72 MB once transformed): that much is allowed
    # whatever the file's size, and nothing beyond it.
    run_cases("gltf_in.read", 1500, case, memory=(MEMORY[0], 256 << 20))


@pytest.fixture(scope="module")
def sample():
    change = cjson.loads((SAMPLE / "change.json").read_bytes())
    entry = change["changes"][0]
    manifest = cjson.loads(Store(SAMPLE).read_object(entry["manifest_id"], "json"))
    return entry["feature_id"], Store(SAMPLE).read_object(manifest["content_id"], "glb")


def test_fuzz_facade_tool(tmp_path, sample):
    """make_bundle on mutated models: a checked bundle, or ContributionError."""
    feature, model = sample
    models = MODELS + [model]
    counter = iter(range(10**9))

    def case(rng: Rng) -> None:
        data = mutate_bytes(rng, model, models) if rng.chance(30) else mutated_model(rng)
        out = tmp_path / f"b{next(counter)}"
        try:
            make_bundle(WORLD, feature, data, out, author="github:fuzz", title="Fuzz", rationale="Fuzz.",
                        derived_from_shell=rng.chance(50), fictional=rng.chance(50))
        except ContributionError:
            return
        change = cjson.loads((out / "change.json").read_bytes())
        assert schemas.errors(change) == []
        shutil.rmtree(out)

    run_cases("make_bundle", 80, case)


_, _WORLD = Store(WORLD).read_world()
_TILE = Store(WORLD).read_object(_WORLD["tiles"][0]["tile_id"], "json")
_SHELL = next(a for a in cjson.loads(_TILE)["assets"] if a["layer"] == "shell")
_MANIFEST = Store(WORLD).read_object(_SHELL["manifest_id"], "json")
_MESH = Store(WORLD).read_object(_SHELL["content_id"], "glb")


def small_store(root: Path, rng: Rng) -> str:
    """A store holding the fixture world.json, its tile index and one
    building's manifest and mesh, with one of them (or none) mutated and
    stored under its own hash, so the reader gets past the hash checks.
    Returns the building's feature id."""
    world, tile_bytes, manifest_bytes, mesh = copy.deepcopy(_WORLD), _TILE, _MANIFEST, _MESH
    tile = cjson.loads(tile_bytes)
    shell = next(a for a in tile["assets"] if a["layer"] == "shell")
    store = Store(root)
    target = rng.below(5)

    def remix(data: bytes) -> bytes:
        if rng.chance(50):
            value = mutate_json(rng, json.loads(data), [world, tile])
            try:
                return cjson.dumps(value)
            except cjson.CanonicalError:
                return json.dumps(value).encode()
        return mutate_bytes(rng, data, [data])

    if target == 0:
        manifest_bytes = remix(manifest_bytes)
        shell["manifest_id"] = store.write_object(manifest_bytes, "json")
        tile_bytes = cjson.dumps(tile)
    elif target == 1:
        mesh = mutate_bytes(rng, mesh, [mesh])
    elif target == 2:
        tile_bytes = remix(tile_bytes)
    store.write_object(manifest_bytes, "json")
    store.write_object(mesh, "glb")
    world["tiles"][0]["tile_id"] = store.write_object(tile_bytes, "json")
    world_bytes = cjson.dumps(world) if target != 3 else remix(cjson.dumps(world))
    (root / "world.json").write_bytes(world_bytes)
    return shell["feature_id"]


def test_fuzz_world_store(tmp_path, sample):
    """The tool reads crafted world stores without crashing and never writes
    outside the directory it was given."""
    _, model = sample
    counter = iter(range(10**9))

    def case(rng: Rng) -> None:
        n = next(counter)
        root = tmp_path / f"w{n}"
        feature = small_store(root, rng)
        out = tmp_path / f"out{n}"
        try:
            assert isinstance(find_building(root, *GRASSMUENSTER), str)
        except ContributionError:
            pass
        try:
            path = export_reference(root, feature, out)
            assert path.parent == out
        except ContributionError:
            pass
        try:
            make_bundle(root, feature, model, out / "bundle", author="github:fuzz", title="t", rationale="r",
                        derived_from_shell=rng.chance(50))
        except ContributionError:
            pass
        assert {p.name for p in tmp_path.iterdir()} <= {f"w{i}" for i in range(n + 1)} | {f"out{i}" for i in range(n + 1)}
        shutil.rmtree(root)
        shutil.rmtree(out, ignore_errors=True)

    run_cases("contribution tool on crafted stores", 50, case)


# Regressions: minimised inputs that crashed or hung ---------------------

_BASE = b'"asset":{"version":"2.0"},"buffers":[{"byteLength":36}],"bufferViews":[{"buffer":0,"byteLength":36}],"meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}]'
_ACCESSOR = b'"accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}]'


@pytest.mark.parametrize(
    "json_chunk",
    [
        # Each node has the next one as a child twice: 2^60 paths, which hung.
        b"{" + _BASE + b"," + _ACCESSOR + b',"nodes":['
        + b",".join(b'{"children":[%d,%d]}' % (i + 1, i + 1) for i in range(60)) + b',{"mesh":0}],"scenes":[{"nodes":[0]}]}',
        # A stride of 0 and two billion elements: one Python tuple each, which hung.
        b'{"asset":{"version":"2.0"},"bufferViews":[{"buffer":0,"byteLength":36,"byteStride":0}],'
        b'"accessors":[{"bufferView":0,"componentType":5126,"count":2000000000,"type":"VEC3"}],'
        b'"meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}],"nodes":[{"mesh":0}],"scenes":[{"nodes":[0]}]}',
        # A stride of 10^400 on one element: too large for numpy's strided view.
        b'{"asset":{"version":"2.0"},"bufferViews":[{"buffer":0,"byteLength":36,"byteStride":1' + b"0" * 400 + b'}],'
        b'"accessors":[{"bufferView":0,"componentType":5126,"count":1,"type":"VEC3"}],'
        b'"meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}],"nodes":[{"mesh":0}]}',
        # TypeError: a count that is not an integer.
        b'{"accessors":[{"bufferView":0,"componentType":5126,"count":3.0,"type":"VEC3"}],"bufferViews":[{"buffer":0}],'
        b'"meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}],"nodes":[{"mesh":0}]}',
        # IndexError: the default scene does not exist.
        b"{" + _BASE + b"," + _ACCESSOR + b',"nodes":[{"mesh":0}],"scene":5,"scenes":[{"nodes":[0]}]}',
        # KeyError: scenes is an object.
        b"{" + _BASE + b"," + _ACCESSOR + b',"nodes":[{"mesh":0}],"scenes":{"a":1}}',
        # ValueError: a matrix of 3 numbers.
        b"{" + _BASE + b"," + _ACCESSOR + b',"nodes":[{"mesh":0,"matrix":[1,2,3]}]}',
        # OverflowError: a rotation of 10^400.
        b"{" + _BASE + b"," + _ACCESSOR + b',"nodes":[{"mesh":0,"rotation":[1' + b"0" * 400 + b',0,0,1]}]}',
        # Accepted: NaN (not JSON) in a transform gave NaN triangles.
        b"{" + _BASE + b"," + _ACCESSOR + b',"nodes":[{"mesh":0,"translation":[NaN,0,0]}]}',
        # Accepted: a translation of 1e308 overflows the vertices to infinity.
        b"{" + _BASE + b"," + _ACCESSOR + b',"nodes":[{"mesh":0,"scale":[1e308,1e308,1e308],"translation":[1e308,0,0]}]}',
        # TypeError: material names of different types could not be sorted.
        b'{"asset":{"version":"2.0"},"bufferViews":[{"buffer":0,"byteLength":36}],' + _ACCESSOR
        + b',"materials":[{"name":5},{"name":"a"}],"meshes":[{"primitives":[{"attributes":{"POSITION":0},"material":0},'
        b'{"attributes":{"POSITION":0},"material":1}]}],"nodes":[{"mesh":0}]}',
        # Accepted: a negative material index picked the last material.
        b'{"asset":{"version":"2.0"},"bufferViews":[{"buffer":0,"byteLength":36}],' + _ACCESSOR
        + b',"materials":[{"name":"x"}],"meshes":[{"primitives":[{"attributes":{"POSITION":0},"material":-1}]}],"nodes":[{"mesh":0}]}',
        # IndexError: float indices.
        b'{"asset":{"version":"2.0"},"bufferViews":[{"buffer":0,"byteLength":36}],' + _ACCESSOR
        + b',"meshes":[{"primitives":[{"attributes":{"POSITION":0},"indices":0}]}],"nodes":[{"mesh":0}]}',
        # ValueError from matmul: SCALAR positions.
        b'{"asset":{"version":"2.0"},"bufferViews":[{"buffer":0,"byteLength":36}],'
        b'"accessors":[{"bufferView":0,"componentType":5126,"count":9,"type":"SCALAR"}],'
        b'"meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}],"nodes":[{"mesh":0}]}',
        # UnicodeDecodeError: the JSON chunk is not UTF-8.
        b"\xff{}",
        # RecursionError: JSON nested 100,000 levels.
        b"[" * 100_000,
        # ValueError: a number of 5,000 digits.
        b'{"scene":' + b"1" * 5000 + b"}",
        # AttributeError: the JSON chunk is not an object.
        b"[]",
    ],
    ids=["dag", "stride-0", "huge-stride", "float-count", "scene-index", "scenes-object", "matrix-size", "huge-rotation", "nan",
         "overflow", "material-names", "negative-material", "float-indices", "scalar-positions", "not-utf8", "deep",
         "long-number", "not-object"],
)
def test_malformed_models_raise_gltf_error(json_chunk):
    with pytest.raises(GltfError):
        read(glb(json_chunk, TRIANGLE))


def test_instancing_is_bounded():
    # 200,000 nodes drawing one mesh were all read (the work grows with
    # the node count, not the file); the reader now stops at its budget
    # (gltf_in.LIMITS) with a clear error.
    doc = json.loads(b"{" + _BASE + b"," + _ACCESSOR + b"}")
    doc["nodes"] = [{"mesh": 0}] * 200_000
    with pytest.raises(GltfError, match="too large"):
        read(glb(doc, TRIANGLE))


def _facade_with_balcony(**fields) -> bytes:
    """Node 0 draws a triangle 10 m east and holds node 1, which draws it
    again 3 m above its parent."""
    doc = json.loads(b"{" + _BASE + b"," + _ACCESSOR + b"}")
    doc["nodes"] = [{"mesh": 0, "children": [1], "translation": [10, 0, 0]}, {"mesh": 0, "translation": [0, 3, 0]}]
    return glb(dict(doc, **fields), TRIANGLE)


def test_a_file_without_scenes_draws_each_node_once():
    # Every node was taken as a root, so the balcony was drawn twice: under
    # its parent, and alone at its own transform (found by review).
    (part,) = read(_facade_with_balcony())
    assert sorted(map(tuple, part.triangles[:, 0])) == [(10.0, 0.0, 0.0), (10.0, 3.0, 0.0)]


def test_a_scene_cannot_list_a_child_node():
    # A node listed in the scene and as a child was drawn twice; the glTF
    # validator reports it (SCENE_NON_ROOT_NODE).
    with pytest.raises(GltfError, match="scene node 1 is the child of another node"):
        read(_facade_with_balcony(scene=0, scenes=[{"nodes": [0, 1]}]))
    (part,) = read(_facade_with_balcony(scene=0, scenes=[{"nodes": [0]}]))
    assert len(part.triangles) == 2


@pytest.mark.parametrize(
    "nodes,scenes",
    [
        ([{"mesh": 0, "children": [1]}, {"children": [0]}], None),  # no root: nothing was drawn
        ([{"mesh": 0, "children": [0]}], None),
        ([{"mesh": 0}, {"children": [2]}, {"mesh": 0, "children": [1]}], [{"nodes": [0]}]),  # outside the scene
    ],
    ids=["two-nodes", "own-child", "beside-the-scene"],
)
def test_nodes_cannot_form_a_loop(nodes, scenes):
    # glTF nodes form trees. Without a scene, only roots are drawn now, so
    # a loop would have been skipped silently (it used to hit the depth limit).
    doc = json.loads(b"{" + _BASE + b"," + _ACCESSOR + b"}")
    doc["nodes"] = nodes
    if scenes is not None:
        doc["scenes"] = scenes
    with pytest.raises(GltfError, match="loop"):
        read(glb(doc, TRIANGLE))


def test_bad_models_are_contribution_errors(tmp_path, sample):
    # A model the reader refuses used to escape the CLI as a GltfError
    # traceback; far-away or too many triangles reached the manifest.
    feature, _ = sample
    far = json.loads(b"{" + _BASE + b"," + _ACCESSOR + b',"nodes":[{"mesh":0,"translation":[2000000,0,0]}]}')
    many = json.loads(b"{" + _BASE + b"," + _ACCESSOR + b"}")
    many["nodes"] = [{"mesh": 0, "translation": [i % 100, i // 100, 0]} for i in range(50_001)]
    for i, model in enumerate([b"not a glb", glb(far, TRIANGLE), glb(many, TRIANGLE)]):
        with pytest.raises(ContributionError):
            make_bundle(WORLD, feature, model, tmp_path / str(i), author="github:t", title="t", rationale="t")


def _world_with_feature(root: Path, feature_id: str) -> None:
    """The fixture world, with one building's feature id replaced."""
    _, world = Store(WORLD).read_world()
    tile = cjson.loads(Store(WORLD).read_object(world["tiles"][0]["tile_id"], "json"))
    shell = next(a for a in tile["assets"] if a["layer"] == "shell")
    shell["feature_id"] = feature_id
    store = Store(root)
    store.write_object(Store(WORLD).read_object(shell["content_id"], "glb"), "glb")
    world["tiles"][0]["tile_id"] = store.write_json(tile)
    store.write_world(world)


@pytest.mark.parametrize("feature_id", ["x:../../escape", "x:..\\..\\escape", "x:/tmp/escape", "x:C:\\escape"])
def test_reference_export_stays_in_the_output_directory(tmp_path, feature_id):
    # A crafted world's feature id used to become a path: "x:../../escape"
    # wrote <out>/../../escape-shell.glb.
    _world_with_feature(tmp_path / "world", feature_id)
    out = tmp_path / "deep" / "er" / "out"
    path = export_reference(tmp_path / "world", feature_id, out)
    assert path.parent == out and path.read_bytes()[:4] == b"glTF"
    assert sorted(p.name for p in tmp_path.rglob("*-shell.glb")) == [path.name]


@pytest.mark.parametrize("feature_id", ["swisstopo:swissbuildings3d:{ABC-1}", "x:a.b+c", "x:..", "x:a_b-1"])
def test_reference_export_keeps_the_names_of_valid_ids(tmp_path, feature_id):
    # Only path separators are replaced: the fix for the case above first
    # renamed every id with a character outside [A-Za-z0-9_-] (found by
    # review), though feature ids may use any of [!-~].
    _world_with_feature(tmp_path / "world", feature_id)
    path = export_reference(tmp_path / "world", feature_id, tmp_path / "out")
    assert path == tmp_path / "out" / f"{feature_id.rsplit(':', 1)[-1]}-shell.glb"


def test_reference_export_checks_the_mesh_hash(tmp_path):
    # The shell mesh was copied without checking it against its content id.
    _world_with_feature(tmp_path / "world", "x:a")
    (mesh,) = (tmp_path / "world" / "objects").rglob("*.glb")
    mesh.write_bytes(mesh.read_bytes() + b"tampered")
    with pytest.raises(ContributionError, match="does not match its hash"):
        export_reference(tmp_path / "world", "x:a", tmp_path / "out")


@pytest.mark.parametrize(
    "world_json",
    [b'{"tiles":5}', b"[]", b'{"format":"streetwide.world"}', b"\xff", b"[" * 100_000],
    ids=["tiles-not-a-list", "array", "fields-missing", "not-utf8", "deep"],
)
def test_crafted_world_json_is_a_contribution_error(tmp_path, world_json):
    # World() indexed world.json without checking its schema: TypeError,
    # KeyError or RecursionError.
    (tmp_path / "world.json").write_bytes(world_json)
    with pytest.raises(ContributionError):
        find_building(tmp_path, *GRASSMUENSTER)


def test_cli_reports_bad_models(tmp_path, capsys, sample):
    feature, _ = sample
    model = tmp_path / "model.glb"
    model.write_bytes(b"glTF" + b"\0" * 16)
    code = main(["facade", "--world", str(WORLD), "--building", feature, "--model", str(model),
                 "--author", "github:t", "--title", "t", "--rationale", "t", "--out", str(tmp_path / "b")])
    assert code == 1 and "error:" in capsys.readouterr().err


def test_cli_reads_no_more_of_a_model_than_the_reader_accepts(tmp_path, monkeypatch):
    # read_bytes() loaded a model of any size (or an endless file) first.
    from tools.contrib import __main__ as cli

    monkeypatch.setattr(cli, "MAX_BYTES", 10)
    (tmp_path / "model.glb").write_bytes(b"x" * 100)
    assert cli._read_model(tmp_path / "model.glb") == b"x" * 11
    with pytest.raises(ContributionError):
        cli._read_model(tmp_path / "missing.glb")


def test_facade_node_limits(sample):
    # Coordinates beyond int64 millimetres used to overflow silently.
    part = facade.Part("wall", np.array([[[0, 0, 0], [1e17, 0, 0], [0, 1, 0]]], dtype=np.float64))
    with pytest.raises(ContributionError, match="from the tile origin"):
        facade.facade_node([part], "x:y")
