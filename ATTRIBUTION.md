# Attribution

StreetWide is built from open data. This file lists every data source used to
build the world, the credit each one requires, and the licence it is used
under. Every asset manifest repeats the sources, licence and credits that
apply to that asset (see [ADR-0001](docs/adr/0001-licensing.md) and
[ADR-0006](docs/adr/0006-ingest-pipeline-and-data-sources.md)).

When you show StreetWide content (in the client, screenshots, videos or
derived maps), display the credits below. Clients show the `attribution`
list of the world manifest, which is the union of all asset credits.

**Required on-screen credit:** © swisstopo, © OpenStreetMap contributors,
and "Quelle: Stadt Zürich" for worlds with the City of Zurich's trees and
lights (the world manifest's `attribution` lists exactly the credits a
world needs)

## The world's licence

The published world is offered as a whole under the
[Open Database License 1.0](LICENSES/ODbL-1.0.txt) (ODbL), because it
contains OpenStreetMap data
([ADR-0019](docs/adr/0019-odbl-and-the-published-world.md), option A).
Inside it, each asset keeps the licence and the credits of its own
sources, stated in its manifest. Our copy of swisstopo data is part of
that ODbL world; the originals stay available from swisstopo on their
own terms.

- **Notices with the world:** every world the ingest writes has, beside
  its `world.json`, `NOTICE.txt` (the ODbL notice), `CREDITS.txt` (each
  source, its licence and a link) and the licence texts `ODbL-1.0.txt`
  and `CC-BY-SA-4.0.txt`. They are not part of the world: no id covers
  them. Keep them with the world when you copy it or pass it on.
- **Pictures and videos:** a screenshot, video or other work made from a
  world and shown in public carries the credits above and this notice
  (ODbL section 4.3), with "the StreetWide world" linked to
  <https://github.com/AndromedaSLS/StreetWide>, "Open Database License"
  to <https://opendatacommons.org/licenses/odbl/1-0/> and "© OpenStreetMap
  contributors" to <https://www.openstreetmap.org/copyright>:

  > Contains information from the StreetWide world, which is made
  > available here under the Open Database License (ODbL).
- **In the client:** the credit line is always on screen and names the
  ODbL, and the credits page (C) lists each source, its licence and a
  link.

## Data sources

