"""Bundles, packing and the chain of world ids (ADR-0020, section 5).

What a bundle holds:
- `add`: the new store's manifest bytes, unchanged (the ingest writes
  `parent_version_id: null`, which an add needs), so the manifest id is the
  new store's;
- `replace`: the new store's manifest with only `parent_version_id`
  rewritten to the current manifest id;
- `remove`: nothing.
A manifest or a content object goes into the bundle only when the current
store lacks it, and content keeps the extension it has in the new store.

Packing: every unit has a purpose `(collider, layer, dataset)`;
purposes go `none` before `trimesh`, then by layer and dataset in byte
order, and units by their smallest change key within a purpose. Packing is
greedy: a new set starts when the purpose changes, or when the next unit
would pass 50 changes or `--max-set-bytes`. A unit is never split. Decision
sets, one per accepted unit, come last.

Set k is planned on the world that sets 1 to k-1 produce, computed
with `tools.worldfmt.changes.apply`, the mirror of the gate's candidate
builder.
"""

from __future__ import annotations

import copy
from dataclasses import dataclass, field
from pathlib import PurePosixPath

from tools.ingest.changes import VERSION as TOOL_VERSION
from tools.ingest.changes import COMMAND, codes
from tools.ingest.changes.classify import Item, Unit
from tools.ingest.changes.common import InternalError, PlanError, clip, counted, joined, show, text_order
from tools.ingest.changes.holds import HoldResult, is_decision_unit
from tools.ingest.changes.stores import SharedObjects, StoreView
from tools.worldfmt import cjson
from tools.worldfmt.changes import (
    CONTRIBUTION_CONTRACT_VERSION, FORMAT, INGEST_AUTHOR, MAX_CHANGES, MAX_TEXT_CHARS, ChangeError, Objects,
    apply, sort_key, to_bytes,
)
from tools.worldfmt.ids import content_id, hex_of
from tools.worldfmt.store import Store

CHANGE_JSON_RESERVE = 1024 + 2 * 4 * MAX_TEXT_CHARS
"""Bytes reserved for `change.json` beyond its change entries when packing
by `--max-set-bytes`: its fixed fields, and a title and a rationale of at
most 4096 characters of up to 4 bytes each. So a packed set never passes
the limit."""


@dataclass
class Emitted:
    """What one item puts into a bundle: its change entries, and the objects
    the current store lacks."""

    entries: list[dict]
    objects: dict[str, bytes]
    """By path in the bundle, `objects/<aa>/<hex>.<ext>`."""
    manifests: dict[str, bytes]
    """Every manifest the entries name, by id (also those the current store
    has), for the chain of world ids."""

    @property
    def bytes(self) -> int:
        return sum(len(cjson.dumps(e)) + 1 for e in self.entries) + sum(len(b) for b in self.objects.values())


@dataclass
class PlannedSet:
    number: int
    path: str
    kind: str
    """`refresh`, or `decision` for an accepted change to a contribution."""
    purpose: tuple[str, str, str]
    units: list[Unit]
    entries: list[dict] = field(default_factory=list)
    objects: dict[str, bytes] = field(default_factory=dict)
    manifests: dict[str, bytes] = field(default_factory=dict)
    change: dict | None = None
    change_bytes: bytes = b""
    change_id: str = ""
    base_world_id: str = ""
    world_after: str = ""
    warnings: list[str] = field(default_factory=list)

    @property
    def size(self) -> int:
        return sum(u.size for u in self.units)

    @property
    def bytes(self) -> int:
        return len(self.change_bytes) + sum(len(b) for b in self.objects.values())

    @property
    def items(self) -> list[Item]:
        return [i for u in self.units for i in u.items]

    @property
    def touches_colliders(self) -> bool:
        return any(a.collider == "trimesh" for i in self.items for a in (i.cur, i.new) if a is not None)


def object_path(cid: str, ext: str) -> str:
    h = hex_of(cid)
    return PurePosixPath("objects", h[:2], f"{h}.{ext}").as_posix()


def check_content(item: Item, new: StoreView) -> None:
    """What the gate checks of a new version's content when it reads the
    manifest: the content hashes to its id and has `size_bytes` bytes, and
    a mesh reads under the strict glb profile and spans the manifest's
    `bounds_mm`. Otherwise the gate could not read the manifest, and the
    plan would predict ids that no gate run gives. A mesh that the current
    version of the key already has is the canonical store's, so only the
    bounds are compared. Raises PlanError naming the asset."""
    ext, _ = new.content(item.new)
    if ext != "glb":
        return
    if item.cur is not None and item.cur.content_id == item.new.content_id:
        bounds = item.cur.manifest["bounds_mm"]
    else:
        bounds = new.mesh(item.new).bounds_mm()
    if bounds != item.new.manifest["bounds_mm"]:
        raise PlanError(f"the new store: tile {item.new.tile}, {show(item.key)}: bounds_mm does not match the mesh")


