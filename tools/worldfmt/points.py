"""Points documents (RFC 0004) in Python, byte for byte as
`op_world::points` reads and writes them: the content of assets in the
`vegetation` and `street_objects` layers.

A points document is canonical JSON (`cjson`) with `format:
"streetwide.points"`: 1 to 65,536 points of one source, sorted by id, each
with its kind, an anchor in integer millimetres relative to the tile
origin, every attribute of its kind (`None`, written `null`, when unknown)
and the attributes a rule set (`estimated`), and `rules`, the rule behind
each estimated attribute.

- `problems(doc)` lists every rule a document breaks: the JSON Schema
  (`schemas/points.schema.json`) first, as "schema: <JSON path>:
  <message>"; for a document that passes it, the rules of
  `PointsDocument::problems` (crates/op_world/src/points.rs), message for
  message. `parse` and `to_bytes` read and write documents with these
  checks and the size limit of points documents (`limits.POINTS`).
- `asset_problems(layer, feature_id, tile, bounds_mm, doc)`: the rules a
  valid document and its asset's manifest must agree on
  (`Summary::asset_problems`): kinds in the layer, the feature id's
  source, `bounds_mm` equal to the anchors' bounds.
- `point_source`, `is_source`, `per_tile_source` and `is_rule_id` mirror
  crates/op_world/src/rules.rs.

Every reader takes any input: `parse` any bytes, `problems` any parsed
JSON value. Each returns a value or raises PointsError (`problems` only
returns), with time and memory in proportion to its input.
"""

from __future__ import annotations

import re
from collections.abc import Mapping
from typing import Any

from tools.worldfmt import cjson
from tools.worldfmt.limits import POINTS
from tools.worldfmt.rust import rust_debug

FORMAT = "streetwide.points"
CONTRACT_VERSION = "0.1.0"
SCHEMA_FILE = "points.schema.json"
MAX_POINTS = 65_536
"""Most points in one document (RFC 0004 section 3)."""
MAX_POINT_ID_CHARS = 256
MAX_RULE_ID_CHARS = 128
MAX_GENUS_CHARS = 64
MAX_SPECIES_CHARS = 128
MAX_TAXON_CHARS = 256
MAX_TEXT_CHARS = 4096

KIND_LAYERS = {"light": "street_objects", "tree": "vegetation"}
"""The layer each kind of point belongs to (`PointKind::layer`)."""
POINT_LAYERS = ("street_objects", "vegetation")
"""The layers whose assets are points documents (`Layer::has_points`)."""
ATTRIBUTES = {
    "light": ("count", "light_height_mm", "method", "mount", "purpose", "yaw"),
    "tree": ("crown_diameter_mm", "genus", "height_mm", "leaf_cycle", "leaf_type", "planted_year", "species",
             "taxon", "trunk_circumference_mm"),
}
"""Each kind's attributes, in byte order (`PointKind::attributes`)."""
ESTIMABLE = {"light": ("yaw",), "tree": ()}
"""The attributes a rule may set in format 0.1 (`PointKind::estimable`):
only a light's yaw (RFC 0004 section 4 and its Decision)."""
RANGES = {
    "crown_diameter_mm": (1, 100_000),
    "height_mm": (1, 150_000),
    "planted_year": (1000, 2999),
    "trunk_circumference_mm": (1, 50_000),
    "count": (1, 64),
    "light_height_mm": (1, 100_000),
    "yaw": (0, 65_535),
}
"""The integer attributes and their ranges, as `Attributes::problems`
checks them, in its order within each kind."""

_SOURCE = re.compile(r"[a-z][a-z0-9_-]*(?::[a-z0-9][a-z0-9_-]*)*")
_LOCAL = re.compile(r"[!-9;-~]+")
_PER_TILE = re.compile(r"tile:17/(0|[1-9][0-9]{0,5})/(0|[1-9][0-9]{0,5})/(.*)", re.DOTALL)
_RULE = re.compile(r"adr-[0-9]{4}:[a-z0-9]+(?:-[a-z0-9]+)*:[1-9][0-9]*")
_GENUS = re.compile(r"[A-Z][a-z]+")
_CONTROL = re.compile(r"[\x00-\x1f\x7f]")
_FEATURE_ID = re.compile(r"[a-z][a-z0-9_-]*:[!-~]+")


