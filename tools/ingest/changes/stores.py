"""Reading the two stores, and what the planner asks of a manifest.

`read_store` reads `world.json`, every tile index and every manifest, and
checks what matching by key relies on: canonical bytes, hashes and the
JSON Schemas (through `tools.worldfmt`), each key listed once, and each
tile-index entry agreeing with its manifest. Meshes and other content are
read only on request: to copy them into a bundle, and for the terrain-edge
rule (H8) and the spawn rule (H14).

A store may come from elsewhere, so every file is read as `op_world` reads
a store (RFC 0001, section 7): with `common.read_input`, which takes only
a regular file itself (never a link, a directory or a FIFO) of at most
`common.MAX_INPUT_BYTES`. An object that is not such a file does not count
as present (`has_object`, `object_names`), and reading it is an error.

`content_key` is rule C of ADR-0020 (Q1, Proposed): an asset's content is
its mesh and its manifest, except `parent_version_id`,
`generator.version` and each source's `version`.
"""

from __future__ import annotations

from collections.abc import Iterator, Mapping
from dataclasses import dataclass, field
from functools import cached_property
from pathlib import Path

from tools.ingest import GENERATOR as INGEST_GENERATOR
from tools.ingest.changes.common import (
    Box, Key, PlanError, UnreadableFile, is_regular_file, key_order, name_order, read_input, show,
)
from tools.worldfmt import cjson, glb, schemas
from tools.worldfmt.changes import ChangeError, Objects, tile_xy
from tools.worldfmt.ids import PREFIX, content_id, hex_of, is_content_id
from tools.worldfmt.limits import BY_FORMAT
from tools.worldfmt.store import Store

CONTRIBUTIONS_DATASET = "StreetWide contributions"
"""The source every contribution lists (RFC 0002 G5)."""

DEPENDS_ON = {"facade": "shell"}
"""A contributed asset in the key's layer depends on the asset with the same
feature id, in the same tile, in the mapped layer. This mirrors G8, the only
rule of the contract that ties two assets together; it grows when an RFC
adds such a rule."""

_INDEX_FIELDS = (
    ("tile", lambda m, a, t: m["tile"] == t),
    ("layer", lambda m, a, t: m["layer"] == a["layer"]),
    ("feature_id", lambda m, a, t: m["feature_id"] == a["feature_id"]),
    ("content_id", lambda m, a, t: m["content_id"] == a["content_id"]),
    ("license", lambda m, a, t: m["provenance"]["license"] == a["license"]),
    ("collider", lambda m, a, t: m["physics"]["collider"] == a["collider"]),
    ("bounds_mm", lambda m, a, t: m["bounds_mm"] == a["bounds_mm"]),
)


class StoreObjects(Mapping):
    """The JSON objects (`objects/<aa>/<hex>.json`) of the store in `root`,
    by content id: a source for `Objects` that reads every file with
    `read_input`, where a `Store` source would follow a link and read a file
    of any size. An object counts as present only as a regular file itself.

    Reading one that is there but is not a regular file, or is larger than
    `common.MAX_INPUT_BYTES`, raises ChangeError ("object <id>: <why>"),
    where `op_world`'s layered store would look in its next layer. The
    planner and `verify` always put a store last, so the two only differ in
    the message."""

    def __init__(self, root: Path):
        self.store = Store(Path(root))

    def __getitem__(self, cid: str) -> bytes:
        if not is_content_id(cid):
            raise KeyError(cid)
        try:
            return read_input(self.store.object_path(cid, "json"))
        except UnreadableFile as e:
            if e.missing:
                raise KeyError(cid) from None
            raise ChangeError(f"object {cid}: {e.why}") from e

    def __iter__(self) -> Iterator[str]:
        """Every content id with a JSON object file here; `Objects` never
        asks."""
        for path in sorted((self.store.root / "objects").glob("*/*.json")):
            cid = PREFIX + path.stem
            if is_content_id(cid) and path.parent.name == path.stem[:2] and is_regular_file(path):
                yield cid

    def __len__(self) -> int:
        return sum(1 for _ in self)


class SharedObjects(Objects):
    """`Objects` whose checked tile indexes and manifests are shared with the
    other readers of one plan. Ids are content ids, so a document that passed
    its schema once passes everywhere; each reader still reads the object
    from its own sources and checks its hash, so a missing or broken object
    is reported by the store that lacks it. This keeps the planner from
    checking the same 1,500 district manifests three times.

    A store given as a source, as a `Store` or a directory, is read as
    `StoreObjects` reads it."""

    def __init__(self, *sources, checked: dict | None = None):
        super().__init__(*sources)
        self.checked = {} if checked is None else checked

    def push(self, source) -> None:
        if isinstance(source, Store):
            source = StoreObjects(source.root)
        elif isinstance(source, (str, Path)):
            source = StoreObjects(Path(source))
        super().push(source)

    def _shared(self, cid: str, fmt: str, read) -> dict:
        doc = self.checked.get((cid, fmt))
        if doc is None:
            doc = read(cid)
            self.checked[(cid, fmt)] = doc
        else:
            self.read(cid, BY_FORMAT[fmt])
        return doc

    def tile_index(self, cid: str) -> dict:
        return self._shared(cid, "streetwide.tile", super().tile_index)

    def manifest(self, cid: str) -> dict:
        return self._shared(cid, "streetwide.asset", super().manifest)


