"""`python -m tools.ingest.changes plan` (ADR-0020, section 2).

    plan --current DIR --new DIR --log DIR --out DIR [--overrides FILE] [--max-set-bytes N]

Reads the canonical store (`--current`), a fresh ingest of the same world
definition (`--new`), the world log (`--log`, required) and the
maintainer's overrides, and writes into `--out`, which must not exist or
must be empty:

    refresh-report.json                      canonical JSON (report.schema.json)
    refresh-report.md                        the same report for people
    sets/001/change.json                     streetwide.change, author streetwide:ingest
    sets/001/objects/<aa>/<hex>.json         manifests the current store lacks
    sets/001/objects/<aa>/<hex>.<ext>        content the current store lacks
    sets/002/...                             (3 digits; 4 above 999 sets)

Exit codes follow op_world: 0 the plan is written (conflicts included), 2 a
usage or input error, 1 an internal error found by the plan-time checks
(nothing is written). A file that cannot be written is an input error,
and what was written is removed again. Every file is written as bytes
with LF line endings, every path in a file is POSIX and relative to
`--out`, and the same inputs give the same bytes on every platform.
"""

from __future__ import annotations

import argparse
import sys
from dataclasses import dataclass
from pathlib import Path

from tools.ingest.changes import checks, layout
from tools.ingest.changes import report as reports
from tools.ingest.changes.classify import Classification, classify
from tools.ingest.changes.common import InternalError, PlanError, counted, show, version_order
from tools.ingest.changes.holds import apply_holds, check_vanished_groups
from tools.ingest.changes.overrides import read_overrides
from tools.ingest.changes.sets import Context, Emitted, PlannedSet, chain, emit, pack
from tools.ingest.changes.stores import (
    CONTRIBUTIONS_DATASET, INGEST_GENERATOR, StoreView, generator_text, is_ingest_manifest, read_store,
)
from tools.worldfmt import cjson
from tools.worldfmt.ids import content_id
from tools.worldfmt.log import LogError, WorldLog, read_log

BASELINE_HINT = ("for a world that has never had a merge or a takedown, write its baseline record first: "
                 "op_world baseline <canonical> --log <log> --reason \"...\"")
"""`--log` starts the log directory with the baseline record and the world
it names (`worlds/<hex>.json`), as `op_world`'s own message says."""


@dataclass
class Plan:
    report: dict
    report_bytes: bytes
    report_id: str
    markdown: bytes
    sets: list[PlannedSet]
    files: dict[str, bytes]
    """Every output file by its POSIX path relative to `--out`, sorted."""

    @property
    def summary(self) -> str:
        return reports.summary(self.report)


def check_paths(current: Path, new: Path, log_dir: Path, out: Path) -> None:
    """The checks on the four directories, before anything is read."""
    current, new, log_dir, out = Path(current), Path(new), Path(log_dir), Path(out)
    if _same(current, new):
        raise PlanError("--current and --new name the same store")
    target = out.resolve()
    for other in (current, new, log_dir):
        base = other.resolve()
        if target == base or base in target.parents:
            raise PlanError("--out must not be inside --current, --new or --log")
    if out.exists() and (not out.is_dir() or any(out.iterdir())):
        raise PlanError(f"{out} exists and is not empty")


def _same(a: Path, b: Path) -> bool:
    try:
        if a.exists() and b.exists():
            return a.samefile(b)
    except OSError:
        pass
    return a.resolve() == b.resolve()


def read_world_log(log_dir: Path | None, cur: StoreView) -> tuple[WorldLog, list[str]]:
    if log_dir is None:
        raise PlanError(f"--log is required; {BASELINE_HINT}")
    try:
        wlog = read_log(log_dir)
    except LogError as e:
        hint = f"; {BASELINE_HINT}" if "has no records" in str(e) else ""
        raise PlanError(f"{e}{hint}") from e
    if wlog.world_id != cur.world_id:
        raise PlanError(f"the world log ends at world {wlog.world_id}, but --current is world {cur.world_id}")
    warnings = []
    if len(wlog.records) == 1:
        contributed = sum(1 for a in cur.assets.values() if not is_ingest_manifest(a.manifest))
        if contributed:
            warnings.append(
                f"the world log holds only its baseline record, but {counted(contributed, 'current asset')} "
                f"{'is a contribution' if contributed == 1 else 'are contributions'} by "
                f"{'its manifest' if contributed == 1 else 'their manifests'}; a pending change to "
                f"{'it' if contributed == 1 else 'them'} gets origin-unclear (H3)")
    return wlog, warnings