class PointsError(ValueError):
    """A points document breaks a rule; the message says which."""


# ------------------------------------------------------------ field rules --


def is_source(s: object) -> bool:
    """`rules::is_source`: `^[a-z][a-z0-9_-]*(:[a-z0-9][a-z0-9_-]*)*$`."""
    return isinstance(s, str) and _SOURCE.fullmatch(s) is not None


def point_source(point_id: object) -> str | None:
    """`rules::point_source`: the source of a point id (the id without its
    last part), or None if it is not a point id
    (`^[a-z][a-z0-9_-]*(:[a-z0-9][a-z0-9_-]*)*:[!-9;-~]+$`, at most 256
    characters)."""
    if not isinstance(point_id, str) or len(point_id) > MAX_POINT_ID_CHARS or ":" not in point_id:
        return None
    source, local = point_id.rsplit(":", 1)
    return source if _LOCAL.fullmatch(local) and is_source(source) else None


def per_tile_source(feature_id: object, tile: str) -> str | None:
    """`rules::per_tile_source`: the source of the per-tile asset id
    `tile:17/{x}/{y}/{source}` of tile `tile` ("17/x/y"), or None."""
    if not isinstance(feature_id, str) or len(feature_id) > 256 or not _FEATURE_ID.fullmatch(feature_id):
        return None
    prefix = f"tile:{tile}/"
    if not feature_id.startswith(prefix):
        return None
    source = feature_id[len(prefix):]
    return source if is_source(source) else None


def is_rule_id(s: object) -> bool:
    """`rules::is_rule_id`: `adr-<4 digits>:<name>:<version>`, at most 128
    characters, such as `adr-0015:yaw-from-orientierung:1`."""
    return isinstance(s, str) and len(s) <= MAX_RULE_ID_CHARS and _RULE.fullmatch(s) is not None


def _is_text(s: str) -> bool:
    return 0 < len(s) <= MAX_TEXT_CHARS and _CONTROL.search(s) is None


def _is_genus(s: str) -> bool:
    return len(s) <= MAX_GENUS_CHARS and _GENUS.fullmatch(s) is not None


def _is_species(s: str) -> bool:
    """A Latin binomial, at most 128 characters: a genus, then one or more
    words, each after a single space, without control characters."""
    words = s.split(" ")
    return (
        len(s) <= MAX_SPECIES_CHARS
        and _GENUS.fullmatch(words[0]) is not None
        and len(words) > 1
        and all(w and _CONTROL.search(w) is None for w in words[1:])
    )


def _utf8(s: str) -> bytes:
    # As Rust compares strings: by their UTF-8 bytes. A lone surrogate,
    # which only a document canonical JSON refuses holds, sorts by its code.
    return s.encode("utf-8", "surrogatepass")


# ---------------------------------------------------------------- checks --


def problems(doc: Any) -> list[str]:
    """Every rule `doc` (any parsed JSON value) breaks; [] when op_world
    accepts it as a points document. The schema's errors come first, as
    "schema: <path>: <message>"; a document that passes the schema gets
    `PointsDocument::problems`'s messages instead. One nested deeper than
    canonical JSON allows gets that one message."""
    if cjson.deeper_than(doc, cjson.MAX_DEPTH):
        return [f"arrays and objects nested deeper than {cjson.MAX_DEPTH} levels"]
    from tools.worldfmt.changes import schema_problems  # changes imports this module

    found = schema_problems(SCHEMA_FILE, doc)
    if found:
        return found
    return rule_problems(doc)


