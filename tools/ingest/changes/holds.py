"""The hold rules H1 to H14 (ADR-0020, section 6), applied to the pending changes.

Evaluation:
1. The per-key rules (H1 to H8, H10a, H11, H12, H13's `unit-too-large`,
   H14) and the group rule H10b run on every pending change, and each
   records every code that applies, with its details.
2. Then H9 and H13 (`unit-held`, `would-empty-tile`) run again and again
   until nothing changes. A code is never removed, so this ends.
3. Then `would-empty-tile` runs on the change sets in merge order: the
   emitted units are packed, and the first set that would leave a tile
   with no asset has its removals in that tile held, even when a later
   set adds to the tile, because the gate judges each set on its own
   (G1). Step 2 runs again after each such hold, and the units are packed
   again, until no set empties a tile.
4. Accepts and allowances remove the codes they name, in every step.
5. A change is emitted when no code is left on it.

Accepting `contribution`, `origin-unclear` or `removed-by-contribution`
marks the unit for a decision set of its own (ADR-0020, section 5).
"""

from __future__ import annotations

import math
from collections import Counter
from collections.abc import Callable
from dataclasses import dataclass, field
from fractions import Fraction

from tools.ingest.changes import codes
from tools.ingest.changes.classify import Classification, Item, Unit
from tools.ingest.changes.common import (
    Box, BoxGrid, Key, PlanError, counted, joined, key_order, overlaps, show, text_order, touches,
)
from tools.ingest.changes.overrides import Accept, Overrides, accept_matches, hold_matches
from tools.ingest.changes.stores import StoreView
from tools.worldfmt import glb
from tools.worldfmt.changes import MAX_CHANGES

SPAWN_CLEARANCE_MM = 100
"""H14: new terrain under the spawn must stay this far below its feet."""
PLAYER_RADIUS_MM = 300
PLAYER_HEIGHT_MM = 1_800
"""H14: the player's capsule, as crates/op_sim/src/player.rs defines it
(`RADIUS`, and twice `CENTRE_ABOVE_FEET`, which is `HALF_HEIGHT` plus
`RADIUS`: from the feet to the top). Changing it there changes the
simulation contract, so these change with it."""

Chain = Callable[[], list[list[Item]]]
"""Packs the units emitted so far into change sets, as `plan` does, and
returns the items of each set in merge order."""


@dataclass
class HoldResult:
    accepted: list[dict] = field(default_factory=list)
    stale: list[dict] = field(default_factory=list)
    allowances_used: list[dict] = field(default_factory=list)
    accepts: dict[int, Accept] = field(default_factory=dict)
    """The accept that applies to each item, by `id(item)`."""


def unpaired_removals(cls: Classification) -> dict[tuple[str, str], list[Item]]:
    """By ingest group: the removals of assets made by the ingest that their
    unit's adds, moves and replaces do not account for. Each of those
    accounts for one removal of its unit, in key order, so a rename or a
    feature re-sourced from another dataset counts 0, while 49 buildings
    replaced by one new building count 48. Per-tile removals are units of
    their own and always count."""
    out: dict[tuple[str, str], list[Item]] = {}
    for unit in cls.units:
        removals = [i for i in unit.items if i.op == "remove"]
        placed = sum(1 for i in unit.items if i.op in ("add", "move", "replace"))
        for i in removals[placed:]:
            if i.origin.kind == "ingest":
                out.setdefault((i.cur.layer, i.cur.dataset), []).append(i)
    return {g: sorted(out[g], key=lambda i: key_order(i.key)) for g in sorted(out, key=key_order)}


def check_vanished_groups(cls: Classification, ov: Overrides) -> None:
    """A whole ingest group that is gone from the new store stops the plan
    (Q4, Proposed), unless an allowance names the group and covers its
    unpaired removals."""
    unpaired = unpaired_removals(cls)
    errors = []
    for group, size in cls.groups.items():
        if cls.new_groups.get(group):
            continue
        layer, dataset = group
        message = (f"the new store has no {layer} from {dataset}, but the current store has "
                   f"{counted(size, 'such asset')} made by the ingest; if upstream really lost them all, "
                   "add an allow_removals entry to the overrides file")
        allowance = ov.allowances.get(group)
        if allowance is None:
            errors.append(message)
            continue
        removals = len(unpaired.get(group, []))
        if removals > allowance.at_most:
            errors.append(f"{message} (the entry for {layer}/{dataset} allows at most {allowance.at_most}, "
                          f"but {removals} would be removed)")
    if errors:
        raise PlanError("\n".join(errors))


