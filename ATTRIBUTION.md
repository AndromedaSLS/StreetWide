# Attribution

StreetWide is built from open data. This file lists every data source used to
build the world, the credit each one requires, and the licence it is used
under. Every asset manifest repeats the sources and licence that apply to
that asset (see [ADR-0001](docs/adr/0001-licensing.md)).

When you show StreetWide content (in the client, screenshots, videos or
derived maps), display the credits below.

**Required on-screen credit:** © swisstopo, © OpenStreetMap contributors

## Data sources

> Complete entries (dataset versions, links, terms of use) are added in
> stage 3, together with the ingest scripts that download each source.

| Source | Used for | Licence / terms | Credit |
|---|---|---|---|
| swisstopo swissBUILDINGS3D 3.0 Beta | Building shells (roofs, walls) | swisstopo terms of use for free geodata | © swisstopo |
| swisstopo swissALTI3D | Terrain | swisstopo terms of use for free geodata | © swisstopo |
| OpenStreetMap | Roads, paths, bridges; building footprints where no 3D model exists | [ODbL 1.0](LICENSES/ODbL-1.0.txt) | © OpenStreetMap contributors |

## Third-party software

Rust and Python dependencies are listed with their licences in `Cargo.lock`
and the `requirements*.txt` files.
