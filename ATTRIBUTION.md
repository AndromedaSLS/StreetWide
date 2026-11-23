# Attribution

StreetWide is built from open data. This file lists every data source used to
build the world, the credit each one requires, and the licence it is used
under. Every asset manifest repeats the sources, licence and credits that
apply to that asset (see [ADR-0001](docs/adr/0001-licensing.md) and
[ADR-0006](docs/adr/0006-ingest-pipeline-and-data-sources.md)).

When you show StreetWide content (in the client, screenshots, videos or
derived maps), display the credits below. Clients show the `attribution`
list of the world manifest, which is the union of all asset credits.

**Required on-screen credit:** © swisstopo, © OpenStreetMap contributors

## Data sources

| Source | Used for | Licence / terms | Credit |
|---|---|---|---|
| swisstopo swissBUILDINGS3D 3.0 Beta | Building shells (roofs, walls, ground plates) | [swisstopo terms for free geodata](https://www.swisstopo.admin.ch/en/terms-of-use-free-geodata-and-geoservices) (`LicenseRef-swisstopo-OGD`) | © swisstopo |
| swisstopo swissALTI3D | Terrain; heights of roads, bridges and OSM fallback buildings | swisstopo terms for free geodata (`LicenseRef-swisstopo-OGD`) | © swisstopo |
| OpenStreetMap | Roads, paths, steps and bridges; building footprints where swissBUILDINGS3D has no building | [Open Database License 1.0](LICENSES/ODbL-1.0.txt) (`ODbL-1.0`) | © OpenStreetMap contributors |

### swisstopo (Federal Office of Topography)

- **swissBUILDINGS3D 3.0 Beta**:
  [product page](https://www.swisstopo.admin.ch/en/landscape-model-swissbuildings3d-3-0-beta).
  We use the CityGML release of each map sheet, the latest year available.
  For the Zurich old town this is sheet 1091-41, release 2025.
- **swissALTI3D**:
  [product page](https://www.swisstopo.admin.ch/en/height-model-swissalti3d).
  We use the 0.5 m GeoTIFF of each 1 km tile, the latest year available.
  For the Zurich old town this is tile 2683-1247, release 2026.
- Files are downloaded from the
  [swisstopo STAC API](https://data.geo.admin.ch/api/stac/v1/) and checked
  against the SHA-256 checksums it publishes.
- Terms: swisstopo's free geodata may be used, distributed, processed and
  used commercially, provided the source is credited. We use "© swisstopo",
  one of the source references the terms list.

### OpenStreetMap

- Data © [OpenStreetMap contributors](https://www.openstreetmap.org/copyright),
  available under the [Open Database License](https://opendatacommons.org/licenses/odbl/1-0/).
- We query it through the [Overpass API](https://overpass-api.de/), one
  query per area. The `timestamp_osm_base` of the answer is recorded as the
  data version in every OSM-derived manifest.
- **ODbL notice:** StreetWide worlds contain information from OpenStreetMap,
  which is made available here under the Open Database License (ODbL).
- **Derived databases:** every asset whose licence contains `ODbL-1.0`
  (roads, paths, bridges and OSM fallback buildings) is a derivative of
  OpenStreetMap. It is offered under the ODbL together with the swisstopo
  terrain heights it uses, so its licence is
  `ODbL-1.0 AND LicenseRef-swisstopo-OGD`. These assets can be extracted
  mechanically from the tile indexes, which list each asset's licence.

## Data in this repository

Only small test data is committed. Full worlds are built locally with
`python -m tools.ingest` into `data/` (gitignored).

| Path | Content | Licence |
|---|---|---|
| `fixtures/format/` | Hand-made fictional sample world | CC BY-SA 4.0, credit "The StreetWide contributors" |
| `fixtures/world/` | One real tile (17/68646/45903: Grossmünster, Münsterbrücke) built by the ingest | Per asset, as stated in its manifest: swisstopo terms for terrain and swissBUILDINGS3D shells; ODbL 1.0 AND swisstopo terms for the OSM-derived assets |

## Third-party software

Rust and Python dependencies are listed with their licences in `Cargo.lock`
and `tools/requirements.txt`. The ingest uses PROJ (via pyproj), GEOS (via
shapely), earcut (via mapbox-earcut), NumPy, tifffile and imagecodecs.