def apply_holds(cls: Classification, cur: StoreView, new: StoreView, ov: Overrides,
                chain: Chain | None = None) -> HoldResult:
    """Runs the hold rules (see the module documentation). Without `chain`,
    `would-empty-tile` looks only at the world after every emitted change;
    with it, also at the world after each set."""
    result = HoldResult()
    items = cls.items
    by_key = {i.key: i for i in items}

    # Which accepts and allowances apply; stale ones are listed.
    for key, accept in ov.accepts.items():
        item = by_key.get(key)
        why = "no pending change for this key" if item is None else \
            accept_matches(accept, item.current_manifest_id, item.new_content_key)
        if why:
            result.stale.append(_stale("accept", key, why))
        else:
            result.accepts[id(item)] = accept
    allowed: set[int] = set()
    unpaired = unpaired_removals(cls)
    for group, allowance in ov.allowances.items():
        removals = unpaired.get(group, [])
        vanished = cls.groups.get(group) and not cls.new_groups.get(group)
        if not removals and not vanished:
            result.stale.append(_stale_group(allowance, "no unpaired removal in this group"))
        elif len(removals) > allowance.at_most:
            result.stale.append(_stale_group(
                allowance, f"{len(removals)} unpaired removals, more than at_most {allowance.at_most}"))
        else:
            allowed.update(id(i) for i in removals)
            result.allowances_used.append({
                "layer": allowance.layer, "dataset": allowance.dataset, "removals": len(removals),
                "at_most": allowance.at_most, "reason": allowance.reason,
            })

    def add(item: Item, code: str, details: str) -> bool:
        if code in item.codes:
            return False
        item.codes[code] = details
        accept = result.accepts.get(id(item))
        if accept is not None and code in accept.lifts:
            item.lifted[code] = "accept"
        elif code in codes.ALLOWANCE and id(item) in allowed:
            item.lifted[code] = "allowance"
        return True

    # 1. The per-key rules and the group rule. H10b counts every unpaired
    # removal of the group, per-tile ones too (Q4), as the allowances do;
    # it counted only per-feature ones (found by review).
    _per_key(cls, cur, new, ov, result, add)
    for group, removals in unpaired.items():
        size = cls.groups.get(group, 0)
        threshold = max(2, (size * 5 + 99) // 100)
        if len(removals) > threshold:
            for i in removals:
                add(i, "mass-removal", f"{len(removals)} unpaired removals in {group[0]}/{group[1]}, more than "
                                       f"max(2, ceil(5 % of {size})) = {threshold}")

    # 2. H9 and H13 until nothing changes.
    def settle() -> None:
        rounds = 0
        while _coupled(cls, cur, add):
            rounds += 1
            if rounds > 10 * len(items) + 10:  # cannot happen: codes are only ever added
                raise RuntimeError("the hold rules did not reach a fixed point")

    settle()
    # 3. `would-empty-tile` on the sets in merge order.
    if chain is not None:
        while _emptied_between_sets(cur, chain(), add):
            settle()

    for key, hold in ov.holds.items():
        item = by_key.get(key)
        if item is None:
            result.stale.append(_stale("hold", key, "no pending change for this key"))
        elif "manual-hold" not in item.codes:
            result.stale.append(_stale("hold", key, hold_matches(hold, item.current_manifest_id,
                                                                   item.new_content_key) or "it did not apply"))
    for item in items:
        accept = result.accepts.get(id(item))
        if accept is None:
            continue
        lifted = [c for c, by in item.lifted.items() if by == "accept"]
        if not lifted:
            result.stale.append(_stale("accept", item.key, "it lifts none of the codes that hold this change"))
            continue
        result.accepted.append({
            "item": item, "layer": item.layer, "feature_id": item.feature_id, "lifts": list(accept.lifts),
            "lifted": codes.in_rule_order(lifted), "reason": accept.reason, "set": None,
        })
    result.stale.sort(key=lambda s: (text_order(s["entry"]), text_order(s["layer"]),
                                     text_order(s["feature_id"] or s["dataset"] or "")))
    return result


def is_decision_unit(unit: Unit) -> bool:
    """An emitted unit with an accepted change to a contribution (ADR-0020, section 5)."""
    return unit.emitted and any(i.lifted.get(c) == "accept" for i in unit.items for c in codes.DECISION)


def _stale(entry: str, key: Key, why: str) -> dict:
    return {"entry": entry, "layer": key[0], "feature_id": key[1], "dataset": None, "why": why}


def _stale_group(allowance, why: str) -> dict:
    return {"entry": "allow_removals", "layer": allowance.layer, "feature_id": None, "dataset": allowance.dataset,
            "why": why}


# --------------------------------------------------------- per-key rules --


def _per_key(cls: Classification, cur: StoreView, new: StoreView, ov: Overrides, result: HoldResult, add) -> None:
    contributions: dict[str, BoxGrid] = {}
    for key, asset in cur.assets.items():
        if cls.origins[key].kind != "ingest":
            contributions.setdefault(asset.layer, BoxGrid()).add(asset.box, asset)
    removed: dict[str, BoxGrid] = {}
    unknown: dict[tuple[str, str], list[Key]] = {}
    for key in sorted(cls.protections, key=key_order):
        prot = cls.protections[key]
        if not prot.removed:
            continue
        if prot.box is None:
            unknown.setdefault((prot.tile, key[0]), []).append(key)
        else:
            removed.setdefault(key[0], BoxGrid()).add(prot.box, key)
    spawn = cur.spawn

    for item in cls.items:
        key = item.key
        prot = cls.protections.get(key)
        origin = item.origin
        # H1.
        if prot is not None and prot.kind == "takedown":
            add(item, "takedown", f"{prot.describe()} {'removed' if prot.removed else 'changed'} it; "
                                  "a refresh never changes it")
        # H2, H3.
        if origin is not None and origin.kind == "contribution":
            add(item, "contribution", _contribution_details(cls, item))
        if origin is not None and origin.kind == "unclear":
            add(item, "origin-unclear", _unclear_details(origin))
        # H4.
        if item.op == "add" and prot is not None and prot.kind == "removed-by-contribution":
            add(item, "removed-by-contribution", f"{prot.author} removed it in {prot.record.path} "
                                                 f"({prot.record.record_id})")
        # H5.
        dependents = cls.dependents.get(key)
        if dependents and (item.op in ("remove", "move") or (item.op == "replace" and not item.manifest_only)):
            if item.op == "remove":
                what = "upstream removes it"
            elif item.op == "move":
                what = f"upstream moves it to tile {item.new.tile}"
            else:
                what = "the new mesh differs"
            on = joined([f"the {d.layer} by {joined(cls.authors.get(d.key) or ['an unknown author'])}"
                         for d in dependents])
            add(item, "dependent-contribution", f"{on} sits on it; {what}")
        placed = item.adds
        # H6 and H6b look at a new feature, and at a replace whose bounds
        # grow: a feature that upstream merged into a neighbour that
        # keeps its id comes back inside that neighbour.
        grown = item.new if item.op == "replace" and _grows(item.cur.box, item.new.box) else None
        if placed is not None or grown is not None:
            new_box = (placed or grown).box
            old_box = None if grown is None else item.cur.box

            def newly(rule, box) -> bool:
                """The new bounds meet `box` by `rule`, and the current ones did not."""
                return rule(new_box, box) and not (old_box is not None and rule(old_box, box))

            # H6.
            hits = []
            grid = contributions.get(item.layer)
            for box, asset in grid.near(new_box) if grid else []:
                if asset.key != key and newly(overlaps, box):
                    by = joined(cls.authors.get(asset.key) or ["an unknown author"])
                    hits.append(f"the contribution {show(asset.key)} by {by}")
            grid = removed.get(item.layer)
            for box, other in grid.near(new_box) if grid else []:
                other_prot = cls.protections[other]
                if other != key and newly(touches if other_prot.kind == "takedown" else overlaps, box):
                    hits.append(f"{show(other)}, which {other_prot.describe()} removed")
            if hits:
                since = "" if old_box is None else "; its current bounds did not"
                add(item, "possible-duplicate", f"its new bounds overlap {joined(hits)}{since}")
            # H6b.
            tile = (placed or grown).tile
            lost = [k for k in unknown.get((tile, item.layer), []) if k != key]
            if lost:
                which = joined([f"{show(k)}, which {cls.protections[k].describe()} removed," for k in lost])
                add(item, "removed-bounds-unknown", f"{which} lay in tile {tile}, and "
                                                    f"{'its bounds' if len(lost) == 1 else 'their bounds'} "
                                                    "cannot be read")
        if placed is not None:
            # H7.
            if placed.tile not in cur.tiles:
                add(item, "tile-not-in-world", f"tile {placed.tile} is not in the current world")
        # H8.
        if item.layer == "terrain" and item.op == "replace" and not item.manifest_only:
            missing = glb.missing_boundary_vertices(cur.mesh(item.cur), new.mesh(item.new))
            if missing:
                verb = "differs" if missing == 1 else "differ"
                add(item, "terrain-edges", f"{counted(missing, 'vertex', 'vertices')} on the open edges of the "
                                           f"current terrain {verb} in the new one; the gate would fail it (G8)")
        # H10a.
        gone = item.removes
        if gone is not None and gone.per_tile:
            add(item, "per-tile-removal", f"it removes the {gone.layer} of tile {gone.tile}")
        # H11.
        if item.cur is not None and item.new is not None:
            old_p, new_p = item.cur.manifest["provenance"], item.new.manifest["provenance"]
            if old_p["sources"] == new_p["sources"] and old_p["generator"] == new_p["generator"]:
                g = new_p["generator"]
                add(item, "unexplained-change",
                    f"the content changed, but its sources (versions included) and its generator "
                    f"({g['name']} {g['version']}) did not: another platform, an ingest change made without "
                    "a VERSION bump, or an item republished in place under the same id")
        # H12.
        hold = ov.holds.get(key)
        if hold is not None and hold_matches(hold, item.current_manifest_id, item.new_content_key) is None:
            add(item, "manual-hold", f"the overrides file holds it: {hold.reason}")
        # H13, unit-too-large.
        if item.unit.size > MAX_CHANGES:
            add(item, "unit-too-large", f"its unit has {item.unit.size} changes, more than {MAX_CHANGES}")
        # H14.
        why = _spawn(item, cur, new, spawn)
        if why:
            add(item, "spawn", why)


def _grows(old: Box, new: Box) -> bool:
    """The new box reaches outside the old one horizontally (x or z)."""
    return new.lo[0] < old.lo[0] or new.hi[0] > old.hi[0] or new.lo[2] < old.lo[2] or new.hi[2] > old.hi[2]


def _contribution_details(cls: Classification, item: Item) -> str:
    by = joined(cls.authors.get(item.key) or ["an unknown author"])
    status = item.status
    if status == "changed":
        what = ("upstream moved the feature to tile " + item.new.tile) if item.op == "move" else \
            "upstream changed the feature since the ingest version the contribution replaced"
    elif status == "new":
        what = "upstream gained this feature, and the contribution replaced no version of the ingest"
    elif status == "gone":
        what = "upstream lost the feature since the ingest version the contribution replaced"
    else:
        what = "a manifest of the contribution's lineage cannot be read"
    return f"the current version is a contribution by {by} (upstream {status}); {what}"


def _unclear_details(origin) -> str:
    if origin.manifest_says_ingest:
        record = origin.last
        return (f"its manifest says it was made by the ingest, but its last change was a {record.decision} "
                f"record by {record.author} in {record.path}")
    if origin.last is None:
        return "its manifest says it is a contribution, but the world log shows no change to it since the baseline"
    return (f"its manifest says it is a contribution, but its last change was a refresh by "
            f"{origin.last.author} in {origin.last.path}")


def player_space(spawn: tuple[int, int, int]) -> Box:
    """The box the player's capsule takes at the spawn (crates/op_sim/src/
    player.rs: 300 mm around the feet's x and z, 1,800 mm tall), from
    `SPAWN_CLEARANCE_MM` below the feet, which terrain must leave clear
    too."""
    sx, sy, sz = spawn
    return Box((sx - PLAYER_RADIUS_MM, sy - SPAWN_CLEARANCE_MM, sz - PLAYER_RADIUS_MM),
               (sx + PLAYER_RADIUS_MM, sy + PLAYER_HEIGHT_MM, sz + PLAYER_RADIUS_MM))


def _reaches(box: Box, space: Box) -> bool:
    """`box` and `space` share a positive volume."""
    return all(box.lo[k] < space.hi[k] and box.hi[k] > space.lo[k] for k in range(3))


def _spawn(item: Item, cur: StoreView, new: StoreView, spawn: tuple[int, int, int]) -> str | None:
    """H14, in integer millimetres in the region frame:
    - terrain: a change that leaves no ground under the spawn's (x, z), a
      removal of that ground included; new ground there (`ground_height`,
      the height of the triangle that holds the point) within
      `SPAWN_CLEARANCE_MM` of the feet, where the current ground was lower;
      and new terrain whose triangles reach into the player's space at the
      spawn (`player_space`) where the current terrain's did not;
    - other layers: a collider whose new mesh reaches into the player's
      space or encloses it (`mesh_meets`, as a solid), while its current
      version had no collider there. A replace of the manifest only can
      switch a collider on, so it counts.
    The triangles themselves are tested, not their bounding boxes: a
    courtyard around the spawn leaves its space free. Found by review: the
    boxes held harmless buildings and let a buried spawn through, and the
    terrain was judged by the highest vertex near the spawn, without
    comparing with the current ground, and not at all when it was
    removed. A spawn on the shared edge of two terrain tiles counts as on
    both."""
    if item.layer == "terrain":
        return _spawn_ground(item, cur, new, spawn)
    asset = item.new
    if asset is None or item.op == "remove":
        return None
    space = player_space(spawn)
    if asset.collider != "trimesh" or not _meets(new, asset, space, solid=True):
        return None
    before = item.cur
    if before is not None and before.collider == "trimesh" and _meets(cur, before, space, solid=True):
        return None
    why = (f"its new collider reaches into the player's space at the spawn {list(spawn)} (x and z within "
           f"{PLAYER_RADIUS_MM} mm, from {SPAWN_CLEARANCE_MM} mm below the feet to {PLAYER_HEIGHT_MM} mm above them)")
    if before is None:
        return why
    return f"{why}, and its current version {'has no collider' if before.collider != 'trimesh' else 'does not'}"


def _spawn_ground(item: Item, cur: StoreView, new: StoreView, spawn: tuple[int, int, int]) -> str | None:
    """H14 for a terrain change (see `_spawn`)."""
    sx, sy, sz = spawn
    before = None if item.cur is None else _ground(cur, item.cur, sx, sz)
    if item.op == "remove":
        if before is None:
            return None
        return (f"it removes the ground under the spawn, {sy - math.floor(before)} mm below the spawn's feet, so "
                "the player would fall")
    if item.manifest_only:
        return None  # the same mesh keeps the ground where it is
    after = _ground(new, item.new, sx, sz)
    if after is None:
        if before is not None:
            return "its new mesh leaves no ground under the spawn, where the current one has it, so the player would fall"
    elif after > sy - SPAWN_CLEARANCE_MM and (before is None or after > before):
        top = math.floor(after)
        gap = sy - top
        where = f"{gap} mm below" if gap >= 0 else f"{-gap} mm above"
        return (f"the new ground under the spawn reaches y = {top} mm, {where} the spawn's feet at "
                f"y = {sy} mm; at least {SPAWN_CLEARANCE_MM} mm must stay clear")
    space = player_space(spawn)
    if _meets(new, item.new, space) and not (item.cur is not None and _meets(cur, item.cur, space)):
        return (f"the new ground reaches into the player's space at the spawn {list(spawn)} (x and z within "
                f"{PLAYER_RADIUS_MM} mm, from {SPAWN_CLEARANCE_MM} mm below the feet to {PLAYER_HEIGHT_MM} mm "
                "above them), where the current ground does not")
    return None


def _ground(view: StoreView, asset, x: int, z: int) -> Fraction | None:
    """`ground_height` of `asset`'s mesh at (x, z), or None outside its
    bounds."""
    if not asset.box.contains_xz(x, z):
        return None
    return ground_height(view.mesh(asset), view.origins.get(asset.tile) or (0, 0, 0), x, z)


def _meets(view: StoreView, asset, space: Box, *, solid: bool = False) -> bool:
    """`mesh_meets` for `asset`'s mesh, after a test of its bounds."""
    return _reaches(asset.box, space) and mesh_meets(view.mesh(asset), view.origins.get(asset.tile) or (0, 0, 0),
                                                     space, solid=solid)


def ground_height(mesh: glb.Mesh, origin, x: int, z: int) -> Fraction | None:
    """The height of `mesh` at `(x, z)`, in the region frame, exactly: of
    the highest triangle whose horizontal projection holds the point (its
    edges included), at that point. None if none holds it; a vertical
    triangle holds no point."""
    ox, oy, oz = origin
    x, z = x - ox, z - oz
    top = None
    for a, b, c in mesh.triangles_mm():
        if not (min(a[0], b[0], c[0]) <= x <= max(a[0], b[0], c[0])
                and min(a[2], b[2], c[2]) <= z <= max(a[2], b[2], c[2])):
            continue
        d = (b[0] - a[0]) * (c[2] - a[2]) - (c[0] - a[0]) * (b[2] - a[2])
        if d == 0:
            continue
        wb = (x - a[0]) * (c[2] - a[2]) - (c[0] - a[0]) * (z - a[2])
        wc = (b[0] - a[0]) * (z - a[2]) - (x - a[0]) * (b[2] - a[2])
        wa = d - wb - wc
        if (d > 0 and min(wa, wb, wc) < 0) or (d < 0 and max(wa, wb, wc) > 0):
            continue
        y = Fraction(a[1] * d + wb * (b[1] - a[1]) + wc * (c[1] - a[1]), d) + oy
        top = y if top is None else max(top, y)
    return top


def ground_under(mesh: glb.Mesh, origin, x: int, z: int) -> int | None:
    """The highest vertex, in the region frame, of the triangles of `mesh`
    whose horizontal box contains `(x, z)`; None if there is none. Only the
    report's `spawn_ground_gap_mm` uses it, as it did when the fixture's
    report id was pinned: the hold rule uses `ground_height`, the height of
    the ground at the point itself."""
    ox, oy, oz = origin
    top = None
    for a, b, c in mesh.triangles_mm():
        if min(a[0], b[0], c[0]) + ox <= x <= max(a[0], b[0], c[0]) + ox and \
                min(a[2], b[2], c[2]) + oz <= z <= max(a[2], b[2], c[2]) + oz:
            y = max(a[1], b[1], c[1]) + oy
            top = y if top is None else max(top, y)
    return top


def mesh_meets(mesh: glb.Mesh, origin, space: Box, *, solid: bool = False) -> bool:
    """Whether `mesh` (placed at `origin`) meets the inside of `space`: one
    of its triangles does (touching the faces does not count), or, with
    `solid`, the mesh encloses `space` (`encloses`). A building's collider
    is a solid, and a player inside one is trapped there however far its
    walls are; the ground is a surface."""
    ox, oy, oz = origin
    lo = (space.lo[0] - ox, space.lo[1] - oy, space.lo[2] - oz)
    hi = (space.hi[0] - ox, space.hi[1] - oy, space.hi[2] - oz)
    triangles = list(mesh.triangles_mm())
    for tri in triangles:
        if all(min(p[k] for p in tri) < hi[k] and max(p[k] for p in tri) > lo[k] for k in range(3)) and \
                triangle_meets_box(tri, lo, hi):
            return True
    return solid and encloses(triangles, lo, hi)


def encloses(triangles, lo, hi) -> bool:
    """Whether the closed surface of `triangles` encloses the box from `lo`
    to `hi`, which none of them meets: the vertical ray up from the box's
    centre crosses them an odd number of times. A surface open at the
    bottom (a shell with no floor) counts as closed below. Exact, in
    integers: coordinates are doubled so the centre is whole, and the ray
    starts infinitesimally off the centre, at (x + e, z + e * e), so it
    never passes through an edge or a corner; a vertical triangle is never
    crossed."""
    cx, cy, cz = (lo[0] + hi[0], lo[1] + hi[1], lo[2] + hi[2])

    def side(ax, az, bx, bz) -> int:
        # The sign of orient(a, b, centre + (e, e * e)), e infinitesimal.
        base = (bx - ax) * (cz - az) - (bz - az) * (cx - ax)
        if base:
            return 1 if base > 0 else -1
        if bz != az:
            return 1 if bz < az else -1
        return 1 if bx > ax else -1

    crossings = 0
    for tri in triangles:
        (x0, y0, z0), (x1, y1, z1), (x2, y2, z2) = ((2 * p[0], 2 * p[1], 2 * p[2]) for p in tri)
        d = (x1 - x0) * (z2 - z0) - (x2 - x0) * (z1 - z0)
        if d == 0:
            continue
        if d < 0:  # the same turn for every triangle
            (x1, y1, z1), (x2, y2, z2) = (x2, y2, z2), (x1, y1, z1)
            d = -d
        if side(x0, z0, x1, z1) < 0 or side(x1, z1, x2, z2) < 0 or side(x2, z2, x0, z0) < 0:
            continue
        # The plane's height at the centre, compared with the centre's.
        wb = (cx - x0) * (z2 - z0) - (x2 - x0) * (cz - z0)
        wc = (x1 - x0) * (cz - z0) - (cx - x0) * (z1 - z0)
        above = y0 * d + wb * (y1 - y0) + wc * (y2 - y0) - cy * d
        if above == 0:  # cannot happen: that triangle would meet the box
            return True
        crossings += above > 0
    return crossings % 2 == 1


def _cross(u, v):
    return (u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0])


