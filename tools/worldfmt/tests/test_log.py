"""The world log from Python (tools/worldfmt/log.py): records, the chain
and the change sets are checked as op_world checks them, with the same
messages, and the planner's helpers give the records in order, their
change sets and what each record touched."""

from __future__ import annotations

import re
import shutil
from pathlib import Path

import pytest

from tools.worldfmt import changes, cjson, log
from tools.worldfmt.changes import Objects
from tools.worldfmt.ids import content_id
from tools.worldfmt.log import LogError
from tools.worldfmt.store import Store

REPO = Path(__file__).resolve().parents[3]
FIXTURES = REPO / "fixtures"
SAMPLE = FIXTURES / "changes" / "facade-sample"
FIXTURE_WORLD = FIXTURES / "world"
TILE = "17/68646/45903"
BETTER = {"reviewer": "github:reviewer", "verdict": "better", "notes": "Matches the building."}
LINKS = "merged and takedown records name the change, both worlds, the report and the previous record"


def cid(text: str) -> str:
    return content_id(text.encode())


def record(decision: str, *, previous: str | None = None, before: str | None = None, after: str = "w1",
           reviews: list[dict] | None = None, change_id: str | None = None, report_id: str | None = None) -> dict:
    """A merge record shaped like the one in crates/op_world/tests/gate.rs."""
    linked = decision != "baseline"
    return {
        "format": "streetwide.merge",
        "contract_version": "0.1.0",
        "decision": decision,
        "change_id": (change_id or cid("change")) if linked else None,
        "world_before": cid(before) if before else None,
        "world_after": cid(after),
        "check_report_id": (report_id or cid("report")) if linked else None,
        "reviews": reviews if reviews is not None else ([dict(BETTER)] if decision == "merged" else []),
        "reasons": "test",
        "previous_record_id": previous,
    }


def rid(r: dict) -> str:
    return content_id(cjson.dumps(r))


# ---------------------------------------------------------------- records --


def test_well_formed_records_pass() -> None:
    for decision in log.DECISIONS:
        r = record(decision, previous=None if decision == "baseline" else cid("p"),
                   before=None if decision == "baseline" else "w0")
        assert log.record_problems(r) == [], decision
        assert log.parse_record(cjson.dumps(r)) == r


def edited(decision: str, edit) -> dict:
    r = record(decision, previous=None if decision == "baseline" else cid("p"),
               before=None if decision == "baseline" else "w0")
    edit(r)
    return r


# (case, decision, edit, messages of `MergeRecord::problems`). The first
# four are the cases of crates/op_world/tests/contrib_schemas.rs.
RECORD_CASES = [
    ("a merge without its change set", "merged", lambda r: r.update(change_id=None), [LINKS]),
    ("a baseline with a previous record", "baseline", lambda r: r.update(previous_record_id=cid("x")),
     ["a baseline record has no change, previous world, report or previous record"]),
    ("an unknown verdict", "merged", lambda r: r["reviews"][0].update(verdict="great"), None),
    ("empty reasons", "merged", lambda r: r.update(reasons=""), ["reasons must be non-empty text"]),
    ("an unreviewed merge", "merged", lambda r: r.update(reviews=[]),
     ["a merge needs at least one review judging the change better"]),
    ("a merge judged not better", "merged", lambda r: r["reviews"][0].update(verdict="not_better"),
     ["a merge needs at least one review judging the change better"]),
    ("a blocked merge", "merged",
     lambda r: r["reviews"].append({"reviewer": "github:zed", "verdict": "blocked", "notes": "No."}),
     ["a blocked change cannot be merged"]),
    ("a blocked takedown", "takedown",
     lambda r: r.update(reviews=[{"reviewer": "github:zed", "verdict": "blocked", "notes": "No."}]), []),
    ("unsorted reviews", "merged", lambda r: r["reviews"].insert(0, dict(BETTER, reviewer="github:zed")),
     ["reviews must be sorted by reviewer, one per reviewer"]),
    ("two reviews by one reviewer", "merged", lambda r: r["reviews"].append(dict(BETTER)),
     ["reviews must be sorted by reviewer, one per reviewer"]),
    ("a reviewer by e-mail", "merged", lambda r: r["reviews"][0].update(reviewer="someone@example.org"),
     ['review by "someone@example.org": reviewer must be a handle and notes text']),
    ("empty notes", "merged", lambda r: r["reviews"][0].update(notes=""),
     ['review by "github:reviewer": reviewer must be a handle and notes text']),
    ("another format", "takedown", lambda r: r.update(format="streetwide.change"), ['format must be "streetwide.merge"']),
    ("another contract version", "baseline", lambda r: r.update(contract_version="0.2.0"),
     ['unknown contribution contract version "0.2.0"']),
    ("a takedown without its report", "takedown", lambda r: r.update(check_report_id=None), [LINKS]),
    ("an unknown decision", "merged", lambda r: r.update(decision="approved"), None),
    ("an unknown field", "merged", lambda r: r.update(note="x"), None),
    ("a world id ending in a newline", "merged", lambda r: r.update(world_after=r["world_after"] + "\n"), None),
]


