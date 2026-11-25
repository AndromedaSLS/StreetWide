"""Matching and classification (ADR-0020, Q2 to Q10).

Assets are matched by key, `(layer, feature_id)`; the tile is compared, not
matched. For every key, `classify` decides what the refresh would do, before
any hold: an `Item` with op `add`, `replace`, `remove` or `move` (a removal
from one tile and an add in another, so two changes), or nothing.

Who made the current version (Q2, Proposed: all three tests together):
- its manifest names the generator `streetwide-ingest` and lists no
  `StreetWide contributions` source;
- the last log record that touched the key is the baseline, or a `merged`
  record whose change set's author is `streetwide:ingest`.
Both true: made by the ingest. Both false: a contribution. A disagreement:
origin unclear (H3); such an asset is not made by the ingest, so it counts
as a contribution wherever ADR-0020 says "contribution", and its pending
change is computed as for an asset of the ingest so that the conflict shows
what the refresh would do.

A contribution is changed only when upstream changed, gained or lost the
feature since the ingest version the contribution replaced (Q10, Proposed);
that version is found by following `parent_version_id` through the current
store. Otherwise it is kept, with no pending change.

Protected keys come from the log: the last record that touched the key is a
takedown (H1, whatever its op), or a `merged` record by another author that
removed the key (H4). Removed protected keys keep their bounds for H6 when
they can be read.

Units (ADR-0020, Q8) and height components (H9) are built here too, with
union-find in key order, so they do not depend on the order of anything
else, and so are the changes draped over a terrain that a contribution
made (Q17).
"""

from __future__ import annotations

from collections import Counter
from dataclasses import dataclass, field

from tools.ingest.changes.common import (
    Box, BoxGrid, Key, UnionFind, key_order, overlaps, text_order, touches, unique_sorted,
)
from tools.ingest.changes.stores import (
    CONTRIBUTIONS_DATASET, DEPENDS_ON, Asset, StoreView, content_view, is_ingest_manifest, read_manifest,
)
from tools.worldfmt.changes import INGEST_AUTHOR, ChangeError, is_feature_id
from tools.worldfmt.log import LogError, Record, Touch, WorldLog

STATUSES_IN_CONFLICT = ("changed", "new", "gone", "unknown")


@dataclass(frozen=True)
class Origin:
    """Who made a current asset."""

    kind: str
    """`ingest`, `contribution` or `unclear`."""
    manifest_says_ingest: bool
    log_says_ingest: bool
    last: Record | None
    """The last record that touched the key; None when only the baseline did."""


@dataclass(frozen=True)
class Protection:
    """A key frozen by the log (H1) or removed by a contribution (H4)."""

    kind: str
    """`takedown` or `removed-by-contribution`."""
    record: Record
    removed: bool
    """The record removed the key (a takedown may also have reverted it)."""
    tile: str | None
    box: Box | None
    """Where a removed key was, in the region frame, or None when unknown."""
    bounds_from: str | None
    """`manifest`, `worlds` or None."""

    @property
    def author(self) -> str | None:
        return self.record.author

    def describe(self) -> str:
        what = "a takedown" if self.kind == "takedown" else f"{self.author}'s change set"
        return f"{what} in {self.record.path} ({self.record.record_id})"


@dataclass(frozen=True)
class Change:
    """One change an item emits: `op` on the key in `tile`."""

    op: str
    tile: str
    layer: str
    feature_id: str

    @property
    def sort_key(self) -> tuple[bytes, bytes, bytes]:
        return (self.tile.encode("utf-8"), self.layer.encode("utf-8"), self.feature_id.encode("utf-8"))