def triangle_meets_box(tri, lo, hi) -> bool:
    """Whether the triangle `tri` (three integer points, edges included)
    meets the inside of the box from `lo` to `hi`, exactly: the
    separating-axis test (Akenine-Moller), in integers. Coordinates are
    doubled so that the box's centre is whole; an axis separates the two
    when their projections at most touch, so a triangle in a face's plane
    meets nothing."""
    h = [hi[k] - lo[k] for k in range(3)]
    v = [tuple(2 * p[k] - lo[k] - hi[k] for k in range(3)) for p in tri]

    def separates(axis) -> bool:
        if axis == (0, 0, 0):
            return False
        ps = [sum(axis[k] * p[k] for k in range(3)) for p in v]
        r = sum(h[k] * abs(axis[k]) for k in range(3))
        return min(ps) >= r or max(ps) <= -r

    edges = [tuple(v[(i + 1) % 3][k] - v[i][k] for k in range(3)) for i in range(3)]
    units = ((1, 0, 0), (0, 1, 0), (0, 0, 1))
    axes = [*units, _cross(edges[0], edges[1]), *(_cross(u, e) for u in units for e in edges)]
    return not any(separates(axis) for axis in axes)


# ---------------------------------------------------------- the fixed point --


COMPONENT_LEVEL = {"height-coupled"}
UNIT_LEVEL = {"unit-held", "unit-too-large"}


