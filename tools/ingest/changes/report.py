"""The refresh report (ADR-0020, Q11): `refresh-report.json`, canonical JSON in
the tool's own format (`report.schema.json`, versioned by
`report_version`), and `refresh-report.md`, the same report for people,
generated from the JSON in a fixed order.

Neither is a contract document: `op_world` and the gate never read them,
and neither goes into a bundle. The JSON is hashable, so a merge record's
reasons can cite its content id; the change sets do not cite it, because
the report lists their ids.
"""

from __future__ import annotations

import json
from collections import Counter
from dataclasses import dataclass
from functools import cache
from pathlib import Path

from tools.ingest.changes import COMMAND, VERSION, codes
from tools.ingest.changes.classify import Classification, Item
from tools.ingest.changes.common import counted, key_order, unique_sorted
from tools.ingest.changes.holds import HoldResult, ground_under
from tools.ingest.changes.overrides import VERSION as OVERRIDES_VERSION
from tools.ingest.changes.overrides import Overrides
from tools.ingest.changes.sets import PlannedSet
from tools.ingest.changes.stores import DEPENDS_ON, StoreView, generator_text
from tools.worldfmt import cjson
from tools.worldfmt.changes import tile_xy
from tools.worldfmt.log import WorldLog

FORMAT = "streetwide refresh plan"
REPORT_VERSION = "0.1.0"
CONTENT_RULE = "mesh and manifest, without parent_version_id, generator.version and sources[].version"
SCHEMA_FILE = Path(__file__).with_name("report.schema.json")


@cache
def schema() -> dict:
    return json.loads(SCHEMA_FILE.read_text(encoding="utf-8"))


def schema_errors(report: dict) -> list[str]:
    from jsonschema import Draft202012Validator

    validator = Draft202012Validator(schema())
    return sorted(f"{e.json_path}: {e.message}" for e in validator.iter_errors(report))


@dataclass
class State:
    """Everything the report is made from."""

    cur: StoreView
    new: StoreView
    wlog: WorldLog
    log_warnings: list[str]
    ov: Overrides
    cls: Classification
    holds: HoldResult
    sets: list[PlannedSet]
    final_world_id: str
    final_world: dict
    max_set_bytes: int | None


def entry(item: Item) -> dict:
    """A conflict or held-back entry: the scope an accept needs, and every
    code still on the change, in rule order."""
    tile = item.cur.tile if item.cur is not None else item.new.tile
    new_tile = item.new.tile if item.new is not None and item.new.tile != tile else None
    return {
        "layer": item.layer,
        "feature_id": item.feature_id,
        "tile": tile,
        "new_tile": new_tile,
        "would_be": item.op,
        "current_manifest_id": item.current_manifest_id,
        "new_manifest_id": None if item.new is None else item.new.manifest_id,
        "new_content_key": item.new_content_key,
        "reasons": [
            {"rule": codes.BY_CODE[c].rule, "code": c, "details": item.codes[c]}
            for c in codes.in_rule_order(item.remaining)
        ],
    }


def is_conflict(item: Item) -> bool:
    return any(codes.is_conflict(c) for c in item.remaining)