def content_view(manifest: dict) -> dict:
    """Rule C's view of a manifest: everything except `parent_version_id`,
    `provenance.generator.version` and every `provenance.sources[i].version`.
    Two versions have the same content when their views are equal. The
    mesh is covered by `content_id`. The manifest is not modified."""
    view = dict(manifest)
    view.pop("parent_version_id", None)
    provenance = dict(view["provenance"])
    provenance["generator"] = {k: v for k, v in provenance["generator"].items() if k != "version"}
    provenance["sources"] = [{k: v for k, v in s.items() if k != "version"} for s in provenance["sources"]]
    view["provenance"] = provenance
    return view


def content_key(manifest: dict) -> str:
    """Rule C as an id: the content id of the canonical `content_view`. An
    accept names the new version by it."""
    return content_id(cjson.dumps(content_view(manifest)))


def without_parent(manifest: dict) -> dict:
    m = dict(manifest)
    m.pop("parent_version_id", None)
    return m


def is_ingest_manifest(manifest: dict) -> bool:
    """The manifest half of "made by the ingest" (Q2, Proposed): the
    ingest's generator, and no `StreetWide contributions` source."""
    provenance = manifest["provenance"]
    return provenance["generator"]["name"] == INGEST_GENERATOR and not any(
        s["dataset"] == CONTRIBUTIONS_DATASET for s in provenance["sources"]
    )


def generator_text(manifest: dict) -> str:
    g = manifest["provenance"]["generator"]
    return f"{g['name']} {g['version']}"


def is_per_tile(layer_tile: str, feature_id: str) -> bool:
    """A per-tile asset's feature id is `tile:<its own tile>`, or starts with
    `tile:<its own tile>/` as RFC 0004's point assets would."""
    own = f"tile:{layer_tile}"
    return feature_id == own or feature_id.startswith(own + "/")


@dataclass(frozen=True)
class Asset:
    """One asset of a store: its tile, its manifest and where it lies."""

    layer: str
    feature_id: str
    tile: str
    manifest_id: str
    manifest: dict = field(repr=False)
    box: Box

    @property
    def key(self) -> Key:
        return (self.layer, self.feature_id)

    @property
    def collider(self) -> str:
        return self.manifest["physics"]["collider"]

    @property
    def content_id(self) -> str:
        return self.manifest["content_id"]

    @property
    def sources(self) -> list[dict]:
        return self.manifest["provenance"]["sources"]

    @property
    def dataset(self) -> str:
        """The first source's dataset: with the layer, the asset's ingest
        group."""
        return self.sources[0]["dataset"]

    @property
    def per_tile(self) -> bool:
        return is_per_tile(self.tile, self.feature_id)

    @cached_property
    def content_view(self) -> dict:
        return content_view(self.manifest)

    @cached_property
    def content_key(self) -> str:
        return content_key(self.manifest)

    def same_content(self, other: Asset) -> bool:
        """Rule C: the same content, the tile included."""
        return self.content_view == other.content_view


