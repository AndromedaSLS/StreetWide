"""Contributions, the world log and accepts in the refresh planner
(ADR-0020: Q2, Q5, Q10, Q18, the hold codes H1 to H6b and decision sets)."""

from __future__ import annotations

from pathlib import Path

import pytest

from tools.ingest.changes.common import PlanError
from tools.ingest.tests.refresh_worlds import (
    A, B, GROUND_MM, Scenario, World, box_mesh, contributed, entries, entry_add, entry_remove, entry_replace,
    facade, held, moved_box, ops, ribbon, swiss, swiss_shell, terrain, uuid,
)
from tools.worldfmt import log as worldlog
from tools.worldfmt.store import Store


def shell(n: int, tile: str = A, *, year: str = "2025", grow: int = 0) -> object:
    x = n * 20_000
    return swiss_shell(n, tile, lo=(x, GROUND_MM, 10_000), hi=(x + 10_000 + grow, GROUND_MM + 10_000, 20_000), year=year)


def base() -> World:
    return World.of(terrain(A), terrain(B), ribbon(A), *[shell(n) for n in range(1, 8)], shell(20, B))


def better(asset, author: str = "github:alice"):
    """A contributor's improved version of an asset made by the ingest."""
    b = asset.mesh.bounds
    mesh = box_mesh(asset.feature_id, b["min"], [b["max"][0], b["max"][1] + 500, b["max"][2]])
    return contributed(asset, author, parent=asset.manifest_id, mesh=mesh)


def contribute(sc: Scenario, asset, author: str = "github:alice"):
    version = better(asset, author)
    sc.log.merge(sc.current, [entry_replace(version)], [version], author=author)
    return version


def changed(n: int, tile: str = A):
    """Shell n as a newer upstream release has it: another year, a taller mesh."""
    return moved_box(shell(n, tile, year="2026"), dy=300)


def contributions(plan) -> dict:
    return {(c["layer"], c["feature_id"]): c for c in plan.report["contributions"]}


def test_contribution_never_replaced(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, base())
    c1, c2, c3, c4 = (contribute(sc, shell(n)) for n in (1, 2, 3, 4))
    # A contributed new building that upstream gains later under the same id.
    newcomer = contributed(shell(5).but(feature_id=f"swisstopo:swissbuildings3d:{uuid(55)}",
                                        mesh=box_mesh(f"swisstopo:swissbuildings3d:{uuid(55)}",
                                                      (110_000, GROUND_MM, 40_000), (120_000, GROUND_MM + 9_000, 50_000))),
                           "github:carol")
    sc.log.merge(sc.current, [entry_add(newcomer)], [newcomer], author="github:carol")
    # The ingest version that contribution 4 replaced is erased.
    Store(sc.current).object_path(shell(4).manifest_id, "json").unlink()
    upstream_newcomer = swiss_shell(55, A, lo=(110_000, GROUND_MM, 40_000), hi=(120_000, GROUND_MM + 9_000, 50_000))
    new = base().put(changed(2), upstream_newcomer).drop(shell(3).key)
    plan = sc.plan(new)

    statuses = {k[1][-3:]: (c["upstream"], c["would_be"]) for k, c in contributions(plan).items()}
    assert statuses == {"002": ("changed", "replace"), "003": ("gone", "remove"), "004": ("unknown", "replace"),
                        "055": ("new", "replace")}, "contribution 1 is unchanged upstream: counted, not listed"
    conflicts = entries(plan)
    assert conflicts == {c2.key: ["contribution"], c3.key: ["contribution"], c4.key: ["contribution"],
                         newcomer.key: ["contribution"]}
    assert plan.sets == [], "a contribution is never replaced or removed by a refresh"
    assert c1.key not in held(plan)
    assert plan.report["residual"]["contributions"] == 5
    details = {e["feature_id"]: e["reasons"][0]["details"] for e in plan.report["conflicts"]}
    assert "upstream changed the feature" in details[c2.feature_id]
    assert "upstream lost the feature" in details[c3.feature_id]
    assert "a manifest of the contribution's lineage cannot be read" in details[c4.feature_id]
    assert "upstream gained this feature" in details[newcomer.feature_id]


def test_contribution_moved_upstream(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, base())
    c1 = contribute(sc, shell(1))
    plan = sc.plan(base().put(moved_box(shell(1, year="2026"), dz=5_000, tile=B)))
    (e,) = plan.report["conflicts"]
    assert (e["feature_id"], e["would_be"], e["tile"], e["new_tile"]) == (c1.feature_id, "move", A, B)
    assert [r["code"] for r in e["reasons"]] == ["contribution"]
    assert "upstream moved the feature to tile" in e["reasons"][0]["details"]
    assert plan.sets == []


