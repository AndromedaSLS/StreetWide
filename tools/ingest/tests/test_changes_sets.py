"""Packing, bundles, the chain of world ids and the plan-time checks of the
refresh planner (ADR-0020, section 5)."""

from __future__ import annotations

import dataclasses
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

from tools.ingest.changes import checks, plan as plan_module
from tools.ingest.changes.__main__ import main
from tools.ingest.changes.classify import classify
from tools.ingest.changes.common import InternalError, PlanError
from tools.ingest.changes.holds import apply_holds
from tools.ingest.changes.overrides import NONE
from tools.ingest.changes.sets import CHANGE_JSON_RESERVE
from tools.ingest.changes.stores import read_store
from tools.ingest.tests.refresh_worlds import (
    A, B, C, D, GROUND_MM, OSM_V2, Scenario, World, box_mesh, contributed, entry_replace, grid_mesh, held,
    moved_box, osm_shell, ribbon, swiss, swiss_shell, terrain, trees,
)
from tools.worldfmt import cjson
from tools.worldfmt.changes import MAX_CHANGES, MAX_TEXT_CHARS, Objects, apply, parse, problems
from tools.worldfmt.ids import content_id
from tools.worldfmt.log import read_log
from tools.worldfmt.store import Store

TILES = (A, B, C, D)
REPO = Path(__file__).resolve().parents[3]


