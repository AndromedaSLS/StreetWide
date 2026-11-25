"""The refresh planner's hold rules H7 to H14 and their fixed point
(ADR-0020, section 6), with the overrides that lift them."""

from __future__ import annotations

from pathlib import Path

import pytest

from tools.ingest.changes.common import PlanError
from tools.ingest.tests.refresh_worlds import (
    A, B, C, D, DEM_2027, E, GROUND_MM, OSM_V2, SPAWN, Scenario, World, box_mesh, contributed, deck, entries,
    entry_add, entry_remove, entry_replace, facade, flat, held, moved_box, ops, osm_shell, ribbon, swiss_shell,
    terrain, water,
)


def shells(first: int, count: int, tile: str = A, *, year: str = "2025", z: int = 150_000) -> list:
    return [swiss_shell(n, tile, lo=((n - first) * 3_000, GROUND_MM, z),
                        hi=((n - first) * 3_000 + 2_000, GROUND_MM + 5_000, z + 2_000), year=year)
            for n in range(first, first + count)]


def draped(item=None) -> list:
    """The assets of tile A that list the height model, on `item`."""
    kw = {} if item is None else {"item": item}
    return [ribbon(A, **kw), deck(7, **kw), osm_shell(5, **kw), water(A, **kw)]


def world() -> World:
    return World.of(terrain(A), terrain(B), *draped(), *shells(100, 4), swiss_shell(9, B))


def edges_moved(heights=None):
    h = [row[:] for row in (heights or flat())]
    h[0][2] += 700  # a vertex on the tile's open edge
    return h


def interior_moved(heights=None, by: int = 500):
    h = [row[:] for row in (heights or flat())]
    h[1][1] += by
    return h


def test_tile_not_in_world_held(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, world())
    outside = swiss_shell(50, E, lo=(10_000, GROUND_MM, 10_000), hi=(20_000, GROUND_MM + 8_000, 20_000))
    plan = sc.plan(world().put(outside))
    assert held(plan) == {outside.key: ["tile-not-in-world"]}
    assert plan.report["world_level"]["tiles_only_new"] == [E]
    with pytest.raises(PlanError, match=r"cannot lift tile-not-in-world .*: the gate would fail it \(G2\)"):
        sc.plan(world(), overrides={"accept": [{"layer": "shell", "feature_id": outside.feature_id,
                                                "current_manifest_id": None, "content_key": None,
                                                "lifts": ["tile-not-in-world"], "reason": "x"}]})


def test_would_empty_tile_held(tmp_path: Path) -> None:
    # Tile C: shell 1 is gone and shell 2 moves to tile D, whose only asset,
    # shell 3, is gone. Holding C's removals holds the move, and then D's
    # removal: the loop goes on until nothing changes.
    s1 = swiss_shell(1, C, lo=(10_000, GROUND_MM, 10_000), hi=(20_000, GROUND_MM + 5_000, 20_000))
    s2 = swiss_shell(2, C, lo=(100_000, GROUND_MM, 190_000), hi=(110_000, GROUND_MM + 5_000, 199_000))
    s3 = swiss_shell(3, D, lo=(150_000, GROUND_MM, 150_000), hi=(160_000, GROUND_MM + 5_000, 160_000))
    base = world().put(s1, s2, s3)
    sc = Scenario.start(tmp_path, base)
    moved = moved_box(s2.but(sources=swiss_shell(2, year="2026").sources), dz=20_000, tile=D)
    plan = sc.plan(world().put(moved))
    assert held(plan) == {s1.key: ["would-empty-tile"], s2.key: ["would-empty-tile"], s3.key: ["would-empty-tile"]}
    assert "would leave tile 17/68646/45904 empty" in plan.report["held_back"][0]["reasons"][0]["details"]
    assert plan.sets == []


