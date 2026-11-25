"""The layout of a plan's output directory, for `plan` and `verify`.

    refresh-report.json                      canonical JSON, report.schema.json
    refresh-report.md                        the same report for people
    sets/<k>/change.json                     one change set per bundle, in merge order
    sets/<k>/objects/<aa>/<hex>.<ext>        objects the current store lacks

`<k>` has 3 digits (4 or more above 999 sets). The report lists the sets in
merge order, each with its path, change id, `base_world_id` and the world id
it predicts (`world_after`); `final_world_id` is the world after the last.
`read_plan` reads a plan back and checks that its files agree with its
report, so `verify` starts from checked data.

A plan may come from someone else, so `read_plan` reads every file with
`common.read_input`: a regular file itself (not a link, a directory or a
FIFO) of at most `common.MAX_INPUT_BYTES`. It refuses an object file in a
set that such a read would refuse, and a link (or a Windows junction) at a
set's `objects` or under it, before `verify` starts.
"""

from __future__ import annotations

import stat
from dataclasses import dataclass
from pathlib import Path, PurePosixPath

from tools.ingest.changes import common
from tools.ingest.changes.common import UnreadableFile, entries_under, printable, read_input
from tools.worldfmt import cjson
from tools.worldfmt.changes import ChangeError, parse
from tools.worldfmt.ids import content_id
from tools.worldfmt.store import is_link

REPORT_JSON = "refresh-report.json"
REPORT_MD = "refresh-report.md"
SETS = "sets"
CHANGE_JSON = "change.json"


class PlanFileError(ValueError):
    """A plan directory is incomplete or disagrees with its report."""


@dataclass(frozen=True)
class PlannedBundle:
    """One set of a written plan: its bundle directory and what the report
    predicts for it."""

    path: str
    """POSIX path relative to the plan directory, such as `sets/001`."""
    directory: Path
    change: dict
    change_id: str
    base_world_id: str
    world_after: str


@dataclass(frozen=True)
class WrittenPlan:
    root: Path
    report: dict
    report_id: str
    bundles: tuple[PlannedBundle, ...]

    @property
    def final_world_id(self) -> str:
        return self.report["final_world_id"]


def read_plan(root: Path | str) -> WrittenPlan:
    """Reads the plan in `root`: the report (canonical JSON that passes
    report.schema.json) and every set it lists, whose `change.json` must
    hash to the listed change id and name the listed base world, and whose
    `objects/` may hold only regular files of at most
    `common.MAX_INPUT_BYTES` besides directories (`verify` reads them
    later). Raises PlanFileError, naming the file relative to `root`."""
    from tools.ingest.changes.report import schema_errors

    root = Path(root)
    try:
        data = read_input(root / REPORT_JSON)
    except UnreadableFile as e:
        raise PlanFileError(f"{REPORT_JSON}: {e.why}") from e
    try:
        report = cjson.loads(data)
    except cjson.CanonicalError as e:
        raise PlanFileError(f"{REPORT_JSON}: not canonical JSON: {e}") from e
    errors = schema_errors(report) if isinstance(report, dict) else ["not a JSON object"]
    if errors:
        raise PlanFileError(f"{REPORT_JSON}: {'; '.join(errors)}")
    bundles = []
    for s in report["sets"]:
        directory = root.joinpath(*s["path"].split("/"))
        try:
            change_bytes = read_input(directory / CHANGE_JSON)
        except UnreadableFile as e:
            raise PlanFileError(f"{s['path']}/{CHANGE_JSON}: {e.why}") from e
        if content_id(change_bytes) != s["change_id"]:
            raise PlanFileError(f"{s['path']}/{CHANGE_JSON} does not hash to {s['change_id']}")
        try:
            change = parse(change_bytes)
        except ChangeError as e:
            raise PlanFileError(f"{s['path']}/{CHANGE_JSON}: {e}") from e
        if change["base_world_id"] != s["base_world_id"]:
            raise PlanFileError(f"{s['path']}/{CHANGE_JSON} names another base world than the report")
        _check_objects(directory, s["path"])
        bundles.append(PlannedBundle(s["path"], directory, change, s["change_id"], s["base_world_id"],
                                     s["world_after"]))
    return WrittenPlan(root, report, content_id(data), tuple(bundles))


def _check_objects(directory: Path, path: str) -> None:
    """Refuses what `read_input` would refuse among the files under
    `<directory>/objects` (the set `path`): a link, a FIFO or a device, or
    a file larger than `common.MAX_INPUT_BYTES`. Directories are searched
    (`common.entries_under`); a link to one, or a Windows junction, is
    refused, and so is a link at `sets`, at the set's directory or at its
    `objects`, which would lead `verify` out of the plan. Names and
    contents are `verify`'s to check."""
    parts = [*path.split("/"), "objects"]
    at = directory.parents[len(parts) - 2]  # the plan's directory
    for k, part in enumerate(parts):
        at = at / part
        try:
            info = at.lstat()
        except OSError:  # missing: no objects; unreadable: verify's own read will say so
            return
        if is_link(info):
            raise PlanFileError(f"{'/'.join(parts[: k + 1])} is not a directory; a plan holds its sets and their "
                                "objects in directories, not in links to them")
    top = directory / "objects"
    for entry in entries_under(top):
        try:
            info = entry.lstat()
        except OSError:  # removed meanwhile: verify's own read will say so
            continue
        rel = printable(PurePosixPath(path, *entry.relative_to(directory).parts).as_posix())
        if not stat.S_ISREG(info.st_mode) or is_link(info):
            raise PlanFileError(f"{rel}: not a regular file")
        if info.st_size > common.MAX_INPUT_BYTES:
            raise PlanFileError(f"{rel}: larger than {common.MAX_INPUT_BYTES} bytes")
