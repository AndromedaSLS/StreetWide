import numpy as np
import pytest
import tifffile

from tools.ingest.dem import Dem, DemError

E0, N1, S = 2683000.0, 1248000.0, 0.5


def _write(path, z, *, epsg=2056, raster_type=1, nodata="-9999"):
    keys = [1, 1, 0, 3, 1024, 0, 1, 1, 1025, 0, 1, raster_type, 3072, 0, 1, epsg]
    tags = [
        (33550, "d", 3, (S, S, 0.0), True),
        (33922, "d", 6, (0.0, 0.0, 0.0, E0, N1, 0.0), True),
        (34735, "H", len(keys), keys, True),
        (42113, "s", 0, nodata, True),
    ]
    tifffile.imwrite(path, z.astype(np.float32), extratags=tags)


def test_bilinear_is_exact_on_a_plane(tmp_path):
    rows, cols = 40, 60
    j, i = np.meshgrid(np.arange(cols), np.arange(rows))
    e = E0 + (j + 0.5) * S
    n = N1 - (i + 0.5) * S
    z = 400 + 0.25 * (e - E0) - 0.5 * (N1 - n)
    _write(tmp_path / "t.tif", z)
    dem = Dem([tmp_path / "t.tif"])
    pe = np.array([E0 + 1.3, E0 + 17.77, E0 + 25.0])
    pn = np.array([N1 - 2.2, N1 - 11.1, N1 - 19.0])
    assert dem.height(pe, pn) == pytest.approx(400 + 0.25 * (pe - E0) - 0.5 * (N1 - pn), abs=1e-4)


def test_outside_and_nodata_are_nan(tmp_path):
    z = np.full((10, 10), 410.0)
    z[5, 5] = -9999
    _write(tmp_path / "t.tif", z)
    dem = Dem([tmp_path / "t.tif"])
    h = dem.height([E0 - 1, E0 + 2.75, E0 + 1.0], [N1 - 1, N1 - 2.75, N1 - 1.0])
    assert np.isnan(h[0]) and np.isnan(h[1]) and h[2] == pytest.approx(410.0)


def test_rejects_other_projections(tmp_path):
    _write(tmp_path / "t.tif", np.zeros((4, 4)), epsg=21781)
    with pytest.raises(DemError):
        Dem([tmp_path / "t.tif"])
