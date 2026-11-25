"""Baseline refresh plans through the real gate, and the rules of
`python -m tools.ingest.changes verify` (ADR-0020, section 2).

The whole file is skipped unless STREETWIDE_GATE=1: most tests run
`op_replay` and `op_world`, so they need cargo. CI runs it in the `tests`
job, which has both cargo and Python.

Through the real gate:
- the fixture water refresh (test_changes_fixture.py) through `verify`, with the default
  command prefixes (`cargo run ...`);
- a synthetic world of two tiles with a ribbon change, a shell replace, a
  move to the other tile, a 1-to-2 split and a removal: two sets, both
  pass the full gate (G9 and G10 run on the second), and every id
  matches;
- the terrain-edge mirror: a terrain change the planner holds back
  (`terrain-edges`), forced into a bundle, fails G8 with as many edge
  vertices as the planner counted;
- a contribution's removal is not added back: the far-building removal of
  crates/op_replay/tests/full_gate.rs is merged with `op_replay merge
  --log`, and planning with `fixtures/world` as the new store gives a
  `removed-by-contribution` conflict. An accept puts the add into a
  decision set, which passes `verify`.

`verify`'s own rules run with fake tools written in Python (`Fake`), which
build the candidate world with `tools.worldfmt.changes.apply`, so that each
report check can be broken on purpose.

STREETWIDE_OP_REPLAY and STREETWIDE_OP_WORLD set the command prefixes. By
default the tests build both tools once with cargo and run the binaries,
except the fixture test, which uses verify's defaults.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

import pytest

from tools.ingest.changes import verify as verify_module
from tools.ingest.changes.__main__ import main
from tools.ingest.changes.layout import REPORT_JSON, REPORT_MD
from tools.ingest.changes.verify import (
    Options, Replan, VerifyError, build_work_store, join_command, plan_differences, split_command, verify,
)
from tools.ingest.tests.refresh_worlds import (
    A, B, DEM_2027, GROUND_MM, OSM_V2, SWISS_YEAR, Log, Scenario, World, flat, moved_box, ribbon, swiss_shell,
    terrain,
)
from tools.ingest.tests.test_changes_fixture import (
    BASELINE_REASON, CHANGE_ID, FIXTURE_WORLD, FIXTURE_WORLD_ID, PRE_WATER_WORLD_ID, TILE, pre_water_store,
)
from tools.worldfmt import cjson, glb
from tools.worldfmt.changes import INGEST_AUTHOR, Objects, apply, parse, to_bytes
from tools.worldfmt.ids import content_id
from tools.worldfmt.store import Store

pytestmark = pytest.mark.skipif(os.environ.get("STREETWIDE_GATE") != "1",
                                reason="runs op_replay and op_world; set STREETWIDE_GATE=1 (needs cargo)")

REPO = Path(__file__).resolve().parents[3]
SAMPLE = REPO / "fixtures" / "replays" / "sample.oprep"
SAMPLE_AUTHOR = "streetwide:fixtures"
"""The author of crates/op_replay/tests/full_gate.rs's change sets."""
REVIEW = "github:reviewer=better:Checked (test)."


# ------------------------------------------------------------ real tools --


@dataclass(frozen=True)
class Rust:
    op_replay: str
    op_world: str
    custom: bool
    """True when STREETWIDE_OP_REPLAY and STREETWIDE_OP_WORLD name the tools."""

    def replay(self, *args: object) -> subprocess.CompletedProcess:
        return _run(self.op_replay, args)

    def world(self, *args: object) -> subprocess.CompletedProcess:
        return _run(self.op_world, args)

    def verify_args(self) -> list[str]:
        return ["--op-replay", self.op_replay, "--op-world", self.op_world]


def _run(prefix: str, args: tuple[object, ...]) -> subprocess.CompletedProcess:
    return subprocess.run([*split_command(prefix), *(str(a) for a in args)], cwd=REPO, capture_output=True,
                          text=True, encoding="utf-8", errors="replace", timeout=1800)


@pytest.fixture(scope="session")
def rust() -> Rust:
    replay, world = os.environ.get("STREETWIDE_OP_REPLAY"), os.environ.get("STREETWIDE_OP_WORLD")
    if replay and world:
        return Rust(replay, world, True)
    done = subprocess.run(["cargo", "build", "-q", "--locked", "-p", "op_replay", "-p", "op_world",
                           "--message-format=json"], cwd=REPO, capture_output=True, text=True, timeout=3600)
    assert done.returncode == 0, done.stderr
    built = {}
    for line in done.stdout.splitlines():
        if not line.startswith("{"):
            continue
        msg = json.loads(line)
        if msg.get("reason") == "compiler-artifact" and msg.get("executable"):
            built[msg["target"]["name"]] = msg["executable"]
    return Rust(replay or join_command([built["op_replay"]]), world or join_command([built["op_world"]]), False)


def gate_outcomes(entry: dict) -> dict[str, str]:
    return {c["id"]: c["outcome"] for c in entry["checks"]}


def files_under(root: Path) -> dict[str, bytes]:
    return {p.relative_to(root).as_posix(): p.read_bytes() for p in sorted(root.rglob("*")) if p.is_file()}


# ------------------------------------------------- the fixture water refresh --