def test_current_only_contribution_is_kept(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, base())
    c3 = contribute(sc, shell(3))
    on1 = facade(shell(1), "github:dora")
    on2 = facade(shell(2), "github:erik")
    sc.log.merge(sc.current, [entry_add(on1), entry_add(on2)], [on1, on2], author="github:dora")
    # Upstream loses shell 3 and changes shell 1's source ids only.
    relabelled = shell(1).but(sources=(swiss("ID_RELABELLED"),))
    plan = sc.plan(base().drop(shell(3).key).put(relabelled))
    listed = contributions(plan)
    assert listed[c3.key]["upstream"] == "gone" and listed[c3.key]["would_be"] == "remove"
    assert listed[on1.key]["depends_on"] == {"layer": "shell", "status": "manifest-only"}
    assert on1.key in listed and on2.key not in listed, "facade 2's shell does not change: counted only"
    assert listed[on1.key]["authors"] == ["github:dora"]
    assert ops(plan) == {shell(1).key: ["replace"]}, "the facades and contribution 3 are kept"


def test_shell_under_facade_held(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, base())
    on1 = facade(shell(1))
    sc.log.merge(sc.current, [entry_add(on1)], [on1])
    cases = {
        "mesh change": base().put(changed(1)),
        "removal": base().drop(shell(1).key),
        "move": base().put(moved_box(shell(1, year="2026"), dx=-5_000, tile=B)),
        "rename": base().drop(shell(1).key).put(swiss_shell(91, A, lo=(20_000, GROUND_MM, 10_000),
                                                            hi=(30_000, GROUND_MM + 10_000, 20_000), year="2026")),
    }
    for case, new in cases.items():
        plan = sc.plan(new)
        assert held(plan)[shell(1).key][0] == "dependent-contribution", case
        assert "the facade by github:alice sits on it" in plan.report["conflicts"][0]["reasons"][0]["details"]
        assert plan.sets == [], case
    plan = sc.plan(cases["rename"])
    assert held(plan)[swiss_shell(91).key] == ["unit-held"]
    # A change of the manifest only keeps the mesh and bounds that G8 checks: emitted.
    plan = sc.plan(base().put(shell(1).but(sources=(swiss("ID_RELABELLED"),))))
    assert ops(plan) == {shell(1).key: ["replace"]} and held(plan) == {}
    # No accept can lift it.
    with pytest.raises(PlanError, match="cannot lift dependent-contribution for shell .*: remove or move the "
                                        r"facade first \(RFC 0002 G8\)"):
        sc.plan(cases["mesh change"], overrides={"accept": [{
            "layer": "shell", "feature_id": shell(1).feature_id, "current_manifest_id": shell(1).manifest_id,
            "content_key": None, "lifts": ["dependent-contribution"], "reason": "I want it."}]})


def test_takedown_removal_frozen_and_revert_liftable(tmp_path: Path) -> None:
    """ADR-0020, Q18 (b): a removal by a takedown stays frozen; a revert may
    be lifted by an accept with a reason, into a decision set of its own."""
    sc = Scenario.start(tmp_path, base())
    # A takedown removes shell 1.
    sc.log.merge(sc.current, [entry_remove(shell(1))], author="github:maintainer", decision="takedown",
                 reason="Privacy request (test).")
    # A contribution replaces shell 2, and a takedown reverts it.
    c2 = contribute(sc, shell(2))
    reverted = shell(2).but(parent=c2.manifest_id)
    sc.log.merge(sc.current, [entry_replace(reverted)], [reverted], author="github:maintainer",
                 decision="takedown", reason="Revert (test).")
    new = base().put(changed(2))
    plan = sc.plan(new)
    found = held(plan)
    assert found[shell(1).key] == ["takedown"], "a removal-only takedown keeps the key out"
    assert found[shell(2).key][0] == "takedown-revert", "a reverted key waits for a maintainer"
    details = {c["feature_id"]: c["reasons"][0]["details"] for c in plan.report["conflicts"]}
    assert "a takedown in log/00000001.json" in details[shell(1).feature_id]
    assert details[shell(1).feature_id].endswith("removed it; a refresh never adds it back")
    assert "reverted it" in details[shell(2).feature_id]
    assert plan.sets == []
    # A removal cannot be lifted.
    with pytest.raises(PlanError, match="cannot lift takedown for shell"):
        sc.plan(base(), overrides={"accept": [{
            "layer": "shell", "feature_id": shell(1).feature_id, "current_manifest_id": None,
            "content_key": None, "lifts": ["takedown"], "reason": "Bring it back."}]})
    # A revert can, and the change goes into a decision set of its own.
    conflict = next(c for c in plan.report["conflicts"] if c["feature_id"] == shell(2).feature_id)
    lifts = [r["code"] for r in conflict["reasons"]]
    plan = sc.plan(new, overrides={"accept": [{
        "layer": "shell", "feature_id": shell(2).feature_id, "current_manifest_id": reverted.manifest_id,
        "content_key": conflict["new_content_key"], "lifts": lifts,
        "reason": "Upstream fixed the building the takedown was about."}]})
    assert [(s.kind, [e["feature_id"] for e in s.entries]) for s in plan.sets] == [
        ("decision", [shell(2).feature_id])]
    assert "that a takedown reverted (maintainer decision)" in plan.sets[0].change["title"]
    assert plan.report["accepted"][0]["lifted"][0] == "takedown-revert"