def build(state: State) -> dict:
    cur, new, cls = state.cur, state.new, state.cls
    held_items = [i for i in cls.items if not i.emitted]
    conflicts = [entry(i) for i in held_items if is_conflict(i)]
    held_back = [entry(i) for i in held_items if not is_conflict(i)]
    set_of = {id(i): s.path for s in state.sets for i in s.items}
    emitted_keys = {i.key for i in cls.items if i.emitted}

    counts = []
    layers = unique_sorted([k[0] for k in cur.assets] + [k[0] for k in new.assets])
    for layer in layers:
        emitted = Counter(i.op for i in cls.items if i.layer == layer and i.emitted)
        counts.append({
            "layer": layer,
            "add": emitted["add"],
            "replace": emitted["replace"],
            "remove": emitted["remove"],
            "move": emitted["move"],
            "manifest_only": sum(1 for i in cls.items if i.layer == layer and i.emitted and i.manifest_only),
            "unchanged": cls.unchanged[layer],
            "provenance_only": cls.provenance_only[layer],
            "contributions": sum(1 for k, o in cls.origins.items() if k[0] == layer and o.kind != "ingest"),
            "conflicts": sum(1 for e in conflicts if e["layer"] == layer),
            "held": sum(1 for e in held_back if e["layer"] == layer),
        })

    units = []
    for u in cls.units:
        if u.size < 2:
            continue
        units.append({
            "status": "emitted" if u.emitted else "held",
            "set": set_of.get(id(u.items[0])) if u.emitted else None,
            "members": [{"op": c.op, "tile": c.tile, "layer": c.layer, "feature_id": c.feature_id}
                        for c, _ in u.changes],
        })

    heights = []
    for h in cls.heights:
        emitted = [i for i in h.items if i.emitted]
        status = "emitted" if len(emitted) == len(h.items) else "held" if not emitted else "partly held"
        heights.append({
            "dem_source_ids": unique_sorted(sid for _, sid in h.dem_ids),
            "changes": sum(i.size for i in h.items),
            "terrain_changes": len(h.terrain),
            "status": status,
        })

    report = {
        "report": FORMAT,
        "report_version": REPORT_VERSION,
        "tool": {"command": COMMAND, "version": VERSION},
        "world": cur.name,
        "current_world_id": cur.world_id,
        "new_world_id": new.world_id,
        "log": {
            "records": len(state.wlog.records),
            "head_record_id": state.wlog.head.record_id,
            "warnings": list(state.log_warnings),
        },
        "overrides_id": state.ov.id,
        "max_set_bytes": state.max_set_bytes,
        "content_rule": CONTENT_RULE,
        "generators": {
            "current": unique_sorted(generator_text(a.manifest) for a in cur.assets.values()),
            "new": unique_sorted(generator_text(a.manifest) for a in new.assets.values()),
        },
        "sources": {"current": _sources(cur), "new": _sources(new)},
        "counts": counts,
        "sets": [{
            "path": s.path,
            "change_id": s.change_id,
            "kind": s.kind,
            "purpose": {"collider": s.purpose[0], "layer": s.purpose[1], "dataset": s.purpose[2]},
            "changes": len(s.entries),
            "bytes": s.bytes,
            "base_world_id": s.base_world_id,
            "world_after": s.world_after,
            "touches_colliders": s.touches_colliders,
            "warnings": list(s.warnings),
        } for s in state.sets],
        "final_world_id": state.final_world_id,
        "conflicts": conflicts,
        "held_back": held_back,
        "accepted": [{k: a[k] for k in ("layer", "feature_id", "lifts", "lifted", "reason", "set")}
                     for a in state.holds.accepted],
        "allowances_used": list(state.holds.allowances_used),
        "stale_overrides": list(state.holds.stale),
        "units": units,
        "height_components": heights,
        "contributions": _contributions(state),
        "world_level": _world_level(state),
        "residual": {
            "provenance_only": len(cls.provenance_only_keys),
            "conflicts": len(conflicts),
            "held": len(held_back),
            "contributions": sum(1 for k, o in cls.origins.items()
                                 if o.kind != "ingest" and k not in emitted_keys),
            "spawn_differs": cur.world["spawn"] != new.world["spawn"],
            "attribution_differs": state.final_world["attribution"] != new.world["attribution"],
        },
    }
    return report


def _sources(view: StoreView) -> list[dict]:
    versions: dict[str, set[str]] = {}
    for a in view.assets.values():
        for s in a.sources:
            versions.setdefault(s["dataset"], set()).add(s["version"])
    return [{"dataset": d, "versions": unique_sorted(versions[d])} for d in unique_sorted(versions)]


