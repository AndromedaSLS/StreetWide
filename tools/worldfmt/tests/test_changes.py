"""Change sets from Python (tools/worldfmt/changes.py) agree with op_world:
`problems` refuses what `ChangeSet::from_bytes` refuses, with the same
messages, documents over the limit for their kind are refused in the same
words, and `apply` builds the gate's candidate world byte for byte,
checked against ids that Rust made."""

from __future__ import annotations

import copy
import json
import re
import shutil
import subprocess
from pathlib import Path

import pytest

from tools.worldfmt import changes, cjson, limits, schemas
from tools.worldfmt.changes import ChangeError, Objects
from tools.worldfmt.ids import content_id
from tools.worldfmt.store import Store

REPO = Path(__file__).resolve().parents[3]
FIXTURES = REPO / "fixtures"
SAMPLE = FIXTURES / "changes" / "facade-sample"
FIXTURE_WORLD = FIXTURES / "world"
DISTRICT = REPO / "data" / "world" / "zurich-altstadt"
DISTRICT_BEFORE_WATER = REPO / "data" / "world" / "zurich-altstadt-before-water"

# Ids that Rust made: fixtures/world, which has the Limmat, and the same
# world before water, as committed at c16572a (world.json and its only
# tile index).
FIXTURE_WORLD_ID = "blake3:fd6446c21c6b55e27ccfb6093ca73a17c444432513c487b9f3ae262ead8480af"
FIXTURE_TILE_ID = "blake3:c014f59cb4193dc9bf376d3919b2554855e32bb817a48375cba200721b589221"
PRE_WATER_COMMIT = "c16572a"
PRE_WATER_WORLD_ID = "blake3:6af9f18605b98e8b2a640452ac9341c41a1716160db9cacb09168172903ee9f6"
PRE_WATER_TILE_ID = "blake3:abbb69b3e109815c74d76925c9ce7a735069c9f14d142ac3e861c54dd91088f0"
TILE = "17/68646/45903"
SORTED = "changes must be sorted by (tile, layer, feature_id) with no duplicates"
OP_RULE = "add needs a manifest and no parent, replace both, remove a parent and no manifest"
ID_X = content_id(b"x")


# ------------------------------------------------------------ field rules --


def test_text_rule() -> None:
    for ok in ["a", "x" * 4096, "é" * 4096, "© swisstopo", "a b"]:
        assert changes.is_text(ok), ok
    for bad in ["", "x" * 4097, "a\nb", "ends with a newline\n", "tab\t", "delete\x7f", None, 5]:
        assert not changes.is_text(bad), bad


def test_feature_id_rule() -> None:
    # crates/op_world/src/rules.rs, test `feature_ids`, and the length limit.
    for ok in ["osm:way:123", "swisstopo:swissbuildings3d:{ABC-1}", "tile:17/1/2", "streetwide:x", "a:" + "x" * 254]:
        assert changes.is_feature_id(ok), ok
    for bad in ["", "osm", "Osm:way:1", "osm:", "osm:way 1", "1osm:x", ":x", "osm:way:1\n", "a:" + "x" * 255,
                "osm:wäy", "someone@example.org", None]:
        assert not changes.is_feature_id(bad), bad


def test_tile_keys_parse_as_in_op_world() -> None:
    # crates/op_world/src/keys.rs, test `parse_and_region`, and more.
    assert changes.tile_xy("17/68646/45908") == (68646, 45908)
    assert changes.tile_xy("17/131071/0") == (131071, 0)
    for bad in ["17/068646/1", "16/1/1", "17/1", "17/1/1/1", "17/+1/1", "17/131072/0", "17/-1/0",
                "17/1/2\n", "17/ 1/2", "17/1_0/2", "17/99999999999/0", "17//2"]:
        with pytest.raises(ValueError):
            changes.tile_xy(bad)
    with pytest.raises(ValueError, match=re.escape("cell 17/131072/0 is out of range (x, y < 131072)")):
        changes.tile_xy("17/131072/0")
    with pytest.raises(ValueError, match=re.escape('"17/068646/1" is not a zoom-17 key')):
        changes.tile_xy("17/068646/1")


def test_change_sets_sort_tiles_as_strings() -> None:
    keys = ["17/9/5", "17/10/40", "17/10/5"]
    assert sorted(keys, key=changes.tile_xy) == ["17/9/5", "17/10/5", "17/10/40"]
    entries = [{"tile": k, "layer": "shell", "feature_id": "osm:way:1"} for k in keys]
    assert [e["tile"] for e in sorted(entries, key=changes.sort_key)] == ["17/10/40", "17/10/5", "17/9/5"]


def test_rust_debug_form() -> None:
    assert changes.rust_debug("0.2.0") == '"0.2.0"'
    assert changes.rust_debug('a"b\\c\n\t\r\0\x01é') == '"a\\"b\\\\c\\n\\t\\r\\0\\u{1}é"'
    assert changes.rust_debug(None) == "null"
    # Rust escapes a character that extends the one before it: a combining
    # acute accent (Mn), a variation selector (Mn), an enclosing circle (Me).
    assert changes.rust_debug("0.1.0́") == '"0.1.0\\u{301}"'
    assert changes.rust_debug("ok️") == '"ok\\u{fe0f}"'
    assert changes.rust_debug("1⃝") == '"1\\u{20dd}"'


