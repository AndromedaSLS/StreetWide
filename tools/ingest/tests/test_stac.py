from pathlib import Path

from tools.ingest import stac
from tools.ingest.geo import BBox

BBOX = BBox(8.5377, 47.3687, 8.5507, 47.3774)
INSIDE = [[8.5375049, 47.3686138], [8.5507426, 47.3684871], [8.5509303, 47.3774805], [8.5376903, 47.3776072], [8.5375049, 47.3686138]]
# Its lon/lat envelope overlaps the bbox, the footprint itself does not.
NEIGHBOUR = [[8.5373195, 47.3594938], [8.5505553, 47.359367], [8.5507426, 47.3684871], [8.5375049, 47.3686138], [8.5373195, 47.3594938]]


def item(id, ring, asset_suffix="_0.5_2056_5728.tif"):
    return {
        "id": id,
        "geometry": {"type": "Polygon", "coordinates": [ring]},
        "assets": {
            f"{id}{asset_suffix}": {"href": f"https://data.geo.admin.ch/x/{id}{asset_suffix}", "file:checksum": "1220" + "AB" * 32},
            f"{id}_2_2056_5728.tif": {"href": "https://data.geo.admin.ch/other", "file:checksum": "1220" + "CD" * 32},
        },
    }


def test_selects_latest_year_per_tile_by_footprint():
    features = [
        item("swissalti3d_2019_2683-1247", INSIDE),
        item("swissalti3d_2026_2683-1247", INSIDE),
        item("swissalti3d_2026_2683-1246", NEIGHBOUR),
        {"id": "swissalti3d_2026", "geometry": {"type": "Polygon", "coordinates": [INSIDE]}, "assets": {}},
    ]
    (got,) = stac.select(features, stac.SWISSALTI3D, BBOX, Path("cache"))
    assert (got.item_id, got.year, got.tile) == ("swissalti3d_2026_2683-1247", "2026", "2683-1247")
    assert got.sha256 == "ab" * 32
    assert got.path == Path("cache/swisstopo/swissalti3d_2026_2683-1247/swissalti3d_2026_2683-1247_0.5_2056_5728.tif")
    assert got.item_url.endswith("/collections/ch.swisstopo.swissalti3d/items/swissalti3d_2026_2683-1247")


def test_item_ids_parse_as_before_and_with_a_hyphen():
    # ADR-0016: swissSURFACE3D Raster's product has a hyphen; the ids that
    # parsed before give the same parts.
    for item_id, parts in [
        ("swissalti3d_2026_2683-1247", ("swissalti3d", "2026", "2683-1247")),
        ("swissalti3d_2019_2600-1199", ("swissalti3d", "2019", "2600-1199")),
        ("swissbuildings3d_3_0_2025_1091-41", ("swissbuildings3d_3_0", "2025", "1091-41")),
        ("swissbuildings3d_3_0_2025_1091-4142", ("swissbuildings3d_3_0", "2025", "1091-4142")),
        ("swisssurface3d-raster_2024_2683-1247", ("swisssurface3d-raster", "2024", "2683-1247")),
    ]:
        m = stac._ITEM_ID.fullmatch(item_id)
        assert m is not None and (m["product"], m["year"], m["tile"]) == parts, item_id
    for not_an_item in ["swissalti3d_26_2683-1247", "swissalti3d_2026", "swissalti3d_2026_2683_1247", "_2026_2683-1247"]:
        assert stac._ITEM_ID.fullmatch(not_an_item) is None, not_an_item


def test_selects_the_latest_surface_model_per_tile():
    features = [
        item("swisssurface3d-raster_2018_2683-1247", INSIDE),
        item("swisssurface3d-raster_2024_2683-1247", INSIDE),
    ]
    (got,) = stac.select(features, stac.SWISSSURFACE3D_RASTER, BBOX, Path("cache"))
    assert (got.item_id, got.year, got.tile) == ("swisssurface3d-raster_2024_2683-1247", "2024", "2683-1247")
    assert got.path == Path("cache/swisstopo/swisssurface3d-raster_2024_2683-1247/"
                            "swisssurface3d-raster_2024_2683-1247_0.5_2056_5728.tif")
    assert got.item_url.endswith("/collections/ch.swisstopo.swisssurface3d-raster/items/"
                                 "swisssurface3d-raster_2024_2683-1247")