def rule_problems(doc: Mapping[str, Any]) -> list[str]:
    """`PointsDocument::problems`, in its order and words, for a document
    that passes the schema. Python's `re` lets the schema's patterns accept
    a final newline, so the field rules run again here."""
    found = []
    if doc["format"] != FORMAT:
        found.append(f"format must be {rust_debug(FORMAT)}")
    if doc["contract_version"] != CONTRACT_VERSION:
        found.append(f"unsupported contract_version {rust_debug(doc['contract_version'])}")
    points = doc["points"]
    if not 1 <= len(points) <= MAX_POINTS:
        found.append(f"a points document has 1..={MAX_POINTS} points")
    ids = [_utf8(p["id"]) for p in points]
    if not all(a < b for a, b in zip(ids, ids[1:])):
        found.append("points must be sorted by id without duplicates")
    sources = set()
    for p in points:
        source = point_source(p["id"])
        if source is not None:
            sources.add(source)
        for problem in _point_problems(p):
            found.append(f"point {rust_debug(p['id'])}: {problem}")
    if len(sources) > 1:
        listed = ", ".join(rust_debug(s) for s in sorted(sources, key=_utf8))
        found.append(f"every point must have the same source (its id without the last part), not {listed}")
    found += _rules_problems(doc)
    return found


def _attributes_kind(attributes: Mapping[str, Any]) -> str | None:
    """The kind whose keys `attributes` has exactly, as op_world's untagged
    reader decides it."""
    keys = set(attributes)
    return next((kind for kind, names in ATTRIBUTES.items() if keys == set(names)), None)


def _point_problems(p: Mapping[str, Any]) -> list[str]:
    """`Point::problems`."""
    found = []
    kind, attributes = p["kind"], p["attributes"]
    if point_source(p["id"]) is None:
        found.append(f"id must be <source>:<local id>, at most {MAX_POINT_ID_CHARS} characters (RFC 0004 section 2)")
    if _attributes_kind(attributes) != kind:
        found.append(f"attributes must be those of a {kind}: {', '.join(ATTRIBUTES[kind])}")
    else:
        found += _attribute_problems(kind, attributes)
    estimated = p["estimated"]
    encoded = [_utf8(e) for e in estimated]
    if not all(a < b for a, b in zip(encoded, encoded[1:])):
        found.append("estimated must be sorted and unique")
    for name in estimated:
        if name not in ESTIMABLE[kind]:
            found.append(f"{rust_debug(name)} may not be estimated for a {kind}")
        elif attributes.get(name) is None:
            found.append(f"estimated {rust_debug(name)} is null")
    return found


def _attribute_problems(kind: str, a: Mapping[str, Any]) -> list[str]:
    """`Attributes::problems` for attributes of `kind`'s keys: the ranges
    (in byte order of the names, as op_world checks them), then a tree's
    texts."""
    found = []
    for name in ATTRIBUTES[kind]:
        if name in RANGES and a[name] is not None:
            lo, hi = RANGES[name]
            if not lo <= a[name] <= hi:
                found.append(f"{name} must be {lo}..={hi}")
    if kind == "tree":
        if a["genus"] is not None and not _is_genus(a["genus"]):
            found.append("genus must match ^[A-Z][a-z]+$ (at most 64 characters)")
        if a["species"] is not None and not _is_species(a["species"]):
            found.append("species must be a Latin binomial of at most 128 characters")
        taxon = a["taxon"]
        if taxon is not None and (not _is_text(taxon) or len(taxon) > MAX_TAXON_CHARS):
            found.append("taxon must be text of at most 256 characters")
    return found


