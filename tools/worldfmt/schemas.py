"""Validate world-format documents against the JSON Schemas in `schemas/`."""

from __future__ import annotations

import json
from functools import cache
from pathlib import Path

from jsonschema import Draft202012Validator
from referencing import Registry, Resource

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
    return Draft202012Validator(schema, registry=_registry())


def errors(document: dict) -> list[str]:
    """Schema errors for a manifest, tile index or world manifest."""
    schema_file = _BY_FORMAT.get(document.get("format"))
    if schema_file is None:
        return [f"unknown format {document.get('format')!r}"]
    return [e.message for e in validator(schema_file).iter_errors(document)]