def _contributions(state: State) -> list[dict]:
    """Every current asset not made by the ingest whose own key, or whose
    dependency, the new ingest changes, every terrain of a contribution
    that the new ingest drapes changes over (Q17), and every asset of
    unclear origin. The counts cover the rest."""
    cls, cur = state.cls, state.cur
    items = {i.key: i for i in cls.items}
    out = []
    for key, origin in cls.origins.items():
        if origin.kind == "ingest":
            continue
        asset = cur.assets[key]
        item = items.get(key)
        status = cls.statuses.get(key)
        depends = None
        base_layer = DEPENDS_ON.get(key[0])
        if base_layer is not None:
            base_key = (base_layer, key[1])
            base = cur.assets.get(base_key)
            base_item = items.get(base_key)
            if base is None or base.tile != asset.tile:
                dep_status = "missing"
            elif base_item is None:
                dep_status = "unchanged"
            elif base_item.emitted:
                dep_status = "manifest-only" if base_item.manifest_only else "changed"
            else:
                dep_status = "held"
            depends = {"layer": base_layer, "status": dep_status}
        listed = (item is not None or origin.kind == "unclear" or status in ("changed", "new", "gone", "unknown")
                  or (depends is not None and depends["status"] != "unchanged") or key in cls.draped)
        if not listed:
            continue
        out.append({
            "layer": key[0],
            "feature_id": key[1],
            "tile": asset.tile,
            "manifest_id": asset.manifest_id,
            "authors": list(cls.authors.get(key, [])),
            "origin": origin.kind,
            "upstream": status,
            "would_be": None if item is None else item.op,
            "depends_on": depends,
        })
    return out


def _world_level(state: State) -> dict:
    cur, new = state.cur, state.new

    def gap(view: StoreView) -> int | None:
        sx, sy, sz = cur.spawn
        top = None
        for a in view.assets.values():
            if a.layer != "terrain" or not a.box.contains_xz(sx, sz):
                continue
            y = ground_under(view.mesh(a), view.origins[a.tile], sx, sz)
            if y is not None:
                top = y if top is None else max(top, y)
        return None if top is None else sy - top

    tiles_cur, tiles_new = set(cur.tiles), set(new.tiles)
    return {
        "spawn": list(cur.world["spawn"]["position_mm"]),
        "spawn_yaw": cur.world["spawn"]["yaw"],
        "spawn_new": list(new.world["spawn"]["position_mm"]),
        "spawn_new_yaw": new.world["spawn"]["yaw"],
        "spawn_ground_gap_mm": {"current": gap(cur), "new": gap(new)},
        "tiles_only_current": sorted(tiles_cur - tiles_new, key=tile_xy),
        "tiles_only_new": sorted(tiles_new - tiles_cur, key=tile_xy),
        "attribution_final": list(state.final_world["attribution"]),
        "attribution_new": list(new.world["attribution"]),
    }


def to_bytes(report: dict) -> bytes:
    return cjson.dumps(report)


# --------------------------------------------------------------- Markdown --


def _cell(value) -> str:
    text = "" if value is None else str(value)
    return text.replace("\\", "\\\\").replace("|", "\\|")


def _code(value) -> str:
    return "-" if value is None else f"`{value}`"


def _table(header: list[str], rows: list[list[str]]) -> list[str]:
    out = ["| " + " | ".join(header) + " |", "|" + "|".join("---" for _ in header) + "|"]
    out += ["| " + " | ".join(r) + " |" for r in rows]
    return out


def summary(report: dict) -> str:
    sets = report["sets"]
    changes = sum(s["changes"] for s in sets)
    kept = f"{report['residual']['provenance_only']} provenance-only differences kept"
    listed = (f"{counted(len(report['conflicts']), 'conflict')} and "
              f"{counted(len(report['held_back']), 'held-back change')} listed")
    if not sets:
        if not report["conflicts"] and not report["held_back"]:
            return f"nothing to refresh: 0 changes ({kept})"
        return f"no change set: 0 changes; {listed} in the report ({kept})"
    return f"{counted(len(sets), 'change set')} with {counted(changes, 'change')}; {listed} in the report ({kept})"