| Source | Used for | Licence / terms | Credit |
|---|---|---|---|
| swisstopo swissBUILDINGS3D 3.0 Beta | Building shells (roofs, walls, ground plates) | [swisstopo terms for free geodata](LICENSES/LicenseRef-swisstopo-OGD.txt) (`LicenseRef-swisstopo-OGD`) | © swisstopo |
| swisstopo swissALTI3D | Terrain; heights of roads, bridges, water surfaces, OSM fallback buildings, trees and light points | swisstopo terms for free geodata (`LicenseRef-swisstopo-OGD`) | © swisstopo |
| swisstopo swissSURFACE3D Raster | Tree heights in Switzerland ([ADR-0016](docs/adr/0016-swisssurface3d-heights.md)) | swisstopo terms for free geodata (`LicenseRef-swisstopo-OGD`) | © swisstopo |
| Baumkataster Stadt Zürich (the city's tree register) | Trees in the City of Zurich ([ADR-0015](docs/adr/0015-zurich-trees-and-lights.md)) | [CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/) (`CC0-1.0`) | Quelle: Stadt Zürich |
| Öffentliche Beleuchtung der Stadt Zürich (ewz's public lighting) | Light points in the City of Zurich ([ADR-0015](docs/adr/0015-zurich-trees-and-lights.md)) | [CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/) (`CC0-1.0`) | Quelle: Stadt Zürich |
| OpenStreetMap | Roads, paths, steps and bridges; water areas (rivers, lakes, canals, ponds); building footprints where swissBUILDINGS3D has no building | [Open Database License 1.0](LICENSES/ODbL-1.0.txt) (`ODbL-1.0`) | © OpenStreetMap contributors |
| StreetWide contributions | What players add to the world, such as facades, once merged | [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/) (`CC-BY-SA-4.0`), [text](LICENSES/CC-BY-SA-4.0.txt); see [CONTRIBUTING.md](CONTRIBUTING.md#contributing-world-content) | The StreetWide contributors |

### swisstopo (Federal Office of Topography)

- **swissBUILDINGS3D 3.0 Beta**:
  [product page](https://www.swisstopo.admin.ch/en/landscape-model-swissbuildings3d-3-0-beta).
  We use the CityGML release of each map sheet, the latest year available.
  For the Zurich old town this is sheet 1091-41, release 2025.
- **swissALTI3D**:
  [product page](https://www.swisstopo.admin.ch/en/height-model-swissalti3d).
  We use the 0.5 m GeoTIFF of each 1 km tile, the latest year available.
  For the Zurich old town this is tile 2683-1247, release 2026.
- **swissSURFACE3D Raster**:
  [product page](https://www.swisstopo.admin.ch/en/height-model-swisssurface3d-raster).
  We use the 0.5 m GeoTIFF of each 1 km tile, the latest year available,
  only to measure the height of each register tree (the surface minus
  swissALTI3D within 1.5 m of the trunk, buildings masked; ADR-0016).
  Nothing else from it enters a world. For the Zurich old town this is
  tile 2683-1247, release 2024.
- Files are downloaded from the
  [swisstopo STAC API](https://data.geo.admin.ch/api/stac/v1/) and checked
  against the SHA-256 checksums it publishes.
- Terms: swisstopo's
  [terms of use for free geodata and geoservices](https://www.swisstopo.admin.ch/en/terms-of-use-free-geodata-and-geoservices),
  stored in [LICENSES/LicenseRef-swisstopo-OGD.txt](LICENSES/LicenseRef-swisstopo-OGD.txt)
  as read on 2026-11-25 ([ADR-0017](docs/adr/0017-licence-allowlist.md)).
  swisstopo's free geodata may be used, distributed, processed and used
  commercially, provided the source is credited. We use "© swisstopo", one
  of the source references the terms list.

### Stadt Zürich (City of Zurich)

Two datasets of the city's open-data portal, both CC0 and updated weekly
([ADR-0015](docs/adr/0015-zurich-trees-and-lights.md)). CC0 requires no
credit; StreetWide credits every source, with "Quelle: Stadt Zürich", the
credit ADR-0015 records for both. Manifests name them
`Baumkataster Stadt Zürich` and `Öffentliche Beleuchtung der Stadt Zürich`.

- **Baumkataster** (the tree register of Grün Stadt Zürich):
  [dataset page](https://data.stadt-zuerich.ch/dataset/geo_baumkataster),
  WFS layer `baumkataster_baumstandorte` at
  `https://www.ogd.stadt-zuerich.ch/wfs/geoportal/Baumkataster`. We keep
  each tree's number, position, genus, species, full scientific name,
  planting year and crown diameter (a crown of 8 m, the register's default,
  counts as unknown); never its street, quarter, status, size class or
  internal ids. Only trees with one of the six statuses of public bodies
  are taken: any other status stops the ingest.
- **Öffentliche Beleuchtung der Stadt Zürich** (ewz's public lighting):
  [dataset page](https://data.stadt-zuerich.ch/dataset/geo_oeffentliche_beleuchtung_der_stadt_zuerich),
  WFS layer `ewz_brennstelle_p` at
  `https://www.ogd.stadt-zuerich.ch/wfs/geoportal/Oeffentliche_Beleuchtung_der_Stadt_Zuerich`.
  We keep each light point's number and position, and its orientation as
  an estimated direction (rule `adr-0015:yaw-from-orientierung:1`).
- **Fetching:** one WFS request per dataset and LV95 kilometre square, in
  LV95, with the service's feature count and the portal's metadata. The
  ingest checks at every fetch that the portal states the licence
  `cc-zero`, and stops otherwise. Answers are cached in
  `data/cache/stadt-zuerich/`, with a sidecar file holding their SHA-256.
- **Version:** the publisher's date (`dateLastUpdated`) and the SHA-256 of
  the answer, as `2026-11-22 sha256:<hex>`, in each source entry. Every
  answer an asset uses has its own source entry.
- **Archive:** after a weekly update the city's earlier data cannot be
  fetched again. Every raw answer a published baseline uses is to be
  archived beside the world store, addressed by its SHA-256 (ADR-0015
  section 4), once that storage exists; until then the answers stay in
  local caches. The fixture's two answers are listed with their SHA-256
  in [fixtures/README.md](fixtures/README.md), to be attached to a GitHub
  release.
- **Positions and heights:** each point stands on the swissALTI3D terrain
  of its tile; tree heights come from swissSURFACE3D Raster. Nothing from
  OpenStreetMap places, filters or corrects these points.
- **Errors** are fixed at the source: Grün Stadt Zürich for trees, ewz for
  light points. The next refresh brings the fixes in.

### OpenStreetMap

- Data © [OpenStreetMap contributors](https://www.openstreetmap.org/copyright),
  available under the [Open Database License](https://opendatacommons.org/licenses/odbl/1-0/).
- We query it through the [Overpass API](https://overpass-api.de/), with
  two queries per area, each cached separately: one for roads, paths and
  building footprints, and one for water areas
  ([ADR-0014](docs/adr/0014-water-areas-from-openstreetmap.md)). The
  `timestamp_osm_base` of each answer is recorded as the data version in
  every manifest derived from it.
- **ODbL notice:** StreetWide worlds contain information from OpenStreetMap,
  which is made available here under the Open Database License (ODbL).
  The worlds themselves are offered under the ODbL too (see
  [The world's licence](#the-worlds-licence)).
- **Assets from OpenStreetMap:** every asset whose licence contains
  `ODbL-1.0` (roads, paths, bridges, water and OSM fallback buildings) is
  derived from OpenStreetMap. It is offered under the ODbL together with
  the swisstopo terrain heights it uses, so its licence is
  `ODbL-1.0 AND LicenseRef-swisstopo-OGD`. The tile indexes list each
  asset's licence, so these assets can be listed. They are not a
  separate database that could be extracted on its own terms from the
  rest: the OSM fallback buildings share the `shell` layer with
  swissBUILDINGS3D's buildings, and the world as a whole is under the
  ODbL. Water stays in its own layer (`water`,
  [RFC 0003](rfcs/0003-water-layer.md)).

## Data in this repository

Only small test data is committed. Full worlds are built locally with
`python -m tools.ingest` into `data/` (gitignored).

| Path | Content | Licence |
|---|---|---|
| `fixtures/format/` | Hand-made fictional sample world | CC BY-SA 4.0, credit "The StreetWide contributors" |
| `fixtures/world/` | One real tile (17/68646/45903: Grossmünster, Münsterbrücke, the Limmat) built by the ingest | Per asset, as stated in its manifest: swisstopo terms for terrain and swissBUILDINGS3D shells; ODbL 1.0 AND swisstopo terms for the OSM-derived assets (roads, paths, bridges, water and one OSM fallback building); CC0 1.0 AND swisstopo terms for the tile's trees and light points from Stadt Zürich (credits "Quelle: Stadt Zürich" and "© swisstopo") |
| `worlds/*.json` `places` | Positions of named squares and streets for the client's travel keys, taken from OpenStreetMap | ODbL 1.0, © OpenStreetMap contributors |

## Third-party software

Rust and Python dependencies are listed with their licences in `Cargo.lock`
and `tools/requirements.txt`. The ingest uses PROJ (via pyproj), GEOS (via
shapely), earcut (via mapbox-earcut), NumPy, tifffile and imagecodecs.
