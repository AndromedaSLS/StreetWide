"""The water layer (RFC 0003) from Python. The Python tools check every
document against the JSON Schemas (`schemas.errors`), so the rules live
there and must agree with `op_world`: `water` is a layer, and a water asset
must have `collider: none`, in manifests and in tile indexes."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from tools.worldfmt import schemas

FIXTURES = Path(__file__).resolve().parents[3] / "fixtures"

# `op_world::Layer::ALL`: in byte order of the names, the order tile indexes
# list assets in.
RUST_LAYERS = ["facade", "furniture", "interactivity", "interior", "shell", "terrain", "transport", "water"]


def _fixture(fmt: str) -> dict:
    """A fresh copy of the first document of this format in the format fixture."""
    for path in sorted((FIXTURES / "format" / "world").rglob("*.json")):
        doc = json.loads(path.read_bytes())
        if doc["format"] == fmt:
            return doc
    raise AssertionError(f"no {fmt} document in the format fixture")


def _manifest(layer: str, collider: str) -> dict:
    m = _fixture("streetwide.asset")
    m["layer"] = layer
    m["physics"]["collider"] = collider
    return m


def _tile(layer: str, collider: str) -> dict:
    t = _fixture("streetwide.tile")
    # The last entry, so that the entries stay sorted.
    t["assets"][-1]["layer"] = layer
    t["assets"][-1]["collider"] = collider
    return t


BUILDERS = pytest.mark.parametrize("build", [_manifest, _tile], ids=["manifest", "tile-index"])


def test_the_layers_are_op_worlds() -> None:
    common = json.loads((schemas.SCHEMA_DIR / "common.schema.json").read_text(encoding="utf-8"))
    assert sorted(common["$defs"]["layer"]["enum"], key=str.encode) == RUST_LAYERS


@BUILDERS
def test_water_assets_never_collide(build) -> None:
    assert schemas.errors(build("water", "none")) == []
    assert schemas.errors(build("water", "trimesh")) != []


@BUILDERS
@pytest.mark.parametrize("layer", [name for name in RUST_LAYERS if name != "water"])
def test_other_layers_may_collide(build, layer: str) -> None:
    assert schemas.errors(build(layer, "trimesh")) == []


@BUILDERS
def test_unknown_layers_are_rejected(build) -> None:
    assert schemas.errors(build("river", "none")) != []


def test_change_sets_may_touch_water() -> None:
    # Well formed: a baseline refresh makes such changes, and the gate (G3)
    # refuses contributed ones.
    change = json.loads((FIXTURES / "changes" / "facade-sample" / "change.json").read_bytes())
    entry = change["changes"][0]
    entry["layer"] = "water"
    entry["feature_id"] = f"tile:{entry['tile']}"
    assert schemas.errors(change) == []
    entry["layer"] = "river"
    assert schemas.errors(change) != []