@pytest.mark.parametrize("case,decision,edit,messages", RECORD_CASES, ids=[c[0] for c in RECORD_CASES])
def test_record_problems_mirror_op_world(case: str, decision: str, edit, messages) -> None:
    r = edited(decision, edit)
    found = log.record_problems(r)
    if messages == []:
        assert found == []
        return
    assert found, f"{case} passed"
    if messages is not None:
        assert [m for m in found if not m.startswith("schema: ")] == messages
    with pytest.raises(LogError):
        log.parse_record(cjson.dumps(r) if "\n" not in r["world_after"] else b"{}")


def test_parse_record_refuses_non_canonical_bytes() -> None:
    with pytest.raises(LogError, match="not canonical JSON"):
        log.parse_record(b'{"decision": "baseline"}')


# ------------------------------------------------------------------ chain --


def chain() -> list[tuple[str, dict]]:
    # crates/op_world/tests/gate.rs, `the_world_log_is_a_hash_chain`.
    base = record("baseline", after="w0")
    merged = record("merged", previous=rid(base), before="w0", after="w1")
    takedown = record("takedown", previous=rid(merged), before="w1", after="w2")
    return [(rid(base), base), (rid(merged), merged), (cid("t"), takedown)]


def test_a_log_is_a_hash_chain() -> None:
    assert log.verify_log(chain()) == cid("w2")


def test_verify_log_messages() -> None:
    (base_id, base), (merged_id, merged), _ = chain()
    gap = record("merged", previous=base_id, before="w9", after="w1")
    cases = [
        ([(base_id, base), (merged_id, gap)],
         f"world log entry 1 ({merged_id}): world_before is not the previous record's world_after"),
        ([(base_id, base), (merged_id, record("merged", previous=merged_id, before="w0", after="w1"))],
         f"world log entry 1 ({merged_id}): previous_record_id does not name the previous record"),
        ([(merged_id, merged)], f"world log entry 0 ({merged_id}): the log must start with a baseline"),
        ([(base_id, base), (base_id, base)], f"world log entry 1 ({base_id}): only the first record is a baseline"),
        ([], "the world log is empty"),
    ]
    for records, message in cases:
        with pytest.raises(LogError) as e:
            log.verify_log(records)
        assert str(e.value) == message


# ------------------------------------------------------------ a whole log --


def test_paths() -> None:
    some = cid("x")
    assert log.record_path(0) == "log/00000000.json"
    assert log.record_path(12) == "log/00000012.json"
    assert log.change_path(some) == f"changes/{some[7:]}/change.json"
    assert log.report_path(some) == f"reports/{some[7:]}.json"
    assert log.world_path(some) == f"worlds/{some[7:]}.json"
    with pytest.raises(ValueError):
        log.change_path("blake3:abc")


class Writer:
    """Writes a world log in the layout of tools/worldfmt/log.py."""

    def __init__(self, root: Path):
        self.root = root
        self.records: list[tuple[str, dict]] = []

    def put(self, rel: str, data: bytes) -> None:
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)

    def baseline(self, world_after: str, world: bytes | None = None) -> dict:
        r = {**record("baseline"), "world_after": world_after}
        return self._append(r, world)

    def merge(self, decision: str, change: bytes, world_after: str, *, report: bytes = b"{}",
              world: bytes | None = None) -> dict:
        previous_id, previous = self.records[-1]
        r = {**record(decision), "change_id": content_id(change), "world_before": previous["world_after"],
             "world_after": world_after, "check_report_id": content_id(report), "previous_record_id": previous_id}
        self.put(log.change_path(content_id(change)), change)
        self.put(log.report_path(content_id(report)), report)
        return self._append(r, world)

    def _append(self, r: dict, world: bytes | None) -> dict:
        if world is not None:
            self.put(log.world_path(r["world_after"]), world)
        data = cjson.dumps(r)
        self.put(log.record_path(len(self.records)), data)
        self.records.append((content_id(data), r))
        return r