def test_license_and_url_rules() -> None:
    for ok in ["CC-BY-SA-4.0", "LicenseRef-swisstopo-OGD", "ODbL-1.0 AND LicenseRef-swisstopo-OGD", "x" * 256]:
        assert changes.is_license(ok), ok
    for bad in ["", "x" * 257, "MIT\n", "Lizenz-ä", "a/b", None]:
        assert not changes.is_license(bad), bad
    for ok in ["https://www.openstreetmap.org/", "https://x", "https://" + "x" * 2040]:
        assert changes.is_https_url(ok), ok
    for bad in ["http://x", "https://", "https://x y", "https://x\n", "https://" + "x" * 2041, "https://ä", None]:
        assert not changes.is_https_url(bad), bad


def test_the_cap_is_the_schemas_and_op_worlds() -> None:
    schema = json.loads((schemas.SCHEMA_DIR / "change-set.schema.json").read_text(encoding="utf-8"))
    assert changes.MAX_CHANGES == schema["properties"]["changes"]["maxItems"] == 50
    rust = (REPO / "crates" / "op_world" / "src" / "contrib.rs").read_text(encoding="utf-8")
    assert re.search(r"pub const MAX_CHANGES: usize = (\d+);", rust).group(1) == str(changes.MAX_CHANGES)
    assert f'pub const INGEST_AUTHOR: &str = "{changes.INGEST_AUTHOR}";' in rust
    assert f'pub const CONTRIBUTION_CONTRACT_VERSION: &str = "{changes.CONTRIBUTION_CONTRACT_VERSION}";' in rust


# --------------------------------------------------------------- problems --


def committed_change_sets() -> list[Path]:
    return sorted((FIXTURES / "changes").glob("*/change.json"))


@pytest.mark.parametrize("path", committed_change_sets(), ids=lambda p: p.parent.name)
def test_committed_change_sets_pass(path: Path) -> None:
    data = path.read_bytes()
    change = changes.parse(data)
    assert changes.problems(change) == []
    assert changes.to_bytes(change) == data


def sample() -> dict:
    return cjson.loads((SAMPLE / "change.json").read_bytes())


WHAT = f"{TILE} facade swisstopo:swissbuildings3d:000f609c-4003-495f-9426-b43e3d770167"


def as_water(d: dict) -> None:
    d["changes"][0].update(layer="water", feature_id=f"tile:{TILE}")


def fifty(d: dict) -> None:
    one = d["changes"][0]
    d["changes"] = [dict(one, feature_id=f"streetwide:building-{k:02d}") for k in range(50)]


def two_tiles(order: list[str]):
    def edit(d: dict) -> None:
        d["changes"] = [dict(d["changes"][0], tile=t) for t in order]
    return edit


def unsorted(d: dict) -> None:
    one = d["changes"][0]
    d["changes"] = [dict(one, feature_id="streetwide:b"), dict(one, feature_id="streetwide:a")]


# (case, edit, messages): `messages` is what `ChangeSet::problems` says,
# [] when op_world accepts the change set, and None when op_world's parser
# refuses it before its rules run. The cases of
# crates/op_world/tests/contrib_schemas.rs and tests/gate.rs come first.
CASES = [
    ("the sample", lambda d: None, []),
    ("water, as a refresh makes it", as_water, []),
    ("an e-mail address as author", lambda d: d.update(author="someone@example.org"),
     ["author must be a handle such as github:<login>"]),
    ("an add with a parent", lambda d: d["changes"][0].update(parent_version_id=d["base_world_id"]),
     [f"{WHAT}: {OP_RULE}"]),
    ("an unknown op", lambda d: d["changes"][0].update(op="rename"), None),
    ("no changes", lambda d: d.update(changes=[]), ["a change set has 1..=50 changes"]),
    ("51 changes", lambda d: d.update(changes=[d["changes"][0]] * 51), ["a change set has 1..=50 changes", SORTED]),
    ("an unknown field", lambda d: d.update(note="x"), None),
    ("an unknown layer", lambda d: d["changes"][0].update(layer="river"), None),
    ("a duplicate change", lambda d: d["changes"].append(copy.deepcopy(d["changes"][0])), [SORTED]),
    ("50 changes", fifty, []),
    ("unsorted changes", unsorted, [SORTED]),
    ("tiles in string order", two_tiles(["17/100000/5", "17/99999/5"]), []),
    ("tiles in numeric order", two_tiles(["17/99999/5", "17/100000/5"]), [SORTED]),
    ("4096 characters of text", lambda d: d.update(title="é" * 4096), []),
    ("a title of 4097 characters", lambda d: d.update(title="x" * 4097), ["title must be non-empty text"]),
    ("an empty rationale", lambda d: d.update(rationale=""), ["rationale must be non-empty text"]),
    ("another format", lambda d: d.update(format="streetwide.check"), ['format must be "streetwide.change"']),
    ("another contract version", lambda d: d.update(contract_version="0.2.0"),
     ['unknown contribution contract version "0.2.0"']),
    ("a remove with a manifest", lambda d: d["changes"][0].update(op="remove", parent_version_id=ID_X),
     [f"{WHAT}: {OP_RULE}"]),
    ("a replace without a parent", lambda d: d["changes"][0].update(op="replace"), [f"{WHAT}: {OP_RULE}"]),
    ("a feature id with a space", lambda d: d["changes"][0].update(feature_id="osm:way 1"),
     [f"{TILE} facade osm:way 1: invalid feature_id"]),
    ("a feature id of 257 bytes", lambda d: d["changes"][0].update(feature_id="a:" + "x" * 255),
     [f"{TILE} facade a:{'x' * 255}: invalid feature_id"]),
    ("an author with a capital", lambda d: d.update(author="Github:octocat"),
     ["author must be a handle such as github:<login>"]),
    ("several rules at once", lambda d: d.update(title="", rationale="", author="x", changes=[]),
     ["title must be non-empty text", "rationale must be non-empty text",
      "author must be a handle such as github:<login>", "a change set has 1..=50 changes"]),
    ("a tile out of range", lambda d: d["changes"][0].update(tile="17/131072/0"), None),
    ("a tile with a leading zero", lambda d: d["changes"][0].update(tile="17/068646/45903"), None),
    ("a world id in capitals", lambda d: d.update(base_world_id="blake3:" + d["base_world_id"][7:].upper()), None),
    ("a short manifest id", lambda d: d["changes"][0].update(manifest_id="blake3:abc"), None),
    ("a missing rationale", lambda d: d.pop("rationale"), None),
    ("a null title", lambda d: d.update(title=None), None),
    ("a change without its manifest_id", lambda d: d["changes"][0].pop("manifest_id"), None),
    ("an op that is a list", lambda d: d["changes"][0].update(op=["add"]), None),
    ("an op that is an object", lambda d: d["changes"][0].update(op={}), None),
]