class StoreView:
    """A world store read by `read_store`. `label` is "current" or "new",
    as messages name the store."""

    def __init__(self, root: Path, label: str, world_id: str, world_bytes: bytes, world: dict,
                 objects: Objects, assets: dict[Key, Asset]):
        self.root = root
        self.label = label
        self.world_id = world_id
        self.world_bytes = world_bytes
        self.world = world
        self.objects = objects
        self.assets = assets
        self.tiles = {wt["tile"]: wt for wt in world["tiles"]}
        self.origins = {wt["tile"]: tuple(wt["origin_mm"]) for wt in world["tiles"]}
        self._content: dict[str, tuple[str, bytes]] = {}
        self._meshes: dict[str, glb.Mesh] = {}

    @property
    def name(self) -> str:
        return self.world["name"]

    @property
    def spawn(self) -> tuple[int, int, int]:
        return tuple(self.world["spawn"]["position_mm"])

    def has_object(self, cid: str, ext: str) -> bool:
        """Whether this store has object `cid` as a regular file itself, as
        `op_world`'s `Store::has_object` asks: a link, a directory or a FIFO
        does not count."""
        return is_regular_file(Store(self.root).object_path(cid, ext))

    def object_names(self, cid: str, *, regular: bool = True) -> list[str]:
        """The files of object `cid` in this store (`<hex>.<ext>`), in byte
        order: an object keeps the extension it was written with. Only
        regular files count, unless `regular` is false."""
        h = hex_of(cid)
        folder = self.root / "objects" / h[:2]
        if not folder.is_dir():
            return []
        return sorted((p.name for p in folder.glob(f"{h}.*") if not regular or is_regular_file(p)), key=name_order)

    def content(self, asset: Asset) -> tuple[str, bytes]:
        """The extension and bytes of `asset`'s content (its mesh, or another
        content object), as the store keeps them, read with `read_input`.
        The extension is read from the file name, never assumed. Raises
        PlanError."""
        cid = asset.content_id
        found = self._content.get(cid)
        if found is None:
            names = self.object_names(cid)
            where = f"the {self.label} store: tile {asset.tile}, {show(asset.key)}"
            if not names:
                why = "not a regular file" if self.object_names(cid, regular=False) else "not found"
                raise PlanError(f"{where}: content {cid}: {why}")
            ext = names[0].split(".", 1)[1]
            try:
                data = read_input(self.root / "objects" / names[0][:2] / names[0])
            except UnreadableFile as e:
                raise PlanError(f"{where}: content {cid}: {e.why}") from e
            if content_id(data) != cid:
                raise PlanError(f"{where}: content {cid}: its bytes hash to {content_id(data)}")
            if len(data) != asset.manifest["size_bytes"]:
                raise PlanError(f"{where}: size_bytes does not match the content")
            found = (ext, data)
            self._content[cid] = found
        return found

    def mesh(self, asset: Asset) -> glb.Mesh:
        """`asset`'s mesh, read with the strict profile reader. Raises
        PlanError, as `content` does or naming why the mesh is refused."""
        mesh = self._meshes.get(asset.content_id)
        if mesh is None:
            ext, data = self.content(asset)
            try:
                mesh = glb.read(data)
            except glb.GlbError as e:
                raise PlanError(f"the {self.label} store: tile {asset.tile}, {show(asset.key)}: mesh: {e}") from e
            self._meshes[asset.content_id] = mesh
        return mesh


def read_store(root: Path, label: str, checked: dict | None = None) -> StoreView:
    """Reads and checks the store in `root`. Raises PlanError naming the
    store ("the current store: ..." or "the new store: ..."). `checked` is
    the plan's shared cache of checked documents (see `SharedObjects`)."""
    root = Path(root)
    where = f"the {label} store"
    try:
        world_bytes = read_input(root / "world.json")
    except UnreadableFile as e:
        if e.missing:
            raise PlanError(f"{where} has no world.json ({root})") from None
        raise PlanError(f"{where}: {e}") from e
    try:
        world = cjson.loads(world_bytes)
    except cjson.CanonicalError as e:
        raise PlanError(f"{where}: world.json: not canonical JSON: {e}") from e
    errors = schemas.errors(world) if isinstance(world, dict) else ["not a JSON object"]
    if not errors:
        for wt in world["tiles"]:
            try:
                tile_xy(wt["tile"])
            except ValueError as e:
                errors.append(str(e))
        keys = [wt["tile"] for wt in world["tiles"]]
        if len(set(keys)) != len(keys):
            errors.append("a tile is listed twice")
    if errors:
        raise PlanError(f"{where}: world.json: {'; '.join(sorted(errors))}")

    objects = SharedObjects(Store(root), checked=checked)
    assets: dict[Key, Asset] = {}
    for wt in world["tiles"]:
        tile = wt["tile"]
        try:
            index = objects.tile_index(wt["tile_id"])
        except ChangeError as e:
            raise PlanError(f"{where}: tile {tile}: {e}") from e
        if index["tile"] != tile:
            raise PlanError(f"{where}: tile {tile}: index describes tile {index['tile']}")
        if index["origin_mm"] != wt["origin_mm"]:
            raise PlanError(f"{where}: tile {tile}: origin_mm differs from the world manifest")
        for a in index["assets"]:
            key = (a["layer"], a["feature_id"])
            if key in assets:
                raise PlanError(f"{where} lists {show(key)} twice")
            try:
                m = objects.manifest(a["manifest_id"])
            except ChangeError as e:
                raise PlanError(f"{where}: tile {tile}, {show(key)}: manifest: {e}") from e
            for name, agrees in _INDEX_FIELDS:
                if not agrees(m, a, tile):
                    raise PlanError(f"{where}: tile {tile}, {show(key)}: {name} in the tile index differs from "
                                    "the manifest")
            assets[key] = Asset(
                layer=a["layer"], feature_id=a["feature_id"], tile=tile, manifest_id=a["manifest_id"],
                manifest=m, box=Box.of(m["bounds_mm"], wt["origin_mm"]),
            )
    ordered = {k: assets[k] for k in sorted(assets, key=key_order)}
    return StoreView(root, label, content_id(world_bytes), world_bytes, world, objects, ordered)


def read_manifest(view: StoreView, cid: str) -> dict | None:
    """Manifest `cid` from `view`'s store, or None when it cannot be read:
    RFC 0002 lets a takedown's objects be erased, so lineage and removed
    manifests may be missing."""
    try:
        return view.objects.manifest(cid)
    except ChangeError:
        return None
