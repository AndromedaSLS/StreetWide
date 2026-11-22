# Governance

StreetWide is an early-stage open-source project. This document says who
decides what, and how that will change as the community grows.

## Roles

- **Contributors**: anyone who opens an issue, reviews a pull request or
  submits code, data or world content under the DCO.
- **Maintainers**: review and merge pull requests, triage issues, cut
  releases, and accept or reject ADRs and RFCs. Maintainers are listed below.

### Maintainers

| GitHub | Areas |
|---|---|
| [@victorpicaud](https://github.com/victorpicaud) | All (founding maintainer) |

## How decisions are made

1. **Everyday changes** (bug fixes, features within existing contracts):
   lazy consensus. A pull request that passes CI and has an approving review
   from a maintainer can be merged, unless someone raises an unresolved
   objection.
2. **Foundational choices** (a technology, a rule, a boundary between
   crates) are recorded as an **ADR** in `docs/adr/`. A maintainer accepts
   or rejects it after it has been open for review.
3. **Contract changes** (world format, module interfaces, replay format,
   merge rules for the canonical world) go through an **RFC** in `rfcs/`.
   An RFC stays open for comment for at least 7 days before maintainers
   decide. The decision and its reasons are written into the RFC.
4. Anything not covered by an ADR or RFC is drafted as options and
   discussed, not decided silently in code.

If consensus can't be reached, the maintainers decide by simple majority.
While there is only one maintainer, that maintainer decides and records the
reasoning in the ADR or RFC.

## The canonical world

The canonical world is the version of the world that StreetWide publishes.
It **only moves upward**: a contribution is merged only if it passes every
automated check (schema, provenance, licence, determinism, performance
budgets) and is judged better than the version it replaces. The concrete
merge criteria are themselves defined by RFC, and changing them requires an
RFC.

## Becoming a maintainer

Contributors who have made sustained, high-quality contributions and reviews
can be nominated by any maintainer. A new maintainer is added by consensus of
the existing maintainers and recorded in this file. Maintainers who become
inactive for six months may be moved to emeritus status.

## Changes to this document

Changes to governance are made by pull request and need the approval of a
majority of maintainers.

## Code of Conduct

All participants follow the [Code of Conduct](CODE_OF_CONDUCT.md).
