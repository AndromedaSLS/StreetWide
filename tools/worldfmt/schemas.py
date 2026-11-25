"""Validate world-format documents against the JSON Schemas in `schemas/`."""

from __future__ import annotations

import json
from functools import cache
from pathlib import Path

from jsonschema import Draft202012Validator, ValidationError, validators
from referencing import Registry, Resource

from tools.worldfmt import cjson

SCHEMA_DIR = Path(__file__).resolve().parents[2] / "schemas"

_BY_FORMAT = {
    "streetwide.asset": "asset-manifest.schema.json",
    "streetwide.tile": "tile-index.schema.json",
    "streetwide.world": "world-manifest.schema.json",
    "streetwide.replay": "replay-header.schema.json",
    "streetwide.change": "change-set.schema.json",
    "streetwide.check": "check-report.schema.json",
    "streetwide.merge": "merge-record.schema.json",
}


def _json_key(v: object) -> tuple:
    """A hashable stand-in for a JSON value, equal exactly when jsonschema
    calls the values equal (true and 1 differ, 1 and 1.0 do not)."""
    if v is True or v is False:
        return ("bool", v)
    if v is None:
        return ("null",)
    if isinstance(v, str):
        return ("str", v)
    if isinstance(v, (int, float)):
        return ("num", v)
    if isinstance(v, list):
        return ("list", tuple(_json_key(x) for x in v))
    if isinstance(v, dict):
        return ("obj", frozenset((k, _json_key(x)) for k, x in v.items()))
    raise TypeError(f"not a JSON value: {type(v).__name__}")


_UNIQUE_ITEMS = Draft202012Validator.VALIDATORS["uniqueItems"]


def _unique_items(validator, unique, instance, schema):
    """jsonschema's `uniqueItems`, in linear time for every input.

    jsonschema compares every pair of items when they cannot be sorted
    (mixed types, objects), so a crafted array of a few ten thousand items
    takes minutes. The verdict and the message are the same as jsonschema's.
    """
    if not unique or not validator.is_type(instance, "array"):
        return
    try:
        keys = {_json_key(item) for item in instance}
    except (TypeError, RecursionError):  # not plain JSON: jsonschema's own rule
        yield from _UNIQUE_ITEMS(validator, unique, instance, schema)
        return
    if len(keys) != len(instance):
        yield ValidationError(f"{instance!r} has non-unique elements")


_Validator = validators.extend(Draft202012Validator, {"uniqueItems": _unique_items})


@cache
def _registry() -> Registry:
    resources = []
    for path in sorted(SCHEMA_DIR.glob("*.schema.json")):
        schema = json.loads(path.read_text(encoding="utf-8"))
        resources.append((schema["$id"], Resource.from_contents(schema)))
    return Registry().with_resources(resources)


@cache
def validator(schema_file: str) -> Draft202012Validator:
    schema = json.loads((SCHEMA_DIR / schema_file).read_text(encoding="utf-8"))
    Draft202012Validator.check_schema(schema)
    return _Validator(schema, registry=_registry())


def errors(document: object) -> list[str]:
    """Schema errors for a world-format or contribution document.

    `document` may be any parsed JSON value (it often comes from an
    untrusted file); anything that is not a known document is an error.
    """
    if not isinstance(document, dict):
        return [f"expected a JSON object, not {type(document).__name__}"]
    # No document is deeper than canonical JSON allows; jsonschema recurses.
    if cjson.deeper_than(document, cjson.MAX_DEPTH):
        return [f"arrays and objects nested deeper than {cjson.MAX_DEPTH} levels"]
    fmt = document.get("format")
    if not isinstance(fmt, str):
        return [f"unknown format {fmt!r}" if fmt is None else f"format is a {type(fmt).__name__}, not text"]
    schema_file = _BY_FORMAT.get(fmt)
    if schema_file is None:
        return [f"unknown format {fmt!r}"]
    return [e.message for e in validator(schema_file).iter_errors(document)]
