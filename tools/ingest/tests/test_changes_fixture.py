"""The refresh planner on committed data: the water refresh of the fixture world, planned from the fixture world
as it was before water to `fixtures/world`, with ids that Rust made and the
plan's own ids pinned, so that every platform must give the same bytes.

The pre-water world is rebuilt from `fixtures/world` with
`tools.worldfmt.changes.apply` (step 1, tools/worldfmt/tests/test_changes.py):
its store holds exactly the objects that world references, so the water's
manifest and mesh must travel in the bundle.

With STREETWIDE_GATE=1 (needs cargo), the planned bundle also goes through
`op_world check-change`, which must pass it and predict the same world.
"""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

import pytest

from tools.ingest.changes.__main__ import main
from tools.ingest.tests.refresh_worlds import Log
from tools.worldfmt import cjson
from tools.worldfmt.changes import Objects, apply
from tools.worldfmt.ids import content_id
from tools.worldfmt.store import Store

REPO = Path(__file__).resolve().parents[3]
FIXTURE_WORLD = REPO / "fixtures" / "world"
TILE = "17/68646/45903"

# Made by Rust (tools/worldfmt/tests/test_changes.py checks them too).
FIXTURE_WORLD_ID = "blake3:fd6446c21c6b55e27ccfb6093ca73a17c444432513c487b9f3ae262ead8480af"
PRE_WATER_WORLD_ID = "blake3:6af9f18605b98e8b2a640452ac9341c41a1716160db9cacb09168172903ee9f6"
WATER_MANIFEST_ID = "blake3:48443eac"
WATER_MESH_ID = "blake3:3fe3966e"

# Made by this planner: they change only when its output changes.
CHANGE_ID = "blake3:98aa8fbd156dd2b1e1566399b777dd4d9c5effc5861a64f4fde95f0d22ac5e23"
REPORT_ID = "blake3:78bfd9343de8a0d4588815a9ae7bb0dd260fcc61ca2a11f0e4405df499f41871"
BASELINE_REASON = "Baseline of the fixture world before water (test)."


def pre_water_store(root: Path) -> Path:
    """The fixture world without its water, in a store that holds only the
    objects that world references."""
    store = Store(FIXTURE_WORLD)
    world_id, world = store.read_world()
    assert world_id == FIXTURE_WORLD_ID
    index = cjson.loads(store.read_object(world["tiles"][0]["tile_id"], "json"))
    (water,) = [a for a in index["assets"] if a["layer"] == "water"]
    candidate = apply(world, {"changes": [{
        "op": "remove", "tile": TILE, "layer": "water", "feature_id": water["feature_id"],
        "parent_version_id": water["manifest_id"], "manifest_id": None}]}, Objects(store))
    assert candidate.world_id == PRE_WATER_WORLD_ID
    out = Store(root)
    for a in index["assets"]:
        if a["layer"] == "water":
            continue
        out.write_object(store.read_object(a["manifest_id"], "json"), "json")
        out.write_object(store.read_object(a["content_id"], "glb"), "glb")
    assert candidate.write(out) == PRE_WATER_WORLD_ID
    return root


@pytest.fixture
def water_plan(tmp_path: Path, capsys) -> tuple[Path, Path]:
    current = pre_water_store(tmp_path / "before-water")
    log = Log(tmp_path / "log")
    log.baseline(current, BASELINE_REASON)
    out = tmp_path / "plan"
    code = main(["plan", "--current", str(current), "--new", str(FIXTURE_WORLD), "--log", str(log.root),
                 "--out", str(out)])
    stdout, stderr = capsys.readouterr()
    assert code == 0, stderr
    assert stdout.splitlines()[0] == ("1 change set with 1 change; 0 conflicts and 0 held-back changes listed in the "
                                      "report (0 provenance-only differences kept)")
    return current, out


def test_the_fixture_water_refresh(water_plan) -> None:
    current, out = water_plan
    report = cjson.loads((out / "refresh-report.json").read_bytes())
    (s,) = report["sets"]
    assert (s["path"], s["kind"], s["changes"], s["base_world_id"], s["world_after"]) == (
        "sets/001", "refresh", 1, PRE_WATER_WORLD_ID, FIXTURE_WORLD_ID)
    assert s["purpose"] == {"collider": "none", "layer": "water", "dataset": "OpenStreetMap"}
    assert s["touches_colliders"] is False
    assert report["final_world_id"] == FIXTURE_WORLD_ID
    assert (report["current_world_id"], report["new_world_id"]) == (PRE_WATER_WORLD_ID, FIXTURE_WORLD_ID)
    assert report["conflicts"] == [] and report["held_back"] == [] and report["residual"]["provenance_only"] == 0

    change = cjson.loads((out / "sets" / "001" / "change.json").read_bytes())
    (entry,) = change["changes"]
    assert entry["op"] == "add" and entry["tile"] == TILE and entry["layer"] == "water"
    assert entry["feature_id"] == f"tile:{TILE}" and entry["parent_version_id"] is None
    assert entry["manifest_id"].startswith(WATER_MANIFEST_ID)
    assert change["base_world_id"] == PRE_WATER_WORLD_ID and change["author"] == "streetwide:ingest"
    assert change["title"] == "Baseline refresh grossmuenster: water from OpenStreetMap (set 1 of 1)"

    bundle = sorted(p.relative_to(out / "sets" / "001").as_posix() for p in (out / "sets" / "001").rglob("*")
                    if p.is_file())
    manifest = f"objects/{entry['manifest_id'][7:9]}/{entry['manifest_id'][7:]}.json"
    mesh = [b for b in bundle if b.endswith(".glb")]
    assert bundle == sorted(["change.json", manifest] + mesh) and len(mesh) == 1
    assert mesh[0].split("/")[-1].startswith(WATER_MESH_ID[7:]), "the mesh is shipped"
    assert (out / "sets" / "001" / manifest).read_bytes() == Store(FIXTURE_WORLD).read_object(entry["manifest_id"], "json")

    assert s["change_id"] == CHANGE_ID
    assert report["world_level"]["spawn_ground_gap_mm"]["current"] == report["world_level"]["spawn_ground_gap_mm"]["new"]


def test_the_report_id_is_pinned(water_plan) -> None:
    _, out = water_plan
    assert content_id((out / "refresh-report.json").read_bytes()) == REPORT_ID


@pytest.mark.skipif(os.environ.get("STREETWIDE_GATE") != "1", reason="runs op_world; set STREETWIDE_GATE=1 (needs cargo)")
def test_the_fixture_plan_passes_op_worlds_gate(water_plan, tmp_path: Path) -> None:
    # A visual-only set: op_world's gate (G1 to G8) is the whole gate for it.
    current, out = water_plan
    prefix = os.environ.get("STREETWIDE_OP_WORLD", "cargo run -q --locked -p op_world --").split()
    report_path = tmp_path / "gate.json"
    done = subprocess.run([*prefix, "check-change", str(current), str(out / "sets" / "001"), "--report",
                           str(report_path)], cwd=REPO, capture_output=True, text=True, timeout=900)
    assert report_path.is_file(), done.stderr
    report = json.loads(report_path.read_bytes())
    assert report["passed"] is True, done.stdout
    assert report["world_id"] == PRE_WATER_WORLD_ID and report["candidate_world_id"] == FIXTURE_WORLD_ID
    assert report["change_id"] == CHANGE_ID
    outcomes = {c["id"]: c["outcome"] for c in report["checks"]}
    assert outcomes["G3"] == "review" and outcomes["G4"] == "not_applicable" and outcomes["G5"] == "not_applicable"