def test_the_fixture_water_refresh_passes_verify(tmp_path: Path, capsys, monkeypatch, rust: Rust) -> None:
    monkeypatch.chdir(REPO)     # verify's default commands run `cargo run` from the working directory
    current = pre_water_store(tmp_path / "before-water")
    log = Log(tmp_path / "log")
    log.baseline(current, BASELINE_REASON)
    plan = tmp_path / "plan"
    assert main(["plan", "--current", str(current), "--new", str(FIXTURE_WORLD), "--log", str(log.root),
                 "--out", str(plan)]) == 0
    # The sample walk on the world before water: water has no collider, so
    # every checkpoint stays.
    suite = tmp_path / "before-water.oprep"
    done = rust.replay("rerecord", SAMPLE, "--world", current, "--out", suite)
    assert done.returncode == 0 and "20 of 20 checkpoints unchanged" in done.stdout, done.stderr
    before = files_under(current)
    capsys.readouterr()

    args = ["verify", "--current", str(current), "--plan", str(plan), "--scratch", str(tmp_path / "scratch"),
            "--replay", str(suite), "--report", str(tmp_path / "verify.json"),
            "--replan", "--new", str(FIXTURE_WORLD), "--log", str(log.root)]
    # Unless the tools are named, verify's defaults run them (cargo run).
    code = main(args + (rust.verify_args() if rust.custom else []))
    out = capsys.readouterr()
    assert code == 0, out.out + out.err

    summary = cjson.loads((tmp_path / "verify.json").read_bytes())
    (s,) = summary["sets"]
    assert s["change_id"] == CHANGE_ID and s["base_world_id"] == PRE_WATER_WORLD_ID
    assert s["candidate_world_id"] == s["predicted_world_after"] == s["world_after"] == FIXTURE_WORLD_ID
    assert summary["final_world_id"] == summary["predicted_final_world_id"] == FIXTURE_WORLD_ID
    assert s["passed"] and s["problems"] == []
    outcomes = gate_outcomes(s)
    assert (outcomes["G3"], outcomes["G4"], outcomes["G5"]) == ("review", "not_applicable", "not_applicable")
    assert (outcomes["G9"], outcomes["G10"]) == ("not_applicable", "not_applicable"), "water has no collider"
    assert summary["replan"] == {"identical": True, "files": 5, "differences": []}
    assert summary["validate"]["passed"] and summary["passed"] and summary["problems"] == []
    comparison = summary["comparison"]
    assert (comparison["changed_assets"], comparison["changed_colliders"], comparison["findings"]) == (1, 0, 0)
    assert summary["replays"] == [content_id(suite.read_bytes())]
    assert files_under(current) == before, "verify never writes to the canonical store"
    assert "every set passes the gate and every id matches the plan" in out.out


# ------------------------------------------------ a synthetic world, two tiles --


def shell(n: int, tile: str, x0: int, x1: int, z0: int, z1: int, *, h: int = 12_000, year: str = SWISS_YEAR):
    return swiss_shell(n, tile, lo=(x0, GROUND_MM, z0), hi=(x1, GROUND_MM + h, z1), year=year)


def two_tile_base() -> World:
    """Flat terrain in tiles A and B, a road along both (z 90 to 110 m), and
    two buildings that do not change."""
    return World.of(terrain(A), terrain(B), ribbon(A), ribbon(B),
                    shell(7, A, 40_000, 50_000, 20_000, 30_000), shell(8, B, 120_000, 130_000, 20_000, 30_000))


def two_tile_worlds() -> tuple[World, World]:
    current = two_tile_base().put(
        shell(1, A, 20_000, 30_000, 40_000, 50_000),        # replaced: 6 m taller
        shell(2, A, 186_000, 196_000, 50_000, 60_000),      # moves 10 m east, into tile B
        shell(3, B, 50_000, 70_000, 20_000, 30_000),        # split in two
        shell(6, A, 100_000, 110_000, 80_000, 89_000),      # gone, 1 m from the road
    )
    new = two_tile_base().put(
        shell(1, A, 20_000, 30_000, 40_000, 50_000, h=18_000, year="2026"),
        moved_box(shell(2, A, 186_000, 196_000, 50_000, 60_000, year="2026"), dx=10_000, tile=B),
        shell(4, B, 50_000, 60_000, 20_000, 30_000, year="2026"),
        shell(5, B, 60_000, 70_000, 20_000, 30_000, year="2026"),
        moved_box(ribbon(B, version=OSM_V2), dy=20, material="road"),
    )
    return current, new


@dataclass
class Planned:
    sc: Scenario
    new: Path
    plan: Path

    @property
    def report(self) -> dict:
        return cjson.loads((self.plan / REPORT_JSON).read_bytes())


def plan_two_tiles(tmp_path: Path, capsys) -> Planned:
    current, new = two_tile_worlds()
    sc = Scenario.start(tmp_path, current)
    new_root = sc.write_new(new, "new")
    out = tmp_path / "plan"
    assert main(sc.args(new_root, out)) == 0
    capsys.readouterr()
    return Planned(sc, new_root, out)


