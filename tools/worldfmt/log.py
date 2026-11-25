"""The world log (RFC 0002 section 6) in a local directory.

The layout is option A of ADR-0010 (docs/adr/0010-hosting-the-canonical-world.md)
with the two changes ADR-0020 proposes (section 4). ADR-0010 is
still Proposed, so this layout is a proposal too:

    log/00000000.json            merge records in log order; the first is the baseline
    changes/<hex>/change.json    the change set of every merged or takedown record
    reports/<hex>.json           the gate's check reports
    worlds/<hex>.json            world manifests (optional)

`<hex>` is a content id without its `blake3:` prefix, because `:` is not
allowed in Windows file names. The `*_path` functions give these paths.

`read_log` reads a log and checks it:
- record files are named `log/NNNNNNNN.json`, numbered from 00000000
  without gaps, and `log/` holds nothing else;
- every record is canonical JSON and passes `record_problems`, a mirror
  of `MergeRecord::problems` (crates/op_world/src/contrib.rs) on top of
  the merge-record schema;
- the records form a chain, as `verify_log` checks it, with op_world's
  messages;
- every merged or takedown record's change set is present, hashes to the
  record's `change_id` and passes `changes.problems`.
Check reports and world manifests are read only on request (`report`,
`world`).

Typical use:

    log = read_log(log_dir)
    if log.world_id != current_world_id: ...      # the log must end at the store
    for (layer, feature_id), touches in log.last_touches().items():
        last = touches[0].record                  # the last record that changed the key
        last.decision, last.author, [t.op for t in touches]
"""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any

from tools.worldfmt import changes, cjson, schemas
from tools.worldfmt.changes import is_feature_id, is_sorted_unique, is_text, rust_debug
from tools.worldfmt.ids import content_id, hex_of

FORMAT = "streetwide.merge"
SCHEMA_FILE = "merge-record.schema.json"
DECISIONS = ("baseline", "merged", "takedown")
VERDICTS = ("better", "not_better", "blocked")

_LINKS = ("change_id", "world_before", "check_report_id", "previous_record_id")


class LogError(ValueError):
    """The world log is incomplete or breaks a rule; the message says where."""


# ----------------------------------------------------------------- paths --


def record_path(index: int) -> str:
    """`log/NNNNNNNN.json`: where record `index` (0 for the baseline) lives."""
    if index < 0:
        raise ValueError("record numbers start at 0")
    return PurePosixPath("log", f"{index:08d}.json").as_posix()


def change_path(change_id: str) -> str:
    """`changes/<hex>/change.json`: where the change set `change_id` lives."""
    return PurePosixPath("changes", hex_of(change_id), "change.json").as_posix()


def report_path(report_id: str) -> str:
    """`reports/<hex>.json`: where the check report `report_id` lives."""
    return PurePosixPath("reports", f"{hex_of(report_id)}.json").as_posix()


def world_path(world_id: str) -> str:
    """`worlds/<hex>.json`: where the world manifest `world_id` lives."""
    return PurePosixPath("worlds", f"{hex_of(world_id)}.json").as_posix()


# ----------------------------------------------------------------- rules --


def record_problems(record: Any) -> list[str]:
    """Every rule a merge record breaks; [] when op_world accepts it. As
    `changes.problems`: the schema's messages (starting with "schema: "),
    then `ContentId::parse`'s for the ids, then `MergeRecord::problems`,
    message for message."""
    found = changes.schema_problems(SCHEMA_FILE, record)
    if not isinstance(record, dict):
        return found
    for field in (*_LINKS, "world_after"):
        found += changes.content_id_problem(field, record.get(field))
    # `MergeRecord::problems`, in its order and words.
    if record.get("format") != FORMAT:
        found.append(f"format must be {rust_debug(FORMAT)}")
    if record.get("contract_version") != changes.CONTRIBUTION_CONTRACT_VERSION:
        found.append(f"unknown contribution contract version {rust_debug(record.get('contract_version'))}")
    if not is_text(record.get("reasons")):
        found.append("reasons must be non-empty text")
    decision = record.get("decision")
    absent = [record.get(field) is None for field in _LINKS]
    if decision == "baseline" and not all(absent):
        found.append("a baseline record has no change, previous world, report or previous record")
    if decision in ("merged", "takedown") and any(absent):
        found.append(
            "merged and takedown records name the change, both worlds, the report and the previous record"
        )
    reviews = record.get("reviews")
    reviews = [r for r in reviews if isinstance(r, dict)] if isinstance(reviews, list) else []
    reviewers = [r.get("reviewer") for r in reviews]
    if all(isinstance(r, str) for r in reviewers) and not is_sorted_unique(
        [r.encode("utf-8") for r in reviewers]
    ):
        found.append("reviews must be sorted by reviewer, one per reviewer")
    for r in reviews:
        if not is_feature_id(r.get("reviewer")) or not is_text(r.get("notes")):
            found.append(f"review by {rust_debug(r.get('reviewer'))}: reviewer must be a handle and notes text")
    verdicts = [r.get("verdict") for r in reviews]
    if decision == "merged" and "better" not in verdicts:
        found.append("a merge needs at least one review judging the change better")
    if decision == "merged" and "blocked" in verdicts:
        found.append("a blocked change cannot be merged")
    return found


