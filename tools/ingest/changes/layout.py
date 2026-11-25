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
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

from tools.worldfmt import cjson
from tools.worldfmt.changes import ChangeError, parse
from tools.worldfmt.ids import content_id

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
    hash to the listed change id and name the listed base world. Raises
    PlanFileError."""
    from tools.ingest.changes.report import schema_errors

    root = Path(root)
    try:
        data = (root / REPORT_JSON).read_bytes()
    except OSError as e:
        raise PlanFileError(f"{REPORT_JSON}: {e.strerror or e}") from e
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
            change_bytes = (directory / CHANGE_JSON).read_bytes()
        except OSError as e:
            raise PlanFileError(f"{s['path']}/{CHANGE_JSON}: {e.strerror or e}") from e
        if content_id(change_bytes) != s["change_id"]:
            raise PlanFileError(f"{s['path']}/{CHANGE_JSON} does not hash to {s['change_id']}")
        try:
            change = parse(change_bytes)
        except ChangeError as e:
            raise PlanFileError(f"{s['path']}/{CHANGE_JSON}: {e}") from e
        if change["base_world_id"] != s["base_world_id"]:
            raise PlanFileError(f"{s['path']}/{CHANGE_JSON} names another base world than the report")
        bundles.append(PlannedBundle(s["path"], directory, change, s["change_id"], s["base_world_id"],
                                     s["world_after"]))
    return WrittenPlan(root, report, content_id(data), tuple(bundles))
