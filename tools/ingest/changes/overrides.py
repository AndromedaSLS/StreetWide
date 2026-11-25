"""The overrides file (ADR-0020, section 6): the maintainer's holds, scoped accepts
and removal allowances, each with a reason.

    {
      "overrides_version": "0.1.0",
      "hold": [{"layer": "shell", "feature_id": "osm:way:123", "reason": "..."}],
      "accept": [{"layer": "shell", "feature_id": "...", "current_manifest_id": "blake3:...",
                  "content_key": "blake3:...", "lifts": ["unexplained-change"], "reason": "..."}],
      "allow_removals": [{"layer": "water", "dataset": "OpenStreetMap", "at_most": 8, "reason": "..."}]
    }

- A **hold** keeps a key's pending change out (`manual-hold`). Its scope
  fields, `current_manifest_id` and `content_key`, are optional: a content
  id scopes the hold to that version, so a hold with a `content_key` lasts
  until the upstream content changes. A missing or null scope field holds
  whatever the version.
- An **accept** must give both `current_manifest_id` (the current
  version) and `content_key` (the new version's, rule C), with null meaning
  "this version is absent". It applies only while both match, and lifts
  only the codes it names, all of them liftable.
- An **allowance** lifts `per-tile-removal` and `mass-removal` for the
  unpaired removals of one ingest group (a layer and a first source's
  dataset) while they number at most `at_most`, and clears the error for a
  group that vanished.

The file is ordinary JSON, not necessarily canonical. The report records
`overrides_id`, the content id of the document in canonical JSON, so the
same overrides give the same plan whatever the file's line endings, byte
order mark, spacing or key order, which differ between editors and
platforms. Everything that cannot be right in any plan is refused here,
as a PlanError (exit code 2): an unknown field or code, a missing reason
or scope, a code no accept can lift, a key both held and accepted,
duplicate entries, and a number canonical JSON cannot hold.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from tools.ingest.changes import codes
from tools.ingest.changes.common import Key, PlanError, key_order, show, text_order
from tools.worldfmt import cjson
from tools.worldfmt.changes import is_feature_id, is_text
from tools.worldfmt.ids import content_id, is_content_id

VERSION = "0.1.0"
"""`overrides_version` this planner reads."""

_SCOPE = ("current_manifest_id", "content_key")
_FIELDS = {
    "hold": {"layer", "feature_id", "current_manifest_id", "content_key", "reason"},
    "accept": {"layer", "feature_id", "current_manifest_id", "content_key", "lifts", "reason"},
    "allow_removals": {"layer", "dataset", "at_most", "reason"},
}
_ABSENT = object()


@dataclass(frozen=True)
class Hold:
    layer: str
    feature_id: str
    scope: dict[str, str]
    """The scope fields given as content ids; others hold any version."""
    reason: str

    @property
    def key(self) -> Key:
        return (self.layer, self.feature_id)


@dataclass(frozen=True)
class Accept:
    layer: str
    feature_id: str
    current_manifest_id: str | None
    content_key: str | None
    lifts: tuple[str, ...]
    reason: str

    @property
    def key(self) -> Key:
        return (self.layer, self.feature_id)


@dataclass(frozen=True)
class Allowance:
    layer: str
    dataset: str
    at_most: int
    reason: str

    @property
    def group(self) -> tuple[str, str]:
        return (self.layer, self.dataset)


@dataclass(frozen=True)
class Overrides:
    id: str | None
    """Content id of the document in canonical JSON, or None without a
    file (see `overrides_id`)."""
    holds: dict[Key, Hold] = field(default_factory=dict)
    accepts: dict[Key, Accept] = field(default_factory=dict)
    allowances: dict[tuple[str, str], Allowance] = field(default_factory=dict)


NONE = Overrides(id=None)


def _reject_duplicates(pairs: list[tuple[str, Any]]) -> dict:
    out: dict[str, Any] = {}
    for k, v in pairs:
        if k in out:
            raise PlanError(f"overrides: the field {json.dumps(k)} appears twice in one object")
        out[k] = v
    return out


def read_overrides(path: Path | None) -> Overrides:
    """Reads and checks the overrides file at `path` (None: no overrides).
    Raises PlanError with "overrides: ..." messages."""
    if path is None:
        return NONE
    try:
        data = Path(path).read_bytes()
    except OSError as e:
        raise PlanError(f"overrides: cannot read {path}: {e.strerror or e}") from e
    try:
        doc = json.loads(data.decode("utf-8-sig"), object_pairs_hook=_reject_duplicates)
    except (UnicodeDecodeError, json.JSONDecodeError) as e:
        raise PlanError(f"overrides: not valid JSON: {e}") from e
    return parse_overrides(doc)


def overrides_id(doc: dict) -> str:
    """The id the report records for an overrides document that
    `parse_overrides` accepted: the content id of its canonical JSON.
    Raises PlanError for what canonical JSON cannot hold, such as a lone
    surrogate escape (`\\ud800`) in a reason."""
    try:
        return content_id(cjson.dumps(doc))
    except (cjson.CanonicalError, UnicodeEncodeError) as e:
        raise PlanError(f"overrides: not representable as canonical JSON: {e}") from e


def parse_overrides(doc: Any) -> Overrides:
    if not isinstance(doc, dict):
        raise PlanError("overrides: the file must hold a JSON object")
    for name in sorted(doc, key=text_order):
        if name not in ("overrides_version", *_FIELDS):
            raise PlanError(f"overrides: unknown field {json.dumps(name)}")
    if doc.get("overrides_version") != VERSION:
        raise PlanError(f'overrides: overrides_version must be "{VERSION}"')
    entries = {}
    for kind in _FIELDS:
        items = doc.get(kind, [])
        if not isinstance(items, list):
            raise PlanError(f"overrides: {kind} must be a list")
        for i, item in enumerate(items):
            if not isinstance(item, dict):
                raise PlanError(f"overrides: {kind}[{i}] must be an object")
            for name in sorted(item, key=text_order):
                if name not in _FIELDS[kind]:
                    raise PlanError(f"overrides: {kind}[{i}] has an unknown field {json.dumps(name)}")
        entries[kind] = items

    holds: dict[Key, Hold] = {}
    for i, item in enumerate(entries["hold"]):
        key = _key(item, f"hold[{i}]")
        what = f"hold {show(key)}"
        scope = {}
        for name in _SCOPE:
            value = item.get(name, _ABSENT)
            if value is _ABSENT or value is None:
                continue
            if not (isinstance(value, str) and is_content_id(value)):
                raise PlanError(f"overrides: {what}: {name} must be a content id or null")
            scope[name] = value
        reason = _reason(item, what)
        if key in holds:
            raise PlanError(f"overrides: {show(key)} is held twice")
        holds[key] = Hold(key[0], key[1], scope, reason)

    accepts: dict[Key, Accept] = {}
    for i, item in enumerate(entries["accept"]):
        key = _key(item, f"accept[{i}]")
        what = f"accept {show(key)}"
        if any(name not in item for name in _SCOPE):
            raise PlanError(f"overrides: {what} needs current_manifest_id and content_key "
                            "(null where that version is absent)")
        for name in _SCOPE:
            value = item[name]
            if value is not None and not (isinstance(value, str) and is_content_id(value)):
                raise PlanError(f"overrides: {what}: {name} must be a content id or null")
        lifts = item.get("lifts")
        if not isinstance(lifts, list) or not lifts or not all(isinstance(c, str) for c in lifts):
            raise PlanError(f"overrides: {what} needs lifts, a list of the codes it lifts")
        for code in lifts:
            known = codes.BY_CODE.get(code)
            if known is None:
                raise PlanError(f"overrides: {what} lifts unknown code {json.dumps(code)}")
            if not known.liftable:
                raise PlanError(f"overrides: cannot lift {code} for {show(key)}: {known.why_not}")
        if len(set(lifts)) != len(lifts):
            raise PlanError(f"overrides: {what} names a code twice in lifts")
        reason = _reason(item, what)
        if key in accepts:
            raise PlanError(f"overrides: {show(key)} is accepted twice")
        accepts[key] = Accept(key[0], key[1], item["current_manifest_id"], item["content_key"],
                              tuple(codes.in_rule_order(lifts)), reason)

    allowances: dict[tuple[str, str], Allowance] = {}
    for i, item in enumerate(entries["allow_removals"]):
        layer, dataset = item.get("layer"), item.get("dataset")
        if not (is_text(layer) and is_text(dataset)):
            raise PlanError(f"overrides: allow_removals[{i}] needs a layer and a dataset")
        what = f"allow_removals {layer}/{dataset}"
        at_most = item.get("at_most")
        if not isinstance(at_most, int) or isinstance(at_most, bool) or not 0 <= at_most <= cjson.MAX_SAFE_INTEGER:
            raise PlanError(f"overrides: {what} needs at_most, a whole number of removals from 0 to "
                            f"{cjson.MAX_SAFE_INTEGER}")
        reason = _reason(item, what)
        if (layer, dataset) in allowances:
            raise PlanError(f"overrides: {layer}/{dataset} has two allow_removals entries")
        allowances[(layer, dataset)] = Allowance(layer, dataset, at_most, reason)

    for key in sorted(holds, key=key_order):
        if key in accepts:
            raise PlanError(f"overrides: {show(key)} is both held and accepted")
    return Overrides(
        id=overrides_id(doc),
        holds={k: holds[k] for k in sorted(holds, key=key_order)},
        accepts={k: accepts[k] for k in sorted(accepts, key=key_order)},
        allowances={g: allowances[g] for g in sorted(allowances, key=key_order)},
    )


def _key(item: dict, where: str) -> Key:
    layer, feature_id = item.get("layer"), item.get("feature_id")
    if not is_text(layer) or not is_feature_id(feature_id):
        raise PlanError(f"overrides: {where} needs a layer and a feature_id")
    return (layer, feature_id)


def _reason(item: dict, what: str) -> str:
    reason = item.get("reason")
    if not isinstance(reason, str) or not reason.strip():
        raise PlanError(f"overrides: {what} needs a reason")
    if not is_text(reason):
        raise PlanError(f"overrides: {what}: the reason must be one line of at most 4096 characters")
    return reason


def hold_matches(hold: Hold, current_manifest_id: str | None, new_content_key: str | None) -> str | None:
    """None when `hold` applies to a change between these versions, or why
    it no longer does."""
    if "current_manifest_id" in hold.scope and hold.scope["current_manifest_id"] != current_manifest_id:
        return "current_manifest_id no longer matches"
    if "content_key" in hold.scope and hold.scope["content_key"] != new_content_key:
        return "content_key no longer matches"
    return None


def accept_matches(accept: Accept, current_manifest_id: str | None, new_content_key: str | None) -> str | None:
    """None when `accept` applies to a change between these versions, or why
    it no longer does."""
    if accept.current_manifest_id != current_manifest_id:
        return "current_manifest_id no longer matches"
    if accept.content_key != new_content_key:
        return "content_key no longer matches"
    return None