def test_a_two_tile_refresh_passes_the_full_gate(tmp_path: Path, capsys, rust: Rust) -> None:
    p = plan_two_tiles(tmp_path, capsys)
    report = p.report
    assert report["conflicts"] == [] and report["held_back"] == []
    entries = [[(e["op"], e["tile"], e["feature_id"].rsplit("-", 1)[-1]) for e in parse(
        (p.plan / s["path"] / "change.json").read_bytes())["changes"]] for s in report["sets"]]
    assert entries == [
        [("replace", B, f"tile:{B}")],
        [("replace", A, "000000000001"), ("remove", A, "000000000002"), ("remove", A, "000000000006"),
         ("add", B, "000000000002"), ("remove", B, "000000000003"), ("add", B, "000000000004"),
         ("add", B, "000000000005")],
    ], "a visual-only set first, then the shells: a replace, a move, a removal and a 1-to-2 split"
    assert sorted(len(u["members"]) for u in report["units"]) == [2, 3]

    # The sample walk's inputs, recorded in this world: it walks along the
    # road, so G10 plays it on the shell set.
    walk = tmp_path / "walk.oprep"
    done = rust.replay("synth", "--world", p.sc.current, "--out", walk)
    assert done.returncode == 0, done.stderr

    summaries = []
    for run in (1, 2):
        code = main(["verify", "--current", str(p.sc.current), "--plan", str(p.plan),
                     "--scratch", str(tmp_path / f"scratch-{run}"), "--replay", str(walk),
                     "--report", str(tmp_path / f"verify-{run}.json"), *rust.verify_args()])
        out = capsys.readouterr()
        assert code == 0, out.out + out.err
        summaries.append((tmp_path / f"verify-{run}.json").read_bytes())
    assert summaries[0] == summaries[1], "the same inputs give the same summary"
    assert b"\\" not in summaries[0] and str(tmp_path).encode() not in summaries[0], "no local path"

    summary = cjson.loads(summaries[0])
    first, second = summary["sets"]
    base = report["current_world_id"]
    for s, planned in zip(summary["sets"], report["sets"]):
        assert s["base_world_id"] == base == planned["base_world_id"]
        assert s["candidate_world_id"] == s["predicted_world_after"] == s["world_after"] == planned["world_after"]
        assert s["passed"] and s["problems"] == []
        base = s["world_after"]
    assert summary["final_world_id"] == report["final_world_id"] == base
    assert (gate_outcomes(first)["G9"], gate_outcomes(first)["G10"]) == ("not_applicable", "not_applicable")
    assert (gate_outcomes(second)["G9"], gate_outcomes(second)["G10"]) == ("pass", "pass"), "the full gate ran"
    details = {c["id"]: c["details"] for c in cjson.loads((tmp_path / "scratch-1" / "gate" / "002.json")
                                                         .read_bytes())["checks"]}
    assert details["G9"] == ["3 probe points on public ways stay free"]
    assert details["G10"] == ["replay b6dea863e9f14d99: all 20 waypoints reached"]
    comparison = summary["comparison"]
    assert comparison["changed_assets"] == 8 and comparison["changed_colliders"] == 7
    # The whole-refresh comparison probes every way the refresh comes near.
    assert comparison["walkability"]["probes"] == 22 and comparison["findings"] == 0
    (moved,) = comparison["replays"]
    assert moved["played"] and moved["off"] == []
    # The replay was moved to each new world, as after a merge.
    assert len(list((tmp_path / "scratch-1" / "replays").rglob("*.oprep"))) == 2


# ------------------------------------------------------- terrain-edge mirror --


def test_the_terrain_edge_count_agrees_with_g8(tmp_path: Path, capsys, rust: Rust) -> None:
    heights = flat()
    for j, i in ((0, 2), (2, 0), (4, 4), (1, 1)):   # two edge vertices, a corner, and one inside
        heights[j][i] += 300
    old_terrain = terrain(A)
    new_terrain = terrain(A, heights, item=DEM_2027)
    world = World.of(old_terrain, shell(1, A, 20_000, 30_000, 40_000, 50_000))
    sc = Scenario.start(tmp_path, world)
    new_root = sc.write_new(World.of(new_terrain, shell(1, A, 20_000, 30_000, 40_000, 50_000)), "new")
    out = tmp_path / "plan"
    assert main(sc.args(new_root, out)) == 0
    capsys.readouterr()
    report = cjson.loads((out / REPORT_JSON).read_bytes())
    assert report["sets"] == []
    (entry,) = [e for e in report["held_back"] if e["layer"] == "terrain"]
    (reason,) = [r for r in entry["reasons"] if r["code"] == "terrain-edges"]
    planned = int(re.match(r"(\d+) vert", reason["details"]).group(1))
    assert planned == 3 == glb.missing_boundary_vertices(glb.read(old_terrain.mesh.data),
                                                          glb.read(new_terrain.mesh.data))

    # Force the held change into a bundle, as a refresh would write it.
    manifest = new_terrain.manifest()
    manifest["parent_version_id"] = old_terrain.manifest_id
    data = cjson.dumps(manifest)
    bundle = tmp_path / "forced"
    store = Store(bundle)
    store.write_object(data, "json")
    store.write_object(new_terrain.mesh.data, "glb")
    (bundle / "change.json").write_bytes(to_bytes({
        "format": "streetwide.change", "contract_version": "0.1.0", "base_world_id": report["current_world_id"],
        "title": "Forced terrain change (test)", "author": INGEST_AUTHOR,
        "rationale": "The planner held this back (terrain-edges); the gate must agree.",
        "changes": [{"op": "replace", "tile": A, "layer": "terrain", "feature_id": f"tile:{A}",
                     "parent_version_id": old_terrain.manifest_id, "manifest_id": content_id(data)}],
    }))
    gate = tmp_path / "gate.json"
    done = rust.replay("check-change", sc.current, bundle, "--report", gate)
    assert done.returncode == 1, done.stdout + done.stderr
    g8 = {c["id"]: c for c in cjson.loads(gate.read_bytes())["checks"]}["G8"]
    assert g8["outcome"] == "fail"
    (detail,) = g8["details"]
    assert detail == (f"{A} terrain tile:{A}: {planned} edge vertices differ from the old terrain; "
                      "tiles would crack")


