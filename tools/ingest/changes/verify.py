"""`python -m tools.ingest.changes verify` (ADR-0020, section 2).

    verify --current DIR --plan DIR --scratch DIR [--op-replay CMD] [--op-world CMD]
           [--replay FILE]... [--report FILE] [--no-compare]
           [--replan --new DIR --log DIR [--overrides FILE]]

Runs every change set of a plan through the real gate, in order, on a work
copy of the canonical store, and checks every world id the plan predicted.
It never writes to `--current`, `--plan`, `--new` or `--log`. Everything
goes under `--scratch`, which must not exist or must be empty:

    work/                    the work store: world.json copied, objects hard-
                             linked from --current (copied where a link
                             fails); each set is applied to it in turn
    gate/<k>.json            the gate's report on set <k>
    replays/<k>/<i>-<name>   the replays, re-recorded on the world after set <k>
    compare-worlds.json      --current compared with the final world
    replan/                  with --replan: the plan made again

Steps:
1. With `--replan`, plan again from the same inputs into `replan/` and
   compare every file with the plan, byte for byte.
2. Build the work store. `world.json` is copied, never linked: it is
   rewritten in place after each set, and through a link that would
   rewrite the canonical store. Objects are never rewritten, only added.
3. For each set, in order:
   - the work world must be the set's `base_world_id`;
   - `<op-replay> check-change <work> <bundle> --report gate/<k>.json
     [--replay ...]` runs the full gate (G1 to G10). Its report must name
     the set's change and base world, predict the plan's `world_after`,
     pass, give G3 `review` as a baseline refresh by `streetwide:ingest`,
     and give G4 and G5 `not_applicable`, which confirms the author;
   - the set is applied to the work store: its objects are copied in, and
     the new tile indexes and `world.json` come from
     `tools.worldfmt.changes.apply`. The work world must then be the
     plan's `world_after`;
   - `<op-replay> rerecord` moves every replay to the new work world, as
     the maintainer does after each merge, so that G10 sees them on the
     next set.
   A set that fails the gate but builds the predicted world does not stop
   the run, so one run lists every failing set. An id that differs stops
   it.
4. At the end, the work world must be the plan's final world, and
   `<op-world> validate <work>` must pass. Then, unless `--no-compare` is
   given, `<op-replay> compare-worlds <current> <work> --replay <original
   replay>...` compares the world before the refresh with the final one:
   probe points on the old ways that are newly blocked, and replay
   waypoints more than 2 m off their old path. These findings are for
   review, so they never change the exit code.

`--replay` names the replays registered for the current world. Each must
have been recorded in it (`op_replay info` says so), or G10 would ignore
it. `--op-replay` and `--op-world` are command prefixes: the tool appends
`info`, `check-change`, `rerecord` and `compare-worlds`, or `validate`, and
passes every path as an absolute path.

With `--report FILE`, a summary in canonical JSON: per set, the change id,
the gate report's id and outcomes, and the predicted and actual ids; then
the final checks and the comparison's own report. It names no local path,
so the same inputs give the same bytes. The file must not be inside
`--scratch`, whose work store links the canonical objects, and it is
written to a new file renamed over it, so that it never writes through a
hard link.

Exit codes: 0 every set passes the gate and every id matches (comparison
findings do not change it); 1 a set fails the gate, an id differs, the
final world does not validate, or the plan made again differs; 2 a usage
or input error, or a tool that could not run.

Every file `verify` reads is read with `common.read_input`: a regular file
itself, never a link, a directory or a FIFO, of at most
`common.MAX_INPUT_BYTES` (a replay: `MAX_REPLAY_BYTES`). A file it cannot
read is an input error.
"""

from __future__ import annotations

import argparse
import os
import re
import shlex
import shutil
import subprocess
import sys
from collections.abc import Callable, Sequence
from dataclasses import dataclass, field
from pathlib import Path, PurePosixPath

from tools.ingest.changes import COMMAND, VERSION
from tools.ingest.changes.common import (
    InternalError, PlanError, UnreadableFile, counted, entries_under, is_regular_file, printable, read_input,
)
from tools.ingest.changes.layout import (
    REPORT_JSON, REPORT_MD, SETS, PlanFileError, PlannedBundle, WrittenPlan, read_plan,
)
from tools.ingest.changes.stores import SharedObjects
from tools.worldfmt import cjson, schemas
from tools.worldfmt.changes import INGEST_AUTHOR, ChangeError, Objects, apply
from tools.worldfmt.ids import content_id
from tools.worldfmt.store import Store