def test_a_tile_emptied_between_sets_is_held(tmp_path: Path) -> None:
    # Tile B holds only a fallback shell, which upstream drops, and a new
    # swissBUILDINGS3D shell apart from it comes into B. The OpenStreetMap
    # shells' set comes first and would leave B empty on its own, which the
    # gate fails (G1), although the next set fills it again.
    lone = osm_shell(5, B, lo=(10_000, GROUND_MM, 10_000), hi=(20_000, GROUND_MM + 9_000, 20_000))
    base = World.of(terrain(A), swiss_shell(1), osm_shell(4), lone)
    sc = Scenario.start(tmp_path, base)
    newcomer = swiss_shell(7, B, lo=(100_000, GROUND_MM, 100_000), hi=(110_000, GROUND_MM + 9_000, 110_000))
    plan = sc.plan(base.copy().drop(lone.key).put(newcomer))
    assert held(plan) == {lone.key: ["would-empty-tile"]} and ops(plan) == {newcomer.key: ["add"]}
    assert plan.report["held_back"][0]["reasons"][0]["details"] == (
        f"its change set would leave tile {B} empty, and the gate fails an empty tile (G1); the assets that come "
        "into the tile arrive only with a later set")
    # One purpose split by --max-set-bytes: the removal's set comes first.
    other = osm_shell(6, B, lo=(100_000, GROUND_MM, 100_000), hi=(110_000, GROUND_MM + 9_000, 110_000))
    plan = sc.plan(base.copy().drop(lone.key).put(other), max_set_bytes=1)
    assert held(plan) == {lone.key: ["would-empty-tile"]} and ops(plan) == {other.key: ["add"]}
    # Visual-only sets come first: water removed under an allowance, from a
    # tile that only gains a shell.
    sc = Scenario.start(tmp_path / "water", World.of(terrain(A), swiss_shell(1), water(A), water(B)))
    allowance = {"allow_removals": [{"layer": "water", "dataset": "OpenStreetMap", "at_most": 1,
                                     "reason": "The pond was filled in."}]}
    new = World.of(terrain(A), swiss_shell(1), water(A), newcomer)
    plan = sc.plan(new, overrides=allowance)
    assert held(plan) == {water(B).key: ["would-empty-tile"]} and ops(plan) == {newcomer.key: ["add"]}
    # Once that set is merged, the next refresh removes the water.
    sc.log.refresh(sc.current, [entry_add(newcomer)], [newcomer])
    plan = sc.plan(new, overrides=allowance)
    assert held(plan) == {} and ops(plan) == {water(B).key: ["remove"]}


def test_a_decision_set_that_refills_a_tile(tmp_path: Path) -> None:
    # A contribution removed shell 3 from tile B; upstream still has it, and
    # drops shell 2, B's only asset. The accepted add comes in a decision
    # set, after the ordinary set that removes shell 2.
    s2 = swiss_shell(2, B, lo=(10_000, GROUND_MM, 10_000), hi=(20_000, GROUND_MM + 9_000, 20_000))
    s3 = swiss_shell(3, B, lo=(100_000, GROUND_MM, 100_000), hi=(110_000, GROUND_MM + 9_000, 110_000))
    base = World.of(terrain(A), swiss_shell(1), s2, s3)
    sc = Scenario.start(tmp_path, base)
    sc.log.merge(sc.current, [entry_remove(s3)], author="github:bob")
    new = base.copy().drop(s2.key)
    plan = sc.plan(new)
    assert held(plan) == {s3.key: ["removed-by-contribution"], s2.key: ["would-empty-tile"]}
    accept = {"layer": "shell", "feature_id": s3.feature_id, "current_manifest_id": None,
              "content_key": plan.report["conflicts"][0]["new_content_key"], "lifts": ["removed-by-contribution"],
              "reason": "The building stands; the contribution removed it by mistake."}
    plan = sc.plan(new, overrides={"accept": [accept]})
    assert held(plan) == {s2.key: ["would-empty-tile"]}
    assert plan.report["held_back"][0]["reasons"][0]["details"].startswith(
        f"its change set would leave tile {B} empty")
    assert [(s.kind, [e["op"] for e in s.entries]) for s in plan.sets] == [("decision", ["add"])]