def emit(item: Item, cur: StoreView, new: StoreView) -> Emitted:
    """The change entries and objects of one item. Raises PlanError
    when the new version's content is not what its manifest says
    (`check_content`)."""
    layer, fid = item.key
    out = Emitted([], {}, {})

    def place(op: str, parent: str | None) -> None:
        check_content(item, new)
        if op == "add":
            data = new.objects.read(item.new.manifest_id)
            manifest_id = item.new.manifest_id
        else:
            manifest = copy.deepcopy(item.new.manifest)
            manifest["parent_version_id"] = parent
            data = cjson.dumps(manifest)
            manifest_id = content_id(data)
        out.entries.append({"op": op, "tile": item.new.tile, "layer": layer, "feature_id": fid,
                            "parent_version_id": parent, "manifest_id": manifest_id})
        out.manifests[manifest_id] = data
        if not cur.has_object(manifest_id, "json"):
            out.objects[object_path(manifest_id, "json")] = data
        if not cur.object_names(item.new.content_id):
            ext, content = new.content(item.new)
            out.objects[object_path(item.new.content_id, ext)] = content

    if item.op in ("remove", "move"):
        out.entries.append({"op": "remove", "tile": item.cur.tile, "layer": layer, "feature_id": fid,
                            "parent_version_id": item.cur.manifest_id, "manifest_id": None})
    if item.op in ("add", "move"):
        place("add", None)
    if item.op == "replace":
        place("replace", item.cur.manifest_id)
    return out


def purpose_order(purpose: tuple[str, str, str]) -> tuple[bytes, bytes, bytes]:
    return tuple(text_order(p) for p in purpose)


def pack(units: list[Unit], emitted: dict[int, Emitted], max_set_bytes: int | None) -> list[PlannedSet]:
    """Greedy packing of the emitted units into sets; decision units
    get a set each, after the others. Paths and ids are filled in later."""
    ordinary = sorted((u for u in units if not is_decision_unit(u)),
                      key=lambda u: (purpose_order(u.purpose), u.smallest))
    decisions = sorted((u for u in units if is_decision_unit(u)), key=lambda u: u.smallest)

    def unit_bytes(u: Unit) -> int:
        return sum(emitted[id(i)].bytes for i in u.items)

    sets: list[PlannedSet] = []
    current: PlannedSet | None = None
    estimate = 0
    for u in ordinary:
        size, extra = u.size, unit_bytes(u)
        if current is None or current.purpose != u.purpose or current.size + size > MAX_CHANGES or (
                max_set_bytes is not None and estimate + extra > max_set_bytes):
            current = PlannedSet(0, "", "refresh", u.purpose, [])
            sets.append(current)
            estimate = CHANGE_JSON_RESERVE
        current.units.append(u)
        estimate += extra
        if max_set_bytes is not None and len(current.units) == 1 and estimate > max_set_bytes:
            current.warnings.append(f"one unit of about {extra} bytes, more than --max-set-bytes {max_set_bytes}")
    for u in decisions:
        s = PlannedSet(0, "", "decision", u.purpose, [u])
        if max_set_bytes is not None and CHANGE_JSON_RESERVE + unit_bytes(u) > max_set_bytes:
            s.warnings.append(f"one unit of about {unit_bytes(u)} bytes, more than --max-set-bytes {max_set_bytes}")
        sets.append(s)
    width = max(3, len(str(len(sets))))
    for k, s in enumerate(sets, 1):
        s.number = k
        s.path = PurePosixPath("sets", f"{k:0{width}d}").as_posix()
        for u in s.units:
            for i in u.items:
                e = emitted[id(i)]
                s.entries.extend(e.entries)
                s.objects.update(e.objects)
                s.manifests.update(e.manifests)
        s.entries.sort(key=sort_key)
        s.objects = dict(sorted(s.objects.items(), key=lambda kv: text_order(kv[0])))
    return sets


@dataclass(frozen=True)
class Context:
    """What titles and rationales say about the whole refresh."""

    world: str
    current_world_id: str
    new_world_id: str
    conflicts: int
    held: int
    overrides_id: str | None
    authors: dict = field(default_factory=dict)
    """Authors of each contribution, by key."""
    removers: dict = field(default_factory=dict)
    """Who removed each key that a contribution removed, by key."""


