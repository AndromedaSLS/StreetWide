# ADR-0001: Licensing of code, content and data

- **Status:** Accepted (2026-11-23, stage 1 checkpoint)
- **Date:** 2026-11-22
- **Deciders:** @victorpicaud

## Context

StreetWide mixes four kinds of material with different legal regimes:

1. **Code** (Rust crates, Python tools), written by contributors.
2. **Contributed world content**: meshes, textures and manifests that
   players create to improve the world.
3. **Content derived from OpenStreetMap**, which is licensed under the Open
   Database License (ODbL 1.0) with share-alike obligations for derivative
   databases.
4. **Content derived from swisstopo** open government data (swissBUILDINGS3D,
   swissALTI3D), free to use, process and redistribute, including
   commercially, on condition that the source is credited ("© swisstopo";
   [terms](https://www.swisstopo.admin.ch/en/terms-and-conditions)).

CLAUDE.md requires that every asset carries provenance and licence in its
manifest. The project also needs a contribution model that is easy for
newcomers and keeps the licensing clean.

## Options considered

### Code licence

- **A. MIT OR Apache-2.0 (dual).** The Rust ecosystem default (Bevy and most
  crates use it). Apache-2.0 adds a patent grant, MIT keeps GPLv2
  compatibility. Familiar to contributors.
- **B. MPL-2.0 or GPL-family.** Copyleft keeps improvements open but deters
  engine reuse and conflicts with the Rust ecosystem's norms.
- **C. Apache-2.0 only.** Patent grant, but not GPLv2-compatible.

### Contributed content licence

- **A. CC BY-SA 4.0.** Share-alike keeps the canonical world and its forks
  open; widely understood by 3D artists; compatible in spirit with ODbL.
- **B. CC BY 4.0.** More permissive; allows closed forks of the world.
- **C. CC0.** Simplest, but drops attribution, which the community values.

### Contribution agreement

- **A. DCO (sign-off per commit).** Lightweight, contributors keep copyright.
- **B. CLA.** Lets a single entity relicense, adds friction and legal review.

## Decision

- **Code:** `MIT OR Apache-2.0`, in `LICENSE-MIT` and `LICENSE-APACHE`.
  Copyright line: "The StreetWide contributors".
- **Contributed world content:** `CC-BY-SA-4.0` (`LICENSES/CC-BY-SA-4.0.txt`).
- **OSM-derived assets:** `ODbL-1.0` (`LICENSES/ODbL-1.0.txt`). OSM-derived
  geometry is **never mixed into the same asset** as geometry from other
  sources, so the ODbL part of the world can be identified and extracted as
  its own database. Rendered views of the world are "Produced Works" under
  ODbL and carry the credit "© OpenStreetMap contributors".
- **swisstopo-derived assets:** licence identifier
  `LicenseRef-swisstopo-OGD` (SPDX `LicenseRef-` convention for licences not
  on the SPDX list), attribution "© swisstopo".
- **Contributions:** Developer Certificate of Origin, enforced in CI by
  `tools/ci/check_dco.py`. No CLA.
- Licence identifiers in manifests use SPDX expressions.

## Consequences

- Every asset manifest names its licence and sources. The world format (RFC
  0001) must make that mandatory and validation must reject assets without
  it.
- The client must show "© swisstopo, © OpenStreetMap contributors" on screen.
- Because content is share-alike, a contribution that combines a CC BY-SA
  asset with an ODbL asset must keep them as separate assets.
- Without a CLA the project cannot relicense existing contributions; a future
  licence change would need consent from contributors.
- Any new data source needs a licence review and an entry in
  ATTRIBUTION.md before its data can be ingested.