def test_contribution_removal_not_readded(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, base())
    sc.log.merge(sc.current, [entry_remove(shell(1))], author="github:bob", reason="A duplicate (test).")
    extra = swiss_shell(60, B, lo=(50_000, GROUND_MM, 50_000), hi=(60_000, GROUND_MM + 8_000, 60_000))
    new = base().put(extra)
    plan = sc.plan(new)
    assert entries(plan) == {shell(1).key: ["removed-by-contribution"]}
    assert "github:bob removed it in log/00000001.json" in plan.report["conflicts"][0]["reasons"][0]["details"]
    assert ops(plan) == {extra.key: ["add"]}

    content_key = plan.report["conflicts"][0]["new_content_key"]
    plan = sc.plan(new, overrides={"accept": [{
        "layer": "shell", "feature_id": shell(1).feature_id, "current_manifest_id": None, "content_key": content_key,
        "lifts": ["removed-by-contribution"], "reason": "Not a duplicate after all."}]})
    assert [(s.kind, [e["feature_id"] for e in s.entries]) for s in plan.sets] == [
        ("refresh", [extra.feature_id]), ("decision", [shell(1).feature_id])]
    assert plan.sets[1].change["title"] == (f"Baseline refresh refresh-test: add back the shell {shell(1).feature_id} "
                                            "that github:bob removed (maintainer decision)")
    assert plan.report["accepted"] == [{"layer": "shell", "feature_id": shell(1).feature_id,
                                        "lifts": ["removed-by-contribution"], "lifted": ["removed-by-contribution"],
                                        "reason": "Not a duplicate after all.", "set": plan.sets[1].path}]


def test_readd_under_new_id_is_possible_duplicate(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, base())
    sc.log.merge(sc.current, [entry_remove(shell(1))], author="github:bob")
    sc.log.merge(sc.current, [entry_remove(shell(2))], author="github:maintainer", decision="takedown")
    c3 = contribute(sc, shell(3))
    # Shell 1 comes back under a new id; shell 2's outline returns shifted,
    # overlapping its old bounds by a tenth; a new building overlaps
    # contribution 3.
    again = swiss_shell(81, A, lo=(20_000, GROUND_MM, 10_000), hi=(30_000, GROUND_MM + 9_000, 20_000))
    shifted = swiss_shell(82, A, lo=(49_000, GROUND_MM, 10_000), hi=(59_000, GROUND_MM + 9_000, 20_000))
    beside = swiss_shell(83, A, lo=(61_000, GROUND_MM, 11_000), hi=(70_000, GROUND_MM + 9_000, 19_000))
    apart = swiss_shell(84, A, lo=(20_000, GROUND_MM, 150_000), hi=(30_000, GROUND_MM + 9_000, 160_000))
    new = base().drop(shell(1).key, shell(2).key).put(again, shifted, beside, apart)
    plan = sc.plan(new)
    found = entries(plan)
    assert found == {again.key: ["possible-duplicate"], shifted.key: ["possible-duplicate"],
                     beside.key: ["possible-duplicate"]}
    details = {e["feature_id"]: e["reasons"][0]["details"] for e in plan.report["conflicts"]}
    assert f"{shell(1).feature_id}, which github:bob's change set in log/00000001.json" in details[again.feature_id]
    assert "a takedown in log/00000002.json" in details[shifted.feature_id]
    assert f"the contribution shell {c3.feature_id} by github:alice" in details[beside.feature_id]
    assert ops(plan) == {apart.key: ["add"]}


