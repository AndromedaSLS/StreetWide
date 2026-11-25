"""Plan-time checks, pure Python, always run before anything
is written. A failure is a bug in the planner: it raises InternalError
("internal error: ...") and the plan is not written.

1. Contract mirrors: every set passes `tools.worldfmt.changes.problems`,
   and every manifest it ships passes the schema and agrees with its
   change (tile, layer, feature id and parent).
2. Objects: every object a set names is in its bundle or the current store.
3. Sets: every emitted unit lies in exactly one set, no set mixes
   purposes, each decision set holds exactly one accepted unit, no set
   passes 50 changes, and only a lone unit passes `--max-set-bytes`.
4. Chain: each set's base is the world before it, and applying every set
   at once gives the same final world as applying them one by one.
5. Residual: the final world differs from the new store, key by key and
   ignoring `parent_version_id`, only where a recorded cause says so: a
   provenance-only difference kept, a conflict, a held-back change, or a
   contribution kept.
"""

from __future__ import annotations

from tools.ingest.changes.classify import Classification
from tools.ingest.changes.common import InternalError, Key, key_order, show
from tools.ingest.changes.holds import is_decision_unit
from tools.ingest.changes.sets import CHANGE_JSON_RESERVE, PlannedSet
from tools.ingest.changes.stores import StoreView, without_parent
from tools.worldfmt import cjson, schemas
from tools.worldfmt.changes import MAX_CHANGES, ChangeError, Objects, apply, problems
from tools.worldfmt.ids import content_id, hex_of


def _fail(message: str) -> None:
    raise InternalError(f"internal error: {message}")


def check_sets(sets: list[PlannedSet], cur: StoreView, cls: Classification, max_set_bytes: int | None) -> None:
    seen: dict[int, str] = {}
    for s in sets:
        found = problems(s.change)
        if found:
            _fail(f"{s.path}/change.json: {'; '.join(found)}")
        if content_id(s.change_bytes) != s.change_id or cjson.dumps(s.change) != s.change_bytes:
            _fail(f"{s.path}/change.json does not hash to its change id")
        if len(s.entries) > MAX_CHANGES:
            _fail(f"{s.path} has {len(s.entries)} changes")
        names = {path.split("/")[-1] for path in s.objects}
        for e in s.entries:
            what = f"{s.path}: {e['tile']} {e['layer']} {e['feature_id']}"
            if e["manifest_id"] is None:
                continue
            data = s.manifests.get(e["manifest_id"])
            if data is None or content_id(data) != e["manifest_id"]:
                _fail(f"{what}: the manifest is missing")
            manifest = cjson.loads(data)
            errors = schemas.errors(manifest)
            if errors:
                _fail(f"{what}: the manifest breaks the schema: {'; '.join(sorted(errors))}")
            if (manifest["tile"], manifest["layer"], manifest["feature_id"], manifest["parent_version_id"]) != \
                    (e["tile"], e["layer"], e["feature_id"], e["parent_version_id"]):
                _fail(f"{what}: the manifest does not agree with its change")
            if f"{hex_of(e['manifest_id'])}.json" not in names and not cur.has_object(e["manifest_id"], "json"):
                _fail(f"{what}: neither the bundle nor the current store has its manifest")
            content = manifest["content_id"]
            if not any(n.startswith(hex_of(content) + ".") for n in names) and not cur.object_names(content):
                _fail(f"{what}: neither the bundle nor the current store has its content {content}")
        purposes = {u.purpose for u in s.units}
        if len(purposes) != 1 or s.purpose not in purposes:
            _fail(f"{s.path} mixes purposes")
        if s.kind == "decision" and (len(s.units) != 1 or not is_decision_unit(s.units[0])):
            _fail(f"{s.path} is a decision set without exactly one accepted unit")
        if s.kind == "refresh" and any(is_decision_unit(u) for u in s.units):
            _fail(f"{s.path} holds an accepted change to a contribution")
        if max_set_bytes is not None and s.bytes > max_set_bytes and len(s.units) > 1:
            _fail(f"{s.path} has {s.bytes} bytes, more than --max-set-bytes {max_set_bytes}")
        if max_set_bytes is not None and s.bytes > max_set_bytes and not s.warnings:
            _fail(f"{s.path} passes --max-set-bytes without a warning")
        for u in s.units:
            if id(u) in seen:
                _fail(f"a unit lies in {seen[id(u)]} and {s.path}")
            seen[id(u)] = s.path
    for u in cls.units:
        if u.emitted and id(u) not in seen:
            _fail(f"the unit of {show(u.items[0].key)} is emitted but in no set")
        if not u.emitted and id(u) in seen:
            _fail(f"the unit of {show(u.items[0].key)} is held but in {seen[id(u)]}")
    assert CHANGE_JSON_RESERVE > 0


def check_chain(sets: list[PlannedSet], cur: StoreView, final_world_id: str, objects: Objects) -> None:
    previous = cur.world_id
    for s in sets:
        if s.base_world_id != previous or s.change["base_world_id"] != previous:
            _fail(f"{s.path} is planned on {s.base_world_id}, but the world before it is {previous}")
        previous = s.world_after
    if previous != final_world_id:
        _fail(f"the last set gives {previous}, not the final world {final_world_id}")
    if not sets:
        if final_world_id != cur.world_id:
            _fail("no set, but the final world is not the current world")
        return
    every = {"changes": [e for s in sets for e in s.entries]}
    try:
        at_once = apply(cur.world, every, objects)
    except ChangeError as e:
        _fail(f"all sets at once cannot be applied: {e}")
    if at_once.world_id != final_world_id:
        _fail(f"all sets at once give {at_once.world_id}, but one by one they give {final_world_id}")


def check_residual(cur: StoreView, new: StoreView, cls: Classification, final_world: dict, objects: Objects) -> None:
    final: dict[Key, tuple[str, str, dict]] = {}
    for wt in final_world["tiles"]:
        for a in objects.tile_index(wt["tile_id"])["assets"]:
            final[(a["layer"], a["feature_id"])] = (wt["tile"], a["manifest_id"], objects.manifest(a["manifest_id"]))
    items = {i.key: i for i in cls.items}
    kept = set(cls.kept)
    for key in sorted(set(final) | set(new.assets), key=key_order):
        f, n, c = final.get(key), new.assets.get(key), cur.assets.get(key)
        item = items.get(key)
        unchanged = (f is None and c is None) or (
            f is not None and c is not None and (f[0], f[1]) == (c.tile, c.manifest_id))
        if item is not None and not item.emitted and not unchanged:
            _fail(f"{show(key)} is held but changed")
        if f is not None and n is not None and f[0] == n.tile and without_parent(f[2]) == without_parent(n.manifest):
            continue
        if item is not None and not item.emitted:
            continue
        if unchanged and (key in cls.provenance_only_keys or key in kept):
            continue
        _fail(f"{show(key)} differs from the new store with no recorded reason")