def test_terrain_edge_change_holds_its_height_component(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, world())
    new = world().put(terrain(A, edges_moved(), item=DEM_2027), terrain(B, interior_moved(), item=DEM_2027),
                      *draped(DEM_2027))
    plan = sc.plan(new)
    found = held(plan)
    assert found.pop(("terrain", f"tile:{A}")) == ["terrain-edges"]
    assert found == {key: ["height-coupled"] for key in [("terrain", f"tile:{B}")] + [a.key for a in draped()]}
    (component,) = plan.report["height_components"]
    assert component["status"] == "held" and component["terrain_changes"] == 2 and component["changes"] == 6
    details = {(e["layer"], e["feature_id"]): e["reasons"][0]["details"] for e in plan.report["held_back"]}
    assert details[("terrain", f"tile:{A}")].startswith("1 vertex on the open edges of the current terrain differs")
    assert plan.sets == []
    with pytest.raises(PlanError, match=r"cannot lift terrain-edges for terrain tile:17/68646/45903: the gate would "
                                        r"fail it \(G8\)"):
        sc.plan(new, overrides={"accept": [{"layer": "terrain", "feature_id": f"tile:{A}", "current_manifest_id": None,
                                            "content_key": None, "lifts": ["terrain-edges"], "reason": "x"}]})


def test_deck_in_neighbour_tile_is_height_coupled(tmp_path: Path) -> None:
    neighbour = deck(8, B, lo=(10_000, GROUND_MM + 2_000, 10_000), hi=(30_000, GROUND_MM + 3_000, 14_000))
    sc = Scenario.start(tmp_path, world().put(neighbour))
    new = world().put(terrain(A, edges_moved(), item=DEM_2027), deck(8, B, lo=(10_000, GROUND_MM + 2_000, 10_000),
                                                                      hi=(30_000, GROUND_MM + 3_100, 14_000), item=DEM_2027))
    plan = sc.plan(new)
    assert held(plan) == {("terrain", f"tile:{A}"): ["terrain-edges"], neighbour.key: ["height-coupled"]}


def test_held_draped_asset_holds_terrain(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, world())
    on = facade(osm_shell(5))
    sc.log.merge(sc.current, [entry_add(on)], [on])
    redraped = moved_box(osm_shell(5, item=DEM_2027), dy=300)
    plan = sc.plan(world().put(redraped, terrain(A, interior_moved(), item=DEM_2027)))
    assert held(plan) == {osm_shell(5).key: ["dependent-contribution"], ("terrain", f"tile:{A}"): ["height-coupled"]}
    assert "osm:way:5, which is held (dependent-contribution)" in plan.report["held_back"][0]["reasons"][0]["details"]


def test_osm_only_refresh_is_not_height_coupled(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, world())
    wider = ribbon(A, lo=(0, GROUND_MM, 80_000), hi=(200_000, GROUND_MM + 100, 120_000), version="2026-12-01T00:00:00Z")
    plan = sc.plan(world().put(wider))
    assert ops(plan) == {wider.key: ["replace"]} and held(plan) == {}
    assert plan.report["height_components"] == []


