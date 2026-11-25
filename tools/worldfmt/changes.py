"""Change sets (RFC 0002 section 2) in Python, in agreement with `op_world`.

- `problems(change)` lists every rule a change set breaks: the JSON Schema
  (`schemas/change-set.schema.json`), the ids and tile keys that op_world
  parses, and the rules of `ChangeSet::problems`
  (crates/op_world/src/contrib.rs) in its own words. `parse` and
  `to_bytes` read and write `change.json` with these checks.
- `apply(world, change, objects)` builds the world that a change set
  produces, byte for byte as the gate's `build_candidate`
  (crates/op_world/src/gate.rs) builds it: the new tile indexes, the new
  world manifest and its id.
- `Objects` reads JSON objects by content id from several sources, where
  the first source that has an object gives it, as op_world's layered
  store reads a bundle over the canonical store. A manifest must pass the
  schema and `manifest_problems`, the rules of `AssetManifest::problems`
  (crates/op_world/src/manifest.rs) that the schema cannot state, as it
  must for the gate to read it.

Change sets and stores come from anyone, so every reader here takes any
input: `parse` any bytes, `problems` any parsed JSON value, `Objects` any
directory and `apply` any parsed world and change set. Each returns a
value or raises ChangeError (`problems` only returns), with time and
memory in proportion to its input. Files are read with
`store.read_file`: a regular file itself (not a link, a FIFO or a
device) of at most MAX_FILE_BYTES. A change set, a tile index or an
asset manifest over the limit for its kind (`limits`, RFC 0001 section
7) is refused before it is parsed, with op_world's message.

The field rules (`is_text`, `is_feature_id`, `is_license`,
`is_https_url`, `is_sorted_unique`, `tile_xy`) mirror
crates/op_world/src/rules.rs and keys.rs.

Messages agree with op_world's word for word where they quote ASCII
values. `rust_debug` escapes what Rust's `{:?}` escapes as far as
Python's Unicode tables allow: where the two Unicode versions differ on
whether a character is printable, the quoted value can differ.

Typical use, for a set planned against the canonical store:

    store = Store(canonical_dir)
    world_id, world = store.read_world()
    objects = Objects(store)
    objects.push(new_manifests)            # {manifest id: bytes} of the set
    assert problems(change) == []
    candidate = apply(world, change, objects)
    objects.push(candidate.objects)        # the next set builds on this one
    world = candidate.world
"""

from __future__ import annotations

import copy
import json
import re
import stat
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Union

from tools.worldfmt import cjson, schemas
from tools.worldfmt.ids import content_id, is_content_id
from tools.worldfmt.limits import BY_FORMAT, CHANGE_SET, TILE_INDEX, WORLD_MANIFEST, Kind
from tools.worldfmt.rust import rust_debug
from tools.worldfmt.store import MAX_FILE_BYTES, Store, read_file

FORMAT = "streetwide.change"
CONTRIBUTION_CONTRACT_VERSION = "0.1.0"
"""Version of the contribution contract (RFC 0002)."""
INGEST_AUTHOR = "streetwide:ingest"
"""Author of baseline refreshes produced by `tools/ingest`."""
OPS = ("add", "remove", "replace")
SCHEMA_FILE = "change-set.schema.json"

MAX_TEXT_CHARS = 4096
MAX_FEATURE_ID_BYTES = 256
TILE_ZOOM = 17
TILE_CELLS = 1 << TILE_ZOOM
"""Tiles along each axis at zoom 17: x and y are below this."""

_FEATURE_ID = re.compile(r"[a-z][a-z0-9_-]*:[!-~]+")
_LICENSE = re.compile(r"[A-Za-z0-9.+:() -]+")
_URL_REST = re.compile(r"[!-~]+")
_CONTROL = re.compile(r"[\x00-\x1f\x7f]")
_TOO_DEEP = f"arrays and objects nested deeper than {cjson.MAX_DEPTH} levels"
_UNSIGNED = re.compile(r"\+?[0-9]+")
_OP_RULE = "add needs a manifest and no parent, replace both, remove a parent and no manifest"


