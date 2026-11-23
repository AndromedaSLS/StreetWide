import zipfile

import numpy as np

from tools.ingest import buildings
from tools.ingest.citygml import read_buildings
from tools.ingest.geo import Frame

HEADER = (
    '<?xml version="1.0" encoding="UTF-8"?>'
    '<core:CityModel xmlns:core="http://www.opengis.net/citygml/2.0"'
    ' xmlns:bldg="http://www.opengis.net/citygml/building/2.0"'
    ' xmlns:gml="http://www.opengis.net/gml"'
    ' xmlns:gen="http://www.opengis.net/citygml/generics/2.0"'
    ' xmlns:app="http://www.opengis.net/citygml/appearance/2.0">'
)


def ring(*pts):
    pts = list(pts) + [pts[0]]
    return " ".join(f"{e} {n} {h}" for e, n, h in pts)


def surface(kind, *rings):
    polys = "".join(
        f'<gml:surfaceMember><gml:Polygon><gml:exterior><gml:LinearRing><gml:posList srsDimension="3">{r}'
        "</gml:posList></gml:LinearRing></gml:exterior></gml:Polygon></gml:surfaceMember>"
        for r in rings
    )
    return f"<bldg:boundedBy><bldg:{kind}><bldg:lod2MultiSurface><gml:MultiSurface>{polys}</gml:MultiSurface></bldg:lod2MultiSurface></bldg:{kind}></bldg:boundedBy>"


def box_building(gml_id, e, n, objektart="Gebaeude Einzelhaus"):
    """A 10 x 10 x 6 m box, written like swissBUILDINGS3D: typed surfaces
    (ground facing up, as in the real data) plus a duplicate solid."""
    h0, h1 = 410.0, 416.0
    a, b, c, d = (e, n), (e + 10, n), (e + 10, n + 10), (e, n + 10)
    walls = [ring((p[0], p[1], h0), (q[0], q[1], h0), (q[0], q[1], h1), (p[0], p[1], h1)) for p, q in [(a, b), (b, c), (c, d), (d, a)]]
    roof = ring((*a, h1), (*b, h1), (*c, h1), (*d, h1))
    ground = ring((*a, h0), (*b, h0), (*c, h0), (*d, h0))
    duplicate = f'<bldg:lod2Solid><gml:Solid><gml:exterior><gml:CompositeSurface><gml:surfaceMember><gml:Polygon><gml:exterior><gml:LinearRing><gml:posList>{roof}</gml:posList></gml:LinearRing></gml:exterior></gml:Polygon></gml:surfaceMember></gml:CompositeSurface></gml:exterior></gml:Solid></bldg:lod2Solid>'
    return (
        f'<core:cityObjectMember><bldg:Building gml:id="{gml_id}">'
        f'<gen:stringAttribute name="OBJEKTART"><gen:value>{objektart}</gen:value></gen:stringAttribute>'
        f"{surface('RoofSurface', roof)}{surface('WallSurface', *walls)}{surface('GroundSurface', ground)}{duplicate}"
        "</bldg:Building></core:cityObjectMember>"
    )


def sheet(tmp_path, members):
    path = tmp_path / "sheet.citygml.zip"
    appearance = '<app:appearanceMember><app:Appearance><app:surfaceDataMember><app:X3DMaterial><app:target>#x</app:target></app:X3DMaterial></app:surfaceDataMember></app:Appearance></app:appearanceMember>'
    with zipfile.ZipFile(path, "w") as z:
        z.writestr("sheet.gml", HEADER + "".join(members) + appearance + "</core:CityModel>")
    return path


def test_reads_typed_surfaces_only_and_filters_by_envelope(tmp_path):
    path = sheet(tmp_path, [box_building("ID_B", 2683100, 1247100), box_building("ID_A", 2683200, 1247200), box_building("ID_FAR", 2690000, 1250000)])
    stats = {}
    got = read_buildings(path, (2683000, 1247000, 2684000, 1248000), stats)
    assert [b.gml_id for b in got] == ["ID_A", "ID_B"]  # sorted, far one dropped
    assert stats["buildings in sheet"] == 3
    kinds = [k for k, _ in got[0].surfaces]
    assert kinds == ["roof", "wall", "wall", "wall", "wall", "ground"]  # the solid is not read twice
    assert all(len(rings[0]) == 4 for _, rings in got[0].surfaces)  # closing point removed
    assert got[0].objektart == "Gebaeude Einzelhaus"


def test_shell_has_outward_normals_and_skips_underground(tmp_path):
    path = sheet(tmp_path, [box_building("ID_A", 2683200, 1247200), box_building("ID_U", 2683300, 1247300, "Unterirdisches Gebaeude")])
    a, u = read_buildings(path, (2683000, 1247000, 2684000, 1248000), {})
    frame = Frame((2145, 1434))
    stats = {}
    assert buildings.swiss_shell(frame, u, [0, 0, 0], stats) is None
    mesh = buildings.swiss_shell(frame, a, [0, 0, 0], stats)
    prims = {p.material: p for p in mesh.node("swisstopo:swissbuildings3d:a").primitives}
    assert list(prims) == ["roof", "wall", "ground"]
    assert min(n[1] for n in prims["roof"].normals) > 0.99
    assert max(n[1] for n in prims["ground"].normals) < -0.99  # turned to face down
    fp = buildings.swiss_footprint(a)
    assert fp.area == 100.0
    walls = prims["wall"]
    centre = np.mean(walls.positions_mm, axis=0)
    for i in range(0, len(walls.indices), 3):
        v = np.array([walls.positions_mm[k] for k in walls.indices[i : i + 3]], float)
        assert np.dot(walls.normals[walls.indices[i]], v.mean(axis=0) - centre) > 0