def parse_record(data: bytes) -> dict:
    """Reads the bytes of a merge record as `MergeRecord::from_bytes` does:
    canonical JSON, then `record_problems`. Raises LogError."""
    try:
        record = cjson.loads(data)
    except cjson.CanonicalError as e:
        raise LogError(f"not canonical JSON: {e}") from e
    found = record_problems(record)
    if found:
        raise LogError("; ".join(found))
    return record


def verify_log(records: Sequence[tuple[str, dict]]) -> str:
    """Checks that `(record id, record)` pairs form a world log, as
    `verify_log` in crates/op_world/src/contrib.rs does: a baseline first,
    then each record continuing from the previous one. Returns the current
    world, the last record's `world_after`. Raises LogError with op_world's
    messages ("world log entry <i> (<id>): ...")."""
    previous: tuple[str, dict] | None = None
    for i, (record_id, r) in enumerate(records):
        where = f"world log entry {i} ({record_id})"
        if previous is None:
            if r["decision"] != "baseline":
                raise LogError(f"{where}: the log must start with a baseline")
        elif r["decision"] == "baseline":
            raise LogError(f"{where}: only the first record is a baseline")
        else:
            prev_id, prev = previous
            if r["previous_record_id"] != prev_id:
                raise LogError(f"{where}: previous_record_id does not name the previous record")
            if r["world_before"] != prev["world_after"]:
                raise LogError(f"{where}: world_before is not the previous record's world_after")
        previous = (record_id, r)
    if previous is None:
        raise LogError("the world log is empty")
    return previous[1]["world_after"]


# ---------------------------------------------------------------- reading --


@dataclass(frozen=True, eq=False)
class Record:
    """One merge record of a log read by `read_log`.

    `index` is its position (0 for the baseline), `path` its file
    (`log/NNNNNNNN.json`), `record_id` the content id of its bytes and
    `record` the document. `change` is its change set, read from
    `changes/<hex>/change.json` and checked against `change_id`; None for
    the baseline. Do not modify the documents. Records compare and hash by
    identity, so `Touch` values can go in sets.
    """

    index: int
    path: str
    record_id: str
    record: dict
    change: dict | None

    @property
    def decision(self) -> str:
        """`baseline`, `merged` or `takedown`."""
        return self.record["decision"]

    @property
    def change_id(self) -> str | None:
        return self.record["change_id"]

    @property
    def world_before(self) -> str | None:
        return self.record["world_before"]

    @property
    def world_after(self) -> str:
        return self.record["world_after"]

    @property
    def author(self) -> str | None:
        """The change set's author, such as `streetwide:ingest`; None for
        the baseline."""
        return None if self.change is None else self.change["author"]

    def touches(self) -> tuple[Touch, ...]:
        """What this record changed: one `Touch` per change of its change
        set, in the set's order. Empty for the baseline."""
        if self.change is None:
            return ()
        return tuple(
            Touch(
                record=self,
                op=c["op"],
                tile=c["tile"],
                layer=c["layer"],
                feature_id=c["feature_id"],
                parent_version_id=c["parent_version_id"],
                manifest_id=c["manifest_id"],
            )
            for c in self.change["changes"]
        )


@dataclass(frozen=True)
class Touch:
    """One change that a merged or takedown record applied: `op` (`add`,
    `replace` or `remove`) on the asset `(layer, feature_id)` in `tile`,
    from version `parent_version_id` (None for an add) to `manifest_id`
    (None for a remove)."""

    record: Record
    op: str
    tile: str
    layer: str
    feature_id: str
    parent_version_id: str | None
    manifest_id: str | None

    @property
    def key(self) -> tuple[str, str]:
        """`(layer, feature_id)`: assets are matched by this key."""
        return (self.layer, self.feature_id)