# Canonical JSON cannot hold control characters, so these exist only as
# documents in memory. Python's `re` lets the schema's patterns accept a
# final newline; the other layers catch it.
DOCUMENT_ONLY = [
    ("a title ending in a newline", lambda d: d.update(title="Sample\n"), ["title must be non-empty text"]),
    ("an author ending in a newline", lambda d: d.update(author="github:octocat\n"),
     ["author must be a handle such as github:<login>"]),
    ("a world id ending in a newline", lambda d: d.update(base_world_id=d["base_world_id"] + "\n"), None),
    ("a tile ending in a newline", lambda d: d["changes"][0].update(tile=TILE + "\n"), None),
]


def edited_sample(edit) -> dict:
    d = sample()
    edit(d)
    return d


@pytest.mark.parametrize("case,edit,messages", CASES + DOCUMENT_ONLY, ids=[c[0] for c in CASES + DOCUMENT_ONLY])
def test_problems_mirror_op_world(case: str, edit, messages: list[str] | None) -> None:
    doc = edited_sample(edit)
    found = changes.problems(doc)
    if messages == []:
        assert found == []
        return
    assert found, f"{case} passed"
    if messages is None:
        # Refused before the rules: by the schema or by the id and key parser.
        assert changes.schema_problems(changes.SCHEMA_FILE, doc) + changes.id_problems(doc)
    else:
        assert changes.rule_problems(doc) == messages
        assert changes.id_problems(doc) == []


def test_the_id_layer_names_the_field() -> None:
    doc = edited_sample(lambda d: d.update(base_world_id=d["base_world_id"] + "\n"))
    assert changes.schema_problems(changes.SCHEMA_FILE, doc) == [], "Python's re lets the pattern pass"
    (message,) = changes.problems(doc)
    assert message == f'base_world_id: "{doc["base_world_id"][:-1]}\\n" is not a content id (blake3:<64 hex>)'
    doc = edited_sample(lambda d: d["changes"][0].update(tile="17/131072/0"))
    assert "changes[0].tile: cell 17/131072/0 is out of range (x, y < 131072)" in changes.problems(doc)


def test_schema_messages_name_their_path() -> None:
    found = changes.problems(edited_sample(lambda d: d["changes"][0].update(layer="river")))
    assert any(m.startswith("schema: $.changes[0].layer: ") for m in found), found
    assert changes.problems([]) != [] and changes.problems(None) != []


def test_parse_and_to_bytes_refuse_what_problems_finds() -> None:
    doc = edited_sample(lambda d: d.update(author="someone@example.org"))
    with pytest.raises(ChangeError, match="author must be a handle"):
        changes.to_bytes(doc)
    with pytest.raises(ChangeError, match="author must be a handle"):
        changes.parse(cjson.dumps(doc))
    with pytest.raises(ChangeError, match="not canonical JSON"):
        changes.parse(b'{"format": "streetwide.change"}')
    with pytest.raises(ChangeError, match=re.escape("schema: $.changes[0].op: ")):
        changes.parse(cjson.dumps(edited_sample(lambda d: d["changes"][0].update(op=[]))))
    good = sample()
    assert content_id(changes.to_bytes(good)) == content_id((SAMPLE / "change.json").read_bytes())


def gate_details(report: dict, check: str) -> list[str]:
    (found,) = [c for c in report["checks"] if c["id"] == check]
    return found["details"]


@pytest.mark.parametrize("case,edit,messages", CASES, ids=[c[0] for c in CASES])
def test_op_world_gives_the_same_verdict_and_messages(case, edit, messages, check_change, tmp_path) -> None:
    bundle = tmp_path / "bundle"
    shutil.copytree(SAMPLE / "objects", bundle / "objects")
    (bundle / "change.json").write_bytes(cjson.dumps(edited_sample(edit)))
    refused = [d for d in gate_details(check_change(FIXTURE_WORLD, bundle), "G1") if d.startswith("change.json: ")]
    if messages == []:
        assert refused == []
    elif messages is None:
        assert len(refused) == 1 and not refused[0].startswith("change.json: invalid: "), refused
    else:
        assert refused == ["change.json: invalid: " + "; ".join(messages)]


