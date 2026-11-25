"""The licence notices beside a world (ADR-0019, `tools.ingest.notices`):
the ingest writes them with every world, they credit every source it
uses, and no reader of a store counts them as part of the world."""

from __future__ import annotations

import re
import shutil
from pathlib import Path

import numpy as np
import pytest

from tools.contrib.facade import make_bundle
from tools.ingest import notices
from tools.ingest.assemble import (
    OSM_CREDIT, OSM_LICENSE, OSM_ON_TERRAIN_LICENSE, SWISSTOPO_CREDIT, SWISSTOPO_LICENSE, Asset, write_world,
)
from tools.ingest.geo import Frame
from tools.ingest.mesh import MeshBuilder
from tools.ingest.pipeline import _replace_dir
from tools.worldfmt import cjson
from tools.worldfmt.ids import content_id
from tools.worldfmt.store import NOTICE_FILES, Store

REPO = Path(__file__).resolve().parents[3]
FIXTURE_WORLD = REPO / "fixtures" / "world"
SAMPLE = REPO / "fixtures" / "changes" / "facade-sample"
FRAME = Frame((2145, 1434))
TILE = (68646, 45903)


def _source(dataset: str, license: str, credit: str) -> dict:
    return {"dataset": dataset, "version": "1", "license": license, "attribution": credit,
            "url": "https://example.org/", "source_ids": ["a"]}


def _world(root: Path, sources: list[dict]) -> str:
    """Writes a one-asset world with the given sources through the ingest's
    writer; returns its id."""
    origin = FRAME.tile_origin_mm(*TILE)
    mesh = MeshBuilder(origin)
    corner = np.array(origin, dtype=np.float64) / 1000.0
    points = corner + np.array([[0.0, 400.0, 0.0], [10.0, 400.0, 0.0], [0.0, 400.0, 10.0]])
    mesh.add("terrain", points, np.array([[0.0, 1.0, 0.0]] * 3), np.array([[0, 2, 1]]))
    license = " AND ".join(sorted({s["license"] for s in sources}))
    asset = Asset(TILE, "terrain", "tile:17/68646/45903", mesh, license, sources, "trimesh")
    world_id, _ = write_world(Store(root), FRAME, "notice-test", [asset], {TILE: origin}, [0, 0, 0], 0)
    return world_id


def _with_notices(store: Path, to: Path) -> Path:
    """A copy of the store in `store`, with the notices beside its world."""
    shutil.copytree(store, to)
    notices.write(to)
    return to


def test_the_ingest_writes_the_notices_beside_the_world(tmp_path: Path) -> None:
    root = tmp_path / "world"
    world_id = _world(root, [_source("swissALTI3D", SWISSTOPO_LICENSE, SWISSTOPO_CREDIT)])
    assert sorted(p.name for p in root.iterdir()) == sorted(["world.json", "objects", *NOTICE_FILES])
    assert {name: (root / name).read_bytes() for name in NOTICE_FILES} == notices.notices()
    # The licence texts are the repository's own.
    for name, licence in [("ODbL-1.0.txt", "ODbL-1.0.txt"), ("CC-BY-SA-4.0.txt", "CC-BY-SA-4.0.txt")]:
        assert (root / name).read_bytes() == (REPO / "LICENSES" / licence).read_bytes()
    # Not part of the world: the world id is that of world.json, with the
    # notices or without them, and no object is a notice.
    assert world_id == content_id((root / "world.json").read_bytes()) == Store(root).read_world()[0]
    for name in NOTICE_FILES:
        (root / name).unlink()
    assert Store(root).read_world()[0] == world_id
    objects = [p.read_bytes() for p in (root / "objects").rglob("*") if p.is_file()]
    assert not set(objects) & set(notices.notices().values())


def test_the_notices_name_the_odbl_and_every_source() -> None:
    notice, credits = notices.NOTICE_TEXT, notices.CREDITS_TEXT
    # The database notice (ODbL 4.2): the licence, its address and its text.
    assert ("It is made available under the Open Database License (ODbL) 1.0:\n"
            "https://opendatacommons.org/licenses/odbl/1-0/\n"
            "The text of the licence is in ODbL-1.0.txt, beside this file.\n") in notice
    assert "© OpenStreetMap contributors, https://www.openstreetmap.org/copyright" in notice
    # The notice for pictures and videos (ODbL 4.3, its example wording).
    assert ("    Contains information from the StreetWide world, which is made\n"
            "    available here under the Open Database License (ODbL).\n") in notice
    # Each source, its licence and a link.
    for s in notices.SOURCES:
        block = notices._source_text(s)
        assert block in credits
        assert s.licence_url in block and s.url in block and s.credit in block and f"({s.license})" in block
    for text in (notice, credits):
        # LF line ends, and no em dash (U+2014).
        assert "\r" not in text and chr(0x2014) not in text and text.endswith("\n")


