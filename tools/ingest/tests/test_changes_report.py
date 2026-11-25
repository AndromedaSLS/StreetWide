"""The refresh report (ADR-0020, Q11): canonical JSON that
passes its own schema, Markdown in a fixed order, the overrides template,
and the plan layout that `verify` reads back."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from tools.ingest.changes import layout
from tools.ingest.changes.__main__ import main
from tools.ingest.changes.report import schema_errors
from tools.ingest.tests.refresh_worlds import (
    A, B, DEM_2027, E, GROUND_MM, Scenario, World, box_mesh, contributed, entry_add, entry_replace, facade, flat,
    moved_box, ribbon, swiss_shell, terrain,
)
from tools.worldfmt import cjson
from tools.worldfmt.ids import content_id

SECTIONS = ["# Baseline refresh plan: refresh-test", "## Conflicts", "## Held back", "## Contributions",
            "## World-level differences", "## Stale overrides", "## Change sets",
            "## Checking and merging, in order", "## Overrides template"]


def shell(n: int, tile: str = A, year: str = "2025"):
    x = n * 20_000
    return swiss_shell(n, tile, lo=(x, GROUND_MM, 10_000), hi=(x + 10_000, GROUND_MM + 10_000, 20_000), year=year)


def base() -> World:
    return World.of(terrain(A), terrain(B), ribbon(A), *[shell(n) for n in range(1, 6)], shell(9, B))


def busy(tmp_path: Path) -> tuple[Scenario, World]:
    """A refresh with a conflict, held-back changes of several codes, a
    unit, a contribution, a tile only in the new store and emitted sets."""
    sc = Scenario.start(tmp_path, base())
    s2 = shell(2)
    mine = contributed(s2, parent=s2.manifest_id, mesh=box_mesh(s2.feature_id, (40_000, GROUND_MM, 10_000),
                                                                 (50_000, GROUND_MM + 10_500, 20_000)))
    sc.log.merge(sc.current, [entry_replace(mine)], [mine])
    on3 = facade(shell(3), "github:dora")
    sc.log.merge(sc.current, [entry_add(on3)], [on3], author="github:dora")
    edges = flat()
    edges[0][1] += 300
    new = base().put(
        moved_box(shell(1, year="2026"), dy=100),                 # emitted
        moved_box(shell(2, year="2026"), dy=100),                 # conflict: a contribution
        moved_box(shell(3, year="2026"), dy=100),                 # conflict: a facade sits on it
        moved_box(shell(4), dy=50),                                # held: unexplained
        terrain(B, edges, item=DEM_2027),                          # held: terrain edges
        swiss_shell(30, E, lo=(0, GROUND_MM, 0), hi=(9_000, GROUND_MM + 5_000, 9_000)),  # held: no such tile
    ).drop(shell(5).key).put(swiss_shell(31, A, lo=(100_000, GROUND_MM, 10_000), hi=(110_000, GROUND_MM + 10_000, 20_000)))
    return sc, new


def test_the_report_passes_its_schema_and_lists_everything(tmp_path: Path) -> None:
    sc, new = busy(tmp_path)
    plan = sc.plan(new)
    report = plan.report
    assert schema_errors(report) == []
    assert cjson.loads(plan.report_bytes) == report and plan.report_id == content_id(plan.report_bytes)
    assert [(e["feature_id"][-3:], [r["code"] for r in e["reasons"]]) for e in report["conflicts"]] == [
        ("002", ["contribution"]), ("003", ["dependent-contribution"])]
    assert [(e["layer"], e["feature_id"][-3:], [r["code"] for r in e["reasons"]]) for e in report["held_back"]] == [
        ("shell", "004", ["unexplained-change"]), ("shell", "030", ["tile-not-in-world"]),
        ("terrain", f"tile:{B}"[-3:], ["terrain-edges"])]
    assert all(r["rule"].startswith("H") for e in report["conflicts"] + report["held_back"] for r in e["reasons"])
    e = report["conflicts"][0]
    assert e["current_manifest_id"] is not None and e["new_content_key"].startswith("blake3:")
    assert [c["layer"] for c in report["counts"]] == ["facade", "shell", "terrain", "transport"]
    shells = report["counts"][1]
    assert (shells["replace"], shells["remove"], shells["add"], shells["contributions"]) == (1, 1, 1, 1)
    assert report["counts"][0]["contributions"] == 1
    (unit,) = report["units"]
    assert unit["status"] == "emitted" and [m["op"] for m in unit["members"]] == ["remove", "add"]
    listed = {c["feature_id"][-3:]: c for c in report["contributions"]}
    assert listed["002"]["upstream"] == "changed" and listed["002"]["authors"] == ["github:alice"]
    assert listed["003"]["depends_on"] == {"layer": "shell", "status": "held"}
    assert report["world_level"]["tiles_only_new"] == [E] and report["world_level"]["tiles_only_current"] == []
    assert report["tool"] == {"command": "tools.ingest.changes", "version": "0.1.0"}
    assert report["generators"] == {"current": ["streetwide-contrib 0.1.0", "streetwide-ingest 0.1.0"],
                                    "new": ["streetwide-ingest 0.1.0"]}
    assert report["log"]["records"] == 3 and report["log"]["head_record_id"] == content_id(
        (sc.log.root / "log" / "00000002.json").read_bytes())


def test_markdown_sections_and_template(tmp_path: Path, capsys) -> None:
    sc, new = busy(tmp_path)
    plan = sc.plan(new)
    md = plan.markdown.decode("utf-8")
    positions = [md.index("\n" + s + "\n") if not s.startswith("# ") else md.index(s) for s in SECTIONS]
    assert positions == sorted(positions), "the sections come in a fixed order"
    assert f"refresh-report.json`, content id `{plan.report_id}`" in md
    assert "`dependent-contribution` (H5): A contributed facade sits on this shell" in md
    assert "### `terrain-edges` (H8)" in md and "### `unexplained-change` (H11)" in md
    assert md.endswith("```\n") and "\r" not in md
    template = json.loads(md[md.index("```json") + 7: md.rindex("```")])
    assert template["overrides_version"] == "0.1.0" and template["hold"] == [] and template["allow_removals"] == []
    assert [(a["feature_id"][-3:], a["lifts"], a["reason"]) for a in template["accept"]] == [
        ("002", ["contribution"], ""), ("004", ["unexplained-change"], "")], \
        "no accept for codes nothing lifts (H5, H7, H8)"
    # Copied unchanged, the template does nothing: an empty reason is refused.
    path = tmp_path / "template.json"
    path.write_bytes(json.dumps(template).encode())
    args = sc.args(new) + ["--overrides", str(path)]
    assert main(args) == 2
    assert capsys.readouterr().err.startswith("error: overrides: accept shell ")
    # With reasons, it lifts what it names.
    for a in template["accept"]:
        a["reason"] = "Reviewed."
    plan = sc.plan(new, overrides={"accept": template["accept"]})
    assert [s.kind for s in plan.sets] == ["refresh", "decision"]
    assert [e["feature_id"][-3:] for e in plan.report["conflicts"]] == ["003"]


def test_read_plan_gives_verify_the_sets(tmp_path: Path, capsys) -> None:
    sc, new = busy(tmp_path)
    args = sc.args(new)
    out = Path(args[args.index("--out") + 1])
    assert main(args) == 0
    capsys.readouterr()
    written = layout.read_plan(out)
    assert written.report_id == content_id((out / layout.REPORT_JSON).read_bytes())
    assert [b.path for b in written.bundles] == [s["path"] for s in written.report["sets"]]
    first = written.bundles[0]
    assert first.directory == out / "sets" / "001" and first.change["author"] == "streetwide:ingest"
    assert first.base_world_id == written.report["current_world_id"]
    assert written.final_world_id == written.bundles[-1].world_after
    (first.directory / layout.CHANGE_JSON).write_bytes((first.directory / layout.CHANGE_JSON).read_bytes() + b" ")
    with pytest.raises(layout.PlanFileError, match="sets/001/change.json does not hash to"):
        layout.read_plan(out)