@dataclass(eq=False)
class Item:
    """The pending change of one key: what the refresh would emit before any
    hold. Items compare by identity."""

    key: Key
    op: str
    cur: Asset | None
    new: Asset | None
    origin: Origin | None = None
    status: str | None = None
    """Upstream status of a contribution (Q10), else None."""
    codes: dict[str, str] = field(default_factory=dict)
    """Every code that applies, with its details."""
    lifted: dict[str, str] = field(default_factory=dict)
    """Codes lifted, by `accept` or `allowance`."""
    unit: Unit | None = None
    height: HeightComponent | None = None

    @property
    def layer(self) -> str:
        return self.key[0]

    @property
    def feature_id(self) -> str:
        return self.key[1]

    @property
    def changes(self) -> list[Change]:
        layer, fid = self.key
        if self.op == "add":
            return [Change("add", self.new.tile, layer, fid)]
        if self.op == "replace":
            return [Change("replace", self.cur.tile, layer, fid)]
        if self.op == "remove":
            return [Change("remove", self.cur.tile, layer, fid)]
        return [Change("remove", self.cur.tile, layer, fid), Change("add", self.new.tile, layer, fid)]

    @property
    def size(self) -> int:
        """How many changes the item emits: 2 for a move, else 1."""
        return 2 if self.op == "move" else 1

    @property
    def smallest(self) -> tuple[bytes, bytes, bytes]:
        return min(c.sort_key for c in self.changes)

    @property
    def manifest_only(self) -> bool:
        """A replace that keeps the mesh: it ships a manifest and no content."""
        return self.op == "replace" and self.cur.content_id == self.new.content_id

    @property
    def per_tile(self) -> bool:
        return any(a.per_tile for a in (self.cur, self.new) if a is not None)

    @property
    def removes(self) -> Asset | None:
        """The current asset a removal or a move takes away."""
        return self.cur if self.op in ("remove", "move") else None

    @property
    def adds(self) -> Asset | None:
        """The new asset an add or a move places."""
        return self.new if self.op in ("add", "move") else None

    @property
    def current_manifest_id(self) -> str | None:
        return None if self.cur is None else self.cur.manifest_id

    @property
    def new_content_key(self) -> str | None:
        return None if self.new is None else self.new.content_key

    @property
    def remaining(self) -> list[str]:
        """The codes that still hold the change."""
        return [c for c in self.codes if c not in self.lifted]

    @property
    def emitted(self) -> bool:
        return not self.remaining

    @property
    def tile(self) -> str:
        return (self.cur or self.new).tile


@dataclass(eq=False)
class Unit:
    """Changes that are emitted or held whole (ADR-0020, Q8)."""

    items: list[Item]

    @property
    def size(self) -> int:
        return sum(i.size for i in self.items)

    @property
    def smallest(self) -> tuple[bytes, bytes, bytes]:
        return min(i.smallest for i in self.items)

    @property
    def changes(self) -> list[tuple[Change, Item]]:
        return sorted(((c, i) for i in self.items for c in i.changes), key=lambda ci: ci[0].sort_key)

    @property
    def purpose(self) -> tuple[str, str, str]:
        """`(collider, layer, dataset)` (ADR-0020, section 5): `trimesh` if any current or
        new version is a trimesh; the layer; and the first source's dataset
        of the new manifest of the smallest add or replace, else of the
        current manifest of the smallest removal."""
        versions = [a for i in self.items for a in (i.cur, i.new) if a is not None]
        collider = "trimesh" if any(a.collider == "trimesh" for a in versions) else "none"
        changes = self.changes
        layer = changes[0][1].layer
        placed = [i.new for c, i in changes if c.op in ("add", "replace")]
        if placed:
            dataset = placed[0].dataset
        else:
            dataset = next(i.cur for c, i in changes if c.op == "remove").dataset
        return (collider, layer, dataset)

    @property
    def emitted(self) -> bool:
        return all(i.emitted for i in self.items)


@dataclass(eq=False)
class HeightComponent:
    """Changes draped on new height-model data, and terrain changes, linked
    by shared DEM items (H9)."""

    items: list[Item]
    dem_ids: list[tuple[str, str]]

    @property
    def terrain(self) -> list[Item]:
        return [i for i in self.items if i.layer == "terrain"]


@dataclass
class Classification:
    items: list[Item]
    origins: dict[Key, Origin]
    protections: dict[Key, Protection]
    statuses: dict[Key, str]
    """Upstream status of every current asset not made by the ingest."""
    kept: list[Key]
    """Contributions (and unclear assets) with no pending change."""
    unchanged: Counter
    provenance_only: Counter
    provenance_only_keys: set[Key]
    groups: dict[tuple[str, str], int]
    """Current assets made by the ingest, by ingest group."""
    new_groups: dict[tuple[str, str], int]
    dem_datasets: list[str]
    dependents: dict[Key, list[Asset]]
    """For a key that contributions depend on, the contributed assets."""
    units: list[Unit]
    heights: list[HeightComponent]
    authors: dict[Key, list[str]]
    draped: dict[Key, list[Item]] = field(default_factory=dict)
    """For each current terrain asset not made by the ingest, the pending
    adds, replaces and moves draped on the height model (their new
    manifest lists DEM data) whose new bounds overlap it, in key order
    (Q17). While that terrain stays, they would float or sink on it."""