# --------------------------------------------- a contribution's removal --


def far_building(rust: Rust) -> tuple[str, str]:
    """The building of full_gate.rs's `removing_a_building_away_from_the_path_
    keeps_the_replay`: the first swisstopo shell of the fixture tile whose
    bounds stay 40 m from every point of the sample walk, along x or z.
    Returns its feature id and manifest id."""
    store = Store(FIXTURE_WORLD)
    _, world = store.read_world()
    (tile,) = world["tiles"]
    done = rust.replay("trace", SAMPLE, "--world", FIXTURE_WORLD, "--every", "60")
    assert done.returncode == 0, done.stderr
    sx, _, sz = world["spawn"]["position_mm"]       # the sim frame's origin: the spawn at height 0
    walk = []
    for line in done.stdout.splitlines()[1:]:
        x, y, z = (round(float(v) * 1000) for v in line.split()[1:4])
        walk.append((x + sx, y, z + sz))
    assert len(walk) == 20
    origin = tile["origin_mm"]
    for a in cjson.loads(store.read_object(tile["tile_id"], "json"))["assets"]:
        if a["layer"] != "shell" or not a["feature_id"].startswith("swisstopo:"):
            continue
        lo = [a["bounds_mm"]["min"][k] + origin[k] for k in range(3)]
        hi = [a["bounds_mm"]["max"][k] + origin[k] for k in range(3)]
        if all(any(p[k] < lo[k] - 40_000 or p[k] > hi[k] + 40_000 for k in (0, 2)) for p in walk):
            return a["feature_id"], a["manifest_id"]
    raise AssertionError("no building 40 m from the sample walk")


def test_a_contribution_removal_is_not_added_back(tmp_path: Path, capsys, rust: Rust) -> None:
    world = tmp_path / "world"
    shutil.copytree(FIXTURE_WORLD, world)
    log = tmp_path / "log"
    done = rust.world("baseline", world, "--log", log, "--reason", "The fixture world (test).")
    assert done.returncode == 0, done.stderr

    # 1. A contributor removes a building away from the sample walk; it
    #    touches a collider, so op_replay merge runs the full gate.
    far, manifest_id = far_building(rust)
    bundle = tmp_path / "far"
    bundle.mkdir()
    (bundle / "change.json").write_bytes(to_bytes({
        "format": "streetwide.change", "contract_version": "0.1.0", "base_world_id": FIXTURE_WORLD_ID,
        "title": "Remove a building away from the sample walk (test)", "author": SAMPLE_AUTHOR,
        "rationale": "Demolished (test).",
        "changes": [{"op": "remove", "tile": TILE, "layer": "shell", "feature_id": far,
                     "parent_version_id": manifest_id, "manifest_id": None}],
    }))
    done = rust.replay("merge", world, bundle, "--require-base", "--log", log, "--reason", "Test merge.",
                       "--review", REVIEW, "--replay", SAMPLE)
    assert done.returncode == 0, done.stdout + done.stderr
    merged = Store(world).read_world()[0]
    assert merged != FIXTURE_WORLD_ID
    assert len(list((log / "log").glob("*.json"))) == 2

    # 2. Plan with fixtures/world, which still has the building, as the new store.
    plan = tmp_path / "plan"
    args = ["plan", "--current", str(world), "--new", str(FIXTURE_WORLD), "--log", str(log)]
    assert main([*args, "--out", str(plan)]) == 0
    capsys.readouterr()
    report = cjson.loads((plan / REPORT_JSON).read_bytes())

    # 3. A conflict, not an add.
    assert report["sets"] == [] and report["held_back"] == []
    (conflict,) = report["conflicts"]
    assert (conflict["layer"], conflict["feature_id"], conflict["would_be"]) == ("shell", far, "add")
    assert [r["code"] for r in conflict["reasons"]] == ["removed-by-contribution"]
    assert SAMPLE_AUTHOR in conflict["reasons"][0]["details"]

    # An accept with a reason puts the add into a decision set of its own,
    # which would bring back the fixture world. Through the full gate it
    # fails G9: the building stands over probe points of a mapped way that
    # are free without it (the case of amendment A4). verify reports it with
    # the gate's details, and every id still matches the plan.
    overrides = tmp_path / "overrides.json"
    overrides.write_bytes(json.dumps({"overrides_version": "0.1.0", "accept": [{
        "layer": "shell", "feature_id": far, "current_manifest_id": None,
        "content_key": conflict["new_content_key"], "lifts": ["removed-by-contribution"],
        "reason": "The building still stands (test)."}]}).encode())
    decided = tmp_path / "decided"
    assert main([*args, "--overrides", str(overrides), "--out", str(decided)]) == 0
    capsys.readouterr()
    report = cjson.loads((decided / REPORT_JSON).read_bytes())
    (s,) = report["sets"]
    assert s["kind"] == "decision" and s["changes"] == 1 and report["final_world_id"] == FIXTURE_WORLD_ID
    suite = tmp_path / "sample-merged.oprep"
    done = rust.replay("rerecord", SAMPLE, "--world", world, "--out", suite)
    assert done.returncode == 0, done.stderr
    code = main(["verify", "--current", str(world), "--plan", str(decided), "--scratch", str(tmp_path / "scratch"),
                 "--replay", str(suite), "--report", str(tmp_path / "verify.json"), "--replan",
                 "--new", str(FIXTURE_WORLD), "--log", str(log), "--overrides", str(overrides),
                 *rust.verify_args()])
    out = capsys.readouterr()
    assert code == 1, out.out + out.err
    summary = cjson.loads((tmp_path / "verify.json").read_bytes())
    (checked,) = summary["sets"]
    assert checked["candidate_world_id"] == checked["world_after"] == FIXTURE_WORLD_ID
    assert summary["final_world_id"] == FIXTURE_WORLD_ID and summary["replan"]["identical"]
    outcomes = gate_outcomes(checked)
    assert (outcomes["G3"], outcomes["G8"], outcomes["G9"], outcomes["G10"]) == ("review", "review", "fail", "pass")
    assert summary["problems"] == ["sets/001: it does not pass the gate: G9 fail"]
    assert any("a public way is newly blocked" in p for p in checked["problems"])
    # The whole-refresh comparison sees the same 3 probe points, for review.
    walkability = summary["comparison"]["walkability"]
    assert (walkability["probes"], len(walkability["newly_blocked"])) == (22, 3)
    assert summary["comparison"]["findings"] == 3
    assert Store(world).read_world()[0] == merged, "verify never writes to the canonical store"