def _coupled(cls: Classification, cur: StoreView, add) -> bool:
    """One round of H9, `unit-held` and `would-empty-tile`, on the codes left
    at the start of the round. True if a code was added.

    H9 holds a member of a height component with a terrain change when
    another member is held; `unit-held` holds a member of a unit when
    another member is held. A member held only by the same coupling (or, in
    a unit, by `unit-too-large`, which holds every member anyway) does not
    count as "another member held": that would only echo the code back to
    the member that caused it.

    H9 also holds what is draped on the height model over a terrain that a
    contribution made (Q17), for as long as that contribution is in the
    current world. That holds even when the terrain's own change is
    accepted: it goes into a decision set, which merges after every other
    set and only if its own review judges it better, so the draped changes
    wait for the plan made after it. Such a change counts as held for its
    component, except for the terrain whose decision it waits for, which it
    would otherwise hold in turn, for ever."""
    left = {id(i): set(i.remaining) for i in cls.items}
    added = False
    by_key = {i.key: i for i in cls.items}
    over_kept: dict[int, set] = {}  # each change held by Q17: the contributed terrains it is draped over
    for key, draped in cls.draped.items():
        own = by_key.get(key)
        by = joined(cls.authors.get(key) or ["an unknown author"])
        if own is not None and not left[id(own)]:
            why = (f"it is draped on the height model over the {show(key)}, a contribution by {by} that a decision "
                   "set of this plan replaces only if its review judges it better; plan again once that set is "
                   "merged (Q17)")
        else:
            why = (f"it is draped on the height model over the {show(key)}, a contribution by {by} that stays, so "
                   "it would float or sink on it (Q17)")
        for item in draped:
            over_kept.setdefault(id(item), set()).add(key)
            added |= add(item, "height-coupled", why)
    for component in cls.heights:
        if not component.terrain:
            continue
        stuck = [i for i in component.items
                 if left[id(i)] - COMPONENT_LEVEL or (id(i) in over_kept and left[id(i)] & COMPONENT_LEVEL)]
        for item in component.items:
            others = [o for o in stuck if o is not item
                      and (left[id(o)] - COMPONENT_LEVEL or not over_kept.get(id(o), set()) <= {item.key})]
            if others:
                more = f" and {counted(len(others) - 1, 'more held change')}" if len(others) > 1 else ""
                added |= add(item, "height-coupled",
                             f"it is draped on the same height-model items as {show(others[0].key)}, which is "
                             f"held ({', '.join(codes.in_rule_order(left[id(others[0])]))}){more}")
    for unit in cls.units:
        if len(unit.items) < 2:
            continue
        stuck = [i for i in unit.items if left[id(i)] - UNIT_LEVEL]
        for item in unit.items:
            others = [o for o in stuck if o is not item]
            if others:
                added |= add(item, "unit-held", f"{show(others[0].key)} in its unit is held "
                                                f"({', '.join(codes.in_rule_order(left[id(others[0])]))})")
    held = {i for i, codes_left in left.items() if codes_left}
    assets = Counter({tile: 0 for tile in cur.tiles})
    for asset in cur.assets.values():
        assets[asset.tile] += 1
    for item in cls.items:
        if id(item) in held:
            continue
        for c in item.changes:
            assets[c.tile] += {"add": 1, "remove": -1}.get(c.op, 0)
    for tile in sorted(cur.tiles, key=text_order):
        if assets[tile] > 0:
            continue
        for item in cls.items:
            if id(item) not in held and any(c.op == "remove" and c.tile == tile for c in item.changes):
                added |= add(item, "would-empty-tile", f"the emitted removals would leave tile {tile} empty")
    return added


