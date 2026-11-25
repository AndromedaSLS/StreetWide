"""The refresh planner's hold rules H7 to H14 and their fixed point
(ADR-0020, section 6), with the overrides that lift them."""

from __future__ import annotations

from pathlib import Path

import pytest

from tools.ingest.changes.common import PlanError
from tools.ingest.changes.holds import encloses, triangle_meets_box
from tools.ingest.tests.refresh_worlds import (
    A, B, C, D, DEM_2027, E, GROUND_MM, OSM_V2, SPAWN, Asset, Scenario, World, box_mesh, box_part, contributed, deck,
    entries, entry_add, entry_remove, entry_replace, facade, flat, grid_part, held, moved_box, ops, osm_shell,
    parts_mesh, ribbon, swiss_shell, terrain, water,
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


def test_only_the_first_set_that_empties_a_tile_is_held_at_a_time(tmp_path: Path) -> None:
    # One purpose, tiles in byte order A < C < B < D. C's only shell X and
    # D's only shell Z are gone upstream; Y and W come into C and D apart
    # from them, and 49 and 48 new shells into A and B. Packed at 50
    # changes, set 1 empties C (X) and set 2 empties D (Z). Holding both at
    # once held Z for nothing: without X, set 2 takes W along with Z, and D
    # is never empty. Found by review: Z was held, with details saying W
    # came only with a later set, although W was in Z's set.
    def shell(n: int, tile: str, x: int, z: int):
        return swiss_shell(n, tile, lo=(x, GROUND_MM, z), hi=(x + 2_000, GROUND_MM + 5_000, z + 2_000))

    x, y, z, w = shell(10, C, 10_000, 10_000), shell(11, C, 100_000, 100_000), \
        shell(20, D, 10_000, 10_000), shell(21, D, 100_000, 100_000)
    kept = shell(30, B, 150_000, 150_000)
    sc = Scenario.start(tmp_path, World.of(terrain(A), x, kept, z))
    new = World.of(terrain(A), kept, y, w, *[shell(1000 + k, A, 3_000 * k, 50_000) for k in range(49)],
                   *[shell(2000 + k, B, 3_000 * k, 50_000) for k in range(48)])
    plan = sc.plan(new)
    assert held(plan) == {x.key: ["would-empty-tile"]}
    assert [len(s.entries) for s in plan.sets] == [50, 50]
    (last,) = [s for s in plan.sets if any(e["feature_id"] == z.feature_id for e in s.entries)]
    assert {(e["op"], e["feature_id"]) for e in last.entries if e["tile"] == D} == {
        ("remove", z.feature_id), ("add", w.feature_id)}


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


def contributed_terrain_scenario(tmp_path: Path) -> Scenario:
    """Terrain A, B and a ribbon over A, with terrain A replaced by
    github:alice in a logged merge."""
    sc = Scenario.start(tmp_path, World.of(terrain(A), terrain(B), ribbon(A), swiss_shell(1), swiss_shell(2, B)))
    mine = contributed(terrain(A), parent=terrain(A).manifest_id,
                       mesh=box_mesh(f"tile:{A}", (0, GROUND_MM - 1_000, 0), (200_000, GROUND_MM, 200_000), "terrain"))
    sc.log.merge(sc.current, [entry_replace(mine)], [mine])
    return sc


def test_what_is_draped_over_a_contributed_terrain_waits_for_its_decision(tmp_path: Path) -> None:
    # The maintainer accepts the new height model over alice's terrain: its
    # change goes into a decision set, which merges after every other set
    # and only if its review judges it better. The ribbon draped over it
    # went into an ordinary set, merged first, so a decision judged not
    # better left the ribbon on the new height model over a contribution
    # that stays, and later plans could not see it. It now waits for the
    # plan made after the decision; it does not hold the decision in turn.
    # Found by review.
    sc = contributed_terrain_scenario(tmp_path)
    redraped = moved_box(ribbon(A, item=DEM_2027), dy=300)  # draped on the 2027 heights: a new mesh
    new = World.of(terrain(A, interior_moved(), item=DEM_2027), terrain(B, item=DEM_2027), redraped,
                   swiss_shell(1), swiss_shell(2, B))
    plan = sc.plan(new)
    # The ribbon is held because of terrain A, so it does not hold terrain A
    # in turn: only its own conflict does.
    assert held(plan) == {("terrain", f"tile:{A}"): ["contribution"],
                          ("terrain", f"tile:{B}"): ["height-coupled"], ribbon(A).key: ["height-coupled"]}
    (t,) = [e for e in plan.report["conflicts"] if e["layer"] == "terrain"]
    accept = {"accept": [{"layer": "terrain", "feature_id": f"tile:{A}", "current_manifest_id": t["current_manifest_id"],
                          "content_key": t["new_content_key"], "lifts": ["contribution"],
                          "reason": "The 2027 height model beats the contribution."}]}
    plan = sc.plan(new, overrides=accept)
    assert ops(plan) == {("terrain", f"tile:{A}"): ["replace"]}
    assert [s.kind for s in plan.sets] == ["decision"]
    assert held(plan) == {("terrain", f"tile:{B}"): ["height-coupled"], ribbon(A).key: ["height-coupled"]}
    details = {(e["layer"], e["feature_id"]): e["reasons"][0]["details"] for e in plan.report["held_back"]}
    assert details[ribbon(A).key] == (
        f"it is draped on the height model over the terrain tile:{A}, a contribution by github:alice that a decision "
        "set of this plan replaces only if its review judges it better; plan again once that set is merged (Q17)")


def test_a_manifest_only_change_over_a_contributed_terrain_is_not_held(tmp_path: Path) -> None:
    # Only the ribbon's OpenStreetMap way ids changed, not its mesh: it
    # cannot come to float or sink on the contribution, which the reason
    # given said it would (Q17). Found by review.
    sc = contributed_terrain_scenario(tmp_path)
    relabelled = ribbon(A, ways=("w900", "w901"))
    assert relabelled.content_id == ribbon(A).content_id
    plan = sc.plan(World.of(terrain(A), terrain(B), relabelled, swiss_shell(1), swiss_shell(2, B)))
    assert held(plan) == {} and ops(plan) == {relabelled.key: ["replace"]}


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


def test_mass_removal_counts_per_tile_removals(tmp_path: Path) -> None:
    # transport/OpenStreetMap mixes ribbons (per tile) and decks (per
    # feature): 4 and 6, so max(2, ceil(5 % of 10)) = 2. Upstream loses a
    # ribbon and two decks. H10b counted the decks alone, let both through
    # and held only the ribbon (per-tile-removal), while ADR-0020 (Q4) and
    # the allowances count all three. Found by review.
    decks = [deck(10 + k, A, lo=(10_000 + 30_000 * k, GROUND_MM + 2_000, 60_000),
                  hi=(30_000 + 30_000 * k, GROUND_MM + 3_000, 64_000)) for k in range(6)]
    tiles = (A, B, C, D)
    base = World.of(*[terrain(t) for t in tiles], *[ribbon(t) for t in tiles], *decks)
    sc = Scenario.start(tmp_path, base)
    new = World.of(*[terrain(t) for t in tiles], *[ribbon(t) for t in tiles[1:]], *decks[2:])
    plan = sc.plan(new)
    assert held(plan) == {ribbon(A).key: ["per-tile-removal", "mass-removal"], decks[0].key: ["mass-removal"],
                          decks[1].key: ["mass-removal"]}
    assert {x["details"] for e in plan.report["held_back"] for x in e["reasons"] if x["code"] == "mass-removal"} == {
        "3 unpaired removals in transport/OpenStreetMap, more than max(2, ceil(5 % of 10)) = 2"}
    allowance = {"layer": "transport", "dataset": "OpenStreetMap", "reason": "The bridge and its ramp were removed."}
    plan = sc.plan(new, overrides={"allow_removals": [dict(allowance, at_most=3)]})
    assert held(plan) == {} and sorted(ops(plan).values()) == [["remove"]] * 3


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


def l_shell(year: str) -> Asset:
    """A shell in two wings around the spawn's corner: its bounds cover the
    spawn, but its walls stay 200 mm or more from the player's space."""
    x, _, z = SPAWN
    s = swiss_shell(70, A, year=year)
    return s.but(mesh=parts_mesh(s.feature_id, "wall", [
        box_part((x - 5_000, GROUND_MM, z - 5_000), (x - 500, GROUND_MM + 9_000, z + 5_000)),
        box_part((x - 5_000, GROUND_MM, z + 1_000), (x + 5_000, GROUND_MM + 9_000, z + 5_000)),
    ]))


def test_a_collider_is_judged_by_its_mesh_not_its_bounds(tmp_path: Path) -> None:
    # H14 compared bounding boxes. A building whose wings stand around the
    # spawn (its bounds cover the spawn, its walls leave the player free)
    # counted as a collider already there, so a new version that fills the
    # spawn was emitted; and a new building around a courtyard holding the
    # spawn was held, by a code nothing lifts. Found by review.
    x, _, z = SPAWN
    base = World.of(terrain(A), swiss_shell(1), l_shell("2025"))
    sc = Scenario.start(tmp_path, base)
    full = swiss_shell(70, A, lo=(x - 5_000, GROUND_MM, z - 5_000), hi=(x + 5_000, GROUND_MM + 9_000, z + 5_000),
                       year="2026")
    plan = sc.plan(World.of(terrain(A), swiss_shell(1), full))
    assert held(plan) == {full.key: ["spawn"]}
    assert plan.report["held_back"][0]["reasons"][0]["details"].endswith(", and its current version does not")
    sc = Scenario.start(tmp_path / "courtyard", World.of(terrain(A), swiss_shell(1)))
    plan = sc.plan(World.of(terrain(A), swiss_shell(1), l_shell("2026")))
    assert held(plan) == {} and ops(plan) == {l_shell("2026").key: ["add"]}


def test_the_ground_is_measured_under_the_feet(tmp_path: Path) -> None:
    # H14 took the highest vertex of every triangle whose horizontal box
    # holds the spawn, not the ground under the feet, and did not compare
    # with the current ground: on a slope, a vertex 50 m away and 1 m up
    # held every change to the spawn's terrain, by a code nothing lifts.
    # Found by review.
    slope = flat()
    slope[2][3] = SPAWN[1]  # 50 m east of the spawn, as high as its feet; the ground under them is 1 m lower
    sc = Scenario.start(tmp_path, World.of(terrain(A, slope), swiss_shell(1)))
    edited = [row[:] for row in slope]
    edited[1][1] += 300
    plan = sc.plan(World.of(terrain(A, edited, item=DEM_2027), swiss_shell(1)))
    assert held(plan) == {} and ops(plan) == {("terrain", f"tile:{A}"): ["replace"]}


def test_terrain_rising_into_the_players_space_is_held(tmp_path: Path) -> None:
    # A fin of new terrain 200 to 280 mm east of the feet, rising 1 m above
    # them: no triangle under the feet changes, but the capsule's 300 mm
    # radius reaches it. Found by review.
    x, y, z = SPAWN
    sc = Scenario.start(tmp_path, World.of(terrain(A), swiss_shell(1)))
    t = terrain(A, item=DEM_2027)
    fin = ([(x + 200, GROUND_MM, z - 50), (x + 280, GROUND_MM, z + 50), (x + 240, y + 1_000, z)], [0, 1, 2, 0, 2, 1])
    finned = t.but(mesh=parts_mesh(t.feature_id, "terrain", [grid_part(flat()), fin]))
    plan = sc.plan(World.of(finned, swiss_shell(1)))
    assert held(plan) == {finned.key: ["spawn"]}
    assert plan.report["held_back"][0]["reasons"][0]["details"] == (
        f"the new ground reaches into the player's space at the spawn {list(SPAWN)} (x and z within 300 mm, from "
        "100 mm below the feet to 1800 mm above them), where the current ground does not")


def test_removing_the_ground_under_the_spawn_is_held(tmp_path: Path) -> None:
    # Upstream loses the spawn's terrain tile, and an allowance lifts the
    # removal guards for the terrain: H14 skipped removals, so the ground
    # under the spawn went, and the gate sees no hole (amendment A8). Found
    # by review.
    base = World.of(terrain(A), terrain(B), swiss_shell(1), swiss_shell(2, B))
    sc = Scenario.start(tmp_path, base)
    allowance = {"allow_removals": [{"layer": "terrain", "dataset": "swissALTI3D", "at_most": 1,
                                     "reason": "swisstopo withdrew the sheet (test)."}]}
    plan = sc.plan(World.of(terrain(B), swiss_shell(1), swiss_shell(2, B)), overrides=allowance)
    assert held(plan) == {("terrain", f"tile:{A}"): ["spawn"]}
    assert plan.report["held_back"][0]["reasons"][0]["details"] == (
        "it removes the ground under the spawn, 1000 mm below the spawn's feet, so the player would fall")
    # Another tile's terrain may go.
    plan = sc.plan(World.of(terrain(A), swiss_shell(1), swiss_shell(2, B)), overrides=allowance)
    assert held(plan) == {} and ops(plan) == {("terrain", f"tile:{B}"): ["remove"]}


def test_triangles_meet_a_box_exactly() -> None:
    lo, hi = (0, 0, 0), (10, 10, 10)
    inside = ((2, 2, 2), (8, 2, 2), (2, 8, 8))
    on_a_face = ((0, -5, -5), (0, 20, -5), (0, -5, 20))  # in the plane x = 0
    through = ((-20, 5, -20), (30, 5, -20), (5, 5, 30))
    past_a_corner = ((11, -5, 5), (-5, 11, 5), (-5, -5, 5))  # in z = 5, up to x + y = 6: over the box's corner
    outside_a_corner = ((10, 12, 5), (12, 10, 5), (20, 20, 5))  # beyond the edge x = y = 10
    segment = ((-5, 5, 5), (15, 5, 5), (5, 5, 5))  # a degenerate triangle through the box
    assert triangle_meets_box(inside, lo, hi) and triangle_meets_box(through, lo, hi)
    assert triangle_meets_box(past_a_corner, lo, hi) and triangle_meets_box(segment, lo, hi)
    assert not triangle_meets_box(on_a_face, lo, hi) and not triangle_meets_box(outside_a_corner, lo, hi)
    # A seeded comparison with what is certain either way: a corner strictly
    # inside means they meet; bounds that do not share a volume mean not.
    state = 12345
    for _ in range(4000):
        tri = []
        for _ in range(3):
            point = []
            for _ in range(3):
                state = (state * 6364136223846793005 + 1442695040888963407) % (1 << 64)
                point.append((state >> 33) % 31 - 10)
            tri.append(tuple(point))
        meets = triangle_meets_box(tri, lo, hi)
        if any(all(0 < p[k] < 10 for k in range(3)) for p in tri):
            assert meets, tri
        if not all(min(p[k] for p in tri) < 10 and max(p[k] for p in tri) > 0 for k in range(3)):
            assert not meets, tri


def test_a_solid_encloses_the_space_its_walls_do_not_meet() -> None:
    space = ((-300, -100, -300), (300, 1_800, 300))
    closed = [(tuple(pos[i] for i in idx[k:k + 3])) for pos, idx in [box_part((-5_000, -1_000, -5_000),
                                                                                (5_000, 9_000, 5_000))]
              for k in range(0, len(idx), 3)]
    assert encloses(closed, *space)
    wings = [tri for lo, hi in [((-5_000, -1_000, -5_000), (-500, 9_000, 5_000)),
                                ((-5_000, -1_000, 1_000), (5_000, 9_000, 5_000))]
             for pos, idx in [box_part(lo, hi)] for tri in (tuple(pos[i] for i in idx[k:k + 3])
                                                            for k in range(0, len(idx), 3))]
    assert not encloses(wings, *space)
    above = [(tuple(pos[i] for i in idx[k:k + 3])) for pos, idx in [box_part((-1_000, 3_000, -1_000),
                                                                               (1_000, 4_000, 1_000))]
             for k in range(0, len(idx), 3)]
    assert not encloses(above, *space), "a closed box above the space crosses the ray twice"


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
