"""Licences and credits against sources (ADR-0017), in Python as
`Provenance::licence_problems` checks them: the cases and messages of
crates/op_world/tests/licences.rs."""

from __future__ import annotations

import copy
from pathlib import Path

from tools.worldfmt import cjson
from tools.worldfmt.licences import is_license_id, licence_problems, license_ids
from tools.worldfmt.store import Store

REPO = Path(__file__).resolve().parents[3]

OSM = {
    "dataset": "OpenStreetMap",
    "version": "2026-11-23T08:22:35Z",
    "license": "ODbL-1.0",
    "attribution": "© OpenStreetMap contributors",
    "url": "https://www.openstreetmap.org/",
    "source_ids": ["way/1"],
}


def house_provenance() -> dict:
    store = Store(REPO / "fixtures" / "format" / "world")
    _, world = store.read_world()
    index = cjson.loads(store.read_object(world["tiles"][0]["tile_id"], "json"))
    (house,) = [a for a in index["assets"] if a["feature_id"] == "streetwide:sample-house"]
    return cjson.loads(store.read_object(house["manifest_id"], "json"))["provenance"]


def covered(p):
    p["sources"].append(dict(OSM))
    p["license"] = "CC-BY-SA-4.0 AND ODbL-1.0"
    p["attribution"] = sorted(p["attribution"] + [OSM["attribution"]])


def uncovered(p):
    p["sources"].append(dict(OSM))
    p["attribution"] = sorted(p["attribution"] + [OSM["attribution"]])


def uncredited(p):
    p["sources"].append(dict(OSM))
    p["license"] = "CC-BY-SA-4.0 AND ODbL-1.0"


def with_or(p):
    p["license"] = "CC-BY-SA-4.0 OR MIT"


def two_licences(p):
    p["sources"][0]["license"] = "CC-BY-SA-4.0 AND CC0-1.0"
    p["license"] = "CC-BY-SA-4.0 AND CC0-1.0"


def everything(p):
    p["license"] = "(CC-BY-SA-4.0)"
    p["sources"][0]["license"] = "MIT OR Apache-2.0"
    p["sources"][0]["attribution"] = "Someone else"
    p["sources"].append(dict(OSM))


CASES = [
    ("as it is", lambda p: None, []),
    ("a second source, covered", covered, []),
    ("a source licence missing from the asset's", uncovered, [
        'provenance.sources[1].license "ODbL-1.0" is not in the asset\'s licence "CC-BY-SA-4.0" (ADR-0017)']),
    ("a source credit missing from the asset's attribution", uncredited, [
        'provenance.sources[1].attribution "© OpenStreetMap contributors" is not in the asset\'s attribution '
        "(ADR-0017)"]),
    ("an asset licence with OR", with_or, [
        'provenance.license "CC-BY-SA-4.0 OR MIT" must join licence ids with AND only (ADR-0017)']),
    ("a source with two licences", two_licences, [
        'provenance.sources[0].license "CC-BY-SA-4.0 AND CC0-1.0" must be one licence id (ADR-0017)']),
    ("everything at once", everything, [
        'provenance.license "(CC-BY-SA-4.0)" must join licence ids with AND only (ADR-0017)',
        'provenance.sources[0].license "MIT OR Apache-2.0" must be one licence id (ADR-0017)',
        'provenance.sources[0].attribution "Someone else" is not in the asset\'s attribution (ADR-0017)',
        'provenance.sources[1].attribution "© OpenStreetMap contributors" is not in the asset\'s attribution '
        "(ADR-0017)"]),
]


def test_licence_ids() -> None:
    for ok in ["CC0-1.0", "LicenseRef-swisstopo-OGD", "GPL-2.0+", "DocumentRef-a:LicenseRef-b"]:
        assert is_license_id(ok), ok
    for bad in ["", "MIT OR Apache-2.0", "CC0-1.0 AND ODbL-1.0", "(MIT)", "CC0 1.0", "©", None, 1]:
        assert not is_license_id(bad), bad
    assert license_ids("CC0-1.0 AND LicenseRef-swisstopo-OGD") == ["CC0-1.0", "LicenseRef-swisstopo-OGD"]
    assert license_ids("ODbL-1.0") == ["ODbL-1.0"]
    for bad in ["MIT OR Apache-2.0", "(MIT AND CC0-1.0)", "MIT  AND CC0-1.0", "MIT AND", "MIT and CC0-1.0"]:
        assert license_ids(bad) is None, bad


def test_licence_problems_as_op_world_names_them() -> None:
    house = house_provenance()
    for name, edit, wanted in CASES:
        p = copy.deepcopy(house)
        edit(p)
        assert licence_problems(p) == wanted, name


def test_every_committed_manifest_keeps_the_rules() -> None:
    for root in [REPO / "fixtures" / "format" / "world", REPO / "fixtures" / "world"]:
        store = Store(root)
        _, world = store.read_world()
        for t in world["tiles"]:
            for a in cjson.loads(store.read_object(t["tile_id"], "json"))["assets"]:
                m = cjson.loads(store.read_object(a["manifest_id"], "json"))
                assert licence_problems(m["provenance"]) == [], (root, a["feature_id"])