# ---------------------------------------------------------------- origin --


def _log_says_ingest(last: tuple[Touch, ...] | None) -> bool:
    if not last:
        return True
    record = last[0].record
    return record.decision == "merged" and record.author == INGEST_AUTHOR


def _removed(touches: tuple[Touch, ...]) -> bool:
    ops = {t.op for t in touches}
    return "remove" in ops and not ops & {"add", "replace"}


def _origins(cur: StoreView, last: dict[Key, tuple[Touch, ...]]) -> dict[Key, Origin]:
    origins = {}
    for key, asset in cur.assets.items():
        touches = last.get(key)
        by_manifest = is_ingest_manifest(asset.manifest)
        by_log = _log_says_ingest(touches)
        kind = "ingest" if by_manifest and by_log else "contribution" if not by_manifest and not by_log else "unclear"
        origins[key] = Origin(kind, by_manifest, by_log, touches[0].record if touches else None)
    return origins


def _protections(cur: StoreView, wlog: WorldLog, last: dict[Key, tuple[Touch, ...]]) -> dict[Key, Protection]:
    found = {}
    for key, touches in last.items():
        record = touches[0].record
        removed = _removed(touches)
        if record.decision == "takedown":
            kind = "takedown"
        elif record.decision == "merged" and record.author != INGEST_AUTHOR and removed:
            kind = "removed-by-contribution"
        else:
            continue
        tile = box = source = None
        if removed:
            removal = next(t for t in touches if t.op == "remove")
            tile = removal.tile
            box, source = _removed_bounds(cur, wlog, removal)
        found[key] = Protection(kind, record, removed, tile, box, source)
    return found


def _removed_bounds(cur: StoreView, wlog: WorldLog, removal: Touch) -> tuple[Box | None, str | None]:
    """Where a removed key was: from its removed manifest, else from the
    tile index of the record's `world_before` kept in `<log>/worlds/`, else
    unknown."""
    manifest = read_manifest(cur, removal.parent_version_id)
    if manifest is not None:
        origin = cur.origins.get(removal.tile)
        if origin is None:
            origin = _origin_in(wlog, removal.record.world_before, removal.tile)
        if origin is not None:
            return Box.of(manifest["bounds_mm"], origin), "manifest"
    try:
        world = wlog.world(removal.record.world_before)
    except LogError:
        world = None
    if world is None:
        return None, None
    for wt in world.get("tiles", []):
        if wt.get("tile") != removal.tile:
            continue
        try:
            index = cur.objects.tile_index(wt["tile_id"])
        except ChangeError:  # an erased or unreadable index: the bounds are unknown
            return None, None
        for a in index["assets"]:
            if (a["layer"], a["feature_id"]) == (removal.layer, removal.feature_id):
                return Box.of(a["bounds_mm"], wt["origin_mm"]), "worlds"
    return None, None


def _origin_in(wlog: WorldLog, world_id: str | None, tile: str) -> tuple[int, int, int] | None:
    try:
        world = wlog.world(world_id) if world_id else None
    except LogError:
        return None
    for wt in (world or {}).get("tiles", []):
        if wt.get("tile") == tile:
            return tuple(wt["origin_mm"])
    return None


def upstream_status(cur: StoreView, asset: Asset, new: Asset | None) -> str:
    """Q10: compare the new data with the ingest version that `asset`
    replaced, found by following `parent_version_id` from the current
    version itself. `unknown` when a manifest of the lineage cannot be
    read."""
    manifest, seen = asset.manifest, {asset.manifest_id}
    while not is_ingest_manifest(manifest):
        parent = manifest["parent_version_id"]
        if parent is None:
            return "new" if new is not None else "none"
        if parent in seen:
            return "unknown"
        seen.add(parent)
        manifest = read_manifest(cur, parent)
        if manifest is None:
            return "unknown"
    if new is None:
        return "gone"
    return "unchanged" if content_view(manifest) == new.content_view else "changed"


