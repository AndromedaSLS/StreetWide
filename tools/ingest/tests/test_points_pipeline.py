"""The ingest's point layers end to end on synthetic sources (no network):
the city's answers, terrain and surface tiles, through `build_points` and
`write_world` into a store, as RFC 0004, ADR-0015 and ADR-0016 ask."""

from __future__ import annotations

from pathlib import Path

import numpy as np
import pytest
from shapely.geometry import box

from tools.ingest import VERSION, pipeline, stac, stadt_zuerich, terrain
from tools.ingest.assemble import Asset, SchemaError, write_world
from tools.ingest.dem import Dem
from tools.ingest.geo import Area, Frame, tile_key
from tools.ingest.point_features import Ground
from tools.ingest.tests.helpers import write_dem_tile
from tools.ingest.tests.test_stadt_zuerich import FakeCity, light, tree
from tools.worldfmt import cjson, points
from tools.worldfmt.licences import licence_problems
from tools.worldfmt.store import Store

FRAME = Frame((2145, 1434))
TILE = (68646, 45903)
SQUARE = (2683, 1247)
GROUND = 410.0


def source_file(product: stac.Product, item_id: str, path: Path) -> stac.SourceFile:
    year, tile = item_id.split("_")[-2:]
    return stac.SourceFile(item_id, tile, year, f"https://data.geo.admin.ch/{item_id}.tif",
                           f"{stac.STAC_ROOT}/collections/{product.collection}/items/{item_id}", "ab" * 32, path)


@pytest.fixture
def district(tmp_path: Path, monkeypatch):
    """One tile of synthetic sources: flat terrain at 410 m, a surface model
    with one 12 m crown, two trees, three lights (one outside the tile) and
    a tree whose status stops nothing."""
    n = 2000
    flat = np.full((n, n), GROUND, dtype=np.float32)
    terrain_tif = write_dem_tile(tmp_path / "terrain.tif", SQUARE, flat, 0.5)
    surface = flat.copy()
    area = Area.from_tile(*TILE)
    area_poly = pipeline._lv95_polygon(pipeline._rect(area.x0, area.y0, area.x1, area.y1))
    c = area_poly.centroid
    e0, n0 = round(c.x, 1), round(c.y, 1)
    col, row = int((e0 + 1.0 - SQUARE[0] * 1000) / 0.5), int((SQUARE[1] * 1000 + 1000 - n0) / 0.5)
    surface[row, col] = GROUND + 12.0
    surface_tif = write_dem_tile(tmp_path / "surface.tif", SQUARE, surface, 0.5)
    dem_files = [source_file(stac.SWISSALTI3D, "swissalti3d_2026_2683-1247", terrain_tif)]
    surface_files = [source_file(stac.SWISSSURFACE3D_RASTER, "swisssurface3d-raster_2024_2683-1247", surface_tif)]
    monkeypatch.setattr(stac, "find", lambda product, *a, **k: surface_files if product is stac.SWISSSURFACE3D_RASTER
                        else dem_files)
    monkeypatch.setattr(stac, "fetch", lambda product, files, **k: files)
    answers = {
        stadt_zuerich.TREES: [tree("AA-2", e0, n0), tree("AA-1", e0 + 20, n0 - 20, pflanzjahr=2025)],
        stadt_zuerich.LIGHTS: [light("LEU2", e0 + 5, n0, 90.0), light("LEU1", e0 + 6, n0, 0.0),
                               light("LEU9", area_poly.bounds[2] + 50, n0)],
    }

    def fake(url, **kwargs):
        dataset = stadt_zuerich.TREES if "baumkataster" in url.lower() else stadt_zuerich.LIGHTS
        return FakeCity(dataset, answers[dataset])(url, **kwargs)

    monkeypatch.setattr(stadt_zuerich, "fetch_bytes", fake)
    dem = Dem([terrain_tif])
    origins = {TILE: FRAME.tile_origin_mm(*TILE)}
    mesh = terrain.build(FRAME, dem, area, TILE, origins[TILE], {})
    return dict(frame=FRAME, dem=dem, dem_files=dem_files, area=area, area_poly=area_poly, origins=origins,
                grounds={TILE: Ground(mesh)}, mesh=mesh, cache=tmp_path / "cache", e0=e0, n0=n0)


def build(d, footprints=()):
    stats: dict[str, int] = {}
    checks = pipeline.Checks(list(footprints), [], [], [])
    assets, report = pipeline.build_points(d["frame"], d["dem"], d["dem_files"], d["area"], d["area_poly"],
                                           d["origins"], d["grounds"], checks, d["cache"], False, False, stats)
    return assets, report, stats


