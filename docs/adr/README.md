# Architecture decision records

One record per foundational decision. Each ADR states the context, the
options considered, the decision and its consequences. ADRs are immutable
once accepted: to change a decision, write a new ADR that supersedes the old
one and link both ways.

Statuses: **Proposed** → **Accepted** | **Rejected**; later **Superseded by
ADR-NNNN**.

To add one, copy [0000-template.md](0000-template.md) to the next free number.
Format and contract changes go through an RFC in [`/rfcs`](../../rfcs/)
instead.

| ADR | Title | Status |
|---|---|---|
| [0001](0001-licensing.md) | Licensing of code, content and data | Accepted |
| [0002](0002-crate-layout-and-dependency-direction.md) | Crate layout and dependency direction | Accepted |
| [0003](0003-determinism-strategy.md) | Determinism strategy | Accepted |
| [0004](0004-physics-and-character-controller.md) | Physics engine and character controller | Accepted |
| [0005](0005-toolchain-and-engine-pinning.md) | Toolchain and engine pinning | Accepted |
| [0006](0006-ingest-pipeline-and-data-sources.md) | Ingest pipeline and data sources | Accepted |
| [0007](0007-replay-file-format.md) | Replay file format | Accepted |
| [0008](0008-client-architecture-and-streaming.md) | Client architecture and streaming | Accepted |
| [0009](0009-simulation-based-gate-checks.md) | Where the simulation-based gate checks run | Accepted |
| [0010](0010-hosting-the-canonical-world.md) | Hosting the canonical world store and world log | Proposed |
| [0011](0011-stop-corner-oscillation.md) | Stop the player oscillating in concave corners | Proposed |
| [0012](0012-client-realism-from-procedural-detail.md) | Client realism from procedural detail and physically based light | Proposed |
| [0013](0013-travel-by-starting-new-sessions.md) | Travel in the client by starting new sessions | Proposed |
| [0014](0014-water-areas-from-openstreetmap.md) | Water areas from OpenStreetMap, placed as level surfaces | Proposed |
| [0015](0015-zurich-trees-and-lights.md) | Trees and lights in Zurich from Stadt Zürich open data | Proposed |
| [0016](0016-swisssurface3d-heights.md) | swissSURFACE3D for measured heights | Proposed |
| [0017](0017-licence-allowlist.md) | An allowlist of licences for world data and client assets | Proposed |
| [0018](0018-performance-tier.md) | A performance tier and a benchmark route | Proposed |
| [0019](0019-odbl-and-the-published-world.md) | How the published world meets the ODbL | Proposed |
| [0020](0020-baseline-refreshes.md) | Baseline refreshes | Proposed |
| [0021](0021-pedestrians.md) | Pedestrians: in the client, the simulation or both | Proposed |