def _schema_max_changes() -> int:
    schema = json.loads((schemas.SCHEMA_DIR / SCHEMA_FILE).read_text(encoding="utf-8"))
    changes = schema["properties"]["changes"]
    if changes.get("minItems") != 1:
        raise RuntimeError(f"{SCHEMA_FILE}: changes must have minItems 1, as op_world requires")
    return changes["maxItems"]


MAX_CHANGES = _schema_max_changes()
"""Most changes in one change set: `maxItems` in the change-set schema
(`MAX_CHANGES` in crates/op_world/src/contrib.rs, RFC 0002 section 2)."""


class ChangeError(ValueError):
    """A change set, or an object it needs, breaks a rule; the message says which."""


# ------------------------------------------------------------ field rules --


def is_text(s: object) -> bool:
    """`rules::is_text`: a non-empty string of at most 4096 characters, with
    no control character (U+0000 to U+001F, U+007F). A lone surrogate,
    which Rust strings cannot hold, is left to canonical JSON to refuse."""
    return isinstance(s, str) and 0 < len(s) <= MAX_TEXT_CHARS and _CONTROL.search(s) is None


# The next three patterns are ASCII, so a string they match has as many
# bytes as characters: the byte limits are checked on its length, which
# needs no encoding (a lone surrogate cannot be encoded).


def is_feature_id(s: object) -> bool:
    """`rules::is_feature_id`: `^[a-z][a-z0-9_-]*:[!-~]+$`, at most 256
    bytes. Handles such as `github:<login>` follow the same rule."""
    return isinstance(s, str) and len(s) <= MAX_FEATURE_ID_BYTES and _FEATURE_ID.fullmatch(s) is not None


def is_license(s: object) -> bool:
    """`rules::is_license`: `^[A-Za-z0-9.+:() -]+$`, at most 256 bytes."""
    return isinstance(s, str) and len(s) <= 256 and _LICENSE.fullmatch(s) is not None


def is_https_url(s: object) -> bool:
    """`rules::is_https_url`: `^https://[!-~]+$`, at most 2048 bytes."""
    return (
        isinstance(s, str)
        and len(s) <= 2048
        and s.startswith("https://")
        and _URL_REST.fullmatch(s[len("https://"):]) is not None
    )


def is_sorted_unique(items: Sequence[Any]) -> bool:
    """`rules::is_sorted_unique`: strictly increasing."""
    return all(a < b for a, b in zip(items, items[1:]))


def _texts_sorted_unique(items: Sequence[Any]) -> bool:
    # As Rust compares `String`s: by their UTF-8 bytes. "surrogatepass" lets
    # a lone surrogate through: only a document canonical JSON refuses has one.
    return all(is_text(t) for t in items) and is_sorted_unique([t.encode("utf-8", "surrogatepass") for t in items])


POINT_LAYERS = ("street_objects", "vegetation")
"""The layers whose assets are points documents (RFC 0004), with feature ids
`tile:17/{x}/{y}/{source}`."""


def point_feature_id_problem(layer: str, feature_id: str, tile: str) -> str | None:
    """`manifest::point_feature_id_problem`: why the valid feature id of an
    asset in `layer` and `tile` breaks RFC 0004's rule for point assets
    (their feature id is `tile:<their own tile>/<source>`), or None."""
    from tools.worldfmt.points import per_tile_source  # points imports this module

    if layer in POINT_LAYERS and per_tile_source(feature_id, tile) is None:
        return f"{layer} feature_id {rust_debug(feature_id)} must be tile:{tile}/<source> (RFC 0004)"
    return None