def test_contributed_terrain_holds_draped_assets(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, world())
    mine = contributed(terrain(A), parent=terrain(A).manifest_id,
                       mesh=box_mesh(f"tile:{A}", (0, GROUND_MM - 1_000, 0), (200_000, GROUND_MM, 200_000), "terrain"))
    sc.log.merge(sc.current, [entry_replace(mine)], [mine])
    new = world().put(terrain(A, interior_moved(), item=DEM_2027), ribbon(A, item=DEM_2027))
    plan = sc.plan(new)
    found = held(plan)
    assert found[("terrain", f"tile:{A}")][0] == "contribution"
    assert found[ribbon(A).key] == ["height-coupled"]
    (e,) = [x for x in plan.report["held_back"] if x["layer"] == "transport"]
    plan = sc.plan(new, overrides={"accept": [{
        "layer": "transport", "feature_id": f"tile:{A}", "current_manifest_id": e["current_manifest_id"],
        "content_key": e["new_content_key"], "lifts": ["height-coupled"],
        "reason": "The ribbon follows the new height model; the contributed terrain is close to it."}]})
    assert ops(plan) == {ribbon(A).key: ["replace"]}
    assert plan.report["height_components"][0]["status"] == "partly held"


def test_contributed_terrain_that_stays_holds_what_is_draped_over_it(tmp_path: Path) -> None:
    # Upstream keeps the height model, so the contributed terrain has no
    # pending change at all. A new deck and an edited ribbon over it are
    # still draped on the height model, not on the contribution (Q17).
    sc = Scenario.start(tmp_path, world())
    mine = contributed(terrain(A), parent=terrain(A).manifest_id,
                       mesh=box_mesh(f"tile:{A}", (0, GROUND_MM - 1_000, 0), (200_000, GROUND_MM, 200_000), "terrain"))
    sc.log.merge(sc.current, [entry_replace(mine)], [mine])
    new_deck = deck(8, A, lo=(120_000, GROUND_MM + 2_000, 20_000), hi=(140_000, GROUND_MM + 3_000, 24_000))
    wider = ribbon(A, lo=(0, GROUND_MM, 80_000), hi=(200_000, GROUND_MM + 100, 120_000), version=OSM_V2)
    far = deck(9, B, lo=(10_000, GROUND_MM + 2_000, 30_000), hi=(30_000, GROUND_MM + 3_000, 34_000))
    new = world().put(new_deck, wider, far)
    plan = sc.plan(new)
    assert held(plan) == {new_deck.key: ["height-coupled"], wider.key: ["height-coupled"]}
    assert ops(plan) == {far.key: ["add"]}, "a deck in another tile is not over it"
    details = {(e["layer"], e["feature_id"]): e["reasons"][0]["details"] for e in plan.report["held_back"]}
    assert details[new_deck.key] == (f"it is draped on the height model over the terrain tile:{A}, a contribution by "
                                     "github:alice that stays, so it would float or sink on it (Q17)")
    (listed,) = plan.report["contributions"]
    assert (listed["layer"], listed["upstream"], listed["would_be"]) == ("terrain", "unchanged", None)
    (e,) = [x for x in plan.report["held_back"] if x["feature_id"] == new_deck.feature_id]
    plan = sc.plan(new, overrides={"accept": [{
        "layer": "transport", "feature_id": new_deck.feature_id, "current_manifest_id": None,
        "content_key": e["new_content_key"], "lifts": ["height-coupled"],
        "reason": "The contributed terrain lies within 1 m of the height model under the deck."}]})
    assert ops(plan) == {far.key: ["add"], new_deck.key: ["add"]} and held(plan) == {wider.key: ["height-coupled"]}


def test_terrain_interior_change_emitted(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, world())
    plan = sc.plan(world().put(terrain(A, interior_moved(), item=DEM_2027)))
    assert ops(plan) == {("terrain", f"tile:{A}"): ["replace"]} and held(plan) == {}
    (s,) = plan.sets
    assert any(path.endswith(".glb") for path in s.objects), "the new terrain mesh is shipped"


def test_terrain_removal_held(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, world())
    plan = sc.plan(world().drop(("terrain", f"tile:{B}")))
    assert held(plan) == {("terrain", f"tile:{B}"): ["per-tile-removal"]}
    assert plan.report["held_back"][0]["reasons"][0]["details"] == f"it removes the terrain of tile {B}"