def test_a_replace_that_grows_over_a_removed_key_is_a_possible_duplicate(tmp_path: Path) -> None:
    # Upstream merges a building into its neighbour, which keeps its id: the
    # removed building would come back inside the neighbour.
    sc = Scenario.start(tmp_path, base())
    sc.log.merge(sc.current, [entry_remove(shell(2))], author="github:maintainer", decision="takedown")
    sc.log.merge(sc.current, [entry_remove(shell(4))], author="github:bob")
    c6 = contribute(sc, shell(6))

    def grown(n: int, x0: int, x1: int, year: str = "2026"):
        return swiss_shell(n, A, lo=(x0, GROUND_MM, 10_000), hi=(x1, GROUND_MM + 10_000, 20_000), year=year)

    over_takedown = grown(1, 20_000, 50_000)        # over shell 2 (40 to 50 m)
    over_removal = grown(3, 60_000, 90_000)         # over shell 4 (80 to 90 m)
    over_contribution = grown(7, 125_000, 150_000)  # over half of contribution 6 (120 to 130 m)
    a_little = grown(5, 100_000, 111_000)           # over nothing
    new = base().drop(shell(2).key, shell(4).key).put(over_takedown, over_removal, over_contribution, a_little)
    plan = sc.plan(new)
    assert entries(plan) == {k.key: ["possible-duplicate"] for k in (over_takedown, over_removal, over_contribution)}
    assert ops(plan) == {a_little.key: ["replace"]}
    details = {e["feature_id"]: e["reasons"][0]["details"] for e in plan.report["conflicts"]}
    assert details[over_takedown.feature_id] == (
        f"its new bounds overlap shell {shell(2).feature_id}, which a takedown in log/00000001.json "
        f"({sc.log.records[1][0]}) removed; its current bounds did not")
    assert f"the contribution shell {c6.feature_id} by github:alice" in details[over_contribution.feature_id]
    # Bounds that already met the removed key's place do not count: the
    # grown shell was merged earlier, and now grows further west.
    merged = over_takedown.but(parent=shell(1).manifest_id)
    sc.log.refresh(sc.current, [entry_replace(merged)], [merged])
    further = grown(1, 15_000, 50_000, "2027")
    plan = sc.plan(base().drop(shell(2).key, shell(4).key).put(further))
    assert held(plan) == {} and ops(plan) == {further.key: ["replace"]}
    # In a tile where a removed key's bounds cannot be read, a growing
    # replace is held like an add.
    for path in (sc.log.root / "worlds").glob("*.json"):
        path.unlink()
    Store(sc.current).object_path(shell(2).manifest_id, "json").unlink()
    plan = sc.plan(base().drop(shell(2).key, shell(4).key).put(grown(1, 10_000, 60_000, "2027")))
    assert held(plan) == {shell(1).key: ["removed-bounds-unknown"]}


def test_removed_manifest_erased(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, base())
    sc.log.merge(sc.current, [entry_remove(shell(2))], author="github:maintainer", decision="takedown")
    Store(sc.current).object_path(shell(2).manifest_id, "json").unlink()
    over = swiss_shell(85, A, lo=(45_000, GROUND_MM, 15_000), hi=(55_000, GROUND_MM + 9_000, 25_000))
    near = swiss_shell(86, A, lo=(20_000, GROUND_MM, 150_000), hi=(30_000, GROUND_MM + 9_000, 160_000))
    elsewhere = swiss_shell(87, B, lo=(20_000, GROUND_MM, 150_000), hi=(30_000, GROUND_MM + 9_000, 160_000))
    new = base().drop(shell(2).key).put(over, near, elsewhere)
    # The log keeps worlds/: the bounds come from the tile index of the
    # world before the takedown.
    plan = sc.plan(new)
    assert held(plan) == {over.key: ["possible-duplicate"]}
    # Without worlds/, the bounds are unknown: every add of a shell in that tile is held.
    record = sc.log.records[1][1]
    (sc.log.root / worldlog.world_path(record["world_before"])).unlink()
    plan = sc.plan(new)
    assert held(plan) == {over.key: ["removed-bounds-unknown"], near.key: ["removed-bounds-unknown"]}
    assert ops(plan) == {elsewhere.key: ["add"]}


def test_generator_with_contribution_source_is_contribution(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, base())
    disguised = better(shell(1)).but(generator=("streetwide-ingest", "0.1.0"))
    sc.log.merge(sc.current, [entry_replace(disguised)], [disguised])
    plan = sc.plan(base())
    assert plan.sets == [] and held(plan) == {}, "kept: upstream did not change since the version it replaced"
    plan = sc.plan(base().put(changed(1)))
    assert entries(plan) == {shell(1).key: ["contribution"]}
    assert contributions(plan)[shell(1).key]["origin"] == "contribution"