def _authors(cur: StoreView, wlog: WorldLog, origins: dict[Key, Origin]) -> dict[Key, list[str]]:
    """Public handles of the authors of each current asset not made by the
    ingest: from the log's change sets that added or replaced it, or else
    from its manifest's `StreetWide contributions` source."""
    by_key: dict[Key, set[str]] = {}
    for t in wlog.touches():
        author = t.record.author
        if t.op in ("add", "replace") and author and author != INGEST_AUTHOR and t.record.decision == "merged":
            by_key.setdefault(t.key, set()).add(author)
    out = {}
    for key, origin in origins.items():
        if origin.kind == "ingest":
            continue
        names = by_key.get(key)
        if not names:
            names = {
                handle for s in cur.assets[key].sources if s["dataset"] == CONTRIBUTIONS_DATASET
                for handle in s["source_ids"] if is_feature_id(handle)
            }
        out[key] = unique_sorted(names)
    return out


# ---------------------------------------------------------------- items --


def classify(cur: StoreView, new: StoreView, wlog: WorldLog) -> Classification:
    last = wlog.last_touches()
    origins = _origins(cur, last)
    protections = _protections(cur, wlog, last)
    statuses: dict[Key, str] = {}
    items: list[Item] = []
    kept: list[Key] = []
    unchanged: Counter = Counter()
    provenance_only: Counter = Counter()
    provenance_only_keys: set[Key] = set()

    for key in sorted(set(cur.assets) | set(new.assets), key=key_order):
        c, n = cur.assets.get(key), new.assets.get(key)
        if c is None:
            items.append(Item(key, "add", None, n))
            continue
        origin = origins[key]
        if origin.kind != "ingest":
            statuses[key] = upstream_status(cur, c, n)
        if origin.kind == "contribution":
            status = statuses[key]
            if status not in STATUSES_IN_CONFLICT or (n is not None and n.same_content(c)):
                kept.append(key)
                continue
            op = "remove" if n is None else "replace" if n.tile == c.tile else "move"
            items.append(Item(key, op, c, n, origin, status))
            continue
        if n is None:
            items.append(Item(key, "remove", c, None, origin, statuses.get(key)))
        elif n.tile != c.tile:
            items.append(Item(key, "move", c, n, origin, statuses.get(key)))
        elif not n.same_content(c):
            items.append(Item(key, "replace", c, n, origin, statuses.get(key)))
        else:
            if origin.kind == "unclear":
                kept.append(key)
            same = (c.manifest["provenance"] == n.manifest["provenance"])
            if same:
                unchanged[key[0]] += 1
            else:
                provenance_only[key[0]] += 1
                provenance_only_keys.add(key)

    groups: Counter = Counter()
    for key, asset in cur.assets.items():
        if origins[key].kind == "ingest":
            groups[(asset.layer, asset.dataset)] += 1
    new_groups: Counter = Counter((a.layer, a.dataset) for a in new.assets.values())

    dem = set()
    for key, asset in cur.assets.items():
        if asset.layer == "terrain" and origins[key].kind == "ingest":
            dem.update(s["dataset"] for s in asset.sources)
    for asset in new.assets.values():
        if asset.layer == "terrain":
            dem.update(s["dataset"] for s in asset.sources)
    dem.discard(CONTRIBUTIONS_DATASET)

    dependents: dict[Key, list[Asset]] = {}
    for key, asset in cur.assets.items():
        base_layer = DEPENDS_ON.get(asset.layer)
        if base_layer is None or origins[key].kind == "ingest":
            continue
        base = cur.assets.get((base_layer, asset.feature_id))
        if base is not None and base.tile == asset.tile:
            dependents.setdefault(base.key, []).append(asset)

    result = Classification(
        items=items, origins=origins, protections=protections, statuses=statuses, kept=kept,
        unchanged=unchanged, provenance_only=provenance_only, provenance_only_keys=provenance_only_keys,
        groups=dict(sorted(groups.items(), key=lambda kv: key_order(kv[0]))),
        new_groups=dict(sorted(new_groups.items(), key=lambda kv: key_order(kv[0]))),
        dem_datasets=unique_sorted(dem), dependents=dependents,
        units=build_units(items), heights=[], authors=_authors(cur, wlog, origins),
    )
    result.heights = build_heights(items, set(result.dem_datasets))
    result.draped = build_draped(cur, items, origins, set(result.dem_datasets))
    return result


# ----------------------------------------------------------------- units --