# ================================================== verify's own rules ==


class Fake:
    """`op_replay` and `op_world` in Python, for verify's own rules.

    - `info` reads a fake replay: JSON naming the world it was recorded in.
    - `check-change` builds the candidate with `tools.worldfmt.changes.apply`
      and writes the report of a passing refresh set. `edit(k, report)`
      may change the report on set k (1-based) before it is written.
    - `rerecord` writes a fake replay for the new world.
    - `compare-worlds` writes a report with no finding.
    - `validate` prints the world id, or fails if `invalid` is set.
    `exits` forces a subcommand's exit code. Every call is kept in `calls`.
    """

    def __init__(self) -> None:
        self.calls: list[tuple[str, list[str]]] = []
        self.edit: Callable[[int, dict], dict] | None = None
        self.exits: dict[str, int] = {}
        self.invalid = False
        self.gate_runs = 0

    def __call__(self, command: list[str]) -> subprocess.CompletedProcess:
        tool, sub, *args = command
        self.calls.append((sub, args))
        if sub in self.exits:
            return self.done(command, self.exits[sub], err=f"{sub} failed (fake)")
        handler = {"info": self.info, "check-change": self.check_change, "rerecord": self.rerecord,
                   "compare-worlds": self.compare_worlds, "validate": self.validate}[sub]
        return handler(command, args)

    @staticmethod
    def done(command, code: int, out: str = "", err: str = "") -> subprocess.CompletedProcess:
        return subprocess.CompletedProcess(command, code, out, err)

    @staticmethod
    def option(args: list[str], name: str) -> list[str]:
        return [args[i + 1] for i, a in enumerate(args) if a == name]

    def info(self, command, args):
        world = json.loads(Path(args[0]).read_bytes())["world_id"]
        return self.done(command, 0, f"world      refresh-test ({world})\nseed       1\n")

    def check_change(self, command, args):
        self.gate_runs += 1
        world, bundle = Path(args[0]), Path(args[1])
        wid, doc = Store(world).read_world()
        change_bytes = (bundle / "change.json").read_bytes()
        change = parse(change_bytes)
        candidate = apply(doc, change, Objects(Store(bundle), Store(world)))
        what = [f"{c['tile']} {c['layer']} {c['feature_id']}" for c in change["changes"]]
        outcomes = {"G1": "pass", "G2": "pass", "G4": "not_applicable", "G5": "not_applicable",
                    "G6": "not_applicable", "G7": "pass", "G8": "pass", "G9": "not_applicable",
                    "G10": "not_applicable"}
        names = ["format", "lineage", "layer policy", "licence", "provenance", "privacy", "budgets", "placement",
                 "walkability", "simulation"]
        checks = []
        for n, name in enumerate(names, 1):
            gid = f"G{n}"
            if gid == "G3":
                checks.append({"id": gid, "name": name, "outcome": "review",
                               "details": [f"{w}: baseline refresh by {INGEST_AUTHOR}; a maintainer reviews it"
                                           for w in what]})
            else:
                checks.append({"id": gid, "name": name, "outcome": outcomes[gid], "details": []})
        report = {"format": "streetwide.check", "contract_version": "0.1.0", "gate_version": "0.1.1",
                  "change_id": content_id(change_bytes), "world_id": wid,
                  "candidate_world_id": candidate.world_id, "passed": True, "checks": checks}
        if self.edit is not None:
            report = self.edit(self.gate_runs, report)
        Path(self.option(args, "--report")[0]).write_bytes(cjson.dumps(report))
        return self.done(command, 0 if report["passed"] else 1, "fake gate\n")

    def rerecord(self, command, args):
        old = json.loads(Path(args[0]).read_bytes())["world_id"]
        new = Store(Path(self.option(args, "--world")[0])).read_world()[0]
        Path(self.option(args, "--out")[0]).write_bytes(json.dumps({"world_id": new}).encode())
        return self.done(command, 0, f"rerecorded in world {new} (recorded in {old}): 20 of 20 checkpoints "
                                     "unchanged, final state 00\n")

    def compare_worlds(self, command, args):
        before, after = (Store(Path(a)).read_world()[0] for a in args[:2])
        report = {"report": "streetwide world comparison", "report_version": "0.1.0", "before_world_id": before,
                  "after_world_id": after, "findings": 0}
        Path(self.option(args, "--report")[0]).write_bytes(cjson.dumps(report))
        return self.done(command, 0, f"worlds {before} -> {after}: compared (fake)\n0 findings, for review\n")

    def validate(self, command, args):
        wid = Store(Path(args[0])).read_world()[0]
        errors = 1 if self.invalid else 0
        return self.done(command, 1 if self.invalid else 0, f"world {wid}: 2 tile(s), {errors} error(s)\n",
                         "error: broken (fake)\n" if self.invalid else "")