def refresh_change(base_world_id: str, entries: list[dict], author: str = changes.INGEST_AUTHOR) -> bytes:
    return changes.to_bytes({
        "format": "streetwide.change", "contract_version": "0.1.0", "base_world_id": base_world_id,
        "title": "Test", "author": author, "rationale": "Test.", "changes": sorted(entries, key=changes.sort_key),
    })


@pytest.fixture
def fixture_log(tmp_path: Path) -> Path:
    """A log over fixtures/world: the baseline; the sample facade merged;
    the facade taken down; the water removed by a refresh; a shell moved
    to another tile (its world is not checked, only its record)."""
    store = Store(FIXTURE_WORLD)
    world_id, world = store.read_world()
    index = cjson.loads(store.read_object(world["tiles"][0]["tile_id"], "json"))
    w = Writer(tmp_path / "log")
    w.baseline(world_id, (FIXTURE_WORLD / "world.json").read_bytes())

    sample = (SAMPLE / "change.json").read_bytes()
    with_facade = changes.apply(world, changes.parse(sample), Objects(SAMPLE, FIXTURE_WORLD))
    w.merge("merged", sample, with_facade.world_id, report=(SAMPLE / "report.json").read_bytes(),
            world=with_facade.world_bytes)

    (facade,) = [c for c in changes.parse(sample)["changes"]]
    takedown = refresh_change(with_facade.world_id, [dict(facade, op="remove", parent_version_id=facade["manifest_id"],
                                                          manifest_id=None)], author="github:maintainer")
    back = changes.apply(with_facade.world, changes.parse(takedown), Objects(with_facade.objects, SAMPLE, FIXTURE_WORLD))
    assert back.world_id == world_id, "a takedown of the facade returns to the baseline (as in op_world's tests)"
    w.merge("takedown", takedown, back.world_id)

    (water,) = [a for a in index["assets"] if a["layer"] == "water"]
    dry = refresh_change(world_id, [{"op": "remove", "tile": TILE, "layer": "water", "feature_id": water["feature_id"],
                                     "parent_version_id": water["manifest_id"], "manifest_id": None}])
    w.merge("merged", dry, changes.apply(world, changes.parse(dry), Objects(FIXTURE_WORLD)).world_id)

    (shell,) = [a for a in index["assets"] if a["feature_id"] == "osm:way:493253453"]
    move = refresh_change(cid("world 3"), [
        {"op": "remove", "tile": TILE, "layer": "shell", "feature_id": shell["feature_id"],
         "parent_version_id": shell["manifest_id"], "manifest_id": None},
        {"op": "add", "tile": "17/68646/45904", "layer": "shell", "feature_id": shell["feature_id"],
         "parent_version_id": None, "manifest_id": cid("moved shell")},
    ])
    w.merge("merged", move, cid("world 4"))
    return tmp_path / "log"


def test_read_log_gives_the_records_and_their_change_sets(fixture_log: Path) -> None:
    world_log = log.read_log(fixture_log)
    assert [r.path for r in world_log.records] == [f"log/0000000{i}.json" for i in range(5)]
    assert [r.index for r in world_log.records] == list(range(5))
    assert [r.decision for r in world_log.records] == ["baseline", "merged", "takedown", "merged", "merged"]
    assert [r.author for r in world_log.records] == [
        None, "streetwide:fixtures", "github:maintainer", "streetwide:ingest", "streetwide:ingest"]
    for r in world_log.records:
        assert r.record_id == content_id((fixture_log / r.path).read_bytes())
        if r.change is not None:
            assert cjson.dumps(r.change) == (fixture_log / log.change_path(r.change_id)).read_bytes()
    assert world_log.baseline.world_after == Store(FIXTURE_WORLD).read_world()[0]
    assert world_log.world_id == world_log.head.world_after == cid("world 4")
    assert world_log.records[2].world_after == world_log.records[0].world_after
    assert world_log.baseline.touches() == ()