def test_all_water_gone_is_an_error(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, world().put(water(B)))
    with pytest.raises(PlanError) as e:
        sc.plan(world().drop(water(A).key))
    assert str(e.value) == ("the new store has no water from OpenStreetMap, but the current store has 2 such assets "
                            "made by the ingest; if upstream really lost them all, add an allow_removals entry to "
                            "the overrides file")


def test_all_fallback_shells_gone_held(tmp_path: Path) -> None:
    fallbacks = [osm_shell(w, A, lo=(w * 1_000, GROUND_MM, 60_000), hi=(w * 1_000 + 500, GROUND_MM + 3_000, 60_500))
                 for w in (11, 12, 13)]
    sc = Scenario.start(tmp_path, world().drop(osm_shell(5).key).put(*fallbacks))
    # Every current fallback shell is gone, and another one appears.
    other = osm_shell(14, B, lo=(80_000, GROUND_MM, 80_000), hi=(90_000, GROUND_MM + 3_000, 90_000))
    plan = sc.plan(world().drop(osm_shell(5).key).put(other))
    assert held(plan) == {f.key: ["mass-removal"] for f in fallbacks}
    assert "3 unpaired removals in shell/OpenStreetMap, more than max(2, ceil(5 % of 3)) = 2" in \
        plan.report["held_back"][0]["reasons"][0]["details"]
    assert ops(plan) == {other.key: ["add"]}
    # None left at all: the group vanished, which stops the plan.
    with pytest.raises(PlanError, match="the new store has no shell from OpenStreetMap, but the current store has 3 "):
        sc.plan(world().drop(osm_shell(5).key))


def test_mass_removal_per_group(tmp_path: Path) -> None:
    group = shells(200, 60, z=50_000)  # with the 5 others, max(2, ceil(5 % of 65)) = 4
    sc = Scenario.start(tmp_path, world().put(*group))
    plan = sc.plan(world().put(*group[4:]))
    assert sorted(ops(plan).values()) == [["remove"]] * 4 and held(plan) == {}
    plan = sc.plan(world().put(*group[5:]))
    assert held(plan) == {a.key: ["mass-removal"] for a in group[:5]}
    details = plan.report["held_back"][0]["reasons"][0]["details"]
    assert details == "5 unpaired removals in shell/swissBUILDINGS3D 3.0 Beta, more than max(2, ceil(5 % of 65)) = 4"
    # Renames do not count: each removal overlaps the add that replaces it.
    renamed = [swiss_shell(500 + k, A, lo=(k * 3_000, GROUND_MM, 50_000),
                           hi=(k * 3_000 + 2_000, GROUND_MM + 5_000, 52_000)) for k in range(5)]
    plan = sc.plan(world().put(*group[5:], *renamed))
    assert held(plan) == {} and len(plan.report["units"]) == 5
    # One new building over ten removed ones accounts for one of them: the
    # other nine count.
    block = swiss_shell(600, A, lo=(0, GROUND_MM, 49_000), hi=(30_000, GROUND_MM + 5_000, 53_000))
    plan = sc.plan(world().put(*group[10:], block))
    found = held(plan)
    assert sorted(found.values()) == [["mass-removal", "unit-held"]] * 9 + [["unit-held"]] * 2 and block.key in found
    assert {x["details"] for e in plan.report["held_back"] for x in e["reasons"] if x["code"] == "mass-removal"} == {
        "9 unpaired removals in shell/swissBUILDINGS3D 3.0 Beta, more than max(2, ceil(5 % of 65)) = 4"}
    allowance = {"layer": "shell", "dataset": "swissBUILDINGS3D 3.0 Beta", "at_most": 9, "reason": "A new block."}
    plan = sc.plan(world().put(*group[10:], block), overrides={"allow_removals": [allowance]})
    assert held(plan) == {} and len(plan.sets[0].entries) == 11


