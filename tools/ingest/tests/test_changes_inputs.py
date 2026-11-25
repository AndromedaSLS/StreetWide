"""The refresh planner's command line, inputs, world log and overrides file
(ADR-0020, sections 2, 4 and 6): every input error exits 2
with one line naming the problem, and nothing is written."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

from tools.ingest.changes.__main__ import main
from tools.ingest.tests.refresh_worlds import (
    A, B, GROUND_MM, ORIGINS, Mesh, Scenario, World, contributed, contribution_source, entry_remove, entry_replace,
    ribbon, swiss_shell, terrain,
)
from tools.worldfmt import cjson, log as worldlog
from tools.worldfmt.ids import content_id
from tools.worldfmt.store import Store

REPO = Path(__file__).resolve().parents[3]
BASELINE_HINT = ("for a world that has never had a merge or a takedown, write its baseline record first: "
                 "op_world baseline <canonical> --log <log> --reason \"...\"")


def world() -> World:
    return World.of(terrain(A), terrain(B), ribbon(A), swiss_shell(1), swiss_shell(2, B))


def run(capsys, args: list[str]) -> tuple[int, str, str]:
    code = main(args)
    out, err = capsys.readouterr()
    return code, out, err


def refused(capsys, args: list[str], message: str) -> None:
    code, out, err = run(capsys, args)
    assert (code, out) == (2, ""), err
    assert err == f"error: {message}\n"


def test_plan_writes_its_files(tmp_path: Path, capsys) -> None:
    sc = Scenario.start(tmp_path, world())
    out = tmp_path / "plan"
    out.mkdir()  # an empty directory is fine
    added = swiss_shell(3, lo=(60_000, GROUND_MM, 60_000), hi=(70_000, GROUND_MM + 6_000, 70_000))
    code, stdout, err = run(capsys, sc.args(world().put(added), out))
    assert (code, err) == (0, "")
    lines = stdout.splitlines()
    assert lines[0] == "1 change set with 1 change; 0 conflicts and 0 held-back changes listed in the report " \
                       "(0 provenance-only differences kept)"
    assert lines[1].startswith(f"report: {(out / 'refresh-report.md').as_posix()} (content id of the JSON: blake3:")
    written = sorted(p.relative_to(out).as_posix() for p in out.rglob("*") if p.is_file())
    assert written == sorted(["refresh-report.json", "refresh-report.md", "sets/001/change.json",
                              f"sets/001/objects/{added.manifest_id[7:9]}/{added.manifest_id[7:]}.json",
                              f"sets/001/objects/{added.content_id[7:9]}/{added.content_id[7:]}.glb"])


def test_a_path_the_console_cannot_show_is_escaped(tmp_path: Path) -> None:
    # Piped output on Windows uses the ANSI code page: the summary that
    # names --out must not fail once the plan is written.
    sc = Scenario.start(tmp_path, world())
    out = tmp_path / "plan-日本"
    done = subprocess.run([sys.executable, "-m", "tools.ingest.changes", *sc.args(world(), out)], cwd=REPO,
                          capture_output=True, timeout=300, env={**os.environ, "PYTHONIOENCODING": "cp1252"})
    assert done.returncode == 0, done.stderr
    assert b"plan-\\u65e5\\u672c/refresh-report.md" in done.stdout
    assert (out / "refresh-report.json").is_file()


def test_nothing_to_refresh_writes_only_the_report(tmp_path: Path, capsys) -> None:
    sc = Scenario.start(tmp_path, world())
    out = tmp_path / "plan"
    code, stdout, _ = run(capsys, sc.args(world(), out))
    assert code == 0 and stdout.splitlines()[0] == "nothing to refresh: 0 changes (0 provenance-only differences kept)"
    assert sorted(p.name for p in out.iterdir()) == ["refresh-report.json", "refresh-report.md"]


def test_log_required(tmp_path: Path, capsys) -> None:
    sc = Scenario.start(tmp_path, world())
    args = sc.args(world())
    i = args.index("--log")
    refused(capsys, args[:i] + args[i + 2:], f"--log is required; {BASELINE_HINT}")
    empty = tmp_path / "empty-log"
    empty.mkdir()
    args[i + 1] = str(empty)
    refused(capsys, args, f"the world log has no records: log/00000000.json, the baseline record, is missing; "
                          f"{BASELINE_HINT}")
    assert not (tmp_path / "out-1").exists() and not (tmp_path / "out-2").exists()


def test_log_must_end_at_current(tmp_path: Path, capsys) -> None:
    sc = Scenario.start(tmp_path, world())
    before = content_id((sc.current / "world.json").read_bytes())
    # The store moves on without the log: a merge that was not logged.
    world().drop(swiss_shell(2, B).key).write(tmp_path / "other")
    (sc.current / "world.json").write_bytes((tmp_path / "other" / "world.json").read_bytes())
    for path in (tmp_path / "other" / "objects").rglob("*.json"):
        Store(sc.current).write_object(path.read_bytes(), "json")
    after = content_id((sc.current / "world.json").read_bytes())
    refused(capsys, sc.args(world()), f"the world log ends at world {before}, but --current is world {after}")


def test_log_change_set_missing(tmp_path: Path, capsys) -> None:
    sc = Scenario.start(tmp_path, world())
    sc.log.merge(sc.current, [entry_remove(swiss_shell(2, B))], author="github:bob")
    change_id = sc.log.records[1][1]["change_id"]
    (sc.log.root / worldlog.change_path(change_id)).unlink()
    refused(capsys, sc.args(world()), f"the world log names change set {change_id} (log/00000001.json) but "
                                      f"{worldlog.change_path(change_id)} is missing or has another id")


def test_log_chain_is_checked(tmp_path: Path, capsys) -> None:
    sc = Scenario.start(tmp_path, world())
    sc.log.merge(sc.current, [entry_remove(swiss_shell(2, B))], author="github:bob")
    path = sc.log.root / "log" / "00000001.json"
    record = cjson.loads(path.read_bytes())
    path.write_bytes(cjson.dumps(dict(record, world_before=content_id(b"elsewhere"))))
    refused(capsys, sc.args(world()), f"world log entry 1 ({content_id(path.read_bytes())}): world_before is not "
                                      "the previous record's world_after")


def test_new_store_older_or_mixed_generator_refused(tmp_path: Path, capsys) -> None:
    v2 = ("streetwide-ingest", "0.2.0")
    sc = Scenario.start(tmp_path, World.of(*[a.but(generator=v2) for a in world().assets.values()]))
    refused(capsys, sc.args(world()), "the new store was made by streetwide-ingest 0.1.0, older than 0.2.0, which "
                                      "made current assets; ingest again with the current tools/ingest")
    mixed = world().put(swiss_shell(1).but(generator=v2))
    refused(capsys, sc.args(mixed), "the new store mixes generator versions (streetwide-ingest 0.1.0, "
                                    "streetwide-ingest 0.2.0)")
    newer = World.of(*[a.but(generator=("streetwide-ingest", "0.10.0")) for a in world().assets.values()])
    assert run(capsys, sc.args(newer))[0] == 0, "0.10.0 is newer than 0.2.0"


def test_only_assets_made_by_the_ingest_date_it(tmp_path: Path, capsys) -> None:
    # A merged contribution that names the ingest's generator, with a higher
    # version: G5 does not check generators, so it must not stop refreshes.
    sc = Scenario.start(tmp_path, world())
    shell = swiss_shell(1)
    mine = contributed(shell, parent=shell.manifest_id).but(generator=("streetwide-ingest", "9.9.9"))
    sc.log.merge(sc.current, [entry_replace(mine)], [mine])
    code, _, err = run(capsys, sc.args(world()))
    assert (code, err) == (0, "")
    # A digit that is not ASCII is text in a version, not a number.
    odd = World.of(*[a.but(generator=("streetwide-ingest", "1²")) for a in world().assets.values()])
    code, _, err = run(capsys, sc.args(odd))
    assert (code, err) == (0, "")


def test_new_store_must_be_a_fresh_ingest(tmp_path: Path, capsys) -> None:
    sc = Scenario.start(tmp_path, world())
    shell = swiss_shell(1)
    cases = [
        (shell.but(parent=content_id(b"x")), "has a parent version"),
        (shell.but(generator=("streetwide-contrib", "0.1.0")), "was made by streetwide-contrib"),
        (shell.but(sources=(contribution_source("github:alice"),) + shell.sources,
                   license="CC-BY-SA-4.0 AND LicenseRef-swisstopo-OGD"),
         'lists a "StreetWide contributions" source'),
    ]
    for asset, why in cases:
        refused(capsys, sc.args(world().put(asset)), f"the new store is not a fresh ingest: shell {shell.feature_id} {why}")


def test_stores_must_be_two_builds_of_one_world(tmp_path: Path, capsys) -> None:
    sc = Scenario.start(tmp_path, world())
    other = world()
    other.name = "grossmuenster"
    refused(capsys, sc.args(other), 'the current store is world "refresh-test" but the new store is world '
                                    '"grossmuenster"; a refresh compares two builds of the same world definition')

    def rewrite(edit) -> Path:
        root = sc.write_new(world())
        store = Store(root)
        doc = cjson.loads((root / "world.json").read_bytes())
        edit(doc, store)
        store.write_world(doc)
        return root

    def region(doc, store) -> None:
        doc["region"] = {"key": "12/2146/1434", "anchor_e9": doc["region"]["anchor_e9"]}

    refused(capsys, sc.args(rewrite(region)), "the stores use different regions (12/2145/1434, 12/2146/1434)")

    def origin(doc, store) -> None:
        wt = doc["tiles"][0]
        index = cjson.loads(store.read_object(wt["tile_id"], "json"))
        index["origin_mm"] = [1, 2, 3]
        wt["tile_id"] = store.write_json(index)
        wt["origin_mm"] = [1, 2, 3]

    refused(capsys, sc.args(rewrite(origin)), f"tile {A} has origin_mm {list(ORIGINS[A])} in the current store and "
                                              "[1, 2, 3] in the new store")

    def twice(doc, store) -> None:
        first = cjson.loads(store.read_object(doc["tiles"][0]["tile_id"], "json"))
        second = cjson.loads(store.read_object(doc["tiles"][1]["tile_id"], "json"))
        shell = next(a for a in first["assets"] if a["layer"] == "shell")
        second["assets"] = sorted(second["assets"] + [shell], key=lambda a: (a["layer"], a["feature_id"]))
        doc["tiles"][1]["tile_id"] = store.write_json(second)

    refused(capsys, sc.args(rewrite(twice)), f"the new store lists shell {swiss_shell(1).feature_id} twice")


def test_a_broken_object_names_its_id(tmp_path: Path, capsys) -> None:
    sc = Scenario.start(tmp_path, world())
    root = sc.write_new(world())
    shell = swiss_shell(1)
    path = Store(root).object_path(shell.manifest_id, "json")
    path.write_bytes(path.read_bytes() + b" ")
    code, _, err = run(capsys, sc.args(root))
    assert code == 2
    assert err.startswith(f"error: the new store: tile {A}, shell {shell.feature_id}: manifest: object "
                          f"{shell.manifest_id}: its bytes hash to blake3:")


def test_a_new_store_the_gate_could_not_read_is_refused(tmp_path: Path, capsys) -> None:
    # Bounds that are not the mesh's, and credits out of order: the schema
    # allows both, the gate reads neither, so no gate run could give the
    # ids of such a plan.
    sc = Scenario.start(tmp_path, world())
    added = swiss_shell(3, lo=(60_000, GROUND_MM, 60_000), hi=(70_000, GROUND_MM + 6_000, 70_000))
    bounds = {"max": [70_000, GROUND_MM + 6_000, 71_000], "min": [60_000, GROUND_MM, 60_000]}
    refused(capsys, sc.args(world().put(added.but(mesh=Mesh(added.mesh.data, bounds)))),
            f"the new store: tile {A}, shell {added.feature_id}: bounds_mm does not match the mesh")

    root = sc.write_new(world().put(added))
    store = Store(root)
    doc = cjson.loads((root / "world.json").read_bytes())
    wt = doc["tiles"][0]
    index = cjson.loads(store.read_object(wt["tile_id"], "json"))
    entry = next(a for a in index["assets"] if (a["layer"], a["feature_id"]) == ribbon(A).key)
    manifest = cjson.loads(store.read_object(entry["manifest_id"], "json"))
    manifest["provenance"]["attribution"].reverse()
    entry["manifest_id"] = store.write_json(manifest)
    wt["tile_id"] = store.write_json(index)
    store.write_world(doc)
    refused(capsys, sc.args(root), f"the new store: tile {A}, transport tile:{A}: manifest: object "
                                   f"{entry['manifest_id']}: provenance.attribution must be non-empty, sorted and "
                                   "unique")


def test_paths_are_checked(tmp_path: Path, capsys) -> None:
    sc = Scenario.start(tmp_path, world())
    args = sc.args(world())
    same = list(args)
    same[same.index("--new") + 1] = str(sc.current)
    refused(capsys, same, "--current and --new name the same store")
    out = tmp_path / "busy"
    out.mkdir()
    (out / "note.txt").write_bytes(b"x")
    busy = list(args)
    busy[busy.index("--out") + 1] = str(out)
    refused(capsys, busy, f"{out} exists and is not empty")
    for inside in (sc.current / "plan", sc.log.root / "plan"):
        nested = list(args)
        nested[nested.index("--out") + 1] = str(inside)
        refused(capsys, nested, "--out must not be inside --current, --new or --log")
    assert not (sc.current / "plan").exists()


def overrides_case(sc: Scenario, capsys, doc: dict, message: str) -> None:
    refused(capsys, sc.args(world(), overrides=doc), f"overrides: {message}")


def test_unscoped_accept_refused(tmp_path: Path, capsys) -> None:
    sc = Scenario.start(tmp_path, world())
    fid = swiss_shell(1).feature_id
    overrides_case(sc, capsys, {"accept": [{"layer": "shell", "feature_id": fid, "lifts": ["unexplained-change"],
                                            "reason": "Checked.", "current_manifest_id": None}]},
                   f"accept shell {fid} needs current_manifest_id and content_key (null where that version is absent)")


def test_overrides_are_checked(tmp_path: Path, capsys) -> None:
    sc = Scenario.start(tmp_path, world())
    fid = swiss_shell(1).feature_id
    accept = {"layer": "shell", "feature_id": fid, "current_manifest_id": None, "content_key": None,
              "lifts": ["unexplained-change"], "reason": "Checked."}
    hold = {"layer": "shell", "feature_id": fid, "reason": "Wait."}
    cases = [
        ({"accept": [dict(accept, reason="")]}, f"accept shell {fid} needs a reason"),
        ({"accept": [dict(accept, lifts=["no-such-code"])]}, f'accept shell {fid} lifts unknown code "no-such-code"'),
        ({"accept": [dict(accept, lifts=[])]}, f"accept shell {fid} needs lifts, a list of the codes it lifts"),
        ({"accept": [dict(accept, lifts=["manual-hold"])]},
         f"cannot lift manual-hold for shell {fid}: remove the hold instead"),
        ({"accept": [dict(accept, lifts=["unit-held"])]},
         f"cannot lift unit-held for shell {fid}: units are emitted whole; lift the codes of the held member instead"),
        ({"accept": [accept, accept]}, f"shell {fid} is accepted twice"),
        ({"accept": [dict(accept, content_key="blake3:abc")]},
         f"accept shell {fid}: content_key must be a content id or null"),
        ({"hold": [hold], "accept": [accept]}, f"shell {fid} is both held and accepted"),
        ({"hold": [hold, hold]}, f"shell {fid} is held twice"),
        ({"hold": [dict(hold, reason="  ")]}, f"hold shell {fid} needs a reason"),
        ({"hold": [dict(hold, reason="x" * 4097)]},
         f"hold shell {fid}: the reason must be one line of at most 4096 characters"),
        ({"hold": [dict(hold, reason="two" + chr(10) + "lines")]},
         f"hold shell {fid}: the reason must be one line of at most 4096 characters"),
        ({"hold": [dict(hold, note="x")]}, 'hold[0] has an unknown field "note"'),
        ({"allow_removals": [{"layer": "water", "dataset": "OpenStreetMap", "at_most": -1, "reason": "x"}]},
         "allow_removals water/OpenStreetMap needs at_most, a whole number of removals from 0 to 9007199254740991"),
        ({"allow_removals": [{"layer": "water", "dataset": "OpenStreetMap", "at_most": 1, "reason": "x"}] * 2},
         "water/OpenStreetMap has two allow_removals entries"),
        ({"extra": []}, 'unknown field "extra"'),
    ]
    for doc, message in cases:
        overrides_case(sc, capsys, doc, message)
    path = tmp_path / "bad.json"
    path.write_bytes(b'{"overrides_version": "0.2.0"}')
    refused(capsys, sc.args(world()) + ["--overrides", str(path)], 'overrides: overrides_version must be "0.1.0"')


def test_usage_errors_exit_2(capsys) -> None:
    assert main([]) == 2
    assert main(["plan", "--current", "x"]) == 2
    assert main(["plan", "--current", "x", "--new", "y", "--out", "z", "--max-set-bytes", "0"]) == 2
    capsys.readouterr()


def test_the_planner_needs_no_numpy() -> None:
    probe = ("import sys, tools.ingest.changes.__main__, tools.ingest.changes.plan; "
             "bad = sorted(m for m in sys.modules if m.split('.')[0] in ('numpy', 'shapely', 'pyproj', 'tifffile') "
             "or m == 'tools.ingest.pipeline'); print(bad)")
    done = subprocess.run([sys.executable, "-c", probe], cwd=REPO, capture_output=True, text=True, timeout=120)
    assert done.returncode == 0, done.stderr
    assert done.stdout.strip() == "[]"


def test_overrides_file_is_ordinary_json(tmp_path: Path) -> None:
    from tools.ingest.changes.overrides import read_overrides

    doc = {"overrides_version": "0.1.0", "hold": [{"layer": "shell", "feature_id": "osm:way:1", "reason": "x"}]}
    text = json.dumps(doc, indent=4).encode()
    ids = set()
    # LF, CRLF and a byte order mark, as editors on each platform save it,
    # and another layout: one document, so one id in the report.
    for name, data in [("lf", text), ("crlf", text.replace(b"\n", b"\r\n")),
                       ("bom", b"\xef\xbb\xbf" + text.replace(b"\n", b"\r\n")),
                       ("compact", json.dumps(doc, separators=(",", ":"), sort_keys=True).encode())]:
        path = tmp_path / f"{name}.json"
        path.write_bytes(data)
        ov = read_overrides(path)
        assert list(ov.holds) == [("shell", "osm:way:1")]
        ids.add(ov.id)
    assert ids == {content_id(cjson.dumps(doc))}


def test_numbers_must_fit_canonical_json(tmp_path: Path, capsys) -> None:
    # The report records at_most and --max-set-bytes in canonical JSON,
    # which holds integers up to 2^53 - 1.
    sc = Scenario.start(tmp_path, world())
    overrides_case(sc, capsys, {"allow_removals": [{"layer": "water", "dataset": "OpenStreetMap", "at_most": 2 ** 60,
                                                    "reason": "x"}]},
                   "allow_removals water/OpenStreetMap needs at_most, a whole number of removals from 0 to "
                   "9007199254740991")
    assert main(sc.args(world()) + ["--max-set-bytes", str(2 ** 53)]) == 2
    assert "--max-set-bytes: must be at most 9007199254740991" in capsys.readouterr().err
    assert main(sc.args(world()) + ["--max-set-bytes", str(2 ** 53 - 1)]) == 0
    capsys.readouterr()
    # Nor can it hold a lone surrogate, which JSON's escapes allow.
    path = tmp_path / "surrogate.json"
    path.write_bytes(b'{"overrides_version": "0.1.0", "hold": [{"layer": "shell", "feature_id": "osm:way:1", '
                     b'"reason": "x \\ud800"}]}')
    code, out, err = run(capsys, sc.args(world()) + ["--overrides", str(path)])
    assert (code, out) == (2, "") and err.startswith("error: overrides: not representable as canonical JSON: ")