def manifest_problems(m: Mapping[str, Any]) -> list[str]:
    """The rules of `AssetManifest::problems` that the asset-manifest
    schema cannot state, in op_world's order and words; [] when op_world
    also accepts a manifest that passes the schema. `m` must pass the
    schema first. The schema states the others, and Python's `re` lets its
    patterns accept a final newline, so the field rules run again here."""
    found = []
    if not is_feature_id(m["feature_id"]):
        found.append(f"feature_id {rust_debug(m['feature_id'])} is not valid")
    else:
        problem = point_feature_id_problem(m["layer"], m["feature_id"], m["tile"])
        if problem is not None:
            found.append(problem)
    bounds = m["bounds_mm"]
    if any(bounds["min"][k] > bounds["max"][k] for k in range(3)):
        found.append("bounds_mm min must not exceed max")
    provenance = m["provenance"]
    if not is_license(provenance["license"]):
        found.append(f"provenance.license {rust_debug(provenance['license'])} is not valid")
    attribution = provenance["attribution"]
    if not attribution or not _texts_sorted_unique(attribution):
        found.append("provenance.attribution must be non-empty, sorted and unique")
    for i, s in enumerate(provenance["sources"]):
        if not all(is_text(s[k]) for k in ("dataset", "version", "attribution")):
            found.append(f"provenance.sources[{i}] has an empty or invalid text field")
        if not is_license(s["license"]):
            found.append(f"provenance.sources[{i}].license is not valid")
        if not is_https_url(s["url"]):
            found.append(f"provenance.sources[{i}].url must be an https URL")
        if not _texts_sorted_unique(s["source_ids"]):
            found.append(f"provenance.sources[{i}].source_ids must be sorted and unique")
    generator = provenance["generator"]
    if not is_text(generator["name"]) or not is_text(generator["version"]):
        found.append("provenance.generator needs a name and a version")
    return found


def tile_xy(key: str) -> tuple[int, int]:
    """The `(x, y)` of a zoom-17 tile key `17/x/y`, parsed as
    `TileKey::parse` parses it: x and y below 131072, written without sign
    or leading zeros. Raises ValueError with op_world's message.

    Sorting by `tile_xy` gives the order of tiles in world manifests;
    change sets sort tiles as strings instead (see `sort_key`)."""
    invalid = ValueError(f"{rust_debug(key)} is not a zoom-{TILE_ZOOM} key")
    parts = key.split("/")
    if len(parts) != 3 or parts[0] != str(TILE_ZOOM):
        raise invalid
    numbers = []
    for part in parts[1:]:
        # `u32::from_str`: an optional plus sign, then ASCII digits.
        if not _UNSIGNED.fullmatch(part) or int(part) >= 1 << 32:
            raise invalid
        numbers.append(int(part))
    x, y = numbers
    if not (x < TILE_CELLS and y < TILE_CELLS):
        raise ValueError(f"cell {TILE_ZOOM}/{x}/{y} is out of range (x, y < {TILE_CELLS})")
    if f"{TILE_ZOOM}/{x}/{y}" != key:
        raise invalid
    return x, y


def sort_key(change: Mapping[str, Any]) -> tuple[bytes, bytes, bytes]:
    """The order of changes in a change set (`Change::sort_key`): tile key,
    layer and feature id, each as a string in byte order. This is not the
    numeric tile order of world manifests: "17/10/5" sorts before "17/9/5".
    A lone surrogate, which only an invalid document holds, sorts by its
    surrogate code (it cannot be UTF-8)."""
    return (
        change["tile"].encode("utf-8", "surrogatepass"),
        change["layer"].encode("utf-8", "surrogatepass"),
        change["feature_id"].encode("utf-8", "surrogatepass"),
    )


# ---------------------------------------------------------------- checks --


def problems(change: Any) -> list[str]:
    """Every rule `change` (a change set document) breaks; [] when op_world
    accepts it. In three layers, as op_world reads a change set:

    1. the JSON Schema: each message starts with "schema: " and the JSON
       path of the value;
    2. the content ids and tile keys that op_world parses
       (`ContentId::parse`, `TileKey::parse`), with their messages. The
       schema states them as patterns, which Python's `re` applies more
       loosely (`$` also matches before a final newline), and it does not
       bound x and y;
    3. `ChangeSet::problems`, message for message: format, contract
       version, title and rationale as text, the author as a handle, 1 to
       `MAX_CHANGES` changes, sorted by `sort_key` with no duplicates,
       valid feature ids and the op rules.

    Layers 2 and 3 pass over values of the wrong JSON type, which layer 1
    reports. The canonical form of the bytes is checked by `parse`.

    `change` may be any parsed JSON value. One nested deeper than
    canonical JSON allows (`cjson.MAX_DEPTH`) gets that one message,
    "arrays and objects nested deeper than 127 levels", as op_world's
    parser refuses it before anything else. A set of more than
    MAX_CHANGES + 1 changes, which breaks the cap whatever they are, is
    checked against the schema as its first MAX_CHANGES + 1 changes:
    jsonschema takes about a millisecond per change, so a crafted set of
    thousands took seconds.
    """
    if cjson.deeper_than(change, cjson.MAX_DEPTH):
        return [_TOO_DEEP]
    return _problems(change)


