"""The refresh planner's matching and classification (ADR-0020, Q1 to
Q8): which key gets which change, rule C, the bundles of
simple changes, and units."""

from __future__ import annotations

from pathlib import Path

from tools.ingest.changes.common import Box, overlap_area, overlaps, touches
from tools.ingest.changes.stores import content_key
from tools.ingest.tests.refresh_worlds import (
    A, B, D, DEM_2027, GROUND_MM, OSM_V2, Scenario, World, deck, emitted, held, moved_box, osm_shell, ribbon,
    swiss_shell, terrain, water,
)
from tools.worldfmt import cjson
from tools.worldfmt.ids import content_id
from tools.worldfmt.store import Store


def district() -> World:
    """A small world with one asset of every kind the ingest makes."""
    return World.of(
        terrain(A), terrain(B), swiss_shell(1),
        swiss_shell(2, lo=(50_000, GROUND_MM, 10_000), hi=(60_000, GROUND_MM + 9_000, 20_000)),
        swiss_shell(3, B), osm_shell(5), ribbon(A), deck(7), water(A),
    )


def bundle_files(plan, path: str) -> list[str]:
    return sorted(k for k in plan.files if k.startswith(path + "/objects/"))


def test_identical_stores_give_nothing(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, district())
    plan = sc.plan(district())
    assert plan.sets == [] and plan.report["sets"] == []
    assert plan.summary == "nothing to refresh: 0 changes (0 provenance-only differences kept)"
    assert sorted(plan.files) == ["refresh-report.json", "refresh-report.md"]
    assert plan.report["final_world_id"] == plan.report["current_world_id"]
    counts = {c["layer"]: c for c in plan.report["counts"]}
    assert counts["shell"]["unchanged"] == 4 and counts["terrain"]["unchanged"] == 2
    assert sum(c["add"] + c["replace"] + c["remove"] + c["move"] for c in counts.values()) == 0


def test_provenance_only_is_kept(tmp_path: Path) -> None:
    # A fresh OpenStreetMap snapshot changes only the OSM source's version.
    sc = Scenario.start(tmp_path, district())
    new = district().put(osm_shell(5, version=OSM_V2), ribbon(A, version=OSM_V2), deck(7, version=OSM_V2),
                         water(A, version=OSM_V2))
    plan = sc.plan(new)
    assert plan.sets == []
    assert plan.summary == "nothing to refresh: 0 changes (4 provenance-only differences kept)"
    assert plan.report["residual"]["provenance_only"] == 4
    counts = {c["layer"]: c["provenance_only"] for c in plan.report["counts"]}
    assert counts == {"shell": 1, "terrain": 0, "transport": 2, "water": 1}


def test_licence_change_same_mesh_is_replaced(tmp_path: Path) -> None:
    # A new ingest VERSION fixes one licence: a replace of the manifest only.
    sc = Scenario.start(tmp_path, district())
    v2 = ("streetwide-ingest", "0.2.0")
    new = World.of(*[a.but(generator=v2) for a in district().assets.values()])
    shell = swiss_shell(1)
    new.put(shell.but(generator=v2, license="CC-BY-4.0"))
    plan = sc.plan(new)
    (s,) = plan.sets
    (e,) = s.entries
    assert (e["op"], e["feature_id"]) == ("replace", shell.feature_id)
    assert {c["layer"]: c["manifest_only"] for c in plan.report["counts"]}["shell"] == 1
    files = bundle_files(plan, s.path)
    assert len(files) == 1 and files[0].endswith(".json"), "the bundle has the manifest and no glb"
    manifest = cjson.loads(plan.files[files[0]])
    assert manifest["content_id"] == shell.content_id and manifest["provenance"]["license"] == "CC-BY-4.0"
    assert plan.report["residual"]["provenance_only"] == len(district().assets) - 1