# ------------------------------------------------------------------ apply --

CREDIT = "© Test data"
TILE_A, TILE_B = "17/9/5", "17/10/5"  # A comes first in worlds, B first in change sets


def manifest(layer: str, feature_id: str, tile: str, lo, hi, *, credits=(CREDIT,), collider="trimesh",
             parent=None) -> dict:
    lic = "CC-BY-4.0"
    return {
        "format": "streetwide.asset",
        "contract_version": "0.1.0",
        "content_id": content_id(f"mesh {layer} {feature_id} {lo} {hi}".encode()),
        "size_bytes": 1000,
        "layer": layer,
        "feature_id": feature_id,
        "tile": tile,
        "bounds_mm": {"max": list(hi), "min": list(lo)},
        "physics": {"collider": collider},
        "fictional": False,
        "parent_version_id": parent,
        "provenance": {
            "license": lic,
            "attribution": sorted(credits),
            "sources": [{"dataset": "Test", "version": "1", "license": lic, "attribution": credits[0],
                         "url": "https://example.org/", "source_ids": []}],
            "generator": {"name": "streetwide-ingest", "version": "0.1.0"},
        },
    }


def mid(m: dict) -> str:
    return content_id(cjson.dumps(m))


def build_world(root: Path, manifests: list[dict]) -> tuple[Store, str, dict]:
    """A store with exactly these assets, built from scratch without
    `apply`: the independent answer that `apply` must reproduce."""
    store = Store(root)
    by_tile: dict[str, list[dict]] = {}
    for m in manifests:
        by_tile.setdefault(m["tile"], []).append({
            "layer": m["layer"], "feature_id": m["feature_id"], "manifest_id": store.write_json(m),
            "content_id": m["content_id"], "license": m["provenance"]["license"],
            "collider": m["physics"]["collider"], "bounds_mm": m["bounds_mm"],
        })
    tiles = []
    for key in sorted(by_tile, key=lambda k: tuple(int(v) for v in k.split("/")[1:])):
        assets = sorted(by_tile[key], key=lambda a: (a["layer"].encode(), a["feature_id"].encode()))
        x, y = (int(v) for v in key.split("/")[1:])
        index = {"format": "streetwide.tile", "contract_version": "0.1.0", "tile": key, "region": f"12/{x >> 5}/{y >> 5}",
                 "origin_mm": [x * 100000, 0, y * 100000], "assets": assets}
        tiles.append({
            "tile": key, "tile_id": store.write_json(index), "origin_mm": index["origin_mm"],
            "bounds_mm": {"max": [max(a["bounds_mm"]["max"][k] for a in assets) for k in range(3)],
                          "min": [min(a["bounds_mm"]["min"][k] for a in assets) for k in range(3)]},
        })
    world = {
        "format": "streetwide.world", "contract_version": "0.1.0", "name": "test-world",
        "region": {"key": "12/0/0", "anchor_e9": {"lat": 0, "lon": 0}},
        "attribution": sorted({c for m in manifests for c in m["provenance"]["attribution"]}, key=str.encode),
        "spawn": {"position_mm": [0, 1000, 0], "yaw": 0},
        "tiles": tiles,
    }
    return store, store.write_world(world), world


def base_assets() -> list[dict]:
    return [
        manifest("shell", "osm:way:9", TILE_A, (0, 0, 0), (10, 10, 10)),
        manifest("shell", "osm:way:10", TILE_A, (-5, 0, 2), (3, 4, 5)),
        manifest("terrain", "tile:17/9/5", TILE_A, (-100, -1, -100), (100, 1, 100)),
        manifest("terrain", "tile:17/10/5", TILE_B, (-100, -2, -100), (100, 2, 100)),
        manifest("water", "tile:17/10/5", TILE_B, (-50, 0, -50), (50, 0, 50), credits=("© Water", CREDIT),
                 collider="none"),
    ]


def entry(op: str, m: dict | None, *, tile=None, layer=None, feature_id=None, parent=None) -> dict:
    return {
        "op": op,
        "tile": tile or m["tile"],
        "layer": layer or m["layer"],
        "feature_id": feature_id or m["feature_id"],
        "parent_version_id": parent,
        "manifest_id": None if op == "remove" else mid(m),
    }


def change_set(base_world_id: str, entries: list[dict]) -> dict:
    return {
        "format": "streetwide.change", "contract_version": "0.1.0", "base_world_id": base_world_id,
        "title": "Test", "author": changes.INGEST_AUTHOR, "rationale": "Test.",
        "changes": sorted(entries, key=changes.sort_key),
    }


@pytest.fixture
def base(tmp_path: Path) -> tuple[Store, str, dict, list[dict]]:
    assets = base_assets()
    store, world_id, world = build_world(tmp_path / "base", assets)
    return store, world_id, world, assets