def test_touches_and_last_touches(fixture_log: Path) -> None:
    world_log = log.read_log(fixture_log)
    touched = [(t.record.index, t.op, t.layer, t.feature_id, t.tile) for t in world_log.touches()]
    facade = "swisstopo:swissbuildings3d:000f609c-4003-495f-9426-b43e3d770167"
    assert touched == [
        (1, "add", "facade", facade, TILE),
        (2, "remove", "facade", facade, TILE),
        (3, "remove", "water", f"tile:{TILE}", TILE),
        (4, "remove", "shell", "osm:way:493253453", TILE),
        (4, "add", "shell", "osm:way:493253453", "17/68646/45904"),
    ]
    last = world_log.last_touches()
    assert list(last) == [("facade", facade), ("shell", "osm:way:493253453"), ("water", f"tile:{TILE}")]
    (takedown,) = last[("facade", facade)]
    assert (takedown.record.decision, takedown.op, takedown.manifest_id) == ("takedown", "remove", None)
    assert takedown.parent_version_id == world_log.records[1].change["changes"][0]["manifest_id"]
    assert [(t.op, t.tile) for t in last[("shell", "osm:way:493253453")]] == [("remove", TILE), ("add", "17/68646/45904")]
    (dry,) = last[("water", f"tile:{TILE}")]
    assert (dry.record.author, dry.record.world_before) == ("streetwide:ingest", world_log.baseline.world_after)
    assert len({t for ts in last.values() for t in ts}) == 4, "touches can go in sets"


def test_worlds_and_reports_are_read_on_request(fixture_log: Path) -> None:
    world_log = log.read_log(fixture_log)
    baseline_world = world_log.world(world_log.baseline.world_after)
    assert cjson.dumps(baseline_world) == (FIXTURE_WORLD / "world.json").read_bytes()
    assert world_log.world(world_log.head.world_after) is None
    report = world_log.report(world_log.records[1].record["check_report_id"])
    assert report["change_id"] == world_log.records[1].change_id and report["passed"] is True
    corrupt = fixture_log / log.world_path(world_log.records[1].world_after)
    corrupt.write_bytes(corrupt.read_bytes() + b" ")
    with pytest.raises(LogError, match=re.escape(f"{log.world_path(world_log.records[1].world_after)} does not hash")):
        world_log.world(world_log.records[1].world_after)


def test_an_empty_log_has_no_records(tmp_path: Path) -> None:
    message = "the world log has no records: log/00000000.json, the baseline record, is missing"
    with pytest.raises(LogError, match=re.escape(message)):
        log.read_log(tmp_path)
    (tmp_path / "log").mkdir()
    with pytest.raises(LogError, match=re.escape(message)):
        log.read_log(tmp_path)


def test_records_are_numbered_without_gaps_or_other_files(fixture_log: Path) -> None:
    rule = "records are numbered from 00000000, with no gaps and no other files"
    (fixture_log / "log" / "README").write_bytes(b"x")
    message = f"the world log has log/README where log/00000005.json should be; {rule}"
    with pytest.raises(LogError, match=re.escape(message)):
        log.read_log(fixture_log)
    (fixture_log / "log" / "README").unlink()
    (fixture_log / "log" / "00000002.json").unlink()
    message = f"the world log has log/00000003.json where log/00000002.json should be; {rule}"
    with pytest.raises(LogError, match=re.escape(message)):
        log.read_log(fixture_log)


def test_bad_records_name_their_file(fixture_log: Path) -> None:
    path = fixture_log / "log" / "00000001.json"
    r = cjson.loads(path.read_bytes())
    path.write_bytes(cjson.dumps(dict(r, reviews=[])))
    with pytest.raises(LogError, match=re.escape(
        "log/00000001.json: a merge needs at least one review judging the change better"
    )):
        log.read_log(fixture_log)
    path.write_bytes(cjson.dumps(r) + b"\n")
    with pytest.raises(LogError, match=re.escape("log/00000001.json: not canonical JSON")):
        log.read_log(fixture_log)