DEFAULT_OP_REPLAY = "cargo run -q -p op_replay --locked --"
DEFAULT_OP_WORLD = "cargo run -q -p op_world --locked --"

_REPLAY_TICKS = 60 * 60 * 60 * 24
MAX_REPLAY_BYTES = 16 + 64 * 1024 + _REPLAY_TICKS * 8 + 4 + _REPLAY_TICKS * (8 + 32)
"""The largest replay `verify` reads (to take its content id), about
237 MiB: `op_replay::format::MAX_REPLAY_BYTES`, the largest header, one
day of input records at 60 Hz and a checkpoint at every tick. `op_replay`
refuses a longer file too."""

SUMMARY = "streetwide refresh verification"
SUMMARY_VERSION = "0.1.0"

WORK = "work"
GATE = "gate"
REPLAYS = "replays"
COMPARE_JSON = "compare-worlds.json"
REPLAN = "replan"

REFRESH_DETAIL = f"baseline refresh by {INGEST_AUTHOR}"
"""What the gate's G3 says of every change of a baseline refresh."""
PRINTED = 20
"""At most this many lines are printed per list; reports have them all."""

_OBJECT = re.compile(r"objects/([0-9a-f]{2})/(([0-9a-f]{64})\.[a-z0-9]+)")
_INFO_WORLD = re.compile(r"^world\s+.*\((blake3:[0-9a-f]{64})\)\s*$", re.MULTILINE)
_RERECORDED = re.compile(r"rerecorded in world (blake3:[0-9a-f]{64}) .*: (\d+) of (\d+) checkpoints unchanged")
_VALIDATED = re.compile(r"world (blake3:[0-9a-f]{64}): ")

Runner = Callable[[list[str]], subprocess.CompletedProcess]
"""Runs a command; returns its exit code and its output as text."""


class VerifyError(Exception):
    """A usage or input error, or a tool that could not run: exit code 2."""


class Stop(Exception):
    """A problem, already recorded, after which no later step can be
    checked: exit code 1."""


# --------------------------------------------------------------- commands --


def split_command(text: str) -> list[str]:
    """A command prefix such as `cargo run -q -p op_replay --locked --`, split
    as the platform's shell splits it. On Windows, backslashes are path
    separators, not escapes, and double quotes group words."""
    if os.name == "nt":
        words = shlex.split(text, posix=False)
        return [w[1:-1] if len(w) >= 2 and w[0] == w[-1] == '"' else w for w in words]
    return shlex.split(text)


def join_command(words: Sequence[str]) -> str:
    """The inverse of `split_command`."""
    return subprocess.list2cmdline(list(words)) if os.name == "nt" else shlex.join(words)


def resolve_command(words: Sequence[str]) -> tuple[str, ...]:
    """`words` with its program made absolute: a path that exists, or the
    program found on PATH."""
    if not words:
        raise VerifyError("a command prefix must name a program")
    program = words[0]
    if os.path.dirname(program) and Path(program).is_file():
        program = str(Path(program).resolve())
    else:
        program = shutil.which(program) or program
    return (program, *words[1:])


def run_tool(command: list[str]) -> subprocess.CompletedProcess:
    return subprocess.run(command, capture_output=True, text=True, encoding="utf-8", errors="replace")


def _tail(text: str, lines: int = 8) -> str:
    kept = [line.strip() for line in text.splitlines() if line.strip()]
    return " / ".join(kept[-lines:]) or "no output"


@dataclass
class Tools:
    op_replay: tuple[str, ...]
    op_world: tuple[str, ...]
    runner: Runner = run_tool

    def replay(self, *args: object) -> subprocess.CompletedProcess:
        return self._run(self.op_replay, args)

    def world(self, *args: object) -> subprocess.CompletedProcess:
        return self._run(self.op_world, args)

    def _run(self, prefix: tuple[str, ...], args: tuple[object, ...]) -> subprocess.CompletedProcess:
        try:
            return self.runner([*prefix, *(str(a) for a in args)])
        except OSError as e:
            raise VerifyError(f"cannot run {join_command(prefix)}: {e}") from e


# ---------------------------------------------------------------- options --


@dataclass(frozen=True)
class Replan:
    new: Path
    log: Path
    overrides: Path | None = None


@dataclass(frozen=True)
class Options:
    current: Path
    plan: Path
    scratch: Path
    op_replay: tuple[str, ...] = tuple(split_command(DEFAULT_OP_REPLAY))
    op_world: tuple[str, ...] = tuple(split_command(DEFAULT_OP_WORLD))
    replays: tuple[Path, ...] = ()
    report: Path | None = None
    compare: bool = True
    replan: Replan | None = None


