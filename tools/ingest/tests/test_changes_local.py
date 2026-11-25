"""The refresh planner on the local district stores. The stores are gitignored, so these tests skip cleanly without them;
CI has neither. Each test writes a baseline record for its current store
into a temporary log, and never writes to the stores.

- Water added: from `data/world/zurich-altstadt-before-water` to
  `data/world/zurich-altstadt-before-points` (the district as it was before
  its trees and light points): one set of 8 water adds, ending exactly at
  the second store's world.
- Points added: from `zurich-altstadt-before-points` to
  `data/world/zurich-altstadt` (ingest 0.2.0): one set per source, 26 tree
  assets and 29 light assets, and every other asset a provenance-only
  difference (the generator's version), which is kept.
- Water lost (the reverse): the water group vanishes, which stops the plan;
  with an allowance for 8 removals, one set of 8 removals.
- A fresh OpenStreetMap snapshot (the store named by
  STREETWIDE_FRESH_STORE, optional): no set, and 76 provenance-only
  differences kept.

With STREETWIDE_GATE=1 (needs cargo), each water set also goes through
`op_world check-change`: water is visual only, so G1 to G8 are its whole
gate. So do the points sets, in turn: G8 places every anchor of the
district on its terrain.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
from pathlib import Path

import pytest

from tools.ingest.changes.__main__ import main
from tools.ingest.tests.refresh_worlds import Log
from tools.worldfmt import cjson
from tools.worldfmt.changes import Objects, apply
from tools.worldfmt.store import Store

REPO = Path(__file__).resolve().parents[3]
DISTRICT = REPO / "data" / "world" / "zurich-altstadt"
BEFORE_POINTS = REPO / "data" / "world" / "zurich-altstadt-before-points"
BEFORE_WATER = REPO / "data" / "world" / "zurich-altstadt-before-water"
BEFORE_POINTS_ID = "blake3:4689c972"
FRESH = os.environ.get("STREETWIDE_FRESH_STORE")

needs_district = pytest.mark.skipif(
    not (BEFORE_POINTS / "world.json").is_file() or not (BEFORE_WATER / "world.json").is_file(),
    reason="needs the local district stores in data/world (gitignored)",
)
needs_points = pytest.mark.skipif(
    not (BEFORE_POINTS / "world.json").is_file() or not (DISTRICT / "world.json").is_file()
    or not Store(DISTRICT).read_world()[1]["attribution"][0].startswith("Quelle"),
    reason="needs the local district stores in data/world (gitignored), the district with its points",
)


def plan(tmp_path: Path, capsys, current: Path, new: Path, overrides: dict | None = None) -> tuple[int, str, Path]:
    log = Log(tmp_path / "log")
    if not (log.root / "log").is_dir():
        log.baseline(current, "Baseline of the local district (test).")
    out = tmp_path / f"plan-{len(list(tmp_path.glob('plan-*')))}"
    args = ["plan", "--current", str(current), "--new", str(new), "--log", str(log.root), "--out", str(out)]
    if overrides is not None:
        path = tmp_path / "overrides.json"
        path.write_bytes(json.dumps({"overrides_version": "0.1.0", **overrides}).encode())
        args += ["--overrides", str(path)]
    code = main(args)
    stdout, stderr = capsys.readouterr()
    return code, stdout + stderr, out


def gate(current: Path, bundle: Path, tmp_path: Path) -> dict:
    prefix = os.environ.get("STREETWIDE_OP_WORLD", "cargo run -q --locked -p op_world --").split()
    report = tmp_path / "gate.json"
    report.unlink(missing_ok=True)
    done = subprocess.run([*prefix, "check-change", str(current), str(bundle), "--report", str(report)],
                          cwd=REPO, capture_output=True, text=True, timeout=1800)
    assert report.is_file(), done.stderr
    return json.loads(report.read_bytes())


@needs_district
def test_water_added(tmp_path: Path, capsys) -> None:
    code, output, out = plan(tmp_path, capsys, BEFORE_WATER, BEFORE_POINTS)
    assert code == 0, output
    report = cjson.loads((out / "refresh-report.json").read_bytes())
    (s,) = report["sets"]
    assert (s["changes"], s["purpose"]["layer"], s["purpose"]["collider"]) == (8, "water", "none")
    change = cjson.loads((out / "sets" / "001" / "change.json").read_bytes())
    assert [c["op"] for c in change["changes"]] == ["add"] * 8
    assert report["final_world_id"].startswith(BEFORE_POINTS_ID)
    assert report["final_world_id"] == Store(BEFORE_POINTS).read_world()[0]
    assert report["conflicts"] == [] and report["held_back"] == [] and report["residual"]["provenance_only"] == 0
    if os.environ.get("STREETWIDE_GATE") == "1":
        checked = gate(BEFORE_WATER, out / "sets" / "001", tmp_path)
        assert checked["passed"] is True and checked["candidate_world_id"] == report["final_world_id"]


@needs_district
def test_water_lost(tmp_path: Path, capsys) -> None:
    code, output, out = plan(tmp_path, capsys, BEFORE_POINTS, BEFORE_WATER)
    assert code == 2 and not out.exists()
    assert output == ("error: the new store has no water from OpenStreetMap, but the current store has 8 such assets "
                      "made by the ingest; if upstream really lost them all, add an allow_removals entry to the "
                      "overrides file\n")
    allowance = {"allow_removals": [{"layer": "water", "dataset": "OpenStreetMap", "at_most": 8,
                                     "reason": "Test: the water of the reverse pair."}]}
    code, output, out = plan(tmp_path, capsys, BEFORE_POINTS, BEFORE_WATER, allowance)
    assert code == 0, output
    report = cjson.loads((out / "refresh-report.json").read_bytes())
    (s,) = report["sets"]
    change = cjson.loads((out / "sets" / "001" / "change.json").read_bytes())
    assert [c["op"] for c in change["changes"]] == ["remove"] * 8 and s["changes"] == 8
    assert report["final_world_id"] == Store(BEFORE_WATER).read_world()[0]
    assert report["allowances_used"][0]["removals"] == 8
    if os.environ.get("STREETWIDE_GATE") == "1":
        checked = gate(BEFORE_POINTS, out / "sets" / "001", tmp_path)
        assert checked["passed"] is True and checked["candidate_world_id"] == report["final_world_id"]


@needs_points
def test_points_added(tmp_path: Path, capsys) -> None:
    code, output, out = plan(tmp_path, capsys, BEFORE_POINTS, DISTRICT)
    assert code == 0, output
    report = cjson.loads((out / "refresh-report.json").read_bytes())
    sets = {s["purpose"]["layer"]: s for s in report["sets"]}
    assert sorted(sets) == ["street_objects", "vegetation"]
    assert (sets["vegetation"]["changes"], sets["street_objects"]["changes"]) == (26, 29)
    assert sets["vegetation"]["purpose"]["dataset"] == "Baumkataster Stadt Zürich"
    assert sets["street_objects"]["purpose"]["dataset"] == "Öffentliche Beleuchtung der Stadt Zürich"
    assert all(s["touches_colliders"] is False for s in report["sets"])
    # Every other asset differs only in the generator's version (0.1.0 to
    # 0.2.0), which a refresh keeps (ADR-0020, rule C).
    assert report["residual"]["provenance_only"] == 1491
    assert report["conflicts"] == [] and report["held_back"] == []
    for s in report["sets"]:
        change = cjson.loads((out / s["path"] / "change.json").read_bytes())
        assert {c["op"] for c in change["changes"]} == {"add"}
    if os.environ.get("STREETWIDE_GATE") != "1":
        return
    world = BEFORE_POINTS
    for s in report["sets"]:
        bundle = out / s["path"]
        checked = gate(world, bundle, tmp_path)
        assert checked["passed"] is True and checked["candidate_world_id"] == s["world_after"], checked
        outcomes = {c["id"]: c["outcome"] for c in checked["checks"]}
        assert (outcomes["G3"], outcomes["G7"], outcomes["G8"]) == ("review", "pass", "pass"), checked
        after = tmp_path / f"after-{bundle.name}"
        shutil.copytree(world / "objects", after / "objects")
        shutil.copytree(bundle / "objects", after / "objects", dirs_exist_ok=True)
        _, base = Store(world).read_world()
        candidate = apply(base, cjson.loads((bundle / "change.json").read_bytes()), Objects(Store(after)))
        assert candidate.write(Store(after)) == s["world_after"]
        world = after
    assert Store(world).read_world()[0] == report["final_world_id"]


@needs_district
@pytest.mark.skipif(not FRESH or not (Path(FRESH or ".") / "world.json").is_file(),
                    reason="set STREETWIDE_FRESH_STORE to a fresh ingest of zurich-altstadt made before water")
def test_fresh_openstreetmap_snapshot(tmp_path: Path, capsys) -> None:
    code, output, out = plan(tmp_path, capsys, BEFORE_WATER, Path(FRESH))
    assert code == 0, output
    assert output.splitlines()[0] == "nothing to refresh: 0 changes (76 provenance-only differences kept)"
    report = cjson.loads((out / "refresh-report.json").read_bytes())
    assert report["sets"] == [] and report["residual"]["provenance_only"] == 76
    assert sorted(p.name for p in out.iterdir()) == ["refresh-report.json", "refresh-report.md"]