@dataclass
class FakeRun:
    planned: Planned
    fake: Fake
    replay: Path
    tmp: Path
    runs: int = 0

    def options(self, **kw) -> Options:
        self.runs += 1
        base = dict(current=self.planned.sc.current, plan=self.planned.plan,
                    scratch=self.tmp / f"scratch-{self.runs}", op_replay=("fake-op-replay",),
                    op_world=("fake-op-world",), replays=(self.replay,),
                    report=self.tmp / f"verify-{self.runs}.json")
        base.update(kw)
        return Options(**base)

    def verify(self, **kw) -> tuple[verify_module.Result, list[str]]:
        lines: list[str] = []
        result = verify(self.options(**kw), runner=self.fake, say=lines.append)
        return result, lines


@pytest.fixture
def fake_run(tmp_path: Path, capsys) -> FakeRun:
    planned = plan_two_tiles(tmp_path, capsys)
    replay = tmp_path / "suite.oprep"
    replay.write_bytes(json.dumps({"world_id": Store(planned.sc.current).read_world()[0]}).encode())
    return FakeRun(planned, Fake(), replay, tmp_path)


def test_verify_checks_every_set_in_order_and_moves_the_replays(fake_run: FakeRun) -> None:
    result, lines = fake_run.verify()
    assert result.code == 0, lines
    report = fake_run.planned.report
    calls = fake_run.fake.calls
    assert [sub for sub, _ in calls] == ["info", "check-change", "rerecord", "check-change", "rerecord", "validate",
                                         "compare-worlds"]
    scratch = fake_run.tmp / "scratch-1"
    # Set 1 is checked with the replay as given, set 2 with the one moved to
    # the world after set 1; the comparison gets the original.
    gate1, gate2 = calls[1][1], calls[3][1]
    assert Fake.option(gate1, "--replay") == [str(fake_run.replay.resolve())]
    assert Fake.option(gate2, "--replay") == [str((scratch / "replays" / "001" / "1-suite.oprep").resolve())]
    assert Fake.option(calls[6][1], "--replay") == [str(fake_run.replay.resolve())]
    moved = json.loads((scratch / "replays" / "002" / "1-suite.oprep").read_bytes())
    assert moved["world_id"] == report["final_world_id"]
    assert all(Path(a).is_absolute() for a in (*gate1[:2], *gate2[:2]))
    assert [s["world_after"] for s in result.summary["sets"]] == [s["world_after"] for s in report["sets"]]
    assert result.summary["final_world_id"] == report["final_world_id"]
    assert lines[-1] == "verify: every set passes the gate and every id matches the plan (0 findings for review)."
    assert (scratch / "gate" / "001.json").is_file() and (scratch / "gate" / "002.json").is_file()


def test_the_work_store_links_objects_and_copies_world_json(tmp_path: Path, monkeypatch) -> None:
    current, _ = two_tile_worlds()
    root = tmp_path / "current"
    current.write(root)
    work = tmp_path / "work"
    linked, copied = build_work_store(root, work)
    objects = sorted(p.relative_to(root).as_posix() for p in (root / "objects").rglob("*") if p.is_file())
    assert linked + copied == len(objects) and linked > 0
    assert sorted(p.relative_to(work).as_posix() for p in (work / "objects").rglob("*") if p.is_file()) == objects
    assert os.path.samefile(root / objects[0], work / objects[0]), "objects are hard links"
    assert not os.path.samefile(root / "world.json", work / "world.json"), "world.json is a copy"
    before = (root / "world.json").read_bytes()
    (work / "world.json").write_bytes(b"{}")
    assert (root / "world.json").read_bytes() == before, "rewriting the work world leaves the canonical one"

    def no_links(*_):
        raise OSError("links are not supported here (test)")

    monkeypatch.setattr(verify_module.os, "link", no_links)
    linked, copied = build_work_store(root, tmp_path / "work-2")
    assert (linked, copied) == (0, len(objects))
    assert not os.path.samefile(root / objects[0], tmp_path / "work-2" / objects[0])


def test_a_set_that_fails_the_gate_does_not_stop_the_run(fake_run: FakeRun) -> None:
    def fail_g9(k: int, report: dict) -> dict:
        if k == 1:
            report["checks"][8].update(outcome="fail", details=["17/68647/45903: 3 probe points blocked (fake)"])
            report["passed"] = False
        return report

    fake_run.fake.edit = fail_g9
    result, lines = fake_run.verify()
    assert result.code == 1
    assert fake_run.fake.gate_runs == 2, "the next set is still checked"
    first, second = result.summary["sets"]
    assert not first["passed"] and second["passed"]
    assert first["problems"] == ["it does not pass the gate: G9 fail",
                                 "G9 walkability: 17/68647/45903: 3 probe points blocked (fake)"]
    assert result.problems == ["sets/001: it does not pass the gate: G9 fail"]
    assert [sub for sub, _ in fake_run.fake.calls][-2:] == ["validate", "compare-worlds"], "the end is still checked"
    assert result.summary["comparison"]["findings"] == 0 and not result.summary["passed"]
    assert any("does NOT pass the gate" in line for line in lines)
    assert "    17/68647/45903: 3 probe points blocked (fake)" in lines