def _inside(path: Path, other: Path) -> bool:
    p, o = path.resolve(), other.resolve()
    return p == o or o in p.parents


def check_paths(o: Options) -> None:
    """The checks on the directories and files, before anything is read.
    Whatever is there must then be readable (`read_input`)."""
    if not os.path.lexists(o.current / "world.json"):
        raise VerifyError(f"{o.current} is not a world store: it has no world.json")
    if not os.path.lexists(o.plan / REPORT_JSON):
        raise VerifyError(f"{o.plan} holds no plan: it has no {REPORT_JSON}")
    inputs = [("--current", o.current), ("--plan", o.plan)]
    if o.replan is not None:
        inputs += [("--new", o.replan.new), ("--log", o.replan.log)]
    for name, directory in inputs:
        if _inside(o.scratch, directory):
            raise VerifyError(f"--scratch must not be inside {name}")
        if o.report is not None and _inside(o.report, directory):
            raise VerifyError(f"--report must not be inside {name}")
    # The work store in --scratch holds hard links to the canonical objects.
    if o.report is not None and _inside(o.report, o.scratch):
        raise VerifyError("--report must not be inside --scratch")
    try:
        busy = o.scratch.exists() and (not o.scratch.is_dir() or any(o.scratch.iterdir()))
    except OSError as e:
        raise VerifyError(f"cannot read {o.scratch}: {e.strerror or e}") from e
    if busy:
        raise VerifyError(f"{o.scratch} exists and is not empty")
    for replay in o.replays:
        if not os.path.lexists(replay):
            raise VerifyError(f"--replay {replay}: no such file")


# ------------------------------------------------------------- work store --


def read_world(store: Path, what: str) -> tuple[str, dict]:
    """The id and manifest of the world in `store`: `world.json`, read with
    `read_input`, in canonical JSON that passes the world-manifest schema.
    Raises VerifyError naming the store as `what`."""
    try:
        data = read_input(store / "world.json")
    except UnreadableFile as e:
        raise VerifyError(f"{what}: world.json: {e.why}") from e
    try:
        world = cjson.loads(data)
    except cjson.CanonicalError as e:
        raise VerifyError(f"{what}: world.json: not canonical JSON: {e}") from e
    errors = _document_errors(world, "streetwide.world")
    if errors:
        raise VerifyError(f"{what}: world.json is not a streetwide.world document: {'; '.join(errors)}")
    return content_id(data), world


def _files_under(root: Path, *, follow_root: bool = False) -> list[Path]:
    """Every entry under directory `root` that is not a directory itself,
    links included, in order; [] if `root` is not a directory itself.
    Links to directories, and Windows junctions, are entries, never
    searched (`common.entries_under`). With `follow_root`, `root` may be
    a link to a directory, as the canonical store's `objects` may be."""
    return entries_under(root, follow_root=follow_root)


def build_work_store(current: Path, work: Path) -> tuple[int, int]:
    """Copies `current/world.json` into `work` and hard-links every object of
    `current/objects`, copying where a link fails. Only regular files are
    taken, as `op_world` reads a store: the gate finds a link or a FIFO in
    `--current` missing from the work store, as it would refuse it there.
    Returns the numbers of objects linked and copied. Raises VerifyError if
    `world.json` cannot be read."""
    try:
        world = read_input(current / "world.json")
    except UnreadableFile as e:
        raise VerifyError(f"{current}: world.json: {e.why}") from e
    work.mkdir(parents=True)
    (work / "world.json").write_bytes(world)
    linked = copied = 0
    for source in _files_under(current / "objects", follow_root=True):
        if not is_regular_file(source):
            continue
        target = work.joinpath(*source.relative_to(current).parts)
        target.parent.mkdir(parents=True, exist_ok=True)
        try:
            os.link(source, target)
            linked += 1
        except OSError:
            shutil.copyfile(source, target)
            copied += 1
    return linked, copied


def copy_bundle_objects(bundle: Path, work: Path) -> list[str]:
    """Copies into `work` the objects of a bundle that it lacks, read with
    `read_input`. Existing files are never touched, since they may be links
    into the canonical store. Returns the problems found: a file not named
    after its content id is not copied. Raises VerifyError for a file that
    cannot be read (`read_plan` has refused any it knew of)."""
    found = []
    for source in _files_under(bundle / "objects"):
        rel = printable(source.relative_to(bundle).as_posix())
        m = _OBJECT.fullmatch(rel)
        if m is None or m.group(1) != m.group(3)[:2]:
            found.append(f"{rel} is not named objects/<aa>/<hex>.<ext>")
            continue
        try:
            data = read_input(source)
        except UnreadableFile as e:
            raise VerifyError(str(e)) from e
        if content_id(data) != "blake3:" + m.group(3):
            found.append(f"{rel} does not hash to its name")
            continue
        target = work.joinpath("objects", m.group(1), m.group(2))
        if not target.exists():
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
    return found