def test_allowance_lifts_removal_guards(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, world().put(water(B), water(C), terrain(C)))
    allowance = {"layer": "water", "dataset": "OpenStreetMap", "reason": "The river was re-mapped as an area."}
    new = world().drop(water(A).key).put(terrain(C), water(C))
    plan = sc.plan(new, overrides={"allow_removals": [dict(allowance, at_most=2)]})
    assert sorted(ops(plan).values()) == [["remove"], ["remove"]] and held(plan) == {}
    assert plan.report["allowances_used"] == [dict(allowance, at_most=2, removals=2)]
    plan = sc.plan(new, overrides={"allow_removals": [dict(allowance, at_most=1)]})
    assert held(plan) == {water(A).key: ["per-tile-removal"], water(B).key: ["per-tile-removal"]}
    assert plan.report["stale_overrides"] == [{"entry": "allow_removals", "layer": "water", "feature_id": None,
                                               "dataset": "OpenStreetMap",
                                               "why": "2 unpaired removals, more than at_most 1"}]
    # A whole group that vanished: the allowance clears the error within at_most.
    gone = world().drop(water(A).key).put(terrain(C))
    with pytest.raises(PlanError, match=r"allows at most 2, but 3 would be removed"):
        sc.plan(gone, overrides={"allow_removals": [dict(allowance, at_most=2)]})
    plan = sc.plan(gone, overrides={"allow_removals": [dict(allowance, at_most=3)]})
    assert len(plan.sets) == 1 and [e["op"] for e in plan.sets[0].entries] == ["remove"] * 3


def test_spawn_ground_held(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, world())
    raised = flat()
    raised[2][2] = SPAWN[1] - 50  # the grid vertex right under the spawn
    plan = sc.plan(world().put(terrain(A, raised, item=DEM_2027)))
    assert held(plan) == {("terrain", f"tile:{A}"): ["spawn"]}
    assert "50 mm below the spawn's feet" in plan.report["held_back"][0]["reasons"][0]["details"]
    assert plan.report["world_level"]["spawn_ground_gap_mm"] == {"current": 1_000, "new": 50}
    raised[2][2] = SPAWN[1] - 100  # exactly 100 mm clear
    assert held(sc.plan(world().put(terrain(A, raised, item=DEM_2027)))) == {}
    with pytest.raises(PlanError, match=r"cannot lift spawn for terrain .*: change sets cannot move the spawn"):
        sc.plan(world(), overrides={"accept": [{"layer": "terrain", "feature_id": f"tile:{A}",
                                                "current_manifest_id": None, "content_key": None, "lifts": ["spawn"],
                                                "reason": "x"}]})


def test_new_collider_over_spawn_held(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, world())
    x, y, z = SPAWN
    over = swiss_shell(70, A, lo=(x - 5_000, GROUND_MM, z - 5_000), hi=(x + 5_000, GROUND_MM + 9_000, z + 5_000))
    plan = sc.plan(world().put(over))
    assert held(plan) == {over.key: ["spawn"]}
    assert plan.report["held_back"][0]["reasons"][0]["details"] == (
        f"its new collider reaches into the player's space at the spawn {list(SPAWN)} (x and z within 300 mm, "
        "from 100 mm below the feet to 1800 mm above them)")
    # Visual assets do not collide.
    plan = sc.plan(world().put(ribbon(A, lo=(0, GROUND_MM, 90_000), hi=(200_000, GROUND_MM + 2_000, 110_000),
                                      version="2026-12-01T00:00:00Z")))
    assert held(plan) == {}