def test_apply_equals_a_world_built_from_scratch(base, tmp_path: Path) -> None:
    store, world_id, world, (way9, way10, terrain_a, terrain_b, water_b) = base
    moved = manifest("shell", "osm:way:9", TILE_B, (20, 0, 20), (30, 10, 30))
    terrain_a2 = manifest("terrain", "tile:17/9/5", TILE_A, (-100, -3, -100), (100, 9, 100), parent=mid(terrain_a))
    way11 = manifest("shell", "osm:way:11", TILE_B, (-200, 0, 0), (-150, 5, 5), credits=("Ä new credit", "Zeta credit"))
    change = change_set(world_id, [
        entry("remove", way9, parent=mid(way9)),
        entry("add", moved),
        entry("replace", terrain_a2, parent=mid(terrain_a)),
        entry("add", way11),
        entry("remove", water_b, parent=mid(water_b)),
    ])
    assert changes.problems(change) == []
    new = {mid(m): cjson.dumps(m) for m in (moved, terrain_a2, way11)}
    candidate = changes.apply(world, change, Objects(new, store))

    _, expected_id, expected = build_world(tmp_path / "expected", [way10, terrain_a2, moved, terrain_b, way11])
    assert candidate.world == expected
    assert candidate.world_id == expected_id
    assert candidate.world_bytes == cjson.dumps(expected)
    assert candidate.tile_ids == {TILE_A: expected["tiles"][0]["tile_id"], TILE_B: expected["tiles"][1]["tile_id"]}
    assert list(candidate.tile_ids) == [TILE_A, TILE_B], "in tile order"
    assert sorted(candidate.objects) == list(candidate.objects) == sorted(candidate.tile_ids.values())
    # The sorted union of credits, by bytes: "Z" < "©" < "Ä". The water's went.
    assert candidate.world["attribution"] == ["Zeta credit", CREDIT, "Ä new credit"]
    assert candidate.world["tiles"][1]["bounds_mm"] == {"max": [100, 10, 100], "min": [-200, -2, -100]}


def test_assets_are_sorted_by_layer_then_feature_id_bytes(base) -> None:
    store, world_id, world, _ = base
    added = [
        manifest("water", "tile:17/9/5", TILE_A, (0, 0, 0), (1, 0, 1), collider="none"),
        manifest("facade", "osm:way:9", TILE_A, (0, 0, 0), (1, 1, 1), collider="none"),
        manifest("transport", "tile:17/9/5", TILE_A, (0, 0, 0), (1, 1, 1), collider="none"),
        manifest("shell", "x:B", TILE_A, (0, 0, 0), (1, 1, 1)),
        manifest("shell", "x:a", TILE_A, (0, 0, 0), (1, 1, 1)),
    ]
    change = change_set(world_id, [entry("add", m) for m in added])
    candidate = changes.apply(world, change, Objects({mid(m): cjson.dumps(m) for m in added}, store))
    index = cjson.loads(candidate.objects[candidate.tile_ids[TILE_A]])
    assert [(a["layer"], a["feature_id"]) for a in index["assets"]] == [
        ("facade", "osm:way:9"),
        ("shell", "osm:way:10"),
        ("shell", "osm:way:9"),
        ("shell", "x:B"),
        ("shell", "x:a"),
        ("terrain", "tile:17/9/5"),
        ("transport", "tile:17/9/5"),
        ("water", "tile:17/9/5"),
    ]


def test_a_tile_may_not_lose_its_last_asset(base) -> None:
    store, world_id, world, (_, _, _, terrain_b, water_b) = base
    change = change_set(world_id, [entry("remove", m, parent=mid(m)) for m in (terrain_b, water_b)])
    with pytest.raises(ChangeError, match=re.escape("tile 17/10/5 would be empty")):
        changes.apply(world, change, store)


def test_edited_tiles_are_checked_in_tile_order(base) -> None:
    # Both tiles would be empty; op_world goes through tiles by (x, y), so
    # it names 17/9/5, although "17/10/5" comes first as a string.
    store, world_id, world, assets = base
    change = change_set(world_id, [entry("remove", m, parent=mid(m)) for m in assets])
    with pytest.raises(ChangeError, match=re.escape("tile 17/9/5 would be empty")):
        changes.apply(world, change, store)


def test_changes_outside_the_world_or_without_a_manifest_fail(base) -> None:
    store, world_id, world, (way9, *_) = base
    outside = manifest("shell", "osm:way:1", "17/11/5", (0, 0, 0), (1, 1, 1))
    with pytest.raises(ChangeError, match=re.escape("tile 17/11/5 is not in the world")):
        objects = Objects({mid(outside): cjson.dumps(outside)}, store)
        changes.apply(world, change_set(world_id, [entry("add", outside)]), objects)
    lost = manifest("shell", "osm:way:12", TILE_A, (0, 0, 0), (1, 1, 1))
    with pytest.raises(ChangeError, match=re.escape("shell osm:way:12: no readable manifest (object ")):
        changes.apply(world, change_set(world_id, [entry("add", lost)]), store)


def test_a_manifest_for_another_key_is_listed_under_its_own_key(base) -> None:
    # As in the gate, the tile index lists what the manifest says (G2
    # fails such a set): here a second terrain of tile B, a duplicate.
    store, world_id, world, (*_, terrain_b, _) = base
    other = manifest("terrain", "tile:17/10/5", TILE_B, (0, 0, 0), (1, 1, 1))
    change = change_set(world_id, [entry("add", other, layer="shell", feature_id="osm:way:12")])
    with pytest.raises(ChangeError, match=re.escape(
        "tile index 17/10/5: assets must be sorted by (layer, feature_id) without duplicates"
    )):
        changes.apply(world, change, Objects({mid(other): cjson.dumps(other)}, store))