def test_dem_reissue_changes_source_ids(tmp_path: Path) -> None:
    # New swissALTI3D item ids with identical heights: rule C counts source
    # ids as content, so every asset that lists the item is replaced, with
    # its manifest only; the terrain keeps its edges (H8 passes).
    sc = Scenario.start(tmp_path, district())
    new = district().put(terrain(A, item=DEM_2027), terrain(B, item=DEM_2027), osm_shell(5, item=DEM_2027),
                         ribbon(A, item=DEM_2027), deck(7, item=DEM_2027), water(A, item=DEM_2027))
    plan = sc.plan(new)
    assert held(plan) == {}
    replaced = sorted((layer, fid) for _, op, layer, fid in emitted(plan) if op == "replace")
    assert replaced == sorted([("terrain", f"tile:{A}"), ("terrain", f"tile:{B}"), ("shell", "osm:way:5"),
                               ("transport", f"tile:{A}"), ("transport", "osm:way:7"), ("water", f"tile:{A}")])
    assert [(s["purpose"]["collider"], s["purpose"]["layer"], s["purpose"]["dataset"]) for s in plan.report["sets"]] == [
        ("none", "transport", "OpenStreetMap"), ("none", "water", "OpenStreetMap"),
        ("trimesh", "shell", "OpenStreetMap"), ("trimesh", "terrain", "swissALTI3D"),
        ("trimesh", "transport", "OpenStreetMap"),
    ]
    assert all(not f.endswith(".glb") for f in plan.files), "manifest-only replaces ship no mesh"
    (component,) = plan.report["height_components"]
    assert component == {"dem_source_ids": ["swissalti3d_2026_2683-1247", "swissalti3d_2027_2683-1247"],
                         "changes": 6, "terrain_changes": 2, "status": "emitted"}


def test_mesh_change_is_replaced_with_lineage(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, district())
    old = swiss_shell(1)
    changed = moved_box(swiss_shell(1, year="2026"), dy=0, dx=200)
    plan = sc.plan(district().put(changed))
    (s,) = plan.sets
    (e,) = s.entries
    assert e["op"] == "replace" and e["parent_version_id"] == old.manifest_id
    manifest = cjson.loads(plan.files[f"{s.path}/objects/{e['manifest_id'][7:9]}/{e['manifest_id'][7:]}.json"])
    expected = changed.manifest()
    expected["parent_version_id"] = old.manifest_id
    assert manifest == expected, "the new store's manifest with only the parent rewritten"
    assert content_id(cjson.dumps(expected)) == e["manifest_id"]
    assert f"{s.path}/objects/{changed.content_id[7:9]}/{changed.content_id[7:]}.glb" in plan.files

    # Merged, then refreshed again: the next replace names the replaced version.
    sc.log.refresh(sc.current, [e], [changed.but(parent=old.manifest_id)])
    again = moved_box(swiss_shell(1, year="2027"), dx=400)
    plan = sc.plan(district().put(again))
    (e2,) = plan.sets[0].entries
    assert e2["parent_version_id"] == e["manifest_id"]


def test_new_feature_is_added_with_new_store_bytes(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, district())
    added = swiss_shell(9, lo=(150_000, GROUND_MM, 20_000), hi=(160_000, GROUND_MM + 8_000, 30_000))
    new_root = sc.write_new(district().put(added))
    plan = sc.plan(new_root)
    (s,) = plan.sets
    (e,) = s.entries
    assert (e["op"], e["parent_version_id"], e["manifest_id"]) == ("add", None, added.manifest_id)
    stored = Store(new_root).object_path(added.manifest_id, "json").read_bytes()
    assert plan.files[f"{s.path}/objects/{added.manifest_id[7:9]}/{added.manifest_id[7:]}.json"] == stored


def test_gone_feature_is_removed(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, district())
    gone = swiss_shell(2, lo=(50_000, GROUND_MM, 10_000), hi=(60_000, GROUND_MM + 9_000, 20_000))
    plan = sc.plan(district().drop(gone.key))
    (s,) = plan.sets
    assert s.entries == [{"op": "remove", "tile": A, "layer": "shell", "feature_id": gone.feature_id,
                          "parent_version_id": gone.manifest_id, "manifest_id": None}]
    assert bundle_files(plan, s.path) == []