def _problems(change: Any) -> list[str]:
    """`problems` of a value nested at most `cjson.MAX_DEPTH` levels deep."""
    entries = change.get("changes") if isinstance(change, dict) else None
    if isinstance(entries, list) and len(entries) > MAX_CHANGES + 1:
        found = _schema_problems(SCHEMA_FILE, dict(change, changes=entries[: MAX_CHANGES + 1]))
    else:
        found = _schema_problems(SCHEMA_FILE, change)
    if isinstance(change, dict):
        found += id_problems(change)
        found += rule_problems(change)
    return found


def schema_problems(schema_file: str, doc: Any) -> list[str]:
    """The errors of `doc` against `schemas/<schema_file>`, as
    "schema: <JSON path>: <message>", sorted. jsonschema recurses and
    quotes values in its messages, so a document nested deeper than
    `cjson.MAX_DEPTH` is not validated: it gets the one message
    "schema: $: arrays and objects nested deeper than 127 levels"."""
    if cjson.deeper_than(doc, cjson.MAX_DEPTH):
        return [f"schema: $: {_TOO_DEEP}"]
    return _schema_problems(schema_file, doc)


def _schema_problems(schema_file: str, doc: Any) -> list[str]:
    errors = sorted((e.json_path, e.message) for e in schemas.validator(schema_file).iter_errors(doc))
    return [f"schema: {path}: {message}" for path, message in errors]


def content_id_problem(field: str, value: Any) -> list[str]:
    """`ContentId::parse`'s message for a string that is not a content id,
    as a one-item list naming `field`; [] for a content id and for any
    other JSON type."""
    if isinstance(value, str) and not is_content_id(value):
        return [f"{field}: {rust_debug(value)} is not a content id (blake3:<64 hex>)"]
    return []


def id_problems(change: dict) -> list[str]:
    """Layer 2 of `problems`: the content ids and tile keys op_world
    parses, each message naming its field."""
    found = content_id_problem("base_world_id", change.get("base_world_id"))
    entries = change.get("changes")
    for i, c in enumerate(entries if isinstance(entries, list) else []):
        if not isinstance(c, dict):
            continue
        tile = c.get("tile")
        if isinstance(tile, str):
            try:
                tile_xy(tile)
            except ValueError as e:
                found.append(f"changes[{i}].tile: {e}")
        for field in ("parent_version_id", "manifest_id"):
            found += content_id_problem(f"changes[{i}].{field}", c.get(field))
    return found


def rule_problems(change: dict) -> list[str]:
    """Layer 3 of `problems`: `ChangeSet::problems`, in its order and
    words. When the change set gets past op_world's parser, op_world's
    gate reports these as "change.json: invalid: <messages joined by '; '>"."""
    found = []
    if change.get("format") != FORMAT:
        found.append(f"format must be {rust_debug(FORMAT)}")
    if change.get("contract_version") != CONTRIBUTION_CONTRACT_VERSION:
        found.append(f"unknown contribution contract version {rust_debug(change.get('contract_version'))}")
    for name in ("title", "rationale"):
        if not is_text(change.get(name)):
            found.append(f"{name} must be non-empty text")
    if not is_feature_id(change.get("author")):
        found.append("author must be a handle such as github:<login>")
    entries = change.get("changes")
    if not isinstance(entries, list):
        return found
    if not 1 <= len(entries) <= MAX_CHANGES:
        found.append(f"a change set has 1..={MAX_CHANGES} changes")
    typed = [
        c for c in entries
        if isinstance(c, dict) and all(isinstance(c.get(k), str) for k in ("tile", "layer", "feature_id"))
    ]
    if len(typed) == len(entries) and not is_sorted_unique([sort_key(c) for c in typed]):
        found.append("changes must be sorted by (tile, layer, feature_id) with no duplicates")
    for c in typed:
        what = f"{c['tile']} {c['layer']} {c['feature_id']}"
        if not is_feature_id(c["feature_id"]):
            found.append(f"{what}: invalid feature_id")
        parent, manifest, op = c.get("parent_version_id"), c.get("manifest_id"), c.get("op")
        # An op of another JSON type is the schema's to report (a list or
        # an object cannot even be looked up).
        ok = {
            "add": parent is None and manifest is not None,
            "replace": parent is not None and manifest is not None,
            "remove": parent is not None and manifest is None,
        }.get(op) if isinstance(op, str) else None
        if ok is False:
            found.append(f"{what}: {_OP_RULE}")
    return found