def test_an_add_over_an_existing_key_replaces_it(base) -> None:
    # Lineage is G2's business: build_candidate drops the key first.
    store, world_id, world, (way9, *_) = base
    new9 = manifest("shell", "osm:way:9", TILE_A, (0, 0, 0), (11, 11, 11))
    objects = Objects({mid(new9): cjson.dumps(new9)}, store)
    added = changes.apply(world, change_set(world_id, [entry("add", new9)]), objects)
    replaced = changes.apply(world, change_set(world_id, [entry("replace", new9, parent=mid(way9))]), objects)
    assert added.world_id == replaced.world_id


def test_removing_a_key_that_is_not_there_changes_nothing(base) -> None:
    # G2 fails such a set; its candidate is the same world.
    store, world_id, world, (way9, *_) = base
    change = change_set(world_id, [entry("remove", way9, feature_id="osm:way:404", parent=mid(way9))])
    candidate = changes.apply(world, change, store)
    assert candidate.world_id == world_id
    assert candidate.tile_ids == {TILE_A: world["tiles"][0]["tile_id"]}


def test_changes_apply_in_listed_order(base) -> None:
    # Not a valid change set (a key twice), but the order is the rule.
    store, world_id, world, (way9, *_) = base
    gone = {"changes": [entry("add", way9), entry("remove", way9, parent=mid(way9))]}
    kept = {"changes": [entry("remove", way9, parent=mid(way9)), entry("add", way9)]}
    without = changes.apply(world, change_set(world_id, [entry("remove", way9, parent=mid(way9))]), store)
    assert changes.apply(world, gone, store).world_id == without.world_id
    assert changes.apply(world, kept, store).world_id == content_id(cjson.dumps(world))


def test_apply_leaves_its_inputs_alone(base) -> None:
    store, world_id, world, (way9, *_) = base
    before = copy.deepcopy(world)
    objects = Objects(store)
    index_before = copy.deepcopy(objects.tile_index(world["tiles"][0]["tile_id"]))
    changes.apply(world, change_set(world_id, [entry("remove", way9, parent=mid(way9))]), objects)
    assert world == before
    assert objects.tile_index(world["tiles"][0]["tile_id"]) == index_before


def test_sets_in_sequence_equal_all_changes_at_once(base) -> None:
    store, world_id, world, (way9, way10, terrain_a, terrain_b, water_b) = base
    terrain_a2 = manifest("terrain", "tile:17/9/5", TILE_A, (-100, -3, -100), (100, 9, 100), parent=mid(terrain_a))
    first = change_set(world_id, [entry("replace", terrain_a2, parent=mid(terrain_a))])
    objects = Objects({mid(terrain_a2): cjson.dumps(terrain_a2)}, store)
    one = changes.apply(world, first, objects)
    objects.push(one.objects)
    second = change_set(one.world_id, [entry("remove", water_b, parent=mid(water_b)),
                                       entry("remove", way10, parent=mid(way10))])
    two = changes.apply(one.world, second, objects)
    at_once = changes.apply(world, {"changes": first["changes"] + second["changes"]}, objects)
    assert two.world_id == at_once.world_id
    assert two.world_bytes == at_once.world_bytes


def test_candidate_write_makes_the_store_of_the_world(base, tmp_path: Path) -> None:
    store, world_id, world, (way9, *_) = base
    candidate = changes.apply(world, change_set(world_id, [entry("remove", way9, parent=mid(way9))]), store)
    out = tmp_path / "merged"
    shutil.copytree(store.root / "objects", out / "objects")
    assert candidate.write(Store(out)) == candidate.world_id
    written_id, written = Store(out).read_world()
    assert (written_id, written) == (candidate.world_id, candidate.world)
    for tile_id in candidate.tile_ids.values():
        assert Store(out).read_object(tile_id, "json") == candidate.objects[tile_id]


def test_objects_read_from_the_first_source_and_check_ids(base, tmp_path: Path) -> None:
    store, _, world, _ = base
    tile_id = world["tiles"][0]["tile_id"]
    data = store.read_object(tile_id, "json")
    assert Objects(str(store.root)).read(tile_id) == data
    assert Objects({}, store.root).read(tile_id) == data
    with pytest.raises(ChangeError, match="its bytes hash to"):
        Objects({tile_id: data + b" "}, store).read(tile_id)
    corrupt = tmp_path / "corrupt"
    path = Store(corrupt).object_path(tile_id, "json")
    path.parent.mkdir(parents=True)
    path.write_bytes(b"{}")
    objects = Objects(store)
    objects.push(Store(corrupt))
    with pytest.raises(ChangeError, match="its bytes hash to"):
        objects.read(tile_id)  # the first source with the file decides, as in op_world
    missing = content_id(b"nothing")
    with pytest.raises(ChangeError, match=re.escape(f"object {missing}: not found")):
        Objects(store).read(missing)
    with pytest.raises(ChangeError, match="is not a content id"):
        Objects(store).read("blake3:abc")
    with pytest.raises(ChangeError, match="is not a streetwide.asset document"):
        Objects(store).manifest(tile_id)
    with pytest.raises(TypeError):
        Objects(42)


def test_objects_cache_documents(base) -> None:
    store, _, world, _ = base
    objects = Objects(store)
    tile_id = world["tiles"][0]["tile_id"]
    assert objects.tile_index(tile_id) is objects.tile_index(tile_id)
    assert objects.doc(tile_id) is objects.tile_index(tile_id)


# ------------------------------------------ manifests as op_world reads them --


def sample_manifest() -> dict:
    (path,) = (SAMPLE / "objects").rglob("*.json")
    return cjson.loads(path.read_bytes())