def world_id_of(store: Path) -> str:
    """The content id of `store`'s `world.json`. Raises VerifyError."""
    try:
        return content_id(read_input(store / "world.json"))
    except UnreadableFile as e:
        raise VerifyError(str(e)) from e


def write_replacing(path: Path, data: bytes) -> None:
    """Writes `path` through a new file renamed over it. Writing in place
    would write through a hard link into whatever file `path` shares its
    bytes with, such as a canonical object; a rename replaces the link."""
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(path.name + ".tmp")
    tmp.unlink(missing_ok=True)
    tmp.write_bytes(data)
    os.replace(tmp, path)


# ------------------------------------------------------------ gate report --


def read_gate_report(data: bytes, name: str) -> dict:
    """A `streetwide.check` report: canonical JSON that passes its schema.
    Raises VerifyError naming the report as `name`."""
    try:
        report = cjson.loads(data)
    except cjson.CanonicalError as e:
        raise VerifyError(f"{name}: the gate's report is not canonical JSON: {e}") from e
    errors = _document_errors(report, "streetwide.check")
    if errors:
        raise VerifyError(f"{name}: the gate's report is not a streetwide.check document: {'; '.join(errors)}")
    return report


def _document_errors(doc: object, fmt: str) -> list[str]:
    """Why `doc` is not a valid document of format `fmt`, sorted; [] if it
    is one."""
    if isinstance(doc, dict) and doc.get("format") != fmt:
        return [f"its format is not {fmt}"]
    return sorted(schemas.errors(doc))


def failing(report: dict) -> list[dict]:
    """The checks that keep a report from passing: `fail` and `not_run`."""
    return [c for c in report["checks"] if c["outcome"] in ("fail", "not_run")]


def gate_problems(report: dict, bundle: PlannedBundle) -> list[str]:
    """What `verify` checks in the gate's report on one set, in words."""
    found = []
    if report["change_id"] != bundle.change_id:
        found.append(f"the gate read change {report['change_id']}, but the plan lists {bundle.change_id}")
    if report["world_id"] != bundle.base_world_id:
        found.append(f"the gate ran on world {report['world_id']}, not on the set's base {bundle.base_world_id}")
    if report["candidate_world_id"] != bundle.world_after:
        found.append(f"the gate's candidate world is {report['candidate_world_id'] or 'none'}, "
                     f"but the plan predicts {bundle.world_after}")
    if not report["passed"]:
        found.append("it does not pass the gate: "
                     + ", ".join(f"{c['id']} {c['outcome']}" for c in failing(report)))
    checks = {c["id"]: c for c in report["checks"]}
    g3 = checks.get("G3")
    if g3 is None:
        found.append("the gate's report has no G3")
    elif g3["outcome"] != "review":
        found.append(f"G3 is {g3['outcome']}, but a {REFRESH_DETAIL} is left to review")
    elif not g3["details"] or not all(REFRESH_DETAIL in d for d in g3["details"]):
        found.append(f"G3 does not call every change a {REFRESH_DETAIL}")
    for gid in ("G4", "G5"):
        c = checks.get(gid)
        if c is None:
            found.append(f"the gate's report has no {gid}")
        elif c["outcome"] != "not_applicable":
            found.append(f"{gid} is {c['outcome']}, but it does not apply to a {REFRESH_DETAIL}")
    return found


# ------------------------------------------------------------------ replan --


def plan_differences(root: Path, files: dict[str, bytes]) -> list[str]:
    """Every file of a plan made again that is missing from `root` or differs
    there, and every file among `root`'s reports and under `sets/` that the
    plan made again does not have. Other files in `root` are not part of
    the plan. A file is read with `read_input`, no further than the file
    made again: one that is longer, or is not a regular file, differs."""
    on_disk = {p.relative_to(root).as_posix(): p for p in _files_under(root, follow_root=True)}
    found = []
    for rel in sorted(files):
        path = on_disk.get(rel)
        if path is None:
            found.append(f"{rel} is missing from the plan")
            continue
        try:
            same = read_input(path, len(files[rel])) == files[rel]
        except UnreadableFile:
            same = False
        if not same:
            found.append(f"{rel} differs")
    for rel in on_disk:
        if rel not in files and (rel in (REPORT_JSON, REPORT_MD) or rel.startswith(SETS + "/")):
            found.append(f"{printable(rel)} was not made by plan")
    return found