def _emptied_between_sets(cur: StoreView, sets: list[list[Item]], add) -> bool:
    """H13 `would-empty-tile` on the sets in merge order. Each set is merged
    on the world the sets before it produce, and the gate fails a set that
    leaves a tile with no asset (G1), even when a later set adds to that
    tile. The first set that leaves a tile empty has its removals in that
    tile held, and nothing else: holding them packs the later sets anew, so
    a later set that empties a tile now may not once they are packed again.
    The caller settles the holds, packs again and asks again. Found by
    review: every tile emptied by any set was held in one pass, and some of
    those holds were not needed after packing again. True if a code was
    added."""
    assets = Counter({tile: 0 for tile in cur.tiles})
    for asset in cur.assets.values():
        assets[asset.tile] += 1
    for items in sets:
        touched = set()
        for item in items:
            for c in item.changes:
                assets[c.tile] += {"add": 1, "remove": -1}.get(c.op, 0)
                touched.add(c.tile)
        emptied = [tile for tile in sorted(touched, key=text_order) if assets[tile] <= 0]
        if not emptied:
            continue
        added = False
        for tile in emptied:
            for item in items:
                if any(c.op == "remove" and c.tile == tile for c in item.changes):
                    added |= add(item, "would-empty-tile",
                                 f"its change set would leave tile {tile} empty, and the gate fails an empty tile "
                                 "(G1); the assets that come into the tile arrive only with a later set")
        if not added:  # cannot happen: a held removal is in no set
            raise RuntimeError("a change set empties a tile, but no removal in it can be held")
        return True
    return False