def reversed_bounds(m: dict) -> None:
    m["bounds_mm"] = {"max": m["bounds_mm"]["min"], "min": m["bounds_mm"]["max"]}


# (case, edit, messages): manifests that pass the schema but not
# `AssetManifest::problems`, with its messages.
MANIFEST_CASES = [
    ("credits out of order", lambda m: m["provenance"]["attribution"].reverse(),
     ["provenance.attribution must be non-empty, sorted and unique"]),
    ("source ids out of order", lambda m: m["provenance"]["sources"][0].update(source_ids=["x:2", "x:1"]),
     ["provenance.sources[0].source_ids must be sorted and unique"]),
    ("bounds the wrong way round", reversed_bounds, ["bounds_mm min must not exceed max"]),
]


def edited_manifest(edit) -> dict:
    m = sample_manifest()
    edit(m)
    assert schemas.errors(m) == [], "the schema lets it through"
    return m


def edited_bundle(root: Path, edit) -> tuple[str, dict]:
    """The facade sample with an edited manifest: its id and the change set."""
    shutil.copytree(SAMPLE / "objects", root / "objects")
    cid = Store(root).write_json(edited_manifest(edit))
    change = edited_sample(lambda d: d["changes"][0].update(manifest_id=cid))
    (root / "change.json").write_bytes(cjson.dumps(change))
    return cid, change


@pytest.mark.parametrize("case,edit,messages", MANIFEST_CASES, ids=[c[0] for c in MANIFEST_CASES])
def test_manifests_are_read_as_op_world_reads_them(case: str, edit, messages: list[str], tmp_path: Path) -> None:
    assert changes.manifest_problems(sample_manifest()) == []
    assert changes.manifest_problems(edited_manifest(edit)) == messages
    cid, change = edited_bundle(tmp_path / "bundle", edit)
    with pytest.raises(ChangeError, match=re.escape(f"object {cid}: {'; '.join(messages)}")):
        Objects(tmp_path / "bundle").manifest(cid)
    # The gate cannot read the manifest, so it builds no candidate world.
    _, world = Store(FIXTURE_WORLD).read_world()
    with pytest.raises(ChangeError, match=re.escape(f"{change['changes'][0]['layer']} "
                                                    f"{change['changes'][0]['feature_id']}: no readable manifest")):
        changes.apply(world, change, Objects(tmp_path / "bundle", FIXTURE_WORLD))


@pytest.mark.parametrize("case,edit,messages", MANIFEST_CASES, ids=[c[0] for c in MANIFEST_CASES])
def test_op_world_refuses_the_same_manifests(case, edit, messages, check_change, tmp_path: Path) -> None:
    edited_bundle(tmp_path / "bundle", edit)
    report = check_change(FIXTURE_WORLD, tmp_path / "bundle")
    assert report["candidate_world_id"] is None and not report["passed"]
    assert any(d.endswith(": " + "; ".join(messages)) for d in gate_details(report, "G1")), gate_details(report, "G1")


# -------------------------------------------- documents over their limit --
# RFC 0001 section 7: refused before they are parsed, in op_world's words.


def oversized_bundle(root: Path) -> tuple[Path, str]:
    """The facade sample with its manifest one byte over the limit for asset
    manifests (a sparse file where the file system allows), and its id."""
    shutil.copytree(SAMPLE / "objects", root / "objects")
    shutil.copy(SAMPLE / "change.json", root / "change.json")
    cid = sample()["changes"][0]["manifest_id"]
    with open(Store(root).object_path(cid, "json"), "wb") as f:
        f.truncate(limits.ASSET_MANIFEST.max_bytes + 1)
    return root, cid


def test_documents_over_their_limit_are_refused_in_op_worlds_words(tmp_path: Path) -> None:
    data = b" " * (limits.CHANGE_SET.max_bytes + 1)
    with pytest.raises(ChangeError, match=f"^{re.escape(limits.CHANGE_SET.too_large())}$"):
        changes.parse(data)
    bundle, cid = oversized_bundle(tmp_path / "bundle")
    with pytest.raises(ChangeError, match=f"^object {cid}: {re.escape(limits.ASSET_MANIFEST.too_large())}$"):
        Objects(bundle, FIXTURE_WORLD).manifest(cid)
    shutil.rmtree(bundle)


def test_op_world_refuses_the_same_documents_over_their_limit(check_change, tmp_path: Path) -> None:
    bundle = tmp_path / "change-set"
    shutil.copytree(SAMPLE / "objects", bundle / "objects")
    (bundle / "change.json").write_bytes(b" " * (limits.CHANGE_SET.max_bytes + 1))
    report = check_change(FIXTURE_WORLD, bundle)
    assert gate_details(report, "G1") == [f"change.json: invalid: {limits.CHANGE_SET.too_large()}"]
    bundle, cid = oversized_bundle(tmp_path / "manifest")
    report = check_change(FIXTURE_WORLD, bundle)
    assert report["candidate_world_id"] is None and not report["passed"]
    message = f": object {cid}: {limits.ASSET_MANIFEST.too_large()}"
    assert any(d.endswith(message) for d in gate_details(report, "G1")), gate_details(report, "G1")
    shutil.rmtree(tmp_path / "change-set")
    shutil.rmtree(bundle)


# ----------------------------------------- agreement with ids Rust made --


def fixture_water() -> tuple[str, dict, dict]:
    store = Store(FIXTURE_WORLD)
    world_id, world = store.read_world()
    index = cjson.loads(store.read_object(world["tiles"][0]["tile_id"], "json"))
    (water,) = [a for a in index["assets"] if a["layer"] == "water"]
    return world_id, world, water