def parse(data: bytes) -> dict:
    """Reads the bytes of a `change.json` as `ChangeSet::from_bytes` does:
    at most the limit for change sets (8 MiB, `limits.CHANGE_SET`),
    checked before parsing, then canonical JSON, then `problems`. Raises
    ChangeError."""
    if not CHANGE_SET.fits(len(data)):
        raise ChangeError(CHANGE_SET.too_large())
    try:
        change = cjson.loads(data)  # nested at most cjson.MAX_DEPTH levels deep
    except cjson.CanonicalError as e:
        raise ChangeError(f"not canonical JSON: {e}") from e
    found = _problems(change)
    if found:
        raise ChangeError("; ".join(found))
    return change


def to_bytes(change: dict) -> bytes:
    """The bytes of `change.json` for `change`, after `problems` and within
    the limit for change sets (as `ChangeSet::to_bytes`). The change id is
    `content_id` of these bytes. Raises ChangeError."""
    found = problems(change)
    if found:
        raise ChangeError("; ".join(found))
    try:
        data = cjson.dumps(change)
    except cjson.CanonicalError as e:  # a value only Python holds, such as a float
        raise ChangeError(f"not canonical JSON: {e}") from e
    if not CHANGE_SET.fits(len(data)):
        raise ChangeError(CHANGE_SET.too_large())
    return data


# --------------------------------------------------------------- objects --

Source = Union[Store, Path, str, Mapping[str, bytes]]