def test_a_collider_in_the_players_space_at_the_spawn_held(tmp_path: Path) -> None:
    x, y, z = SPAWN
    # A canopy from just above the feet to chest height: the spawn point
    # itself is free, the player's capsule is not.
    canopy = swiss_shell(71, A, lo=(x - 3_000, y + 10, z - 3_000), hi=(x + 3_000, y + 1_500, z + 3_000))
    # A platform whose top stays 200 mm under the feet, and a wall 400 mm
    # beside them: neither reaches the player.
    platform = swiss_shell(72, A, lo=(x - 3_000, y - 1_000, z - 3_000), hi=(x + 3_000, y - 200, z + 3_000))
    wall = swiss_shell(73, A, lo=(x + 400, GROUND_MM, z - 3_000), hi=(x + 900, GROUND_MM + 9_000, z + 3_000))
    sc = Scenario.start(tmp_path, world())
    plan = sc.plan(world().put(canopy, platform, wall))
    assert held(plan) == {canopy.key: ["spawn"]} and ops(plan) == {platform.key: ["add"], wall.key: ["add"]}

    # A visual-only box around the spawn whose new version has a collider:
    # the mesh stays, so only the manifest is replaced.
    visual = swiss_shell(74, A, lo=(x - 5_000, GROUND_MM, z - 5_000), hi=(x + 5_000, GROUND_MM + 5_000, z + 5_000))
    sc = Scenario.start(tmp_path / "switched", world().put(visual.but(collider="none")))
    switched = swiss_shell(74, A, lo=(x - 5_000, GROUND_MM, z - 5_000), hi=(x + 5_000, GROUND_MM + 5_000, z + 5_000),
                           year="2026")
    plan = sc.plan(world().put(switched))
    assert held(plan) == {switched.key: ["spawn"]}
    assert plan.report["held_back"][0]["reasons"][0]["details"].endswith(", and its current version has no collider")
    # A collider that was there already may change.
    sc = Scenario.start(tmp_path / "kept", world().put(visual))
    plan = sc.plan(world().put(moved_box(switched, dy=300)))
    assert held(plan) == {} and ops(plan) == {visual.key: ["replace"]}


def test_unexplained_change_held(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, world())
    same_sources = moved_box(swiss_shell(9, B), dy=200)
    plan = sc.plan(world().put(same_sources))
    (e,) = plan.report["held_back"]
    assert [r["code"] for r in e["reasons"]] == ["unexplained-change"]
    assert "an item republished in place under the same id" in e["reasons"][0]["details"]
    plan = sc.plan(world().put(same_sources), overrides={"accept": [{
        "layer": "shell", "feature_id": same_sources.feature_id, "current_manifest_id": e["current_manifest_id"],
        "content_key": e["new_content_key"], "lifts": ["unexplained-change"],
        "reason": "swisstopo republished the sheet in place (checked)."}]})
    assert ops(plan) == {same_sources.key: ["replace"]}
    assert plan.report["accepted"][0]["set"] == "sets/001"


def test_manual_hold_lasts_until_upstream_changes(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, world())
    v2026 = moved_box(swiss_shell(9, B, year="2026"), dy=200)
    plan = sc.plan(world().put(v2026))
    (e,) = [x for x in plan.report["sets"][0:1]]
    key_scope = plan.sets[0].items[0].new_content_key
    hold = {"layer": "shell", "feature_id": v2026.feature_id, "content_key": key_scope,
            "reason": "Reviewed: wait for the next release."}
    plan = sc.plan(world().put(v2026), overrides={"hold": [hold]})
    assert held(plan) == {v2026.key: ["manual-hold"]} and e["changes"] == 1
    v2027 = moved_box(swiss_shell(9, B, year="2027"), dy=400)
    plan = sc.plan(world().put(v2027), overrides={"hold": [hold]})
    assert ops(plan) == {v2027.key: ["replace"]}
    assert plan.report["stale_overrides"][0]["why"] == "content_key no longer matches"
    # An unscoped hold holds whatever the version.
    plan = sc.plan(world().put(v2027), overrides={"hold": [dict(hold, content_key=None)]})
    assert held(plan) == {v2027.key: ["manual-hold"]}
    assert entries(plan, "held_back")[v2027.key] == ["manual-hold"]