@dataclass(frozen=True)
class WorldLog:
    """A world log read and checked by `read_log`. `root` is its directory
    and `records` its records in log order; the first is the baseline."""

    root: Path
    records: tuple[Record, ...]

    @property
    def baseline(self) -> Record:
        return self.records[0]

    @property
    def head(self) -> Record:
        """The last record."""
        return self.records[-1]

    @property
    def world_id(self) -> str:
        """The current world by the log: the last record's `world_after`."""
        return self.head.world_after

    def touches(self) -> list[Touch]:
        """Every change of every merged or takedown record, in log order,
        then in the order of each change set."""
        return [t for r in self.records for t in r.touches()]

    def last_touches(self) -> dict[tuple[str, str], tuple[Touch, ...]]:
        """For each key `(layer, feature_id)` that a record changed, the
        changes of the last record that changed it, in its set's order.
        That is one change, or two for a move (a remove in one tile and an
        add in another). Keys are in byte order. A key that no record
        changed is absent: it is as the baseline made it."""
        last: dict[tuple[str, str], list[Touch]] = {}
        for t in self.touches():
            group = last.get(t.key)
            if group is None or group[0].record.index != t.record.index:
                last[t.key] = [t]
            else:
                group.append(t)
        ordered = sorted(last, key=lambda k: (k[0].encode("utf-8"), k[1].encode("utf-8")))
        return {k: tuple(last[k]) for k in ordered}

    def world(self, world_id: str) -> dict | None:
        """World manifest `world_id` from `worlds/<hex>.json`, or None if
        the log does not keep it."""
        return self._optional(world_path(world_id), world_id)

    def report(self, report_id: str) -> dict | None:
        """Check report `report_id` from `reports/<hex>.json`, or None if
        the log does not keep it."""
        return self._optional(report_path(report_id), report_id)

    def _optional(self, rel: str, cid: str) -> dict | None:
        path = self.root / rel
        if not path.is_file():
            return None
        data = path.read_bytes()
        if content_id(data) != cid:
            raise LogError(f"{rel} does not hash to {cid}")
        try:
            doc = cjson.loads(data)
        except cjson.CanonicalError as e:
            raise LogError(f"{rel}: not canonical JSON: {e}") from e
        errors = schemas.errors(doc) if isinstance(doc, dict) else ["not a JSON object"]
        if errors:
            raise LogError(f"{rel}: {'; '.join(sorted(errors))}")
        return doc


def read_log(root: Path | str) -> WorldLog:
    """Reads and checks the world log in directory `root` (see the module
    documentation). Raises LogError, naming files relative to `root`."""
    root = Path(root)
    log_dir = root / "log"
    names = sorted((p.name for p in log_dir.iterdir()), key=lambda s: s.encode("utf-8")) if log_dir.is_dir() else []
    if not names:
        raise LogError(f"the world log has no records: {record_path(0)}, the baseline record, is missing")
    for i, name in enumerate(names):
        expected = record_path(i)
        if f"log/{name}" != expected or not (log_dir / name).is_file():
            raise LogError(
                f"the world log has log/{name} where {expected} should be; "
                "records are numbered from 00000000, with no gaps and no other files"
            )

    read: list[tuple[str, str, dict]] = []
    for i in range(len(names)):
        path = record_path(i)
        data = (root / path).read_bytes()
        try:
            record = parse_record(data)
        except LogError as e:
            raise LogError(f"{path}: {e}") from e
        read.append((path, content_id(data), record))
    verify_log([(record_id, record) for _, record_id, record in read])

    records = []
    for i, (path, record_id, record) in enumerate(read):
        change = None if record["decision"] == "baseline" else _read_change(root, record["change_id"], path)
        records.append(Record(index=i, path=path, record_id=record_id, record=record, change=change))
    return WorldLog(root=root, records=tuple(records))


def _read_change(root: Path, change_id: str, record_file: str) -> dict:
    rel = change_path(change_id)
    try:
        data = (root / rel).read_bytes()
    except OSError:
        data = None
    if data is None or content_id(data) != change_id:
        raise LogError(
            f"the world log names change set {change_id} ({record_file}) "
            f"but {rel} is missing or has another id"
        )
    try:
        return changes.parse(data)
    except changes.ChangeError as e:
        raise LogError(f"{rel}: {e}") from e
