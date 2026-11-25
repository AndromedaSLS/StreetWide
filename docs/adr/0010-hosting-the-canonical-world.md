# ADR-0010: Host the canonical world as a Git world log plus an object store

- **Status:** Proposed
- **Date:** 2026-11-23
- **Deciders:** @victorpicaud
- **Supersedes / superseded by:** none

## Context

RFC 0002 needs two places, and leaves choosing them to an ADR:
- a **canonical object store**: tile indexes, manifests and meshes, about
  50 MB per district today and growing with every contribution, append-only
  and addressed by content id;
- a **world log**: merge records, check reports and change sets. These are
  small text documents in a hash chain.

Constraints:
- No large data in the code repository (CLAUDE.md).
- Contributors should be able to propose a change with a normal GitHub
  pull request and DCO sign-off.
- Players and tools download worlds over plain HTTPS.
- Every object can be verified by its content id, so the storage itself
  need not be trusted.

## Options considered

### A. A separate Git repository for the world log, an S3-compatible bucket for objects (recommended)

- **The world log** lives in a repository such as
  `AndromedaSLS/StreetWide-world`:
  - `log/00000000.json`, `log/00000001.json`, …: merge records in log
    order;
  - `changes/<change-id>/change.json`;
  - `reports/<report-id>.json`.
- **A contribution** is a pull request adding a bundle
  (`changes/<name>/change.json` and `objects/`), limited in size (e.g.
  8 MB per pull request). CI runs the gate and posts the report. On merge,
  a bot:
  1. uploads the bundle's objects and the new tile indexes and world
     manifest to the bucket;
  2. commits the merge record, change set and report;
  3. deletes the bundle's binary files from the branch, so the world
     repository's history stays text.
- **The bucket** is any S3-compatible storage with public read access:
  - layout `objects/<aa>/<hex>.<ext>` and `worlds/<world-id>.json`, plus a
    small `current.json` naming the current world;
  - providers include Cloudflare R2, Backblaze B2 and AWS S3; R2 has no
    egress fees.
- Pros:
  - contributors only need GitHub;
  - the code repository stays small;
  - objects are cheap to serve and cache;
  - anyone can mirror the bucket and verify it.
- Cons:
  - a cloud account and a bot with write credentials;
  - binary files pass briefly through pull requests.

### B. Everything in a Git LFS repository

- Pros: one system.
- Cons:
  - GitHub LFS storage and bandwidth quotas cost money and throttle
    players' downloads;
  - cloning becomes heavy.

### C. GitHub Releases as the object store

One release per world version, with objects as release assets.

- Pros: no extra account.
- Cons:
  - assets are not content-addressed paths;
  - there are per-release limits;
  - de-duplication across versions is awkward.

### D. IPFS

- Pros: natively content-addressed.
- Cons:
  - needs pinning services;
  - content ids differ from BLAKE3, so a mapping is needed;
  - unfamiliar to most contributors.

## Decision

*Proposed:* option A. The storage provider and account are the
maintainer's choice and are recorded here when made.

Until then, the canonical world exists only on disk.
`op_world check-change` and `op_world merge` operate on local
directories, so tooling does not depend on this decision.

**A local log** (a note proposed with
[ADR-0020](0020-baseline-refreshes.md), not decided). A world log kept
in a local directory uses option A's layout, with two differences:
- **`<hex>` in paths:** `changes/<hex>/change.json` and
  `reports/<hex>.json`, where `<hex>` is the content id without
  `blake3:`. A `:` is not allowed in Windows paths, so
  `changes/<change-id>/` cannot be written there. A clone of option A's
  log repository on Windows would meet the same limit.
- **`worlds/<hex>.json`:** the world manifest after each record. A
  local store keeps only its current `world.json`, and the baseline
  refresh tool reads earlier ones to find where a removed asset was.

## Consequences

- **Accounts:** a bucket, a public URL and a CI secret for the merge bot.
  Someone has to own those accounts and costs.
- **Client:** the client gains an HTTP-backed `op_stream` store that
  verifies every object it downloads against its content id.
- **Mirrors:** anyone can mirror the world by copying the bucket and the
  log repository; the hash chain proves the mirror complete and unchanged.
- **Source archive:** [ADR-0015](0015-zurich-trees-and-lights.md)
  proposes a `sources/` area in the same bucket for raw source answers
  that a published baseline uses, keyed by SHA-256. It is not part of any
  world store, and no world id covers it.