def markdown(report: dict, report_id: str) -> bytes:
    r = report
    lines = [f"# Baseline refresh plan: {r['world']}", "", summary(r) + ".", ""]
    lines += [
        f"- Current world: `{r['current_world_id']}`",
        f"- New ingest: `{r['new_world_id']}` ({', '.join(r['generators']['new'])})",
        f"- World log: {counted(r['log']['records'], 'record')}, the last `{r['log']['head_record_id']}`",
        f"- Overrides file: {_code(r['overrides_id']) if r['overrides_id'] else 'none'}",
        f"- Final world after every set: `{r['final_world_id']}`",
        f"- This report: `refresh-report.json`, content id `{report_id}`",
        f"- Content rule (Q1, Proposed): {r['content_rule']}",
        "",
    ]
    for w in r["log"]["warnings"]:
        lines += [f"**Warning:** {w}", ""]

    lines += ["## Conflicts", ""]
    if r["conflicts"]:
        lines += _table(["Layer", "Feature", "Tile", "Would be", "Reasons"],
                        [[_cell(e["layer"]), _cell(e["feature_id"]), _cell(_tiles(e)), e["would_be"],
                          _cell("; ".join(f"{x['code']}: {x['details']}" for x in e["reasons"]))]
                         for e in r["conflicts"]])
        lines += ["", "What to do:", ""]
        present = codes.in_rule_order({x["code"] for e in r["conflicts"] for x in e["reasons"]})
        lines += [f"- `{c}` ({codes.BY_CODE[c].rule}): {codes.BY_CODE[c].todo}" for c in present]
    else:
        lines += ["None."]
    lines += ["", "## Held back", ""]
    if r["held_back"]:
        first = {}
        for e in r["held_back"]:
            first.setdefault(e["reasons"][0]["code"], []).append(e)
        for c in codes.in_rule_order(first):
            lines += [f"### `{c}` ({codes.BY_CODE[c].rule})", "", codes.BY_CODE[c].todo, ""]
            lines += _table(["Layer", "Feature", "Tile", "Would be", "Reasons"],
                            [[_cell(e["layer"]), _cell(e["feature_id"]), _cell(_tiles(e)), e["would_be"],
                              _cell("; ".join(f"{x['code']}: {x['details']}" for x in e["reasons"]))]
                             for e in first[c]])
            lines += [""]
    else:
        lines += ["None.", ""]

    total = sum(c["contributions"] for c in r["counts"])
    lines += ["## Contributions", "",
              f"The current world holds {counted(total, 'asset')} not made by the ingest. A refresh never "
              "changes a contribution without a maintainer's decision. Listed here: those whose own key, or "
              "whose dependency, the new ingest changes, and those of unclear origin.", ""]
    if r["contributions"]:
        lines += _table(["Layer", "Feature", "Tile", "Authors", "Origin", "Upstream", "Would be", "Depends on"],
                        [[_cell(c["layer"]), _cell(c["feature_id"]), c["tile"], _cell(", ".join(c["authors"])),
                          c["origin"], c["upstream"] or "-", c["would_be"] or "-", _depends(c["depends_on"])]
                         for c in r["contributions"]])
    else:
        lines += ["None listed."]

    w = r["world_level"]
    gap = w["spawn_ground_gap_mm"]
    lines += ["", "## World-level differences", "",
              f"- Spawn: kept at `{w['spawn']}` (yaw {w['spawn_yaw']}); change sets cannot move it (amendment A5). "
              f"The new ingest puts it at `{w['spawn_new']}` (yaw {w['spawn_new_yaw']}).",
              f"- Ground under the spawn's feet: {_mm(gap['current'])} below them in the current world, "
              f"{_mm(gap['new'])} in the new ingest.",
              f"- Tiles only in the current store: {_list(w['tiles_only_current'])}.",
              f"- Tiles only in the new store: {_list(w['tiles_only_new'])}.",
              ]
    if w["attribution_final"] != w["attribution_new"]:
        lines += [f"- Attribution after the refresh: {_list(w['attribution_final'])}; in the new ingest: "
                  f"{_list(w['attribution_new'])}."]
    else:
        lines += ["- Attribution: the same as the new ingest's after the refresh."]

    lines += ["", "## Stale overrides", ""]
    if r["stale_overrides"]:
        lines += _table(["Entry", "Layer", "Feature or dataset", "Why"],
                        [[s["entry"], _cell(s["layer"]), _cell(s["feature_id"] or s["dataset"]), _cell(s["why"])]
                         for s in r["stale_overrides"]])
    else:
        lines += ["None."]
    if r["accepted"] or r["allowances_used"]:
        lines += ["", "Applied:", ""]
        lines += [f"- accept {a['layer']} {_cell(a['feature_id'])}: lifted {', '.join(a['lifted'])}"
                  f" ({a['set'] or 'still held'}): {_cell(a['reason'])}" for a in r["accepted"]]
        lines += [f"- allow_removals {a['layer']}/{a['dataset']}: {a['removals']} of at most {a['at_most']}: "
                  f"{_cell(a['reason'])}" for a in r["allowances_used"]]

    lines += ["", "## Change sets", ""]
    if r["sets"]:
        lines += _table(["Set", "Kind", "Purpose", "Changes", "Bytes", "Base world", "World after"],
                        [[s["path"], s["kind"],
                          _cell(f"{s['purpose']['layer']} from {s['purpose']['dataset']}, "
                                f"collider {s['purpose']['collider']}"),
                          str(s["changes"]), str(s["bytes"]), f"`{s['base_world_id']}`", f"`{s['world_after']}`"]
                         for s in r["sets"]])
        for s in r["sets"]:
            for x in s["warnings"]:
                lines += ["", f"**Warning:** {s['path']}: {x}"]
    else:
        lines += ["None."]

    lines += ["", "## Checking and merging, in order", ""]
    if r["sets"]:
        lines += [
            "Run these from the repository root, one set at a time and in order. Replace `<canonical>` with "
            "the canonical store, `<plan>` with this plan's directory, `<log>` with the world log and "
            "`<suite>` with each registered replay (repeat `--replay`, or leave it out if the world has none). "
            "Each set was planned on the world named in its `base_world_id`; `--require-base` refuses it on any "
            "other world, and then the refresh is planned again. Under the proposed refresh window (ADR-0020, "
            "Proposed), no other merge happens between the sets of one refresh.",
            "",
            "The gate reports G3 as `review` for every change of a refresh, and a merge needs a review judging "
            "the set `better`. Until Q22 is decided, the proposed default lets the maintainer who planned the "
            "refresh approve it while there is one maintainer, naming the planner in `--reason`.",
            "",
        ]
        n = len(r["sets"])
        for k, s in enumerate(r["sets"], 1):
            lines += [f"Set {k} of {n} (`{s['path']}`, change `{s['change_id']}`):", "", "```sh",
                      f"cargo run -p op_replay -- check-change <canonical> <plan>/{s['path']} --replay <suite> "
                      f"--report <gate-{s['path'].split('/')[-1]}.json>",
                      f"cargo run -p op_replay -- merge <canonical> <plan>/{s['path']} --require-base --log <log> "
                      f"--reason \"Baseline refresh {r['world']}, set {k} of {n}, refresh report {report_id}; "
                      f"planned by <handle>\" --review <handle>=better:<notes> --replay <suite>",
                      "cargo run -p op_replay -- rerecord <suite> --world <canonical> --out <suite>",
                      "```", ""]
        lines += [f"After the last set, the canonical world is `{r['final_world_id']}`."]
    else:
        lines += ["Nothing to merge."]

    lines += ["", "## Overrides template", ""]
    template = _template(r)
    if template["accept"]:
        lines += ["One accept per conflict or held-back change that an accept can lift, with its scope and "
                  "codes filled in. Each `reason` is left empty, and an empty reason is refused: copying the "
                  "template unchanged does nothing.", "", "```json",
                  json.dumps(template, indent=2, ensure_ascii=False), "```"]
    else:
        lines += ["No entry can be lifted by an accept."]
    text = "\n".join(lines).rstrip("\n") + "\n"
    return text.encode("utf-8")


def _template(r: dict) -> dict:
    accepts = []
    for e in sorted(r["conflicts"] + r["held_back"], key=lambda e: key_order((e["layer"], e["feature_id"]))):
        names = [x["code"] for x in e["reasons"]]
        hard = [c for c in names if not codes.BY_CODE[c].liftable and c != "unit-held"]
        lifts = [c for c in names if codes.BY_CODE[c].liftable]
        if hard or not lifts:
            continue
        accepts.append({
            "layer": e["layer"], "feature_id": e["feature_id"],
            "current_manifest_id": e["current_manifest_id"], "content_key": e["new_content_key"],
            "lifts": lifts, "reason": "",
        })
    return {"overrides_version": OVERRIDES_VERSION, "hold": [], "accept": accepts, "allow_removals": []}


def _depends(depends: dict | None) -> str:
    return "-" if depends is None else f"{depends['layer']}: {depends['status']}"


def _tiles(e: dict) -> str:
    return e["tile"] if e["new_tile"] is None else f"{e['tile']} to {e['new_tile']}"


def _mm(value) -> str:
    return "no terrain under it" if value is None else f"{value} mm"


def _list(values: list) -> str:
    return "none" if not values else ", ".join(f"`{v}`" for v in values)