def test_the_chain_is_checked(fixture_log: Path) -> None:
    path = fixture_log / "log" / "00000002.json"
    r = cjson.loads(path.read_bytes())
    path.write_bytes(cjson.dumps(dict(r, world_before=cid("elsewhere"))))
    with pytest.raises(LogError) as e:
        log.read_log(fixture_log)
    assert str(e.value) == (
        f"world log entry 2 ({content_id(path.read_bytes())}): world_before is not the previous record's world_after"
    )


def test_every_change_set_must_be_there(fixture_log: Path) -> None:
    world_log = log.read_log(fixture_log)
    change_id = world_log.records[3].change_id
    rel = log.change_path(change_id)
    message = f"the world log names change set {change_id} (log/00000003.json) but {rel} is missing or has another id"
    data = (fixture_log / rel).read_bytes()
    (fixture_log / rel).write_bytes(data + b" ")
    with pytest.raises(LogError, match=re.escape(message)):
        log.read_log(fixture_log)
    (fixture_log / rel).unlink()
    with pytest.raises(LogError, match=re.escape(message)):
        log.read_log(fixture_log)


def test_a_change_set_must_pass_its_rules(tmp_path: Path) -> None:
    w = Writer(tmp_path)
    w.baseline(cid("w0"))
    bad = cjson.dumps(dict(changes.parse((SAMPLE / "change.json").read_bytes()), author="someone@example.org"))
    w.merge("merged", bad, cid("w1"))
    with pytest.raises(LogError) as e:
        log.read_log(tmp_path)
    assert str(e.value).startswith(f"{log.change_path(content_id(bad))}: schema: $.author: ")
    assert str(e.value).endswith("; author must be a handle such as github:<login>")


# ----------------------------------------------- a log that op_world wrote --


def test_reads_a_log_that_op_world_wrote(op_world, tmp_path: Path) -> None:
    # Records written by `op_world baseline` and `op_world merge` (with
    # --record and --previous), laid out as tools/worldfmt/log.py reads
    # them: the baseline, the sample facade, its takedown.
    world = tmp_path / "world"
    shutil.copytree(FIXTURE_WORLD, world)
    root = tmp_path / "log"
    (root / "log").mkdir(parents=True)
    writer = Writer(root)

    def run(*args: str) -> None:
        done = op_world(*args)
        assert done.returncode == 0, done.stderr

    def merge(n: int, bundle: Path, *flags: str) -> None:
        run("merge", str(world), str(bundle), "--previous", str(root / log.record_path(n - 1)),
            "--record", str(root / log.record_path(n)), "--report", str(tmp_path / f"report-{n}.json"), *flags)
        change = (bundle / "change.json").read_bytes()
        writer.put(log.change_path(content_id(change)), change)

    run("baseline", str(world), "--record", str(root / log.record_path(0)), "--reason", "The fixture.")
    base_id = Store(world).read_world()[0]
    merge(1, SAMPLE, "--reason", "Test facade.", "--review", "github:reviewer=better:Checked.")
    with_facade_id = Store(world).read_world()[0]
    (facade,) = changes.parse((SAMPLE / "change.json").read_bytes())["changes"]
    takedown = tmp_path / "takedown"
    takedown.mkdir()
    (takedown / "change.json").write_bytes(refresh_change(with_facade_id, [
        dict(facade, op="remove", parent_version_id=facade["manifest_id"], manifest_id=None)], author="github:maintainer"))
    merge(2, takedown, "--reason", "Privacy request (test).", "--takedown")

    world_log = log.read_log(root)
    assert [r.decision for r in world_log.records] == ["baseline", "merged", "takedown"]
    assert [r.world_after for r in world_log.records] == [base_id, with_facade_id, base_id]
    assert world_log.world_id == Store(world).read_world()[0] == base_id
    assert world_log.records[1].record["check_report_id"] == content_id((tmp_path / "report-1.json").read_bytes())
    sample = changes.parse((SAMPLE / "change.json").read_bytes())
    predicted = changes.apply(Store(FIXTURE_WORLD).read_world()[1], sample, Objects(SAMPLE, FIXTURE_WORLD))
    assert predicted.world_id == with_facade_id
    ((key, (last,)),) = world_log.last_touches().items()
    assert key == ("facade", facade["feature_id"]) and (last.record.decision, last.op) == ("takedown", "remove")