def test_a_candidate_that_differs_from_the_plan_stops_the_run(fake_run: FakeRun) -> None:
    other = "blake3:" + "0" * 64

    def wrong(k: int, report: dict) -> dict:
        report["candidate_world_id"] = other
        return report

    fake_run.fake.edit = wrong
    result, _ = fake_run.verify()
    assert result.code == 1 and fake_run.fake.gate_runs == 1
    (s,) = result.summary["sets"]
    assert s["candidate_world_id"] == other and s["world_after"] is None
    assert result.problems == [f"sets/001: the gate's candidate world is {other}, but the plan predicts "
                               f"{s['predicted_world_after']}"]
    assert result.summary["final_world_id"] is None and result.summary["validate"] is None


@pytest.mark.parametrize("edit, expected", [
    (lambda r: r["checks"][2].update(outcome="pass", details=[]),
     "G3 is pass, but a baseline refresh by streetwide:ingest is left to review"),
    (lambda r: r["checks"][2].update(details=["17/68646/45903 shell x: a contribution (fake)"]),
     "G3 does not call every change a baseline refresh by streetwide:ingest"),
    (lambda r: r["checks"][3].update(outcome="pass"),
     "G4 is pass, but it does not apply to a baseline refresh by streetwide:ingest"),
    (lambda r: r["checks"][4].update(outcome="review", details=["x (fake)"]),
     "G5 is review, but it does not apply to a baseline refresh by streetwide:ingest"),
    (lambda r: r.update(change_id="blake3:" + "1" * 64), "the gate read change blake3:" + "1" * 64),
    (lambda r: r.update(world_id="blake3:" + "2" * 64), "the gate ran on world blake3:" + "2" * 64),
])
def test_each_report_check(fake_run: FakeRun, edit, expected: str) -> None:
    def once(k: int, report: dict) -> dict:
        if k == 1:
            edit(report)
        return report

    fake_run.fake.edit = once
    result, _ = fake_run.verify()
    assert result.code == 1
    assert any(p.startswith(f"sets/001: {expected}") for p in result.problems), result.problems


def test_input_errors_exit_2(fake_run: FakeRun, tmp_path: Path) -> None:
    p = fake_run.planned
    with pytest.raises(VerifyError, match="exists and is not empty"):
        fake_run.verify(scratch=p.new)
    with pytest.raises(VerifyError, match="--scratch must not be inside --plan"):
        fake_run.verify(scratch=p.plan / "sets")
    with pytest.raises(VerifyError, match="--scratch must not be inside --current"):
        fake_run.verify(scratch=p.sc.current / "scratch")
    with pytest.raises(VerifyError, match="--report must not be inside --plan"):
        fake_run.verify(report=p.plan / "verify.json")
    # The work store links the canonical objects: a summary written over
    # one would change the canonical store.
    work_object = tmp_path / "s" / "work" / "objects" / "3f" / f"3f{'0' * 62}.glb"
    with pytest.raises(VerifyError, match="--report must not be inside --scratch"):
        fake_run.verify(scratch=tmp_path / "s", report=work_object)
    with pytest.raises(VerifyError, match="holds no plan"):
        fake_run.verify(plan=tmp_path)
    with pytest.raises(VerifyError, match="no such file"):
        fake_run.verify(replays=(tmp_path / "missing.oprep",))

    stale = tmp_path / "stale.oprep"
    stale.write_bytes(json.dumps({"world_id": "blake3:" + "3" * 64}).encode())
    with pytest.raises(VerifyError, match="so G10 would ignore it"):
        fake_run.verify(replays=(stale,))

    fake_run.fake.exits["check-change"] = 2
    with pytest.raises(VerifyError, match="check-change exited with 2: check-change failed"):
        fake_run.verify()
    fake_run.fake.exits.clear()

    # The canonical world moved after planning: plan again.
    moved = two_tile_worlds()[0].drop(shell(7, A, 0, 1, 0, 1).key)
    other = tmp_path / "moved"
    moved.write(other)
    with pytest.raises(VerifyError, match="the plan was made on world .*, but --current is world .*; plan the"):
        fake_run.verify(current=other)


def test_the_summary_replaces_a_link_instead_of_writing_through_it(fake_run: FakeRun, tmp_path: Path) -> None:
    kept = tmp_path / "elsewhere.bin"
    kept.write_bytes(b"another file's bytes")
    report = tmp_path / "linked.json"
    os.link(kept, report)
    result, _ = fake_run.verify(report=report)
    assert result.code == 0
    assert kept.read_bytes() == b"another file's bytes"
    assert cjson.loads(report.read_bytes())["passed"] is True
    assert not report.with_name("linked.json.tmp").exists()