def build_units(items: list[Item]) -> list[Unit]:
    """Units (ADR-0020, Q8): each per-tile item alone; per-feature items linked when
    one removes (a removal, or a move's old place) and the other places (an
    add, a move's new place, or a replace whose bounds change), in the same
    layer and with the same collider, and they overlap. The two halves of a
    move are one item already."""
    position = {id(i): n for n, i in enumerate(items)}
    order = lambda i: position[id(i)]  # noqa: E731 (items are in key order)
    per_feature = [i for i in items if not i.per_tile]
    uf = UnionFind(per_feature, order)
    grids: dict[tuple[str, str], BoxGrid] = {}
    for i in per_feature:
        placed = _placed_box(i)
        if placed is not None:
            grids.setdefault((i.layer, i.new.collider), BoxGrid()).add(placed, i)
    for i in per_feature:
        gone = i.removes
        if gone is None:
            continue
        grid = grids.get((i.layer, gone.collider))
        for box, other in grid.near(gone.box) if grid else []:
            if other is not i and overlaps(gone.box, box):
                uf.union(i, other)
    units = [Unit(list(group)) for group in uf.groups()]
    units += [Unit([i]) for i in items if i.per_tile]
    units.sort(key=lambda u: u.smallest)
    for u in units:
        for i in u.items:
            i.unit = u
    return units


def _placed_box(item: Item) -> Box | None:
    if item.op in ("add", "move"):
        return item.new.box
    if item.op == "replace" and item.new.box != item.cur.box:
        return item.new.box
    return None


# ------------------------------------------------------ height components --


def dem_entries(asset: Asset | None, dem: set[str]) -> list[tuple[str, str, tuple[str, ...]]]:
    """The asset's sources in a DEM dataset, as `(dataset, version,
    source_ids)`."""
    if asset is None:
        return []
    return sorted((s["dataset"], s["version"], tuple(s["source_ids"])) for s in asset.sources if s["dataset"] in dem)


def dem_ids(asset: Asset | None, dem: set[str]) -> set[tuple[str, str]]:
    return {(d, sid) for d, _, ids in dem_entries(asset, dem) for sid in ids}


def is_redraped(item: Item, dem: set[str]) -> bool:
    """A pending add, replace or move whose new manifest lists DEM data, and
    whose DEM entries or generator version differ from the current
    version's. An add with DEM entries always counts."""
    if item.new is None or item.op == "remove":
        return False
    entries = dem_entries(item.new, dem)
    if not entries:
        return False
    if item.cur is None:
        return True
    old_gen = item.cur.manifest["provenance"]["generator"]["version"]
    new_gen = item.new.manifest["provenance"]["generator"]["version"]
    return entries != dem_entries(item.cur, dem) or old_gen != new_gen


def build_heights(items: list[Item], dem: set[str]) -> list[HeightComponent]:
    """Height components (H9): re-draped changes and terrain changes,
    linked when their current or new manifests share a DEM source id."""
    members = [i for i in items if i.layer == "terrain" or is_redraped(i, dem)]
    if not members:
        return []
    position = {id(i): n for n, i in enumerate(members)}
    uf = UnionFind(members, lambda i: position[id(i)])
    first: dict[tuple[str, str], Item] = {}
    ids_of = {}
    for i in members:
        ids = dem_ids(i.cur, dem) | dem_ids(i.new, dem)
        ids_of[id(i)] = ids
        for sid in sorted(ids, key=lambda p: (text_order(p[0]), text_order(p[1]))):
            if sid in first:
                uf.union(first[sid], i)
            else:
                first[sid] = i
    components = []
    for group in uf.groups():
        ids = set().union(*(ids_of[id(i)] for i in group))
        component = HeightComponent(list(group), sorted(ids, key=lambda p: (text_order(p[0]), text_order(p[1]))))
        for i in group:
            i.height = component
        components.append(component)
    return components


def build_draped(cur: StoreView, items: list[Item], origins: dict[Key, Origin],
                 dem: set[str]) -> dict[Key, list[Item]]:
    """Q17: for each current terrain asset not made by the ingest, the
    pending adds, replaces and moves of other layers whose new manifest
    lists DEM data and whose new bounds overlap the terrain's by a positive
    area. The ingest drapes them on the height model, not on that
    terrain."""
    grid = BoxGrid()
    for key, asset in cur.assets.items():
        if asset.layer == "terrain" and origins[key].kind != "ingest":
            grid.add(asset.box, asset)
    found: dict[Key, list[Item]] = {}
    for i in items:
        if i.op == "remove" or i.layer == "terrain" or not dem_entries(i.new, dem):
            continue
        for box, terrain in grid.near(i.new.box):
            if touches(i.new.box, box):
                found.setdefault(terrain.key, []).append(i)
    return {k: found[k] for k in sorted(found, key=key_order)}

