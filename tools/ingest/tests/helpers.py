"""Synthetic stand-ins for the downloaded sources."""

from __future__ import annotations

import numpy as np


class PlaneDem:
    """A terrain whose height is a plane over LV95 (and NaN outside a box)."""

    def __init__(self, h0=400.0, de=0.01, dn=-0.02, box=None):
        self.h0, self.de, self.dn, self.box = h0, de, dn, box

    def height(self, e, n):
        e = np.asarray(e, dtype=np.float64)
        n = np.asarray(n, dtype=np.float64)
        h = self.h0 + self.de * (e - 2683000.0) + self.dn * (n - 1247000.0)
        if self.box is not None:
            e0, n0, e1, n1 = self.box
            h = np.where((e >= e0) & (e <= e1) & (n >= n0) & (n <= n1), h, np.nan)
        return h


def way(id, highway, en, nodes=None, **tags):
    from tools.ingest.osm import Way

    en = np.asarray(en, dtype=np.float64)
    return Way(id, {"highway": highway, **tags}, nodes or list(range(id * 100, id * 100 + len(en))), en)