class Objects:
    """JSON objects (tile indexes and asset manifests) by content id.

    Sources are searched in order, and the first one that has an object
    gives it, as op_world's layered store reads a bundle over the canonical
    store: `Objects(bundle_dir, canonical_dir)`. A source is a `Store`, a
    store directory, or a mapping from content id to bytes for objects that
    exist only in memory (a planned set's manifests, or the tile indexes
    `apply` returns). `push` puts a source in front of the others.

    Every object read must hash to its id. A tile index or an asset
    manifest over the limit for its kind (`limits`) is refused before it is
    parsed, and a file before it is read, as op_world's
    `Store::read_document` refuses it. Parsed documents are cached and
    shared between calls, so callers must not modify them. Sources can only
    be added, never removed, so whatever was read once stays readable.
    """

    def __init__(self, *sources: Source) -> None:
        self._sources: list[Store | Mapping[str, bytes]] = []
        self._docs: dict[str, dict] = {}
        self._sizes: dict[str, int] = {}
        self._checked: dict[tuple[str, str], dict] = {}
        for source in reversed(sources):
            self.push(source)

    def push(self, source: Source) -> None:
        """Puts `source` in front of the other sources."""
        if isinstance(source, (str, Path)):
            source = Store(Path(source))
        if not isinstance(source, (Store, Mapping)):
            raise TypeError(f"an object source is a Store, a directory or a mapping, not {type(source).__name__}")
        self._sources.insert(0, source)

    def read(self, cid: str, kind: Kind | None = None) -> bytes:
        """The bytes of JSON object `cid`, from the first source that has it.

        A directory has an object only as a regular file: as in op_world's
        layered store, a link, a directory or a FIFO in one source does not
        hide the object in the next. The file is read with
        `store.read_file`, at most MAX_FILE_BYTES. Given its `kind` (a
        `limits.Kind`), an object over the limit for its kind is refused
        with op_world's message, a file before it is read. Raises
        ChangeError when `cid` is not a content id, no source has the
        object, its file cannot be read, it is too large, or its bytes hash
        to another id."""
        if not is_content_id(cid):
            raise ChangeError(f"{rust_debug(cid)} is not a content id (blake3:<64 hex>)")
        for source in self._sources:
            if isinstance(source, Store):
                path = source.object_path(cid, "json")
                size = _regular_file_size(path)
                if size is None:
                    continue
                if kind is not None and not kind.fits(size):
                    raise ChangeError(f"object {cid}: {kind.too_large()}")
                try:
                    data = read_file(path, MAX_FILE_BYTES)
                except (OSError, ValueError) as e:  # too large, or replaced since it was looked up
                    raise ChangeError(f"object {cid}: {e}") from e
            else:
                data = source.get(cid)
                if data is None:
                    continue
            if kind is not None and not kind.fits(len(data)):
                raise ChangeError(f"object {cid}: {kind.too_large()}")
            actual = content_id(data)
            if actual != cid:
                raise ChangeError(f"object {cid}: its bytes hash to {actual}")
            return data
        raise ChangeError(f"object {cid}: not found")

    def doc(self, cid: str, kind: Kind | None = None) -> dict:
        """JSON object `cid`, parsed as canonical JSON. Raises ChangeError as
        `read` does (given its `kind`, for an object over its limit too),
        or when the object is not canonical JSON."""
        # An id that is not a content id (any JSON value, a list included)
        # goes to `read`, which refuses it, not to the cache.
        doc = self._docs.get(cid) if is_content_id(cid) else None
        if doc is None:
            data = self.read(cid, kind)
            try:
                doc = cjson.loads(data)
            except cjson.CanonicalError as e:
                raise ChangeError(f"object {cid}: not canonical JSON: {e}") from e
            self._docs[cid] = doc
            self._sizes[cid] = len(data)
        elif kind is not None and not kind.fits(self._sizes[cid]):
            # Parsed before, as a kind with a larger limit or none.
            raise ChangeError(f"object {cid}: {kind.too_large()}")
        return doc

    def tile_index(self, cid: str) -> dict:
        """Tile index `cid`, checked against the tile-index schema."""
        return self._checked_doc(cid, "streetwide.tile")

    def manifest(self, cid: str) -> dict:
        """Asset manifest `cid`, checked against the asset-manifest schema
        and `manifest_problems`, as op_world reads a manifest."""
        return self._checked_doc(cid, "streetwide.asset")

    def _checked_doc(self, cid: str, fmt: str) -> dict:
        doc = self._checked.get((cid, fmt)) if is_content_id(cid) else None
        if doc is None:
            doc = self.doc(cid, BY_FORMAT[fmt])
            if not isinstance(doc, dict) or doc.get("format") != fmt:
                raise ChangeError(f"object {cid} is not a {fmt} document")
            errors = sorted(schemas.errors(doc))
            if not errors and fmt == "streetwide.asset":
                errors = manifest_problems(doc)
            if errors:
                raise ChangeError(f"object {cid}: {'; '.join(errors)}")
            self._checked[(cid, fmt)] = doc
        return doc


def _regular_file_size(path: Path) -> int | None:
    """The size of the file `path` names if it is a regular file itself, not
    a link to one (op_world's `store::is_regular_file`); None otherwise."""
    try:
        info = path.lstat()
    except (OSError, ValueError):  # missing, or a path the system cannot name
        return None
    return info.st_size if stat.S_ISREG(info.st_mode) else None


# ----------------------------------------------------------------- apply --


