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
| 0003 | Determinism strategy | *planned, stage 4* |
| 0004 | Physics engine and character controller | *planned, stage 4* |
| [0005](0005-toolchain-and-engine-pinning.md) | Toolchain and engine pinning | Accepted |
| [0006](0006-ingest-pipeline-and-data-sources.md) | Ingest pipeline and data sources | Proposed |
| 0007 | Replay file format | *planned, stage 4* |