def _rules_problems(doc: Mapping[str, Any]) -> list[str]:
    """`PointsDocument::rule_problems`."""
    found = []
    rules = doc["rules"]
    used = set()
    for p in doc["points"]:
        kind = p["kind"]
        for name in p["estimated"]:
            used.add((kind, name))
            ruled = name in rules.get(kind, {})
            if not ruled and name in ESTIMABLE[kind]:
                found.append(f"point {rust_debug(p['id'])}: estimated {rust_debug(name)} has no rule in rules.{kind}")
    for kind in sorted(rules, key=_utf8):
        named = rules[kind]
        if kind not in ATTRIBUTES:
            found.append(f"rules: {rust_debug(kind)} is not a kind of point")
            continue
        if not named:
            found.append(f"rules.{kind} must name at least one estimated attribute")
        for name in sorted(named, key=_utf8):
            rule = named[name]
            if name not in ESTIMABLE[kind]:
                found.append(f"rules.{kind}: {rust_debug(name)} may not be estimated for a {kind}")
                continue
            if not is_rule_id(rule):
                found.append(f"rules.{kind}.{name}: {rust_debug(rule)} is not a rule id (adr-<4 digits>:<name>:<version>)")
            if (kind, name) not in used:
                found.append(f"rules.{kind}.{name}: no {kind} point has an estimated {name}")
    return found


# ------------------------------------------------------- reading, writing --


def parse(data: bytes) -> dict:
    """Reads the bytes of a points document as `PointsDocument::from_bytes`
    does: at most the limit for points documents (`limits.POINTS`),
    checked before parsing, then canonical JSON, then `problems`. Raises
    PointsError."""
    if not POINTS.fits(len(data)):
        raise PointsError(POINTS.too_large())
    try:
        doc = cjson.loads(data)
    except cjson.CanonicalError as e:
        raise PointsError(f"not canonical JSON: {e}") from e
    found = problems(doc)
    if found:
        raise PointsError("; ".join(found))
    return doc


def to_bytes(doc: Mapping[str, Any]) -> bytes:
    """The canonical bytes of a points document, after `problems` and
    within the limit for points documents (`PointsDocument::to_bytes`).
    The document's content id is `ids.content_id` of these bytes. Raises
    PointsError."""
    found = problems(doc)
    if found:
        raise PointsError("; ".join(found))
    try:
        data = cjson.dumps(doc)
    except cjson.CanonicalError as e:  # a value only Python holds, such as a float
        raise PointsError(f"not canonical JSON: {e}") from e
    if not POINTS.fits(len(data)):
        raise PointsError(f"points document: {POINTS.too_large()}")
    return data


def source(doc: Mapping[str, Any]) -> str | None:
    """The source of every point, if they share one
    (`PointsDocument::source`)."""
    found = {point_source(p["id"]) for p in doc["points"]}
    return next(iter(found)) if len(found) == 1 and None not in found else None


def bounds(doc: Mapping[str, Any]) -> dict | None:
    """The exact bounds of the anchors, `{"max": [...], "min": [...]}`, as an
    asset's `bounds_mm` must be (`PointsDocument::bounds`); None without
    points."""
    anchors = [p["anchor_mm"] for p in doc["points"]]
    if not anchors:
        return None
    return {"max": [max(a[k] for a in anchors) for k in range(3)],
            "min": [min(a[k] for a in anchors) for k in range(3)]}


def asset_problems(layer: str, feature_id: str, tile: str, bounds_mm: Mapping[str, Any],
                   doc: Mapping[str, Any]) -> list[str]:
    """`Summary::asset_problems`: what a valid points document and the
    manifest of its asset (its `layer`, `feature_id`, `tile` and
    `bounds_mm`) must agree on, in op_world's words; [] when they agree."""
    if layer not in POINT_LAYERS:
        return [f"{layer} assets are meshes, not points documents"]
    found = []
    for kind in sorted({p["kind"] for p in doc["points"]}):
        if KIND_LAYERS[kind] != layer:
            found.append(f"points of kind {kind} belong in the {KIND_LAYERS[kind]} layer, not {layer}")
    wanted = per_tile_source(feature_id, tile)
    if wanted is not None and source(doc) != wanted:
        found.append(f"the points' source must be {rust_debug(wanted)}, the source of feature id "
                     f"{rust_debug(feature_id)}")
    if bounds(doc) != {"max": list(bounds_mm["max"]), "min": list(bounds_mm["min"])}:
        found.append("bounds_mm are not the exact bounds of the anchors")
    return found