def test_one_asset_per_tile_and_source_with_provenance(district, tmp_path: Path):
    assets, report, stats = build(district)
    trees, lights = assets
    key = tile_key(*TILE)
    assert (trees.layer, trees.feature_id, trees.collider) == ("vegetation", f"tile:{key}/stadt-zuerich:baumkataster",
                                                               "none")
    assert (lights.layer, lights.feature_id) == ("street_objects", f"tile:{key}/stadt-zuerich:beleuchtung")
    assert trees.license == lights.license == "CC0-1.0 AND LicenseRef-swisstopo-OGD"
    assert trees.attribution == lights.attribution == ["Quelle: Stadt Zürich", "© swisstopo"]
    city, alti, surface = trees.sources
    answer = report["answers"][0]
    assert city == {"dataset": "Baumkataster Stadt Zürich", "version": f"2026-11-22 sha256:{answer['sha256']}",
                    "license": "CC0-1.0", "attribution": "Quelle: Stadt Zürich",
                    "url": "https://data.stadt-zuerich.ch/dataset/geo_baumkataster", "source_ids": ["AA-1", "AA-2"]}
    assert (alti["dataset"], alti["version"], alti["source_ids"]) == ("swissALTI3D", "2026",
                                                                      ["swissalti3d_2026_2683-1247"])
    assert (surface["dataset"], surface["version"], surface["license"], surface["source_ids"]) == (
        "swissSURFACE3D Raster", "2024", "LicenseRef-swisstopo-OGD", ["swisssurface3d-raster_2024_2683-1247"])
    assert surface["url"].endswith("/collections/ch.swisstopo.swisssurface3d-raster")
    assert [s["dataset"] for s in lights.sources] == ["Öffentliche Beleuchtung der Stadt Zürich", "swissALTI3D"]
    assert lights.sources[0]["source_ids"] == ["LEU1", "LEU2"]

    by_id = {p["id"]: p for p in trees.points["points"]}
    # Measured from the anchor: the crown 12 m above flat ground.
    assert by_id["stadt-zuerich:baumkataster:AA-2"]["attributes"]["height_mm"] == 12000
    # Planted after the flight: no height.
    assert by_id["stadt-zuerich:baumkataster:AA-1"]["attributes"]["height_mm"] is None
    yaws = {p["id"]: p["attributes"]["yaw"] for p in lights.points["points"]}
    assert yaws["stadt-zuerich:beleuchtung:LEU1"] is None and yaws["stadt-zuerich:beleuchtung:LEU2"] is not None
    assert lights.points["rules"] == {"light": {"yaw": "adr-0015:yaw-from-orientierung:1"}}

    assert report["vegetation"]["heights"]["measured"] == 1
    assert report["vegetation"]["heights"]["null"] == {"planted in or after the surface model's year": 1}
    assert report["street_objects"]["points"] == 2 and report["street_objects"]["orientations_of_0"] == 1
    assert report["street_objects"]["left_out"] == {"outside the area": 1}
    assert stats == {"Öffentliche Beleuchtung der Stadt Zürich left out: outside the area": 1}

    # Written with the terrain: documents and manifests as RFC 0004 asks.
    store = Store(tmp_path / "world")
    ground = Asset(TILE, "terrain", f"tile:{key}", district["mesh"], "LicenseRef-swisstopo-OGD",
                   pipeline._dem_sources(district["dem_files"]), "trimesh")
    world_id, counts = write_world(store, FRAME, "t", [ground, *assets], district["origins"], [0, 0, 0], 0)
    assert counts == {"terrain": 1, "vegetation": 1, "street_objects": 1}
    _, world = store.read_world()
    assert world["attribution"] == ["Quelle: Stadt Zürich", "© swisstopo"]
    index = cjson.loads(store.read_object(world["tiles"][0]["tile_id"], "json"))
    assert [a["layer"] for a in index["assets"]] == ["street_objects", "terrain", "vegetation"]
    for entry in index["assets"]:
        m = cjson.loads(store.read_object(entry["manifest_id"], "json"))
        assert licence_problems(m["provenance"]) == []
        assert m["provenance"]["generator"] == {"name": "streetwide-ingest", "version": VERSION}
        if entry["layer"] == "terrain":
            continue
        data = store.read_object(m["content_id"], "json")
        doc = points.parse(data)
        assert m["size_bytes"] == len(data) and m["bounds_mm"] == points.bounds(doc)
        assert m["fictional"] is False and m["physics"] == {"collider": "none"}
        assert points.asset_problems(m["layer"], m["feature_id"], m["tile"], m["bounds_mm"], doc) == []
        # Every anchor on the terrain mesh, to the millimetre.
        g = district["grounds"][TILE]
        assert all(g.height(p["anchor_mm"][0], p["anchor_mm"][2]) == p["anchor_mm"][1] for p in doc["points"])


def test_buildings_mask_the_surface(district):
    e0, n0 = district["e0"], district["n0"]
    # A footprint over the crown: the tree measures the bare surface, 0 m.
    assets, report, _ = build(district, [box(e0 + 0.5, n0 - 3, e0 + 7, n0 + 3)])
    heights = {p["id"]: p["attributes"]["height_mm"] for p in assets[0].points["points"]}
    assert heights["stadt-zuerich:baumkataster:AA-2"] is None
    assert report["vegetation"]["heights"]["null"]["under 3 m"] == 1
    assert report["street_objects"]["inside_swissBUILDINGS3D_footprints"] == 2


def test_a_licence_that_does_not_cover_a_source_is_never_written(district, tmp_path: Path):
    assets, _, _ = build(district)
    trees = assets[0]
    trees.license = "CC0-1.0"
    with pytest.raises(SchemaError, match="is not in the asset"):
        write_world(Store(tmp_path / "w"), FRAME, "t", [trees], district["origins"], [0, 0, 0], 0)
    assert not (tmp_path / "w" / "world.json").exists()


def test_no_city_squares_no_points(district):
    # An area outside the city asks the city for nothing.
    d = dict(district, area_poly=box(2600000, 1199000, 2600500, 1199500))
    assets, report, _ = build(d)
    assert assets == [] and report["squares"] == [] and report["answers"] == []