def test_moved_feature_is_one_unit(tmp_path: Path) -> None:
    # 49 shells change in tile A; a 50th moves from tile B to tile D. The
    # move's two halves would pass the cap, so both go to the next set.
    shells = [swiss_shell(100 + k, A, lo=(k * 3_000, GROUND_MM, 0), hi=(k * 3_000 + 2_000, GROUND_MM + 5_000, 2_000))
              for k in range(49)]
    mover = swiss_shell(200, B, lo=(50_000, GROUND_MM, 190_000), hi=(60_000, GROUND_MM + 5_000, 199_000))
    world = World.of(terrain(A), terrain(B), terrain(D), mover, *shells)
    sc = Scenario.start(tmp_path, world)
    new = World.of(terrain(A), terrain(B), terrain(D),
                   *[moved_box(s.but(sources=swiss_shell(100 + k, year="2026").sources), dy=100)
                     for k, s in enumerate(shells)],
                   moved_box(mover.but(sources=swiss_shell(200, year="2026").sources), dz=2_000, tile=D))
    plan = sc.plan(new)
    assert [len(s.entries) for s in plan.sets] == [49, 2]
    assert [(e["op"], e["tile"]) for e in plan.sets[1].entries] == [("remove", B), ("add", D)]
    (unit,) = plan.report["units"]
    assert unit["status"] == "emitted" and unit["set"] == plan.sets[1].path
    assert [(m["op"], m["tile"]) for m in unit["members"]] == [("remove", B), ("add", D)]
    assert plan.sets[1].entries[1]["parent_version_id"] is None, "lineage starts again"


def overlap_world() -> World:
    return World.of(terrain(A), terrain(B), *[swiss_shell(n, A, lo=(n * 1_000, GROUND_MM, 150_000),
                                                         hi=(n * 1_000 + 500, GROUND_MM + 3_000, 150_500))
                                              for n in range(60, 70)])


def box(n: int, x0: int, x1: int, z0: int = 0, z1: int = 30_000, tile: str = A, year: str = "2025"):
    return swiss_shell(n, tile, lo=(x0, GROUND_MM, z0), hi=(x1, GROUND_MM + 6_000, z1), year=year)


def unit_members(plan) -> list[list[str]]:
    return [sorted(m["feature_id"][-3:] for m in u["members"]) for u in plan.report["units"]]


def test_split_one_to_three(tmp_path: Path) -> None:
    whole = box(10, 0, 30_000)
    world = overlap_world().put(whole)
    sc = Scenario.start(tmp_path, world)
    new = overlap_world().put(box(11, 0, 10_000), box(12, 10_000, 20_000), box(13, 20_000, 30_000))
    plan = sc.plan(new)
    assert unit_members(plan) == [["010", "011", "012", "013"]]
    assert len(plan.sets) == 1 and len(plan.sets[0].entries) == 4
    # A hold on one member holds the whole unit.
    plan = sc.plan(new, overrides={"hold": [{"layer": "shell", "feature_id": box(12, 0, 1).feature_id,
                                             "reason": "Check the split first."}]})
    assert plan.sets == []
    assert held(plan) == {
        whole.key: ["unit-held"], box(11, 0, 1).key: ["unit-held"], box(12, 0, 1).key: ["manual-hold"],
        box(13, 0, 1).key: ["unit-held"],
    }


def test_merge_three_to_one(tmp_path: Path) -> None:
    world = overlap_world().put(box(21, 0, 10_000), box(22, 10_000, 20_000), box(23, 20_000, 30_000))
    sc = Scenario.start(tmp_path, world)
    plan = sc.plan(overlap_world().put(box(24, 0, 30_000)))
    assert unit_members(plan) == [["021", "022", "023", "024"]]
    assert held(plan) == {}, "paired removals do not count as mass removals"