def check_stores(cur: StoreView, new: StoreView) -> None:
    """The same world definition, region and tile origins, and a
    new store that is a fresh ingest by one generator. `check_generator`
    compares that generator with the current store's, once the log has
    said which current assets the ingest made."""
    if cur.name != new.name:
        raise PlanError(f'the current store is world "{cur.name}" but the new store is world "{new.name}"; '
                        "a refresh compares two builds of the same world definition")
    if cur.world["region"] != new.world["region"]:
        raise PlanError(f"the stores use different regions ({cur.world['region']['key']}, "
                        f"{new.world['region']['key']})")
    for tile, origin in cur.origins.items():
        other = new.origins.get(tile)
        if other is not None and other != origin:
            raise PlanError(f"tile {tile} has origin_mm {list(origin)} in the current store and {list(other)} "
                            "in the new store")
    for key, asset in new.assets.items():
        m = asset.manifest
        if m["parent_version_id"] is not None:
            raise PlanError(f"the new store is not a fresh ingest: {show(key)} has a parent version")
        name = m["provenance"]["generator"]["name"]
        if name != INGEST_GENERATOR:
            raise PlanError(f"the new store is not a fresh ingest: {show(key)} was made by {name}")
        if any(s["dataset"] == CONTRIBUTIONS_DATASET for s in asset.sources):
            raise PlanError(f'the new store is not a fresh ingest: {show(key)} lists a "{CONTRIBUTIONS_DATASET}" '
                            "source")
    generators = sorted({generator_text(a.manifest) for a in new.assets.values()}, key=version_order)
    if len(generators) > 1:
        raise PlanError(f"the new store mixes generator versions ({', '.join(generators)})")


def check_generator(cur: StoreView, new: StoreView, cls: Classification) -> None:
    """The new store is not older than the ingest that made
    current assets. Only assets made by the ingest count (Q2: the
    manifest and the log agree), because any manifest can name the
    ingest's generator and G5 does not check it: a contribution that does
    must not stop every later refresh."""
    if not new.assets:
        return
    version = next(iter(new.assets.values())).manifest["provenance"]["generator"]["version"]
    made = [a.manifest["provenance"]["generator"]["version"] for key, a in cur.assets.items()
            if cls.origins[key].kind == "ingest"]
    newest = max(made, key=version_order, default=None)
    if newest is not None and version_order(version) < version_order(newest):
        raise PlanError(f"the new store was made by {INGEST_GENERATOR} {version}, older than {newest}, which "
                        "made current assets; ingest again with the current tools/ingest")


def make_plan(current: Path, new: Path, log_dir: Path | None, overrides: Path | None = None,
              max_set_bytes: int | None = None) -> Plan:
    """Plans a refresh in memory. Raises PlanError for input errors and
    InternalError when a plan-time check fails."""
    ov = read_overrides(overrides)
    checked: dict = {}
    cur = read_store(Path(current), "current", checked)
    nxt = read_store(Path(new), "new", checked)
    wlog, log_warnings = read_world_log(None if log_dir is None else Path(log_dir), cur)
    check_stores(cur, nxt)

    cls = classify(cur, nxt, wlog)
    check_generator(cur, nxt, cls)
    check_vanished_groups(cls, ov)
    emitted: dict[int, Emitted] = {}

    def packed() -> list[PlannedSet]:
        # Holds only ever take items out, so each item is emitted once.
        for i in cls.items:
            if i.emitted and id(i) not in emitted:
                emitted[id(i)] = emit(i, cur, nxt)
        return pack([u for u in cls.units if u.emitted], emitted, max_set_bytes)

    holds = apply_holds(cls, cur, nxt, ov, lambda: [s.items for s in packed()])
    sets = packed()
    held = [i for i in cls.items if not i.emitted]
    conflicts = sum(1 for i in held if reports.is_conflict(i))
    removers = {k: p.author for k, p in cls.protections.items() if p.kind == "removed-by-contribution"}
    ctx = Context(cur.name, cur.world_id, nxt.world_id, conflicts, len(held) - conflicts, ov.id,
                  authors=cls.authors, removers=removers)
    final_id, final_world, objects = chain(sets, cur, holds, ctx)

    checks.check_sets(sets, cur, cls, max_set_bytes)
    checks.check_chain(sets, cur, final_id, objects)
    checks.check_residual(cur, nxt, cls, final_world, objects)

    state = reports.State(cur, nxt, wlog, log_warnings, ov, cls, holds, sets, final_id, final_world, max_set_bytes)
    report = reports.build(state)
    errors = reports.schema_errors(report)
    if errors:
        raise InternalError(f"internal error: the report breaks report.schema.json: {'; '.join(errors)}")
    report_bytes = reports.to_bytes(report)
    report_id = content_id(report_bytes)
    md = reports.markdown(report, report_id)
    files = {layout.REPORT_JSON: report_bytes, layout.REPORT_MD: md}
    for s in sets:
        files[f"{s.path}/{layout.CHANGE_JSON}"] = s.change_bytes
        for path, data in s.objects.items():
            files[f"{s.path}/{path}"] = data
    files = dict(sorted(files.items()))
    for path, data in files.items():
        if path.endswith((".json", ".md")) and b"\r" in data:
            raise InternalError(f"internal error: {path} would contain a carriage return")
    return Plan(report, report_bytes, report_id, md, sets, files)