@dataclass(frozen=True)
class Candidate:
    """The world a change set produces (see `apply`).

    `world` is the new world manifest, `world_bytes` its canonical bytes and
    `world_id` their content id. `tile_ids` gives the new tile id of each
    edited tile, by tile key in tile order (x, then y). `objects` holds the
    new tile indexes by content id, sorted by id: with the set's manifests
    and meshes, they are the objects a merge adds to the store. Do not
    modify `world`.
    """

    world: dict
    world_bytes: bytes
    world_id: str
    tile_ids: dict[str, str]
    objects: dict[str, bytes]

    def write(self, store: Store) -> str:
        """Writes the new tile indexes into `store`, then its `world.json`,
        and returns the world id. Copy the set's manifests and meshes in
        first, as a merge does."""
        for cid, data in self.objects.items():
            if store.write_object(data, "json") != cid:
                raise ChangeError(f"tile index {cid} does not hash to its id")
        written = store.write_world(self.world)
        if written != self.world_id:
            raise ChangeError(f"the written world {written} is not the candidate {self.world_id}")
        return written


def apply(world: dict, change: Mapping[str, Any], objects: Objects | Source) -> Candidate:
    """The world that `change` produces on `world`, built byte for byte as
    the gate's `build_candidate` builds its candidate world.

    - `world` is the base world manifest, as `Store.read_world` returns it.
      It is not modified.
    - Only `change["changes"]` is read, in its listed order. `apply` does
      not check the set's own rules: call `problems` first, as the gate
      parses a change set before it builds anything. So it can also apply
      the changes of several sets at once. Nor does it check lineage (G2):
      an add over an existing key replaces that asset, as in the gate.
    - `objects` must hold the base world's tile indexes and the manifest of
      every asset in the result: typically the set's new manifests over
      the canonical store. A single source (a `Store`, a directory or a
      mapping) is accepted too.

    Steps, as in the gate: for each change, its tile's index loses the
    asset `(layer, feature_id)` and, unless the change removes it, lists
    the new manifest's asset (`layer`, `feature_id`, `content_id`,
    `license`, `collider` and `bounds_mm` copied from the manifest). Each
    edited index is sorted by (layer, feature_id) in byte order, and its
    tile in the world manifest gets the new tile id and the union of its
    asset bounds. The world's attribution becomes the sorted union of
    every asset's credits. Name, region, spawn and tile origins stay.

    Raises ChangeError, with op_world's message where it has one:
    "tile <key> is not in the world", "<layer> <feature_id>: no readable
    manifest", "tile <key> would be empty", "tile index <key>: assets must
    be sorted by (layer, feature_id) without duplicates", "tile index
    <key>: larger than 16777216 bytes (16 MiB), the limit for a tile
    index". Unlike the gate, `apply` never reads meshes, so it does not
    check that a manifest agrees with its mesh.

    `world` and `change` may be any parsed JSON values: a world that is
    not a valid world manifest in canonical JSON, or a change that is not
    an object naming a tile, a layer, a feature id and an op, is a
    ChangeError too.
    """
    if not isinstance(objects, Objects):
        objects = Objects(objects)
    _world_bytes(world, "the base world")
    base = copy.deepcopy(world)
    position = {wt["tile"]: i for i, wt in enumerate(base["tiles"])}
    edited: dict[str, dict] = {}
    entries = change.get("changes") if isinstance(change, Mapping) else None
    if not isinstance(entries, list):
        raise ChangeError("a change set lists its changes in `changes`")
    # `changes` is the second level of a change set. Deeper, op_world could
    # not parse it, and quoting a change in a message would recurse too far.
    if cjson.deeper_than(entries, cjson.MAX_DEPTH - 1):
        raise ChangeError(f"the change set: {_TOO_DEEP}")
    for c in entries:
        _check_entry(c)
        key = c["tile"]
        if key not in position:
            raise ChangeError(f"tile {key} is not in the world")
        index = edited.get(key)
        if index is None:
            index = copy.deepcopy(objects.tile_index(base["tiles"][position[key]]["tile_id"]))
            edited[key] = index
        index["assets"] = [
            a for a in index["assets"] if not (a["layer"] == c["layer"] and a["feature_id"] == c["feature_id"])
        ]
        if c["op"] == "remove":
            continue
        try:
            m = objects.manifest(c["manifest_id"])
        except ChangeError as e:
            raise ChangeError(f"{c['layer']} {c['feature_id']}: no readable manifest ({e})") from e
        index["assets"].append({
            "layer": m["layer"],
            "feature_id": m["feature_id"],
            "manifest_id": c["manifest_id"],
            "content_id": m["content_id"],
            "license": m["provenance"]["license"],
            "collider": m["physics"]["collider"],
            "bounds_mm": copy.deepcopy(m["bounds_mm"]),
        })

    tile_ids: dict[str, str] = {}
    new_objects: dict[str, bytes] = {}
    for key in sorted(edited, key=tile_xy):
        index = edited[key]
        index["assets"].sort(key=lambda a: (a["layer"].encode("utf-8"), a["feature_id"].encode("utf-8")))
        if not index["assets"]:
            raise ChangeError(f"tile {key} would be empty")
        order = [(a["layer"].encode("utf-8"), a["feature_id"].encode("utf-8")) for a in index["assets"]]
        if not is_sorted_unique(order):
            raise ChangeError(
                f"tile index {key}: assets must be sorted by (layer, feature_id) without duplicates"
            )
        data = cjson.dumps(index)
        if not TILE_INDEX.fits(len(data)):
            raise ChangeError(f"tile index {key}: {TILE_INDEX.too_large()}")
        tile_id = content_id(data)
        wt = base["tiles"][position[key]]
        wt["tile_id"] = tile_id
        wt["bounds_mm"] = union_bounds(a["bounds_mm"] for a in index["assets"])
        tile_ids[key] = tile_id
        new_objects[tile_id] = data

    credits: set[str] = set()
    for wt in base["tiles"]:
        index = edited.get(wt["tile"]) or objects.tile_index(wt["tile_id"])
        for a in index["assets"]:
            credits.update(objects.manifest(a["manifest_id"])["provenance"]["attribution"])
    base["attribution"] = sorted(credits, key=lambda s: s.encode("utf-8"))
    world_bytes = _world_bytes(base, "the candidate world")
    return Candidate(
        world=base,
        world_bytes=world_bytes,
        world_id=content_id(world_bytes),
        tile_ids=tile_ids,
        objects=dict(sorted(new_objects.items())),
    )