def grid_of_shells(first: int, per_tile: int, *, year: str = "2025") -> list:
    out = []
    for t, tile in enumerate(TILES):
        for k in range(per_tile):
            n = first + t * per_tile + k
            x, z = (k % 10) * 15_000, 20_000 + (k // 10) * 15_000
            out.append(swiss_shell(n, tile, lo=(x, GROUND_MM, z), hi=(x + 10_000, GROUND_MM + 6_000, z + 10_000), year=year))
    return out


def base(per_tile: int = 30) -> World:
    return World.of(*[terrain(t) for t in TILES], *grid_of_shells(1000, per_tile))


def test_split_cap_purpose_and_order(tmp_path: Path) -> None:
    osm = [osm_shell(w, A, lo=(w * 1_000, GROUND_MM, 0), hi=(w * 1_000 + 500, GROUND_MM + 3_000, 500)) for w in range(1, 6)]
    ribbons = [ribbon(t) for t in (A, B, C)]
    sc = Scenario.start(tmp_path, base().put(*osm, *ribbons))
    new = World.of(*[terrain(t) for t in TILES],
                   *[moved_box(s, dy=100) for s in grid_of_shells(1000, 30, year="2026")],
                   *[moved_box(osm_shell(w, A, lo=(w * 1_000, GROUND_MM, 0), hi=(w * 1_000 + 500, GROUND_MM + 3_000, 500),
                                         version=OSM_V2), dy=100) for w in range(1, 6)],
                   *[moved_box(ribbon(t, version=OSM_V2), dy=10, material="road") for t in (A, B, C)])
    plan = sc.plan(new)
    assert [(s["purpose"]["collider"], s["purpose"]["layer"], s["purpose"]["dataset"], s["changes"])
            for s in plan.report["sets"]] == [
        ("none", "transport", "OpenStreetMap", 3),
        ("trimesh", "shell", "OpenStreetMap", 5),
        ("trimesh", "shell", "swissBUILDINGS3D 3.0 Beta", 50),
        ("trimesh", "shell", "swissBUILDINGS3D 3.0 Beta", 50),
        ("trimesh", "shell", "swissBUILDINGS3D 3.0 Beta", 20),
    ]
    # Tile keys sort as strings: 17/68646/45903 (A), 17/68646/45904 (C), 17/68647/45903 (B), 17/68647/45904 (D).
    tiles = [[e["tile"] for e in s.entries] for s in plan.sets[2:]]
    assert tiles[0] == [A] * 30 + [C] * 20 and tiles[1] == [C] * 10 + [B] * 30 + [D] * 10 and tiles[2] == [D] * 20
    for s in plan.sets:
        assert [e["tile"] for e in s.entries] == sorted(e["tile"] for e in s.entries)
        assert s.change["title"].startswith("Baseline refresh refresh-test: ")
    assert plan.sets[3].change["title"] == ("Baseline refresh refresh-test: shell from swissBUILDINGS3D 3.0 Beta, "
                                            "with colliders (set 4 of 5)")
    assert [s["touches_colliders"] for s in plan.report["sets"]] == [False, True, True, True, True]


def test_points_assets_are_planned_as_assets_of_their_layers(tmp_path: Path) -> None:
    # RFC 0004: a points asset is one asset per tile and source, in its
    # layer, with a points document as content; the planner adds, replaces
    # and removes it like any other per-tile asset.
    current = World.of(terrain(A), terrain(B), terrain(C), trees(A), trees(C, first=201))
    sc = Scenario.start(tmp_path, current)
    # A new version of the register: the replaced trees say why they changed.
    new = World.of(terrain(A), terrain(B), terrain(C), trees(A, genus="Acer", version="2026-11-29"),
                   trees(B, first=101, version="2026-11-29"))
    plan = sc.plan(new)
    # Removing a tile's trees is held like a tile's terrain or water.
    assert held(plan) == {("vegetation", f"tile:{C}/stadt-zuerich:baumkataster"): ["per-tile-removal"]}
    (s,) = plan.sets
    assert [(e["op"], e["tile"], e["layer"]) for e in s.entries] == [("replace", A, "vegetation"),
                                                                     ("add", B, "vegetation")]
    assert plan.report["sets"][0]["purpose"] == {"collider": "none", "layer": "vegetation", "dataset": "Baumkataster"}
    assert not s.touches_colliders
    # The bundle holds both points documents as .json content, and the
    # chain of world ids is the one `apply` gives.
    for asset in (new[("vegetation", f"tile:{A}/stadt-zuerich:baumkataster")],
                  new[("vegetation", f"tile:{B}/stadt-zuerich:baumkataster")]):
        h = asset.content_id[7:]
        assert plan.files[f"{s.path}/objects/{h[:2]}/{h}.json"] == asset.mesh.data
        assert f"{s.path}/objects/{h[:2]}/{h}.glb" not in plan.files
    store = Store(sc.current)
    world_id, world = store.read_world()
    objects = Objects(store)
    objects.push(s.manifests)
    assert problems(s.change) == []
    assert apply(world, s.change, objects).world_id == s.world_after
    assert s.base_world_id == world_id


def test_a_points_document_must_agree_with_its_manifest(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, World.of(terrain(A)))
    good = trees(A)
    # A manifest whose bounds are not its anchors': the gate could not read
    # it, so the plan stops.
    wrong = good.but(mesh=dataclasses.replace(good.mesh, bounds={"max": [1, 1, 1], "min": [0, 0, 0]}))
    with pytest.raises(PlanError, match="bounds_mm are not the exact bounds of the anchors"):
        sc.plan(World.of(terrain(A), wrong))


def test_split_byte_limit(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, base(per_tile=1))
    adds = [swiss_shell(2000 + k, A, lo=(k * 9_000, GROUND_MM, 120_000), hi=(k * 9_000 + 5_000, GROUND_MM + 4_000, 125_000))
            for k in range(20)]
    fid = f"swisstopo:swissbuildings3d:big"
    heights = [[GROUND_MM + (i * 7 + j * 3) % 500 for i in range(65)] for j in range(65)]
    big = swiss_shell(3000, B).but(feature_id=fid, mesh=grid_mesh(fid, heights, x0=0, z0=150_000, size=40_000,
                                                                     material="wall"))
    limit = CHANGE_JSON_RESERVE + 12_000
    plan = sc.plan(base(per_tile=1).put(*adds, big), max_set_bytes=limit)
    sizes = [(s["bytes"], s["changes"], s["warnings"]) for s in plan.report["sets"]]
    assert len(sizes) > 3
    for (size, n, warnings), s in zip(sizes, plan.sets):
        assert size == len(s.change_bytes) + sum(len(b) for b in s.objects.values())
        if warnings:
            assert n == 1 and size > limit and warnings[0].startswith("one unit of about ")
        else:
            assert size <= limit
    assert sum(1 for _, _, w in sizes if w) == 1
    assert sum(n for _, n, _ in sizes) == 21
    assert plan.report["max_set_bytes"] == limit
    assert b"**Warning:** sets/" in plan.markdown
    # Without the limit, one set.
    assert len(sc.plan(base(per_tile=1).put(*adds, big)).sets) == 1


def test_chain_ids_and_residual(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, base())
    # Adds and removals only: the final world is the new store exactly.
    new = base().drop(*[s.key for s in grid_of_shells(1000, 30)[:2]])
    new.put(*[swiss_shell(4000 + k, D, lo=(k * 2_000, GROUND_MM, 180_000),
                          hi=(k * 2_000 + 1_000, GROUND_MM + 3_000, 181_000)) for k in range(70)])
    new_root = sc.write_new(new)
    plan = sc.plan(new_root)
    assert [s["changes"] for s in plan.report["sets"]] == [50, 22]
    current_id = Store(sc.current).read_world()[0]
    assert plan.report["sets"][0]["base_world_id"] == current_id
    assert plan.report["sets"][1]["base_world_id"] == plan.report["sets"][0]["world_after"]
    assert plan.report["final_world_id"] == plan.report["sets"][1]["world_after"] == Store(new_root).read_world()[0]
    current_world = Store(sc.current).read_world()[1]
    at_once = apply(current_world, {"changes": [e for s in plan.sets for e in s.entries]},
                    Objects({cid: b for s in plan.sets for cid, b in s.manifests.items()}, Store(sc.current)))
    assert at_once.world_id == plan.report["final_world_id"], "one-shot equals chained"

    # Merging the bundles in order, as op_world would, gives the predicted ids.
    work = tmp_path / "work"
    shutil.copytree(sc.current, work)
    store = Store(work)
    for s in plan.sets:
        world_id, world = store.read_world()
        assert world_id == s.change["base_world_id"]
        for path, data in s.objects.items():
            store.write_object(data, path.rsplit(".", 1)[1])
        candidate = apply(world, parse(s.change_bytes), Objects(store))
        assert candidate.write(store) == s.world_after
    assert store.read_world()[0] == plan.report["final_world_id"]


def test_the_residual_must_be_explained(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, base())
    new_root = sc.write_new(base().put(moved_box(grid_of_shells(1000, 30, year="2026")[0], dy=100)))
    cur, new = read_store(sc.current, "current"), read_store(new_root, "new")
    cls = classify(cur, new, read_log(sc.log.root))
    apply_holds(cls, cur, new, NONE)
    (item,) = cls.items
    # As if the set were lost: the final world is the current one.
    with pytest.raises(InternalError) as e:
        checks.check_residual(cur, new, cls, cur.world, Objects(Store(sc.current)))
    assert str(e.value) == f"internal error: shell {item.feature_id} differs from the new store with no recorded reason"


def test_an_internal_error_writes_nothing(tmp_path: Path, monkeypatch, capsys) -> None:
    sc = Scenario.start(tmp_path, base())
    args = sc.args(base().put(moved_box(grid_of_shells(1000, 30, year="2026")[0], dy=100)))
    real_chain = plan_module.chain

    def lossy_chain(sets, cur, holds, ctx):
        real_chain(sets, cur, holds, ctx)
        return cur.world_id, cur.world, Objects(Store(cur.root))

    monkeypatch.setattr(plan_module, "chain", lossy_chain)
    assert main(args) == 1
    out, err = capsys.readouterr()
    assert out == "" and err.startswith("internal error: the last set gives blake3:")
    assert not Path(args[args.index("--out") + 1]).exists()


def test_a_plan_that_cannot_be_written_leaves_nothing(tmp_path: Path, monkeypatch, capsys) -> None:
    # As on Windows when a path passes 260 characters: the first manifest
    # of a bundle cannot be written.
    sc = Scenario.start(tmp_path, base())
    args = sc.args(base().put(moved_box(grid_of_shells(1000, 30, year="2026")[0], dy=100)))
    out = Path(args[args.index("--out") + 1])
    real = Path.write_bytes
    written = []

    def write_bytes(self: Path, data: bytes) -> int:
        if self.suffix == ".json" and self.parent.parent.name == "objects":
            raise FileNotFoundError(2, "The system cannot find the path specified")
        written.append(self.name)
        return real(self, data)

    monkeypatch.setattr(Path, "write_bytes", write_bytes)
    for existing in (False, True):
        if existing:
            out.mkdir()  # an empty --out stays, empty
        assert main(args) == 2
        _, err = capsys.readouterr()
        assert err.startswith("error: cannot write sets/001/objects/") and err.endswith(
            ".json: The system cannot find the path specified; the files written so far were removed\n"), err
        assert written and "refresh-report.json" not in written, "the report comes last"
        assert out.is_dir() == existing and (not existing or not any(out.iterdir()))
        assert not any(p.name == "sets" for p in tmp_path.rglob("sets"))
    monkeypatch.undo()
    assert main(args) == 0, "planning again works"
    capsys.readouterr()


def scenario_with_everything(tmp_path: Path) -> tuple[Scenario, list[str]]:
    sc = Scenario.start(tmp_path, base())
    shell = grid_of_shells(1000, 30)[5]
    mine = contributed(shell, parent=shell.manifest_id,
                       mesh=box_mesh(shell.feature_id, shell.mesh.bounds["min"],
                                     [v + 300 for v in shell.mesh.bounds["max"]]))
    sc.log.merge(sc.current, [entry_replace(mine)], [mine])
    new = base()
    new.put(*[moved_box(s, dy=100) for s in grid_of_shells(1000, 30, year="2026")[:60]])
    new.put(moved_box(grid_of_shells(1000, 30)[70], dy=50))  # unexplained
    new.drop(grid_of_shells(1000, 30)[80].key)
    accept = {"layer": "shell", "feature_id": shell.feature_id, "current_manifest_id": mine.manifest_id,
              "content_key": None, "lifts": ["contribution"], "reason": "Stale accept, for the report."}
    return sc, sc.args(new, overrides={"accept": [accept]})


def test_rerun_is_byte_identical(tmp_path: Path, capsys) -> None:
    # Once in this process, and twice in fresh processes with other hash
    # seeds, so that no output depends on the order of a set of strings.
    sc, args = scenario_with_everything(tmp_path)
    outs = [tmp_path / name for name in ("one", "two", "three")]
    a = list(args)
    a[a.index("--out") + 1] = str(outs[0])
    assert main(a) == 0
    capsys.readouterr()
    for seed, out in (("1", outs[1]), ("2", outs[2])):
        b = list(args)
        b[b.index("--out") + 1] = str(out)
        done = subprocess.run([sys.executable, "-m", "tools.ingest.changes", *b], cwd=REPO, capture_output=True,
                              text=True, timeout=300, env={**os.environ, "PYTHONHASHSEED": seed})
        assert done.returncode == 0, done.stderr
    trees = [sorted(p.relative_to(out).as_posix() for p in out.rglob("*") if p.is_file()) for out in outs]
    assert trees[0] == trees[1] == trees[2] and len(trees[0]) > 10
    for rel in trees[0]:
        assert (outs[0] / rel).read_bytes() == (outs[1] / rel).read_bytes() == (outs[2] / rel).read_bytes(), rel


def test_output_paths_and_line_endings(tmp_path: Path, capsys) -> None:
    sc, args = scenario_with_everything(tmp_path)
    out = Path(args[args.index("--out") + 1])
    assert main(args) == 0
    capsys.readouterr()
    report = cjson.loads((out / "refresh-report.json").read_bytes())
    assert report["conflicts"] and report["held_back"] and len(report["sets"]) == 2
    assert report["stale_overrides"][0]["why"] == "content_key no longer matches"
    roots = {str(tmp_path), str(tmp_path).replace("\\", "/"), tmp_path.as_posix(), str(Path.home())}
    for path in out.rglob("*"):
        if not path.is_file() or path.suffix not in (".json", ".md"):
            continue
        data = path.read_bytes()
        assert b"\r" not in data and b"\\" not in data, path
        text = data.decode("utf-8")
        for root in roots:
            assert root not in text, (path, root)
    for s in report["sets"]:
        assert s["path"] in ("sets/001", "sets/002") and "\\" not in s["path"]
    md = (out / "refresh-report.md").read_text(encoding="utf-8")
    assert "<canonical>" in md and "--require-base --log <log>" in md and "<plan>/sets/001" in md


def test_titles_and_rationales_stay_text(tmp_path: Path) -> None:
    sc = Scenario.start(tmp_path, base())
    shell = grid_of_shells(1000, 30)[3]
    mine = contributed(shell, "github:" + "a" * 30, parent=shell.manifest_id,
                       mesh=box_mesh(shell.feature_id, shell.mesh.bounds["min"],
                                     [v + 300 for v in shell.mesh.bounds["max"]]))
    sc.log.merge(sc.current, [entry_replace(mine)], [mine], author="github:" + "a" * 30)
    # Twelve source years in one set, and an accept with a very long reason.
    new = base()
    years = [str(2000 + k) for k in range(12)]
    new.put(*[moved_box(s.but(sources=(swiss(s.sources[0]["source_ids"][0], years[k]),)), dy=100)
              for k, s in enumerate(grid_of_shells(1000, 30)[10:22])])
    upstream = moved_box(grid_of_shells(1000, 30, year="2026")[3], dy=100)
    new.put(upstream)
    plan = sc.plan(new)
    (e,) = plan.report["conflicts"]
    plan = sc.plan(new, overrides={"accept": [{
        "layer": "shell", "feature_id": shell.feature_id, "current_manifest_id": mine.manifest_id,
        "content_key": e["new_content_key"], "lifts": ["contribution"], "reason": "Long reason. " * 315}]})
    for s in plan.sets:
        assert problems(s.change) == []
        assert len(s.change["title"]) <= MAX_TEXT_CHARS and len(s.change["rationale"]) <= MAX_TEXT_CHARS
        assert "\n" not in s.change["rationale"]
    ordinary, decision = plan.sets
    assert "swissBUILDINGS3D 3.0 Beta 2000, swissBUILDINGS3D 3.0 Beta 2001" in ordinary.change["rationale"]
    assert "and 4 more" in ordinary.change["rationale"]
    assert ordinary.change["rationale"].startswith("Made by tools.ingest.changes 0.1.0 under RFC 0002 section 7 ")
    assert len(decision.change["rationale"]) == MAX_TEXT_CHARS and decision.change["rationale"].endswith('...".')
    assert decision.change["author"] == "streetwide:ingest"
    assert len(ordinary.entries) <= MAX_CHANGES
    assert content_id(ordinary.change_bytes) == ordinary.change_id