def chain(sets: list[PlannedSet], cur: StoreView, holds: HoldResult, ctx: Context) -> tuple[str, dict, Objects]:
    """Fills in each set's change set and ids: set 1 is planned on the
    current world, set k on the world sets 1 to k-1 produce. Returns the
    final world id and manifest, and the objects that read every world of
    the chain."""
    objects = SharedObjects(Store(cur.root), checked=cur.objects.checked)
    world_id, world = cur.world_id, cur.world
    accepted = {id(a["item"]): a for a in holds.accepted}
    for s in sets:
        s.base_world_id = world_id
        if s.kind == "refresh":
            title = _title(ctx, s, len(sets))
        else:
            title = _decision_title(ctx, s, accepted)
        s.change = {
            "format": FORMAT,
            "contract_version": CONTRIBUTION_CONTRACT_VERSION,
            "base_world_id": world_id,
            "title": title,
            "author": INGEST_AUTHOR,
            "rationale": _rationale(ctx, s, accepted),
            "changes": s.entries,
        }
        try:
            s.change_bytes = to_bytes(s.change)
        except ChangeError as e:
            raise InternalError(f"internal error: {s.path}/change.json: {e}") from e
        s.change_id = content_id(s.change_bytes)
        objects.push(dict(sorted(s.manifests.items())))
        try:
            candidate = apply(world, s.change, objects)
        except ChangeError as e:
            raise InternalError(f"internal error: {s.path} cannot be applied: {e}") from e
        objects.push(candidate.objects)
        world_id, world = candidate.world_id, candidate.world
        s.world_after = world_id
        for a in (accepted.get(id(i)) for i in s.items):
            if a is not None:
                a["set"] = s.path
    return world_id, world, objects


# --------------------------------------------------- titles and rationales --


def _title(ctx: Context, s: PlannedSet, total: int) -> str:
    collider, layer, dataset = s.purpose
    colliders = ", with colliders" if collider == "trimesh" else ""
    return clip(f"Baseline refresh {ctx.world}: {layer} from {dataset}{colliders} (set {s.number} of {total})",
                MAX_TEXT_CHARS)


def _decision_title(ctx: Context, s: PlannedSet, accepted: dict[int, dict]) -> str:
    items = [i for i in s.items if id(i) in accepted and set(accepted[id(i)]["lifted"]) & set(codes.DECISION)]
    first = items[0] if items else s.items[0]
    lifted = accepted[id(first)]["lifted"] if id(first) in accepted else []
    by = joined(ctx.authors.get(first.key) or ["an unknown author"])
    layer, fid = first.key
    if "takedown-revert" in lifted:
        action = f"{first.op} the {layer} {fid} that a takedown reverted"
    elif "removed-by-contribution" in lifted:
        action = f"add back the {layer} {fid} that {ctx.removers.get(first.key) or 'a contribution'} removed"
    elif "contribution" in lifted:
        action = f"{first.op} the contributed {layer} {fid} by {by}"
    else:
        action = f"{first.op} the {layer} {fid}, whose origin is unclear"
    more = s.size - first.size
    tail = f" and {counted(more, 'more change')}" if more else ""
    return clip(f"Baseline refresh {ctx.world}: {action}{tail} (maintainer decision)", MAX_TEXT_CHARS)


def _sources(assets) -> str:
    pairs = sorted({(s["dataset"], s["version"]) for a in assets for s in a.sources},
                   key=lambda p: (text_order(p[0]), text_order(p[1])))
    return joined([f"{d} {v}" for d, v in pairs])


def _rationale(ctx: Context, s: PlannedSet, accepted: dict[int, dict]) -> str:
    collider, layer, _ = s.purpose
    ops = [f"{sum(1 for e in s.entries if e['op'] == op)} {op}" for op in ("add", "replace", "remove")
           if any(e["op"] == op for e in s.entries)]
    placed = [i.new for i in s.items if i.op != "remove"]
    if placed:
        origin = f"from {_sources(placed)}"
    else:
        origin = f"of assets from {_sources([i.cur for i in s.items])} that are gone upstream"
    text = (
        f"Made by {COMMAND} {TOOL_VERSION} under RFC 0002 section 7 from a new ingest (world "
        f"{ctx.new_world_id}) of world {ctx.current_world_id}. This set: {', '.join(ops)} in layer {layer}, "
        f"collider {collider}, {origin}; it was planned on world {s.base_world_id}, and its predicted ids hold "
        f"only there (merge it with --require-base). Assets made by the ingest are replaced only when their "
        f"content changed; contributions are never replaced without a maintainer's decision; "
        f"{counted(ctx.conflicts, 'conflict')} and {counted(ctx.held, 'held-back change')} are listed in the "
        f"refresh report."
    )
    if s.kind == "decision":
        decided = [accepted[id(i)] for i in s.items if id(i) in accepted]
        lead = (f" Maintainer decision (overrides file {ctx.overrides_id}): "
                f"{joined([a['layer'] + ' ' + a['feature_id'] for a in decided])}, lifting "
                f"{joined(codes.in_rule_order({c for a in decided for c in a['lifted']}))}, because: \"")
        room = MAX_TEXT_CHARS - len(text) - len(lead) - 2
        reason = " / ".join(a["reason"] for a in decided)
        text += lead + clip(reason, max(room, 0)) + "\"."
    return clip(text, MAX_TEXT_CHARS)