def union_bounds(bounds: Iterable[Mapping[str, Sequence[int]]]) -> dict:
    """The union of `bounds_mm` boxes (`BoundsMm::union`), as a new
    `{"max": [...], "min": [...]}`. Raises ValueError for no box."""
    boxes = list(bounds)
    if not boxes:
        raise ValueError("the union of no bounds is undefined")
    return {
        "max": [max(b["max"][k] for b in boxes) for k in range(3)],
        "min": [min(b["min"][k] for b in boxes) for k in range(3)],
    }


def _world_bytes(world: Any, what: str) -> bytes:
    """The canonical bytes of `world`, once it is checked as a world
    manifest: its format (`schemas.errors` would check another document
    against its own schema), its schema and its tile keys. Raises
    ChangeError naming `what`."""
    if not isinstance(world, dict):
        errors = ["not a JSON object"]
    elif world.get("format") != "streetwide.world":
        errors = ["not a streetwide.world document"]
    else:
        errors = schemas.errors(world)
    for wt in world["tiles"] if not errors else []:
        try:
            tile_xy(wt["tile"])
        except ValueError as e:
            errors.append(str(e))
    if errors:
        raise ChangeError(f"{what}: {'; '.join(sorted(errors))}")
    try:
        data = cjson.dumps(world)
    except cjson.CanonicalError as e:  # a value only Python holds, such as a float
        raise ChangeError(f"{what}: not canonical JSON: {e}") from e
    if not WORLD_MANIFEST.fits(len(data)):
        raise ChangeError(f"{what}: {WORLD_MANIFEST.too_large()}")
    return data


def _check_entry(c: Any) -> None:
    # What `apply` reads from a change; `problems` checks the rest.
    if not (
        isinstance(c, Mapping)
        and all(isinstance(c.get(k), str) for k in ("op", "tile", "layer", "feature_id"))
        and c["op"] in OPS
    ):
        raise ChangeError(f"not a change: {c!r}")
    if c["op"] != "remove" and not isinstance(c.get("manifest_id"), str):
        raise ChangeError(f"{c['tile']} {c['layer']} {c['feature_id']}: {_OP_RULE}")