def test_chain_a_b_c(tmp_path: Path) -> None:
    # Removal 31 overlaps add 41, which overlaps removal 32, which overlaps add 42.
    world = overlap_world().put(box(31, 0, 20_000, 0, 10_000), box(32, 20_000, 40_000, 0, 10_000))
    sc = Scenario.start(tmp_path, world)
    new = overlap_world().put(box(41, 10_000, 30_000, 0, 10_000), box(42, 30_000, 50_000, 0, 10_000))
    plan = sc.plan(new)
    assert unit_members(plan) == [["031", "032", "041", "042"]]
    plan = sc.plan(new, overrides={"hold": [{"layer": "shell", "feature_id": box(42, 0, 1).feature_id,
                                             "reason": "Held for the test."}]})
    assert sorted(held(plan)) == sorted([box(n, 0, 1).key for n in (31, 32, 41, 42)])


def test_move_add_overlaps_removal(tmp_path: Path) -> None:
    mover = box(50, 190_000, 199_000, 0, 9_000)
    gone = box(51, 0, 10_000, 0, 10_000, tile=B)
    world = overlap_world().put(mover, gone)
    sc = Scenario.start(tmp_path, world)
    moved = moved_box(mover.but(sources=box(50, 0, 1, year="2026").sources), dx=10_000, tile=B)
    plan = sc.plan(overlap_world().put(moved))
    assert unit_members(plan) == [["050", "050", "051"]]
    assert [(e["op"], e["tile"], e["feature_id"][-3:]) for e in plan.sets[0].entries] == [
        ("remove", A, "050"), ("add", B, "050"), ("remove", B, "051")]
    plan = sc.plan(overlap_world().put(moved), overrides={"hold": [{"layer": "shell", "feature_id": gone.feature_id,
                                                                     "reason": "Held for the test."}]})
    assert held(plan) == {gone.key: ["manual-hold"], mover.key: ["unit-held"]}


def test_zero_area_box_links_nothing(tmp_path: Path) -> None:
    wall = swiss_shell(70, A, lo=(5_000, GROUND_MM, 0), hi=(5_000, GROUND_MM + 3_000, 10_000))
    world = overlap_world().put(wall)
    sc = Scenario.start(tmp_path, world)
    plan = sc.plan(overlap_world().put(box(71, 0, 10_000, 0, 10_000)))
    assert plan.report["units"] == [], "a box with no area overlaps nothing"
    assert sorted(op for _, op, _, _ in emitted(plan)) == ["add", "remove"]
    flat = Box((0, 0, 0), (0, 5, 10))
    assert overlap_area(flat, Box((-5, 0, 0), (5, 5, 10))) == 0 and not overlaps(flat, flat)


def test_replace_growing_into_removal_is_linked(tmp_path: Path) -> None:
    grower, neighbour = box(80, 0, 10_000, 0, 10_000), box(81, 10_000, 20_000, 0, 10_000)
    sc = Scenario.start(tmp_path, overlap_world().put(grower, neighbour))
    grown = box(80, 0, 20_000, 0, 10_000, year="2026")
    plan = sc.plan(overlap_world().put(grown))
    assert unit_members(plan) == [["080", "081"]]
    assert [(e["op"], e["feature_id"][-3:]) for e in plan.sets[0].entries] == [("replace", "080"), ("remove", "081")]


def test_unit_over_50_held(tmp_path: Path) -> None:
    big = box(300, 0, 102_000, 0, 10_000)
    sc = Scenario.start(tmp_path, overlap_world().put(big))
    parts = [box(301 + k, k * 2_000, k * 2_000 + 2_000, 0, 10_000) for k in range(51)]
    plan = sc.plan(overlap_world().put(*parts))
    assert plan.sets == []
    found = held(plan)
    assert len(found) == 52 and all(codes == ["unit-too-large"] for codes in found.values())
    assert plan.report["units"][0]["status"] == "held" and len(plan.report["units"][0]["members"]) == 52