def water_change(base_world_id: str, op: str, water: dict) -> dict:
    return change_set(base_world_id, [{
        "op": op, "tile": TILE, "layer": "water", "feature_id": water["feature_id"],
        "parent_version_id": water["manifest_id"] if op == "remove" else None,
        "manifest_id": water["manifest_id"] if op == "add" else None,
    }])


def git_show(spec: str) -> bytes | None:
    """A file as committed, or None where git or the commit is missing."""
    try:
        done = subprocess.run(["git", "show", spec], cwd=REPO, capture_output=True, timeout=60)
    except (OSError, subprocess.SubprocessError):
        return None
    return done.stdout if done.returncode == 0 else None


def test_without_its_water_the_fixture_is_the_world_rust_made_before_water(tmp_path: Path) -> None:
    world_id, world, water = fixture_water()
    assert world_id == FIXTURE_WORLD_ID
    remove = water_change(world_id, "remove", water)
    assert changes.problems(remove) == []
    candidate = changes.apply(world, remove, Objects(FIXTURE_WORLD))
    assert candidate.world_id == PRE_WATER_WORLD_ID
    assert candidate.tile_ids == {TILE: PRE_WATER_TILE_ID}
    assert list(candidate.objects) == [PRE_WATER_TILE_ID]

    # In a temporary store: the fixture's objects, then the candidate.
    root = tmp_path / "before-water"
    shutil.copytree(FIXTURE_WORLD / "objects", root / "objects")
    store = Store(root)
    assert candidate.write(store) == PRE_WATER_WORLD_ID
    for spec, written in [
        (f"{PRE_WATER_COMMIT}:fixtures/world/world.json", root / "world.json"),
        (f"{PRE_WATER_COMMIT}:fixtures/world/objects/ab/{PRE_WATER_TILE_ID[7:]}.json",
         store.object_path(PRE_WATER_TILE_ID, "json")),
    ]:
        committed = git_show(spec)
        if committed is not None:  # a shallow clone lacks the commit; the ids pin the bytes anyway
            assert written.read_bytes() == committed

    # Adding the water back gives fixtures/world exactly.
    before_id, before = store.read_world()
    add = water_change(before_id, "add", water)
    again = changes.apply(before, add, Objects(store))
    assert again.world_id == FIXTURE_WORLD_ID
    assert again.tile_ids == {TILE: FIXTURE_TILE_ID}
    assert again.world_bytes == (FIXTURE_WORLD / "world.json").read_bytes()
    assert again.objects[FIXTURE_TILE_ID] == Store(FIXTURE_WORLD).read_object(FIXTURE_TILE_ID, "json")


def test_op_world_builds_the_same_candidates(check_change, tmp_path: Path) -> None:
    world_id, world, water = fixture_water()
    remove = water_change(world_id, "remove", water)
    bundle = tmp_path / "remove"
    bundle.mkdir()
    (bundle / "change.json").write_bytes(changes.to_bytes(remove))
    report = check_change(FIXTURE_WORLD, bundle)
    assert report["passed"] and report["candidate_world_id"] == PRE_WATER_WORLD_ID

    root = tmp_path / "before-water"
    shutil.copytree(FIXTURE_WORLD / "objects", root / "objects")
    changes.apply(world, remove, Objects(FIXTURE_WORLD)).write(Store(root))
    add = water_change(PRE_WATER_WORLD_ID, "add", water)
    bundle = tmp_path / "add"
    manifest_bytes = Store(FIXTURE_WORLD).read_object(water["manifest_id"], "json")
    Store(bundle).write_object(manifest_bytes, "json")
    Store(bundle).write_object(Store(FIXTURE_WORLD).read_object(water["content_id"], "glb"), "glb")
    (bundle / "change.json").write_bytes(changes.to_bytes(add))
    report = check_change(root, bundle)
    assert report["passed"] and report["candidate_world_id"] == FIXTURE_WORLD_ID


@pytest.mark.skipif(
    not (DISTRICT / "world.json").is_file() or not (DISTRICT_BEFORE_WATER / "world.json").is_file(),
    reason="needs the local district stores in data/world (gitignored)",
)
def test_the_district_without_its_water_is_the_store_built_before_water() -> None:
    # Two ingests, before and after water (RFC 0003): everything else is
    # byte-identical, so removing the 8 water assets gives the older store.
    current_id, current = Store(DISTRICT).read_world()
    before_id, _ = Store(DISTRICT_BEFORE_WATER).read_world()
    objects = Objects(DISTRICT)
    removals = []
    for wt in current["tiles"]:
        for a in objects.tile_index(wt["tile_id"])["assets"]:
            if a["layer"] == "water":
                removals.append({"op": "remove", "tile": wt["tile"], "layer": "water", "feature_id": a["feature_id"],
                                 "parent_version_id": a["manifest_id"], "manifest_id": None})
    assert len(removals) == 8
    remove = change_set(current_id, removals)
    assert changes.problems(remove) == []
    candidate = changes.apply(current, remove, objects)
    assert candidate.world_id == before_id
    objects.push(candidate.objects)
    adds = [dict(r, op="add", parent_version_id=None, manifest_id=r["parent_version_id"]) for r in removals]
    assert changes.apply(candidate.world, change_set(before_id, adds), objects).world_id == current_id