def test_baseline_only_log_with_contributions(tmp_path: Path) -> None:
    world = base()
    world.put(better(shell(1)))
    sc = Scenario.start(tmp_path, world)
    plan = sc.plan(base())
    (warning,) = plan.report["log"]["warnings"]
    assert warning.startswith("the world log holds only its baseline record, but 1 current asset is a contribution")
    assert entries(plan) == {shell(1).key: ["origin-unclear"]}
    assert "the world log shows no change to it since the baseline" in plan.report["conflicts"][0]["reasons"][0]["details"]
    assert contributions(plan)[shell(1).key]["origin"] == "unclear"
    assert b"**Warning:** the world log holds only its baseline record" in plan.markdown


def test_all_reasons_and_named_lifts(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, base())
    sc.log.merge(sc.current, [entry_remove(shell(1))], author="github:bob")
    # Contribution 2 grows over the place where shell 1 was.
    c2 = shell(2)
    grown = contributed(c2, parent=c2.manifest_id,
                        mesh=box_mesh(c2.feature_id, (18_000, GROUND_MM, 10_000), (50_000, GROUND_MM + 10_000, 20_000)))
    sc.log.merge(sc.current, [entry_replace(grown)], [grown])
    plan = sc.plan(base())
    (e,) = plan.report["conflicts"]
    assert (e["feature_id"], [r["code"] for r in e["reasons"]]) == (
        shell(1).feature_id, ["removed-by-contribution", "possible-duplicate"])
    scope = {"layer": "shell", "feature_id": shell(1).feature_id, "current_manifest_id": None,
             "content_key": e["new_content_key"], "reason": "Checked: not a duplicate."}
    plan = sc.plan(base(), overrides={"accept": [dict(scope, lifts=["removed-by-contribution"])]})
    assert entries(plan) == {shell(1).key: ["possible-duplicate"]}, "only the named code is lifted"
    assert plan.report["accepted"][0]["set"] is None and plan.sets == []
    plan = sc.plan(base(), overrides={"accept": [dict(scope, lifts=["removed-by-contribution", "possible-duplicate"])]})
    assert [s.kind for s in plan.sets] == ["decision"] and held(plan) == {}


def test_accepted_contribution_replacement_own_set(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, base())
    c2 = contribute(sc, shell(2))
    extra = swiss_shell(61, B, lo=(50_000, GROUND_MM, 50_000), hi=(60_000, GROUND_MM + 8_000, 60_000))
    new = base().put(changed(2), changed(5), extra)
    plan = sc.plan(new)
    (e,) = plan.report["conflicts"]
    plan = sc.plan(new, overrides={"accept": [{
        "layer": "shell", "feature_id": c2.feature_id, "current_manifest_id": c2.manifest_id,
        "content_key": e["new_content_key"], "lifts": ["contribution"],
        "reason": "The new release is better than the contribution."}]})
    assert [(s.kind, sorted(x["feature_id"][-3:] for x in s.entries)) for s in plan.sets] == [
        ("refresh", ["005", "061"]), ("decision", ["002"])]
    decision = plan.sets[1]
    (entry,) = decision.entries
    assert (entry["op"], entry["parent_version_id"]) == ("replace", c2.manifest_id)
    assert decision.change["title"] == (f"Baseline refresh refresh-test: replace the contributed shell "
                                        f"{c2.feature_id} by github:alice (maintainer decision)")
    assert '"The new release is better than the contribution."' in decision.change["rationale"]
    assert plan.report["sets"][1]["kind"] == "decision"
    assert decision.base_world_id == plan.sets[0].world_after


def test_stale_accept_meets_new_contribution(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, base())
    unexplained = moved_box(shell(1), dy=200)
    plan = sc.plan(base().put(unexplained))
    (e,) = plan.report["held_back"]
    accept = {"layer": "shell", "feature_id": shell(1).feature_id, "current_manifest_id": e["current_manifest_id"],
              "content_key": e["new_content_key"], "lifts": ["unexplained-change"], "reason": "Explained."}
    assert ops(sc.plan(base().put(unexplained), overrides={"accept": [accept]})) == {shell(1).key: ["replace"]}
    # A contribution is merged on the same key before the next plan.
    contribute(sc, shell(1))
    plan = sc.plan(base().put(unexplained), overrides={"accept": [accept]})
    assert plan.report["stale_overrides"] == [{"entry": "accept", "layer": "shell", "feature_id": shell(1).feature_id,
                                               "dataset": None, "why": "current_manifest_id no longer matches"}]
    assert entries(plan) == {shell(1).key: ["contribution"]}
    assert plan.sets == []