def test_per_tile_assets_are_units_of_their_own(tmp_path: Path) -> None:
    # A per-tile ribbon and a per-feature way without a collider, in the same
    # layer and overlapping: never linked.
    footway = deck(90, A, lo=(0, GROUND_MM, 90_000), hi=(50_000, GROUND_MM + 100, 110_000)).but(collider="none")
    world = district().put(footway)
    sc = Scenario.start(tmp_path, world)
    wider = ribbon(A, lo=(0, GROUND_MM, 80_000), hi=(200_000, GROUND_MM + 100, 120_000), version=OSM_V2)
    plan = sc.plan(district().put(wider))
    assert plan.report["units"] == []
    assert sorted(op for _, op, _, _ in emitted(plan)) == ["remove", "replace"]


def test_overlap_counts_half_the_smaller_box() -> None:
    a = Box((0, 0, 0), (10, 1, 10))
    assert overlaps(a, Box((5, 0, 0), (15, 1, 10)))          # 50 of 100: exactly half
    assert not overlaps(a, Box((6, 0, 0), (16, 1, 10)))      # 40 of 100
    assert overlaps(a, Box((9, 0, 9), (10, 5, 10)))          # all of a 1 x 1 box
    assert touches(a, Box((9, 0, 9), (30, 5, 30))) and not overlaps(a, Box((9, 0, 9), (30, 5, 30)))
    assert not touches(a, Box((10, 0, 0), (20, 1, 10))), "boxes that only share an edge do not overlap"


def test_content_key_is_rule_c() -> None:
    m = swiss_shell(1).manifest()
    versions = cjson.loads(cjson.dumps(m))
    versions["parent_version_id"] = content_id(b"x")
    versions["provenance"]["generator"]["version"] = "9.9.9"
    versions["provenance"]["sources"][0]["version"] = "2099"
    assert content_key(versions) == content_key(m)
    for edit in (
        lambda d: d["provenance"]["sources"][0].update(source_ids=["ID_OTHER"]),
        lambda d: d["provenance"].update(license="CC0-1.0"),
        lambda d: d.update(fictional=True),
        lambda d: d["physics"].update(collider="none"),
        lambda d: d.update(tile=B),
        lambda d: d["provenance"]["generator"].update(name="other"),
    ):
        other = cjson.loads(cjson.dumps(m))
        edit(other)
        assert content_key(other) != content_key(m)


def test_large_boxes_are_compared_with_everything() -> None:
    from tools.ingest.changes.common import CELL_MM, BoxGrid

    grid = BoxGrid()
    huge = Box((-50 * CELL_MM, 0, -50 * CELL_MM), (50 * CELL_MM, 1, 50 * CELL_MM))
    small = Box((10, 0, 10), (20, 1, 20))
    far = Box((40 * CELL_MM, 0, 40 * CELL_MM), (40 * CELL_MM + 5, 1, 40 * CELL_MM + 5))
    grid.add(huge, "huge")
    grid.add(small, "small")
    grid.add(far, "far")
    assert [v for _, v in grid.near(Box((0, 0, 0), (30, 1, 30)))] == ["huge", "small"]
    assert [v for _, v in grid.near(huge)] == ["huge", "small", "far"]
    assert grid.near(Box((0, 0, 0), (0, 1, 30))) == []


def test_content_keeps_its_extension(tmp_path: Path) -> None:
    # RFC 0004's point documents are `.json` content: the bundle copies the
    # content under the extension the new store gives it, never assuming glb.
    sc = Scenario.start(tmp_path, district())
    added = swiss_shell(9, lo=(150_000, GROUND_MM, 20_000), hi=(160_000, GROUND_MM + 8_000, 30_000))
    root = sc.write_new(district().put(added))
    glb_path = Store(root).object_path(added.content_id, "glb")
    json_path = Store(root).object_path(added.content_id, "json")
    glb_path.rename(json_path)
    plan = sc.plan(root)
    (s,) = plan.sets
    content = f"{s.path}/objects/{added.content_id[7:9]}/{added.content_id[7:]}"
    assert plan.files[content + ".json"] == json_path.read_bytes()
    assert content + ".glb" not in plan.files
