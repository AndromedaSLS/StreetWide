"""Points documents (RFC 0004) in Python: the golden documents of the
format fixture come out byte for byte as op_world writes them, and the
reader refuses what op_world refuses, with op_world's messages
(crates/op_world/tests/points.rs pins the same ones)."""

from __future__ import annotations

import copy
import json
from pathlib import Path

import pytest

from tools.worldfmt import changes, cjson, ids, limits, points, schemas
from tools.worldfmt.store import Store

FORMAT_DIR = Path(__file__).resolve().parents[3] / "fixtures" / "format"
SPECS = json.loads((FORMAT_DIR / "points.json").read_text(encoding="utf-8"))
PINNED = json.loads((FORMAT_DIR / "expected.json").read_text(encoding="utf-8"))["points"]
LAYERS = ("street_objects", "vegetation")


def golden(layer: str) -> dict:
    """A fresh copy of the committed points document of `layer`."""
    data = Store(FORMAT_DIR / "world").read_object(PINNED[layer], "json")
    return json.loads(data)


def manifest_of(layer: str) -> dict:
    for path in sorted((FORMAT_DIR / "world").rglob("*.json")):
        doc = json.loads(path.read_bytes())
        if doc.get("format") == "streetwide.asset" and doc["layer"] == layer:
            return doc
    raise AssertionError(f"no {layer} manifest in the format fixture")


# ----------------------------------------------------------------- parity --


@pytest.mark.parametrize("layer", LAYERS)
def test_python_writes_the_golden_documents_byte_for_byte(layer: str) -> None:
    data = points.to_bytes(SPECS[layer])
    assert ids.content_id(data) == PINNED[layer]
    assert Store(FORMAT_DIR / "world").read_object(PINNED[layer], "json") == data
    assert points.parse(data) == json.loads(data)
    assert schemas.errors(json.loads(data)) == []


def test_the_spec_is_written_in_reading_order_not_canonically() -> None:
    # So the parity test above exercises the canonical writer: the spec's
    # keys are not sorted, and it has whitespace.
    raw = (FORMAT_DIR / "points.json").read_text(encoding="utf-8")
    assert raw.index('"format"') < raw.index('"contract_version"')
    assert set(PINNED) == set(LAYERS) == {k for k in SPECS if k != "comment"}


@pytest.mark.parametrize("layer", LAYERS)
def test_the_golden_assets_agree_with_their_manifests(layer: str) -> None:
    m = manifest_of(layer)
    doc = golden(layer)
    assert m["content_id"] == PINNED[layer]
    assert m["physics"]["collider"] == "none"
    assert points.asset_problems(m["layer"], m["feature_id"], m["tile"], m["bounds_mm"], doc) == []
    assert m["bounds_mm"] == points.bounds(doc)
    assert m["feature_id"] == f"tile:{m['tile']}/{points.source(doc)}"
    assert changes.manifest_problems(m) == []


# ------------------------------------------------------------- the reader --

RULE_CASES = [
    ("unsorted ids", "vegetation", lambda d: d["points"].reverse(),
     "points must be sorted by id without duplicates"),
    ("a duplicate id", "vegetation", lambda d: d["points"][1].update(id=d["points"][0]["id"]),
     "points must be sorted by id without duplicates"),
    ("two sources", "vegetation", lambda d: d["points"][1].update(id="streetwide:urban-trees:T-2"),
     'every point must have the same source (its id without the last part), not "streetwide:sample-trees", '
     '"streetwide:urban-trees"'),
    ("an estimate without a rule", "street_objects", lambda d: d.update(rules={}),
     'point "streetwide:sample-lights:L-1": estimated "yaw" has no rule in rules.light'),
    ("a rule no point uses", "street_objects", lambda d: d["points"][0].update(estimated=[]),
     "rules.light.yaw: no light point has an estimated yaw"),
    ("an estimated null", "street_objects", lambda d: d["points"][0]["attributes"].update(yaw=None),
     'point "streetwide:sample-lights:L-1": estimated "yaw" is null'),
    ("a genus with a final newline", "vegetation", lambda d: d["points"][0]["attributes"].update(genus="Platanus\n"),
     None),
]