def test_the_credits_follow_attribution_md() -> None:
    """ATTRIBUTION.md is where the project's credits are kept: the notices
    must credit what it credits, with the same links."""
    attribution = (REPO / "ATTRIBUTION.md").read_text(encoding="utf-8")
    for s in notices.SOURCES:
        for text in (s.dataset, s.credit, s.license, s.licence_url, s.url):
            assert text in attribution, (s.dataset, text)


def test_the_ingests_sources_are_credited() -> None:
    """Every source the ingest writes into a manifest, and the contribution
    tool's, are in the credits, with the same licence and credit."""
    notices.check_credited([
        _source("swissBUILDINGS3D 3.0 Beta", SWISSTOPO_LICENSE, SWISSTOPO_CREDIT),
        _source("swissALTI3D", SWISSTOPO_LICENSE, SWISSTOPO_CREDIT),
        _source("swissSURFACE3D Raster", SWISSTOPO_LICENSE, SWISSTOPO_CREDIT),
        _source("Baumkataster Stadt Zürich", "CC0-1.0", "Quelle: Stadt Zürich"),
        _source("Öffentliche Beleuchtung der Stadt Zürich", "CC0-1.0", "Quelle: Stadt Zürich"),
        _source("OpenStreetMap", OSM_LICENSE, OSM_CREDIT),
        _source("StreetWide contributions", "CC-BY-SA-4.0", "The StreetWide contributors"),
    ])
    store = Store(FIXTURE_WORLD)
    _, world = store.read_world()
    credits = set()
    for tile in world["tiles"]:
        for entry in cjson.loads(store.read_object(tile["tile_id"], "json"))["assets"]:
            manifest = cjson.loads(store.read_object(entry["manifest_id"], "json"))
            notices.check_credited(manifest["provenance"]["sources"])
            credits.update(manifest["provenance"]["attribution"])
    assert credits == set(world["attribution"]) <= {s.credit for s in notices.SOURCES}


@pytest.mark.parametrize("source,why", [
    (_source("Somewhere's open data", "CC0-1.0", "Somewhere"), "is not in the credits"),
    (_source("OpenStreetMap", OSM_ON_TERRAIN_LICENSE, OSM_CREDIT), "but the credits say"),
    (_source("swissALTI3D", SWISSTOPO_LICENSE, "swisstopo"), "but the credits say"),
])
def test_a_source_the_notices_do_not_credit_stops_the_ingest(tmp_path: Path, source: dict, why: str) -> None:
    with pytest.raises(notices.NoticeError, match=why):
        _world(tmp_path / "world", [source])
    assert not (tmp_path / "world").exists(), "nothing is written"


def test_the_ingest_replaces_its_own_store_but_not_a_canonical_one(tmp_path: Path) -> None:
    out = tmp_path / "out"
    _world(out, [_source("swissALTI3D", SWISSTOPO_LICENSE, SWISSTOPO_CREDIT)])
    # A store the ingest wrote, notices and all, is replaced by the next build.
    new = tmp_path / "out.tmp"
    new_id = _world(new, [_source("OpenStreetMap", OSM_LICENSE, OSM_CREDIT)])
    _replace_dir(new, out)
    assert Store(out).read_world()[0] == new_id and (out / "NOTICE.txt").is_file()
    # A canonical store carries a marker, and is never replaced.
    (out / "CANONICAL").write_bytes(b"")
    again = tmp_path / "again.tmp"
    _world(again, [_source("swissALTI3D", SWISSTOPO_LICENSE, SWISSTOPO_CREDIT)])
    with pytest.raises(RuntimeError, match="is not a world store"):
        _replace_dir(again, out)
    assert Store(out).read_world()[0] == new_id


def test_readers_of_a_store_ignore_the_notices(tmp_path: Path) -> None:
    """The store reader and the contribution tool see the same world, and
    make the same bundle, with the notices beside it or without them."""
    world = _with_notices(FIXTURE_WORLD, tmp_path / "world")
    assert Store(world).read_world() == Store(FIXTURE_WORLD).read_world()
    (entry,) = cjson.loads((SAMPLE / "change.json").read_bytes())["changes"]
    manifest = cjson.loads(Store(SAMPLE).read_object(entry["manifest_id"], "json"))
    model = Store(SAMPLE).read_object(manifest["content_id"], "glb")
    ids = []
    for k, source in enumerate([FIXTURE_WORLD, world]):
        out = tmp_path / f"bundle-{k}"
        ids.append(make_bundle(source, manifest["feature_id"], model, out, author="github:tester", title="Test",
                               rationale="Test.", derived_from_shell=True))
        assert not any((out / name).exists() for name in NOTICE_FILES), "a bundle carries no notices"
    assert ids[0] == ids[1]


def test_op_world_names_the_same_notice_files() -> None:
    rust = (REPO / "crates" / "op_world" / "src" / "store.rs").read_text(encoding="utf-8")
    found = re.search(r"pub const NOTICE_FILES: \[&str; \d+\] = \[([^\]]*)\];", rust)
    assert found is not None
    assert tuple(re.findall(r'"([^"]+)"', found.group(1))) == NOTICE_FILES