# ------------------------------------------------------------------ verify --


def _say(line: str = "") -> None:
    print(line, flush=True)


@dataclass
class Result:
    code: int
    summary: dict | None = None
    problems: list[str] = field(default_factory=list)


class Verifier:
    def __init__(self, options: Options, runner: Runner = run_tool, say: Callable[[str], None] = _say):
        self.o = options
        self.tools = Tools(options.op_replay, options.op_world, runner)
        self.say = say
        self.work = options.scratch / WORK
        self.problems: list[str] = []
        self.sets: list[dict] = []
        self.replays: list[Path] = [p.resolve() for p in options.replays]
        """The replay suite on the work store's current world."""

    def fail(self, text: str, entry: dict | None = None) -> Stop:
        """Records a problem that stops the run, and returns the Stop to raise."""
        self.problems.append(text)
        if entry is not None:
            entry["problems"].append(text)
        self.say(text)
        return Stop(text)

    # -- inputs --

    def read_inputs(self) -> tuple[WrittenPlan, str, str]:
        try:
            plan = read_plan(self.o.plan)
        except PlanFileError as e:
            raise VerifyError(f"the plan in {self.o.plan}: {e}") from e
        current_id, world = read_world(self.o.current, str(self.o.current))
        planned = plan.report["current_world_id"]
        if current_id != planned:
            raise VerifyError(f"the plan was made on world {planned}, but --current is world {current_id}; "
                              "plan the refresh again")
        return plan, current_id, world["name"]

    def check_replays(self, current_id: str) -> list[str]:
        """Each replay must be readable, a regular file of at most
        `MAX_REPLAY_BYTES`, and have been recorded in the current world.
        Returns their content ids."""
        ids = []
        for path in self.o.replays:
            try:
                replay_id = content_id(read_input(path, MAX_REPLAY_BYTES))
            except UnreadableFile as e:
                raise VerifyError(f"--replay {path}: {e.why}") from e
            done = self.tools.replay("info", path.resolve())
            if done.returncode != 0:
                raise VerifyError(f"op_replay info {path}: {_tail(done.stderr)}")
            m = _INFO_WORLD.search(done.stdout)
            if m is None:
                raise VerifyError(f"op_replay info {path}: its output names no world")
            if m.group(1) != current_id:
                raise VerifyError(f"{path} was recorded in world {m.group(1)}, not in --current "
                                  f"(world {current_id}), so G10 would ignore it; move it with op_replay "
                                  "rerecord, or leave it out")
            ids.append(replay_id)
        return ids

    def replan(self, plan: WrittenPlan) -> dict:
        """`--replan`: plan again from the same inputs and compare every
        file. The inputs are checked first, so that a difference can only
        come from the planner."""
        from tools.ingest.changes.overrides import read_overrides
        from tools.ingest.changes.plan import make_plan, write_plan
        from tools.worldfmt.log import LogError, read_log

        r, report = self.o.replan, plan.report
        try:
            overrides_id = read_overrides(r.overrides).id
        except PlanError as e:
            raise VerifyError(f"--replan: {e}") from e
        if overrides_id != report["overrides_id"]:
            raise VerifyError(f"--replan: the plan was made with {report['overrides_id'] or 'no overrides file'}, "
                              f"but --overrides gives {overrides_id or 'none'}")
        new_id = read_world(r.new, f"--new {r.new}")[0]
        if new_id != report["new_world_id"]:
            raise VerifyError(f"--replan: the plan was made from the new store {report['new_world_id']}, "
                              f"but --new is {new_id}")
        try:
            head = read_log(r.log).head.record_id
        except LogError as e:
            raise VerifyError(f"--replan: {e}") from e
        except (OSError, ValueError) as e:  # a log file the log reader could not read
            raise VerifyError(f"--replan: the world log in {r.log}: {e}") from e
        if head != report["log"]["head_record_id"]:
            raise VerifyError(f"--replan: the plan was made when the world log ended at record "
                              f"{report['log']['head_record_id']}, but --log ends at {head}")
        try:
            again = make_plan(self.o.current, r.new, r.log, r.overrides, report["max_set_bytes"])
        except PlanError as e:
            raise VerifyError(f"--replan: {e}") from e
        except InternalError as e:
            raise self.fail(f"--replan: {e}") from e
        try:
            write_plan(again, self.o.scratch / REPLAN)
        except PlanError as e:
            raise VerifyError(f"--replan: {e}") from e
        differences = plan_differences(plan.root, again.files)
        if not differences:
            self.say(f"Plan made again: all {counted(len(again.files), 'file')} are byte-identical.")
        else:
            self.say(f"Plan made again: {counted(len(differences), 'difference')}:")
            for d in differences[:PRINTED]:
                self.say(f"  {d}")
            if len(differences) > PRINTED:
                self.say(f"  and {len(differences) - PRINTED} more")
            if report["tool"]["version"] != VERSION:
                self.say(f"  (the plan was made by {COMMAND} {report['tool']['version']}; this is {VERSION})")
            self.problems.append(f"the plan made again differs: {counted(len(differences), 'file')}")
        return {"identical": not differences, "files": len(again.files), "differences": differences}

    # -- sets --

    def check_set(self, k: int, bundle: PlannedBundle, objects: Objects) -> None:
        planned = self.plan.report["sets"][k - 1]
        name = PurePosixPath(bundle.path).name
        head = f"{bundle.path} ({k} of {len(self.plan.bundles)}, {counted(planned['changes'], 'change')})"
        entry = {
            "path": bundle.path,
            "kind": planned["kind"],
            "change_id": bundle.change_id,
            "changes": planned["changes"],
            "base_world_id": bundle.base_world_id,
            "predicted_world_after": bundle.world_after,
            "gate_report_id": None,
            "passed": False,
            "checks": [],
            "candidate_world_id": None,
            "world_after": None,
            "problems": [],
        }
        self.sets.append(entry)

        before = world_id_of(self.work)
        if before != bundle.base_world_id:
            raise self.fail(f"{head}: the work world is {before}, but the set was planned on "
                            f"{bundle.base_world_id}", entry)

        gate_path = self.o.scratch / GATE / f"{name}.json"
        gate_path.parent.mkdir(parents=True, exist_ok=True)
        replay_args = [a for p in self.replays for a in ("--replay", p)]
        done = self.tools.replay("check-change", self.work.resolve(), bundle.directory.resolve(), "--report",
                                 gate_path.resolve(), *replay_args)
        if done.returncode not in (0, 1) or not os.path.lexists(gate_path):
            raise VerifyError(f"{bundle.path}: op_replay check-change exited with {done.returncode}: "
                              f"{_tail(done.stderr)}")
        try:
            data = read_input(gate_path)
        except UnreadableFile as e:
            raise VerifyError(f"{bundle.path}: the gate's report {GATE}/{name}.json: {e.why}") from e
        gate = read_gate_report(data, f"{GATE}/{name}.json")
        entry.update(gate_report_id=content_id(data), passed=gate["passed"],
                     checks=[{"id": c["id"], "outcome": c["outcome"]} for c in gate["checks"]],
                     candidate_world_id=gate["candidate_world_id"])
        outcomes = {c["id"]: c["outcome"] for c in gate["checks"]}
        verdict = "passes the gate" if gate["passed"] else "does NOT pass the gate"
        self.say(f"{head}: {verdict} (G9 {outcomes.get('G9')}, G10 {outcomes.get('G10')}; "
                 f"report {entry['gate_report_id']})")
        found = gate_problems(gate, bundle)
        for text in found:
            self.say(f"  {text}")
        details = []
        for c in failing(gate):
            self.say(f"  {c['id']} {c['name']}:")
            for d in c["details"][:PRINTED]:
                self.say(f"    {d}")
            if len(c["details"]) > PRINTED:
                self.say(f"    and {len(c['details']) - PRINTED} more")
            details += [f"{c['id']} {c['name']}: {d}" for d in c["details"]]
        entry["problems"] += found + details
        self.problems += [f"{bundle.path}: {text}" for text in found]
        if gate["world_id"] != bundle.base_world_id or gate["candidate_world_id"] != bundle.world_after:
            raise Stop()

        unnamed = copy_bundle_objects(bundle.directory, self.work)
        if unnamed:
            raise self.fail(f"{bundle.path}: {'; '.join(unnamed)}", entry)
        try:
            candidate = apply(self.world, bundle.change, objects)
        except ChangeError as e:
            raise self.fail(f"{bundle.path} cannot be applied to the work store: {e}", entry) from e
        candidate.write(Store(self.work))
        self.world = candidate.world
        after = world_id_of(self.work)
        entry["world_after"] = after
        if after != bundle.world_after:
            raise self.fail(f"{bundle.path}: the work world after it is {after}, but the plan predicts "
                            f"{bundle.world_after}", entry)
        self.say(f"  world {after}, as planned")
        if self.replays:
            self.rerecord(name, after)

    def rerecord(self, name: str, world_id: str) -> None:
        """Moves the replay suite to the work store's new world (ADR-0020, section 3)."""
        kept = total = 0
        moved = []
        for i, (path, original) in enumerate(zip(self.replays, self.o.replays), 1):
            out = self.o.scratch / REPLAYS / name / f"{i}-{original.name}"
            out.parent.mkdir(parents=True, exist_ok=True)
            done = self.tools.replay("rerecord", path, "--world", self.work.resolve(), "--out", out.resolve())
            m = _RERECORDED.search(done.stdout)
            if done.returncode != 0 or m is None:
                raise VerifyError(f"op_replay rerecord {path.name} on the world after {name} exited with "
                                  f"{done.returncode}: {_tail(done.stderr or done.stdout)}")
            if m.group(1) != world_id:
                raise VerifyError(f"op_replay rerecord {path.name} moved it to world {m.group(1)}, not to the work "
                                  f"world {world_id}")
            kept, total = kept + int(m.group(2)), total + int(m.group(3))
            moved.append(out.resolve())
        self.replays = moved
        self.say(f"  {counted(len(moved), 'replay')} moved to it: {kept} of {total} checkpoints unchanged")

    # -- the end --

    def validate(self, final_id: str) -> dict:
        done = self.tools.world("validate", self.work.resolve())
        if done.returncode not in (0, 1):
            raise VerifyError(f"op_world validate exited with {done.returncode}: {_tail(done.stderr)}")
        line = next((x.strip() for x in done.stdout.splitlines() if _VALIDATED.match(x.strip())), "")
        m = _VALIDATED.match(line)
        passed = done.returncode == 0 and m is not None and m.group(1) == final_id
        if passed:
            self.say(f"op_world validate: {line}")
        else:
            self.say(f"op_world validate: FAILED: {line or _tail(done.stdout)}")
            for e in done.stderr.splitlines()[:PRINTED]:
                self.say(f"  {e}")
            self.problems.append(f"the final world does not validate: {line or 'no summary'}")
        return {"passed": passed, "summary": line}

    def compare(self) -> tuple[dict, str]:
        path = self.o.scratch / COMPARE_JSON
        replay_args = [a for p in self.o.replays for a in ("--replay", p.resolve())]
        done = self.tools.replay("compare-worlds", self.o.current.resolve(), self.work.resolve(), *replay_args,
                                 "--report", path.resolve())
        if done.returncode != 0 or not os.path.lexists(path):
            raise VerifyError(f"op_replay compare-worlds exited with {done.returncode}: {_tail(done.stderr)}")
        try:
            data = read_input(path)
        except UnreadableFile as e:
            raise VerifyError(f"{COMPARE_JSON}: {e.why}") from e
        try:
            report = cjson.loads(data)
        except cjson.CanonicalError as e:
            raise VerifyError(f"{COMPARE_JSON}: not canonical JSON: {e}") from e
        if not isinstance(report, dict) or not isinstance(report.get("findings"), int):
            raise VerifyError(f"{COMPARE_JSON}: not a world comparison")
        self.say("The world before the refresh against the final world, for review:")
        for line in done.stdout.splitlines():
            self.say(f"  {line}")
        return report, content_id(data)

    def run(self) -> Result:
        o = self.o
        check_paths(o)
        self.plan, current_id, name = self.read_inputs()
        plan = self.plan
        replay_ids = self.check_replays(current_id)
        changes = sum(s["changes"] for s in plan.report["sets"])
        self.say(f"Plan {plan.report_id} for world {name}: {counted(len(plan.bundles), 'change set')} with "
                 f"{counted(changes, 'change')}, from world {current_id} to {plan.final_world_id}.")
        o.scratch.mkdir(parents=True, exist_ok=True)
        replanned = final_id = validated = comparison = comparison_id = None
        try:
            if o.replan is not None:
                replanned = self.replan(plan)
            linked, copied = build_work_store(o.current, self.work)
            self.say(f"Work store: world.json copied, {linked} objects linked and {copied} copied from --current.")
            self.world = read_world(self.work, "the work store")[1]
            objects = SharedObjects(Store(self.work))
            for k, bundle in enumerate(plan.bundles, 1):
                self.check_set(k, bundle, objects)
            final_id = world_id_of(self.work)
            if final_id != plan.final_world_id:
                raise self.fail(f"the final world is {final_id}, but the plan predicts {plan.final_world_id}")
            self.say(f"Final world {final_id}, as planned.")
            validated = self.validate(final_id)
            if o.compare:
                comparison, comparison_id = self.compare()
            else:
                self.say("Comparison skipped (--no-compare).")
        except Stop:
            pass
        summary = {
            "report": SUMMARY,
            "report_version": SUMMARY_VERSION,
            "tool": {"command": f"{COMMAND} verify", "version": VERSION},
            "world": name,
            "plan_report_id": plan.report_id,
            "current_world_id": current_id,
            "replays": sorted(set(replay_ids)),
            "replan": replanned,
            "sets": self.sets,
            "predicted_final_world_id": plan.final_world_id,
            "final_world_id": final_id,
            "validate": validated,
            "comparison_report_id": comparison_id,
            "comparison": comparison,
            "problems": list(self.problems),
            "passed": not self.problems,
        }
        findings = ("no comparison" if comparison is None
                    else f"{counted(comparison['findings'], 'finding')} for review")
        if self.problems:
            self.say(f"verify: FAILED, {counted(len(self.problems), 'problem')} ({findings}):")
            for p in self.problems:
                self.say(f"  - {p}")
        else:
            self.say(f"verify: every set passes the gate and every id matches the plan ({findings}).")
        if o.report is not None:
            write_replacing(o.report, cjson.dumps(summary))
        return Result(1 if self.problems else 0, summary, list(self.problems))