def write_plan(plan: Plan, out: Path) -> None:
    """Writes every file of `plan` under `out` (as bytes, LF only): the
    sets first and `refresh-report.json` last, since `verify` takes a
    directory with that file for a plan. `out` must be missing or empty.
    If a file cannot be written (on Windows, a path over 260 characters
    without long paths, for example), whatever was written is removed
    again and a PlanError names the file, so that no partial plan is
    left behind."""
    out = Path(out)
    order = sorted(plan.files, key=lambda rel: (rel == layout.REPORT_JSON, rel == layout.REPORT_MD, rel))
    made: list[Path] = []  # every directory and file this call made, in order

    def make_dirs(directory: Path) -> None:
        missing = []
        while not directory.exists() and directory.parent != directory:
            missing.append(directory)
            directory = directory.parent
        for d in reversed(missing):
            d.mkdir()
            made.append(d)

    rel = None
    try:
        make_dirs(out)
        for rel in order:
            path = out.joinpath(*rel.split("/"))
            make_dirs(path.parent)
            made.append(path)
            path.write_bytes(plan.files[rel])
    except OSError as e:
        for path in reversed(made):
            try:
                if path.is_dir():
                    path.rmdir()
                else:
                    path.unlink(missing_ok=True)
            except OSError:
                pass
        raise PlanError(f"cannot write {rel or out}: {e.strerror or e}; the files written so far were removed") from e


# ----------------------------------------------------------- command line --


def _positive(text: str) -> int:
    try:
        value = int(text)
    except ValueError:
        raise argparse.ArgumentTypeError(f"{text!r} is not a whole number") from None
    if value <= 0:
        raise argparse.ArgumentTypeError("must be at least 1")
    if value > cjson.MAX_SAFE_INTEGER:  # the report records it in canonical JSON
        raise argparse.ArgumentTypeError(f"must be at most {cjson.MAX_SAFE_INTEGER}")
    return value


def register(sub: argparse._SubParsersAction) -> None:
    p = sub.add_parser(
        "plan",
        help="plan a baseline refresh: change sets and a refresh report",
        description="Compare a fresh ingest with the canonical store and its world log, and write change sets "
                    "authored by streetwide:ingest plus a refresh report (RFC 0002 section 7, ADR-0020).",
    )
    p.add_argument("--current", type=Path, required=True, metavar="DIR", help="the canonical world store")
    p.add_argument("--new", type=Path, required=True, metavar="DIR",
                   help="a fresh ingest of the same world, in a directory of its own")
    p.add_argument("--log", type=Path, metavar="DIR", help="the world log (required)")
    p.add_argument("--out", type=Path, required=True, metavar="DIR",
                   help="output directory; must not exist or must be empty")
    p.add_argument("--overrides", type=Path, metavar="FILE",
                   help="holds, scoped accepts and removal allowances, each with a reason")
    p.add_argument("--max-set-bytes", type=_positive, metavar="N",
                   help="also split sets so that each bundle stays within N bytes (default: no limit; "
                        "8000000 when bundles go through pull requests, ADR-0010)")
    p.set_defaults(run=run)


def run(args: argparse.Namespace) -> int:
    try:
        if args.log is None:
            raise PlanError(f"--log is required; {BASELINE_HINT}")
        check_paths(args.current, args.new, args.log, args.out)
        plan = make_plan(args.current, args.new, args.log, args.overrides, args.max_set_bytes)
        write_plan(plan, args.out)
    except PlanError as e:
        for line in str(e).splitlines():
            print(f"error: {line}", file=sys.stderr)
        return 2
    except InternalError as e:
        print(str(e), file=sys.stderr)
        return 1
    print(plan.summary)
    print(f"report: {(Path(args.out) / layout.REPORT_MD).as_posix()} (content id of the JSON: {plan.report_id})")
    return 0