@pytest.mark.parametrize("name, layer, mutate, expected", RULE_CASES, ids=[c[0] for c in RULE_CASES])
def test_the_rules_the_schema_cannot_state(name, layer, mutate, expected) -> None:
    doc = golden(layer)
    mutate(doc)
    found = points.problems(doc)
    assert found, name
    if expected is not None:
        # The schema passes; op_world's message, word for word.
        assert found == [expected]
        assert schemas.errors(doc) == []
        with pytest.raises(points.PointsError) as e:
            points.parse(cjson.dumps(doc))
        assert str(e.value) == expected


SCHEMA_CASES = [
    ("no points", "vegetation", lambda d: d.update(points=[])),
    ("an unknown kind", "vegetation", lambda d: d["points"][0].update(kind="bench")),
    ("a light's attributes on a tree", "vegetation",
     lambda d: d["points"][0].update(attributes=copy.deepcopy(golden("street_objects")["points"][1]["attributes"]))),
    ("an omitted null", "vegetation", lambda d: d["points"][1]["attributes"].pop("taxon")),
    ("an unknown attribute", "street_objects", lambda d: d["points"][1]["attributes"].update(colour=None)),
    ("a yaw of a whole turn", "street_objects", lambda d: d["points"][0]["attributes"].update(yaw=65536)),
    ("a crown of 0 mm", "vegetation", lambda d: d["points"][0]["attributes"].update(crown_diameter_mm=0)),
    ("planted in 3000", "vegetation", lambda d: d["points"][0]["attributes"].update(planted_year=3000)),
    ("a lowercase genus", "vegetation", lambda d: d["points"][0]["attributes"].update(genus="platanus")),
    ("a one-word species", "vegetation", lambda d: d["points"][0]["attributes"].update(species="Platanus")),
    ("a species with two spaces", "vegetation",
     lambda d: d["points"][0]["attributes"].update(species="Platanus  hispanica")),
    ("a taxon of 257 characters", "vegetation", lambda d: d["points"][0]["attributes"].update(taxon="x" * 257)),
    ("an unknown method", "street_objects", lambda d: d["points"][0]["attributes"].update(method="plasma")),
    ("an id without a source", "vegetation", lambda d: d["points"][1].update(id="tree-2")),
    ("a space in a local id", "vegetation", lambda d: d["points"][1].update(id="streetwide:sample-trees:T 2")),
    ("an estimated mount", "street_objects", lambda d: d["points"][0].update(estimated=["mount", "yaw"])),
    ("an estimated tree height", "vegetation", lambda d: d["points"][0].update(estimated=["height_mm"])),
    ("estimates twice", "street_objects", lambda d: d["points"][0].update(estimated=["yaw", "yaw"])),
    ("a rule id that is not one", "street_objects", lambda d: d["rules"]["light"].update(yaw="orientierung")),
    ("a rule for a tree", "vegetation", lambda d: d["rules"].update(tree={"height_mm": "adr-0016:height:1"})),
    ("an empty rule set", "street_objects", lambda d: d["rules"].update(light={})),
    ("a short anchor", "vegetation", lambda d: d["points"][0].update(anchor_mm=[1, 2])),
    ("another format", "vegetation", lambda d: d.update(format="streetwide.asset")),
    ("another contract version", "vegetation", lambda d: d.update(contract_version="0.2.0")),
]


@pytest.mark.parametrize("name, layer, mutate", SCHEMA_CASES, ids=[c[0] for c in SCHEMA_CASES])
def test_the_schema_refuses(name, layer, mutate) -> None:
    doc = golden(layer)
    mutate(doc)
    found = points.problems(doc)
    assert found and all(f.startswith("schema: ") for f in found), (name, found)
    with pytest.raises(points.PointsError):
        points.to_bytes(doc)


def test_floats_and_non_canonical_bytes_are_refused() -> None:
    doc = golden("vegetation")
    doc["points"][0]["attributes"]["height_mm"] = 18500.5
    with pytest.raises(points.PointsError):
        points.to_bytes(doc)
    data = cjson.dumps(golden("vegetation"))
    with pytest.raises(points.PointsError, match="not canonical JSON"):
        points.parse(data[:1] + b" " + data[1:])
    with pytest.raises(points.PointsError, match="not canonical JSON"):
        points.parse(data.replace(b"9000", b"9000.0"))


def test_a_document_over_its_limit_is_refused_unparsed() -> None:
    assert limits.POINTS.max_bytes == 64 << 20
    with pytest.raises(points.PointsError) as e:
        points.parse(b"[" + b" " * limits.POINTS.max_bytes)
    assert str(e.value) == "larger than 67108864 bytes (64 MiB), the limit for a points document"