def test_the_cli_refuses_bad_options(fake_run: FakeRun, capsys) -> None:
    p = fake_run.planned
    base = ["verify", "--current", str(p.sc.current), "--plan", str(p.plan), "--scratch", str(fake_run.tmp / "s")]
    assert main(base + ["--new", str(p.new)]) == 2
    assert "error: --new, --log and --overrides go with --replan" in capsys.readouterr().err
    assert main(base + ["--replan", "--new", str(p.new)]) == 2
    assert "error: --replan needs --new and --log" in capsys.readouterr().err
    assert main(base + ["--op-replay", str(fake_run.tmp / "no-such-tool")]) == 2
    assert "error: cannot run" in capsys.readouterr().err
    assert main(["verify", "--plan", str(p.plan)]) == 2
    capsys.readouterr()


def test_a_failing_validation_exits_1(fake_run: FakeRun) -> None:
    fake_run.fake.invalid = True
    result, lines = fake_run.verify()
    assert result.code == 1
    assert result.summary["validate"] == {"passed": False, "summary": result.summary["validate"]["summary"]}
    assert result.problems == [f"the final world does not validate: {result.summary['validate']['summary']}"]
    assert "  error: broken (fake)" in lines


def test_a_bundle_object_must_hash_to_its_name(fake_run: FakeRun) -> None:
    bundle = fake_run.planned.plan / "sets" / "002"
    glb_file = sorted((bundle / "objects").rglob("*.glb"))[0]
    glb_file.write_bytes(glb_file.read_bytes() + b"\0")
    result, _ = fake_run.verify()
    assert result.code == 1
    rel = glb_file.relative_to(bundle).as_posix()
    assert result.problems == [f"sets/002: {rel} does not hash to its name"]


def test_the_summary_is_the_same_for_the_same_inputs(fake_run: FakeRun) -> None:
    first, _ = fake_run.verify()
    second, _ = fake_run.verify(compare=False)
    third, _ = fake_run.verify()
    a, c = ((fake_run.tmp / f"verify-{k}.json").read_bytes() for k in (1, 3))
    assert a == c and first.code == third.code == 0
    assert b"\\" not in a and str(fake_run.tmp).encode() not in a
    assert cjson.loads(a)["comparison"]["findings"] == 0
    assert second.summary["comparison"] is None and second.code == 0


def test_replan_compares_every_file(fake_run: FakeRun, tmp_path: Path) -> None:
    p = fake_run.planned
    replan = Replan(p.new, p.sc.log.root)
    result, lines = fake_run.verify(replan=replan)
    assert result.code == 0 and result.summary["replan"]["identical"], lines
    assert files_under(fake_run.tmp / "scratch-1" / "replan") == files_under(p.plan)

    # A changed report and an extra file in a bundle are differences.
    md = p.plan / REPORT_MD
    md.write_bytes(md.read_bytes() + b"edited\n")
    (p.plan / "sets" / "001" / "notes.txt").write_bytes(b"x")
    (p.plan / "gate-001.json").write_bytes(b"{}")      # not part of the plan: ignored
    result, lines = fake_run.verify(replan=replan)
    assert result.code == 1
    assert result.summary["replan"]["differences"] == [f"{REPORT_MD} differs",
                                                       "sets/001/notes.txt was not made by plan"]
    assert result.problems[0] == "the plan made again differs: 2 files"
    assert fake_run.fake.gate_runs == 4, "the sets are still checked"

    # The inputs must be the plan's own.
    overrides = tmp_path / "overrides.json"
    overrides.write_bytes(b'{"overrides_version": "0.1.0"}')
    with pytest.raises(VerifyError, match="made with no overrides file, but --overrides gives blake3:"):
        fake_run.verify(replan=Replan(p.new, p.sc.log.root, overrides))
    with pytest.raises(VerifyError, match="the plan was made from the new store"):
        fake_run.verify(replan=Replan(p.sc.current, p.sc.log.root))


def test_plan_differences_names_missing_and_extra_files(tmp_path: Path) -> None:
    (tmp_path / "sets" / "001").mkdir(parents=True)
    (tmp_path / REPORT_JSON).write_bytes(b"a")
    (tmp_path / "sets" / "001" / "change.json").write_bytes(b"b")
    (tmp_path / "sets" / "001" / "stray.json").write_bytes(b"c")
    (tmp_path / "README.txt").write_bytes(b"d")
    files = {REPORT_JSON: b"a", REPORT_MD: b"m", "sets/001/change.json": b"B"}
    assert plan_differences(tmp_path, files) == [
        f"{REPORT_MD} is missing from the plan", "sets/001/change.json differs",
        "sets/001/stray.json was not made by plan",
    ]


def test_commands_split_and_join_as_the_platform_does() -> None:
    assert split_command(verify_module.DEFAULT_OP_REPLAY) == ["cargo", "run", "-q", "-p", "op_replay", "--locked",
                                                               "--"]
    path = r"C:\Program Files\StreetWide\op_replay.exe" if os.name == "nt" else "/opt/street wide/op_replay"
    words = [path, "--flag"]
    assert split_command(join_command(words)) == words
    if os.name == "nt":
        assert split_command(r"target\debug\op_replay.exe check") == [r"target\debug\op_replay.exe", "check"]


def test_every_written_file_has_lf_only(fake_run: FakeRun) -> None:
    result, _ = fake_run.verify(replan=Replan(fake_run.planned.new, fake_run.planned.sc.log.root))
    assert result.code == 0
    written = [fake_run.tmp / "verify-1.json", *(fake_run.tmp / "scratch-1" / "replan").rglob("*.json"),
               *(fake_run.tmp / "scratch-1" / "replan").rglob("*.md")]
    for path in written:
        assert b"\r" not in path.read_bytes(), path