def verify(options: Options, runner: Runner = run_tool, say: Callable[[str], None] = _say) -> Result:
    """Runs `verify`. Raises VerifyError for input errors (exit code 2)."""
    return Verifier(options, runner, say).run()


# ----------------------------------------------------------- command line --


def register(sub: argparse._SubParsersAction) -> None:
    p = sub.add_parser(
        "verify",
        help="run every change set of a plan through the real gate, in order, and check every predicted id",
        description="Check a plan's change sets in order with op_replay check-change on a work copy of the "
                    "canonical store, apply each one, re-record the replays, and check every world id the plan "
                    "predicted; then validate the final world and compare it with the current one (baseline "
                    "refresh guide, docs/contributing/baseline-refresh.md; ADR-0020).",
    )
    p.add_argument("--current", type=Path, required=True, metavar="DIR", help="the canonical world store")
    p.add_argument("--plan", type=Path, required=True, metavar="DIR", help="the plan's output directory")
    p.add_argument("--scratch", type=Path, required=True, metavar="DIR",
                   help="working directory; must not exist or must be empty")
    p.add_argument("--op-replay", default=DEFAULT_OP_REPLAY, metavar="CMD",
                   help=f"command prefix for op_replay (default: {DEFAULT_OP_REPLAY})")
    p.add_argument("--op-world", default=DEFAULT_OP_WORLD, metavar="CMD",
                   help=f"command prefix for op_world (default: {DEFAULT_OP_WORLD})")
    p.add_argument("--replay", type=Path, action="append", default=[], metavar="FILE",
                   help="a replay registered for the current world (repeat for each)")
    p.add_argument("--report", type=Path, metavar="FILE", help="also write a summary in canonical JSON")
    p.add_argument("--no-compare", action="store_true",
                   help="skip the comparison of the current world with the final one")
    p.add_argument("--replan", action="store_true",
                   help="plan again from the same inputs and compare every file byte for byte")
    p.add_argument("--new", type=Path, metavar="DIR", help="with --replan: the fresh ingest the plan was made from")
    p.add_argument("--log", type=Path, metavar="DIR", help="with --replan: the world log")
    p.add_argument("--overrides", type=Path, metavar="FILE",
                   help="with --replan: the overrides file, if the plan used one")
    p.set_defaults(run=run)


def options_from(args: argparse.Namespace) -> Options:
    if args.replan:
        if args.new is None or args.log is None:
            raise VerifyError("--replan needs --new and --log, and --overrides if the plan used one")
        replan = Replan(args.new, args.log, args.overrides)
    elif args.new is not None or args.log is not None or args.overrides is not None:
        raise VerifyError("--new, --log and --overrides go with --replan")
    else:
        replan = None
    return Options(
        current=args.current,
        plan=args.plan,
        scratch=args.scratch,
        op_replay=resolve_command(split_command(args.op_replay)),
        op_world=resolve_command(split_command(args.op_world)),
        replays=tuple(args.replay),
        report=args.report,
        compare=not args.no_compare,
        replan=replan,
    )


def run(args: argparse.Namespace) -> int:
    try:
        return verify(options_from(args)).code
    except VerifyError as e:
        for line in str(e).splitlines():
            print(f"error: {line}", file=sys.stderr)
        return 2