@pytest.mark.parametrize("value", [None, 5, "x", [], {"format": "streetwide.points"}, {"points": [1]}])
def test_problems_takes_any_json_value(value) -> None:
    found = points.problems(value)
    assert found and all(isinstance(f, str) for f in found)


# -------------------------------------------------------------- the asset --


def test_asset_problems_are_op_worlds() -> None:
    trees = golden("vegetation")
    tile = "17/68646/45908"
    fid = f"tile:{tile}/streetwide:sample-trees"
    b = points.bounds(trees)
    assert points.asset_problems("vegetation", fid, tile, b, trees) == []
    # A kind in the wrong layer.
    assert points.asset_problems("street_objects", fid, tile, b, trees) == [
        "points of kind tree belong in the vegetation layer, not street_objects"]
    # A feature id naming another source (a wrong prefix).
    assert points.asset_problems("vegetation", f"tile:{tile}/osm:node", tile, b, trees) == [
        'the points\' source must be "osm:node", the source of feature id "tile:17/68646/45908/osm:node"']
    wider = {"max": b["max"], "min": [b["min"][0] - 1, *b["min"][1:]]}
    assert points.asset_problems("vegetation", fid, tile, wider, trees) == [
        "bounds_mm are not the exact bounds of the anchors"]
    assert points.asset_problems("shell", fid, tile, b, trees) == ["shell assets are meshes, not points documents"]


@pytest.mark.parametrize("layer", LAYERS)
def test_point_manifests_name_their_own_tile_and_never_collide(layer: str) -> None:
    m = manifest_of(layer)
    assert schemas.errors(m) == []
    # A trimesh collider: the schema refuses it.
    trimesh = copy.deepcopy(m)
    trimesh["physics"]["collider"] = "trimesh"
    assert schemas.errors(trimesh) != []
    # No source, or not a per-tile id: the schema refuses them.
    for fid in ("tile:17/68646/45908", "tile:17/68646/45908/", "osm:node:1", "tile:17/68646/45908/OSM:node"):
        bad = dict(m, feature_id=fid)
        assert schemas.errors(bad) != [], fid
    # Another tile's id passes the schema; op_world compares it with the
    # manifest's tile, and so does `manifest_problems`.
    other = dict(m, feature_id="tile:17/1/2/streetwide:sample-trees")
    assert schemas.errors(other) == []
    assert changes.manifest_problems(other) == [
        f'{layer} feature_id "tile:17/1/2/streetwide:sample-trees" must be tile:17/68646/45908/<source> (RFC 0004)']


@pytest.mark.parametrize("ok, source", [
    ("stadt-zuerich:baumkataster:AA-1656", "stadt-zuerich:baumkataster"),
    ("stadt-zuerich:beleuchtung:LEU105227", "stadt-zuerich:beleuchtung"),
    ("osm:node:123", "osm:node"),
    ("streetwide:0-a", "streetwide"),
    ('a:1b:"x\\;', "a:1b"),
    ("a:" + "x" * 254, "a"),
])
def test_point_ids(ok: str, source: str) -> None:
    assert points.point_source(ok) == source


@pytest.mark.parametrize("bad", ["", "osm", "osm:", ":1", "Osm:node:1", "osm::1", "osm:Node:1", "osm:node:1 2",
                                 "osm:node:é", "1osm:node:1", "osm:_node:1", "a:" + "x" * 255,
                                 "osm:node:1\n", 5, None])
def test_not_point_ids(bad) -> None:
    assert points.point_source(bad) is None


def test_rule_ids() -> None:
    for ok in ("adr-0015:yaw-from-orientierung:1", "adr-0016:a:10", "adr-9999:x2-y:7",
               "adr-0015:" + "x" * 117 + ":1"):
        assert points.is_rule_id(ok), ok
    for bad in ("", "adr-0015:yaw:0", "adr-0015:yaw:01", "adr-015:yaw:1", "rfc-0004:yaw:1", "adr-0015:Yaw:1",
                "adr-0015:yaw--x:1", "adr-0015::1", "adr-0015:yaw", "adr-0015:yaw:1:2",
                "adr-0015:" + "x" * 118 + ":1", "adr-0015:yaw:1\n", 1):
        assert not points.is_rule_id(bad), bad
