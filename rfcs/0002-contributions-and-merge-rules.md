# RFC 0002: Contributions and merge rules for the canonical world

- **Status:** Draft
- **Authors:** @victorpicaud
- **Created:** 2026-11-23 (comment period until at least 2026-11-30)
- **Contract version affected:** new contribution contract `0.1.0`; world
  format 0.1 unchanged

## Summary

This RFC defines how the canonical world changes:

- what a **contribution** is: a *change set* of new, replaced or removed
  asset versions, made against a known world version;
- the **gate**: automatic, deterministic checks every change set must
  pass, which anyone can re-run;
- what **"beats what is there"** means: no regression on any measured
  property, and an improvement, measured or judged by reviewers;
- how a merge produces the next canonical world and is recorded in an
  append-only, hash-chained **world log**;
- the one exception to "only upward": **takedowns** for privacy and
  legal reasons.

It also proposes which layers accept contributions first. Facades come
first because they are visual only.

## Motivation

The project's first principle is that the canonical world only moves
upward: every merge must pass the checks and beat the current version
(CLAUDE.md, GOVERNANCE.md). GOVERNANCE.md says the concrete merge criteria
are defined by RFC. Until now nothing defines them.

The world format (RFC 0001) already provides the building blocks:
- one asset per feature per layer;
- `parent_version_id` lineage;
- content ids that pin every byte;
- licence and provenance on every asset.

This RFC turns those into a process that:

1. **contributors can rely on:** they know in advance what will be
   accepted;
2. **anyone can verify:** the checks are deterministic, and every merge is
   recorded with its reasons;
3. **scales from one maintainer to a community** without changing the
   contract;
4. **keeps the world legal and private:** licences are never mixed
   wrongly, interiors stay fictional, and takedowns are possible.

## Design

### 1. Terms

| Term | Meaning |
|---|---|
| **Canonical world** | The world StreetWide publishes. Each version is a world manifest (RFC 0001), named by its world id. |
| **Asset version** | One manifest plus its mesh: one layer of one feature in one tile. |
| **Current version** | The version of `(tile, layer, feature_id)` that the canonical world lists now. |
| **Change set** | A proposal: a list of asset additions, replacements and removals, made against one world version. |
| **Gate** | The automatic checks (section 4). Deterministic: the same inputs always give the same report. |
| **Review** | Human judgement where measurement is not enough (section 5). |
| **Merge record** | The decision about a change set and its evidence, appended to the world log (section 6). |

### 2. Change sets

A change set is a canonical-JSON document (RFC 0001 §5) with
`format: "streetwide.change"`. Its id is the BLAKE3 content id of its
bytes. It travels with the new manifests and meshes it references,
forming a *bundle*: a directory with the change set as `change.json` and
the new objects in the world-store layout (`objects/<aa>/<hex>.json|glb`,
RFC 0001 §7). (The JSON examples in this RFC list keys in reading order; the
canonical form sorts them.)

```json
{
  "format": "streetwide.change",
  "contract_version": "0.1.0",
  "base_world_id": "blake3:…",
  "title": "Facade of a house on the Neumarkt",
  "author": "github:octocat",
  "rationale": "Street-level facade modelled from my own photos (CC BY-SA 4.0).",
  "changes": [
    {
      "op": "add",
      "tile": "17/68647/45902",
      "layer": "facade",
      "feature_id": "swisstopo:swissbuildings3d:0b6f…",
      "parent_version_id": null,
      "manifest_id": "blake3:…"
    }
  ]
}
```

**Fields:**
- `base_world_id`: the world version the change set was made against.
- `author`: a public handle, such as `github:<login>`. Never an e-mail
  address.
- `title`, `rationale`: text, explaining what and why for reviewers.
- `changes`: one entry per affected asset, sorted by
  `(tile, layer, feature_id)` in byte order, at most one entry per
  `(tile, layer, feature_id)`.

| `op` | `parent_version_id` | `manifest_id` | Meaning |
|---|---|---|---|
| `add` | `null` | new manifest | The feature has no asset in this layer yet. |
| `replace` | the current version | new manifest | A new version of an existing asset. The new manifest's own `parent_version_id` must equal this field. |
| `remove` | the current version | `null` | The asset should no longer exist, e.g. a demolished building or a duplicate. |

**Atomicity.** A change set is applied whole or not at all. It should stay
small and coherent: at most 50 changes, all serving one stated purpose.

**Concurrency is optimistic.** Every `replace` and `remove` names the
version it expects. If the canonical world has moved on since
`base_world_id` and any named version is no longer current, the change set
is **stale**. The contributor rebases it: new parents, re-run gate. A change
set whose parents are all still current can be merged on top of a newer
world without rebasing, after the gate runs against that newer world.

### 3. Which layers accept contributions (contribution contract 0.1)

| Layer | Contributions in 0.1 | Collider | Notes |
|---|---|---|---|
| `facade` | **yes, first** | `none` | Visual only, so it cannot change the simulation. Must lie on the building's shell (section 4). |
| `shell` | yes, with evidence | `trimesh` | Only with better measured data than the baseline (section 5). |
| `terrain` | yes, with evidence | `trimesh` | Tile edges must stay identical to the neighbouring tiles, so the terrain stays crack-free. |
| `interior`, `furniture` | later (RFC needed) | — | Always `fictional: true`. Needs rules for doors and the walkable connection to the street first. |
| `interactivity` | later (RFC needed) | — | No interaction module exists yet. |
| `transport` | **no: fix it in OpenStreetMap** | — | OSM-derived (ODbL). Improvements belong upstream; the next baseline refresh brings them in. |
| `shell` from the OSM fallback | **no: fix it in OpenStreetMap** | — | Same reason. |

Contributions to swisstopo-derived assets that correct an error should
also be reported to swisstopo. The contribution can be merged meanwhile.

### 4. The gate

The gate builds the candidate world: the base world with the change set
applied, through a new world manifest and new tile indexes. It then runs
the checks below. Every check is deterministic, and the gate writes a
canonical-JSON **check report** (`format: "streetwide.check"`). The report
contains:
- the `gate_version`;
- the `change_id`, the `world_id` it ran against, and the
  `candidate_world_id` (`null` if the candidate could not be built);
- one entry per check, `G1` to `G10`, with an `outcome` and `details`.

The report's id is its content id.

Each check has one of five outcomes:
- `pass`: everything it checked is fine;
- `fail`: a rule is broken;
- `not_applicable`: nothing in the change set concerns it;
- `not_run`: it applies, but could not run;
- `review`: the decision is left to the reviewers of section 5.

A change set **passes the gate** (`passed: true`) only if every outcome is
`pass`, `not_applicable` or `review`.

| # | Check | Rule |
|---|---|---|
| G1 | Format | The candidate world validates completely (`op_world validate`, RFC 0001), and every new document is canonical and schema-valid. |
| G2 | Lineage | Every `parent_version_id` is current in the world the change set is applied to (section 2), and every `add` targets a free `(tile, layer, feature_id)`. |
| G3 | Layer policy | Only layers open to contributions (section 3) are changed, with the collider that section prescribes. A baseline refresh (author `streetwide:ingest`, section 7) may change any layer; its outcome is `review`, because a maintainer reviews it. |
| G4 | Licence | **Contributed geometry:** licensed `CC-BY-SA-4.0`. **Reused geometry:** an asset that reuses geometry from another source (e.g. vertices copied from a swisstopo shell) lists that source and states the combined licence (e.g. `CC-BY-SA-4.0 AND LicenseRef-swisstopo-OGD`). **ODbL:** ODbL-derived geometry is never combined with CC BY-SA content (ADR-0001). **Forbidden sources:** proprietary imagery and maps (e.g. Google, Apple, Bing street-level or aerial imagery), copyrighted plans, and anything without an open, compatible licence. |
| G5 | Provenance | Every new manifest lists its sources: a source entry `StreetWide contributions` whose `source_ids` contain the author's handle, plus every upstream dataset used. Its `generator` names the tool and version (including AI tools, see section 8). Its attribution includes `The StreetWide contributors` and every upstream credit. |
| G6 | Privacy | `fictional: true` for interiors and furniture (already in RFC 0001). Facades show only what is visible from public ground, and never: people, vehicle number plates, name plates on doors, bells or letterboxes, or the inside of buildings. In format 0.1 meshes carry no textures, so for every added or replaced facade the outcome is `review`, backed by the automatic placement check (G8). |
| G7 | Budgets | Per asset, by layer: facade ≤ 50,000 triangles and ≤ 4 MiB; shell ≤ 100,000 triangles. Per tile: ≤ 1,000,000 rendered triangles and ≤ 300,000 collider triangles after the change. |
| G8 | Placement | **Facades:** every vertex is within 0.5 m of the building's shell in the candidate world (same feature id and tile) and at most 10 mm above the shell's highest point. Heights are compared in exact millimetres. A facade without a shell fails. A facade over its G7 budget, or whose shell is over the shell budget (meshes G7 refuses), is `not_run`, and a facade that would take more than 20,000,000 steps to measure (grid steps and vertex-to-shell distances; the sample takes about 3,600) fails as too complex to check, so that no mesh can keep the gate busy. **Shells:** a replaced shell keeps its footprint (horizontal bounds) within 1 m of the previous version's; a larger move, or a new building, is `review`. **Terrain:** a replaced terrain mesh keeps every vertex on the open edges of the previous mesh at exactly the same position, so neighbouring tiles still meet; new terrain is `review`. |
| G9 | Walkability | Public space must stay walkable. On the transport ribbons within 2 m of the changed assets, the gate places the player capsule (ADR-0004) at probe points 1 to 2 m apart. A point is blocked if the capsule overlaps a collider, or if it is enclosed (rays in the four horizontal directions and up all hit within 30 m). A probe point that was free in the base world must still be free in the candidate world. This applies to collider changes only; facades are `none` in 0.1. Implemented in `op_replay` (ADR-0009). |
| G10 | Simulation | Every recorded replay of the CI replay suite whose world is the base world, and whose path crosses an affected tile, is re-recorded on the candidate world. It must still reach its recorded waypoints. Hash equality is not expected: the world changed. |

Numbers in G7 and G8 are starting values (see Unresolved questions).

### 5. "Beats what is there"

Passing the gate is necessary, not sufficient. A change set is merged only
if **every change in it beats the version it replaces** (or, for `add`,
beats having nothing). A change beats the current version when:

1. **No measured regression.** For every metric the gate measures for its
   layer, the candidate is not worse than the current version beyond the
   metric's tolerance; and
2. **An improvement**, either:
   - **measured:** at least one metric improves beyond its tolerance; or
   - **judged:** the required reviewers (below) judge the change better,
     using the layer's checklist.

**Metrics** in contribution contract 0.1:

| Layer | Metric | Better is |
|---|---|---|
| `shell` | Root-mean-square distance from LiDAR roof and wall points (swissSURFACE3D, open data) to the shell surface | lower |
| `terrain` | Root-mean-square distance from LiDAR ground points to the terrain surface | lower |
| `facade` | none measured in 0.1: judged only | — |
| all | Triangle count, mesh bytes | not a criterion, only bounded by G7 |

swissSURFACE3D would be a new data source. It needs its own ADR (like
ADR-0006) and an ATTRIBUTION.md entry before it is used.

**Reviewer checklists** (judged layers):

- **Facade:**
  - it matches the real building's public appearance (openings,
    proportions, storeys, roof edge);
  - it is consistent with its neighbours in level of detail;
  - it respects G6;
  - its sources are plausible and open.
- **Shell or terrain with evidence:** the evidence (open data, survey
  measurements) supports the change, and the metrics agree.

**Required reviews.**

- **Now:** with one maintainer, one approving maintainer review, which
  may not be the change set's author.
- **Later**, when there are enough reviewers:
  - two approvals from reviewers with merge rights for the layer
    (GOVERNANCE.md roles);
  - one of them must be a maintainer for `shell` and `terrain`, which
    affect the simulation.
- A reviewer may **block** with a reason (licence or privacy doubt). A
  block stands until resolved.

### 6. Merging and the world log

**Merging** a change set that passed the gate and beats the current
versions:

1. Writes its new objects into the canonical world store. The store is
   append-only and content-addressed, so nothing is ever overwritten.
2. Builds the new tile indexes and world manifest. This is exactly the
   candidate world of the gate run.
3. Appends a **merge record** to the world log.

**Merge record** (canonical JSON):

```json
{
  "format": "streetwide.merge",
  "contract_version": "0.1.0",
  "decision": "merged",
  "change_id": "blake3:…",
  "world_before": "blake3:…",
  "world_after": "blake3:…",
  "check_report_id": "blake3:…",
  "reviews": [
    { "reviewer": "github:…", "verdict": "better", "notes": "…" }
  ],
  "reasons": "…",
  "previous_record_id": "blake3:…"
}
```

- `decision` is `merged`, `baseline` or `takedown`.
- `reviews` is sorted by reviewer; `verdict` is `better`, `not_better` or
  `blocked`. A `merged` record needs at least one `better` verdict and no
  `blocked` one.
- The first record (`baseline`) has `null` for `change_id`,
  `world_before`, `check_report_id` and `previous_record_id`. Every other
  record has all four.

The world log is the chain of merge records:
- each record names the previous record's id;
- its `world_before` equals the previous record's `world_after`;
- the **current canonical world** is the `world_after` of the last
  record.

Anyone can verify the whole history: every id is a content id, every
check report can be re-run, and every world version stays available.
Rejected change sets are not logged; their discussion lives with the
proposal (e.g. the pull request).

Merges are applied one at a time, in log order. Several change sets that
touch disjoint assets may be merged in one record. Its `change_id` then
names a change set listing all of them, so the record still has one gate
run.

### 7. Baseline refreshes and takedowns

**Baseline refresh.** When `tools/ingest` is run with newer source data
(a new swisstopo release, a newer OpenStreetMap snapshot), its output is a
change set authored by `streetwide:ingest`:
- **An asset whose current version was itself produced by the ingest**
  (its `provenance.generator.name` is `streetwide-ingest`) is replaced
  when its content changed. Newer upstream data counts as an
  improvement, provided the gate passes.
- **An asset whose current version is a contribution** is not replaced
  automatically. The refresh lists it as a conflict for review, so a
  better contribution is never overwritten by a newer but worse baseline.

**Takedown.** A maintainer may remove or revert an asset immediately,
without the "beats" rule, for:
- a privacy complaint, e.g. an owner asking for a facade to be replaced by
  a generic one;
- a licence or copyright problem;
- a legal order.

The takedown is a merge record with `decision: "takedown"` and its reason.
It still passes G1 and G2. This is the only way the canonical world moves
other than upward. The affected objects remain in the store but are no
longer referenced. Physically erasing them, if legally required, is an
operational step outside the log.

### 8. AI-generated contributions

Content produced or assisted by AI tools is welcome under the same rules.
The manifest's `generator` names the tool and version, and its sources
list the inputs (G5). The contributor certifies under the DCO that they
have the right to submit it under CC BY-SA 4.0. Inputs that G4 forbids
stay forbidden when an AI tool processes them.

## Options considered

**Where the canonical world and contributions live.**

1. *Documents in Git, objects in a content-addressed object store*
   (proposed):
   - Change sets, merge records and check reports are small and go
     through pull requests, with DCO and review.
   - Meshes live in an append-only store addressed by content id, which
     respects "no large data in Git".
   - Hosting the store is an ADR decision (S3-compatible bucket, GitHub
     release assets, IPFS).
2. *Everything in Git (with Git LFS).* Simple, but repositories with
   millions of meshes do not scale, and LFS quotas cost money.
3. *A dedicated service with its own database.* Scales, but needs
   operating, and the history is harder for anyone to verify than a hash
   chain.

**How "better" is decided.**

1. *Automatic score only.* Objective where reference data exists, but
   blind to appearance and easy to game where it doesn't.
2. *Human review only.* Simple and flexible, but subjective and slow, and
   it cannot see measurable regressions.
3. *Gate, plus no-regression metrics, plus review where measurement ends*
   (proposed). Measurable properties never get worse; reviewers decide
   only what cannot be measured. The review rules can grow with the
   community without a format change.
4. *In-game voting and A/B tests.* Scales with players, but needs players,
   anti-abuse measures and a network layer that don't exist yet. It is a
   candidate input for a later version of section 5.

**Granularity.** Single-asset contributions are simplest to judge but
cannot express "this facade and these two neighbours together". Change
sets with per-asset judgement (proposed) keep the judgement simple and
allow coherent edits.

**OSM-derived layers.** Editing them in StreetWide would fork
OpenStreetMap, put StreetWide's own edits under ODbL, and lose them at the
next refresh. Upstream-first (proposed) improves OSM for everyone, and the
refresh brings the fix in.

**Facades with colliders.** Allowing balconies or arcades that collide
would make facade changes affect the simulation, and need G9 and G10 for
every facade. Visual-only facades (proposed) keep the first contributable
layer risk-free; colliders can come with a later contract version.

## Compatibility and migration

- **World format:** unchanged (0.1). Change sets, check reports and merge
  records are new document types with their own contract version
  (`0.1.0`), schemas and validation.
- **First log record:** the current Zurich world becomes the first record
  (`decision: "baseline"`).
- **Replays:** they stay checkable forever. They name their world id, and
  the store keeps every world version. The CI replay suite (G10) moves to
  each new canonical world by re-recording, never by editing hashes.
- **Changing these rules** (thresholds, layers, review counts) needs an
  RFC that bumps the contribution contract version. A change set states
  the version it was made under; a merge applies the rules of the version
  current at merge time.

## Determinism, provenance and privacy

- **Determinism:** the gate is deterministic, so anyone can re-run a merge
  decision's checks and get the same report. Changes that reach the
  simulation (colliders) are covered by G9 and G10. Facades cannot reach
  it in 0.1.
- **Provenance:** every merged asset names its author's handle, its
  upstream sources and its generator. Every merge names its reviewers and
  reasons. The collective credit "The StreetWide contributors" satisfies
  CC BY-SA attribution in the client; individual credits stay in the
  manifests and the log.
- **Privacy:** contributors appear by public handle only. Facades are
  limited to what is publicly visible, with explicit exclusions (G6).
  Interiors stay fictional. Owners can request a takedown.

## Tests and checks

To be built after acceptance:

- JSON Schemas for `streetwide.change`, `streetwide.check` and
  `streetwide.merge`, with Rust and Python agreement tests like those for
  RFC 0001.
- `op_world` gains a change-set module, extending the world-format crate:
  - apply a change set to a store, giving the candidate world;
  - check lineage (G2), layer policy (G3), licence and provenance (G4, G5)
    and budgets (G7);
  - write the check report.
- Placement (G8) and the walkability probe (G9) need geometry and the
  player capsule. Where they live (a new tool in `op_replay`, or a new
  crate) is an ADR decision under ADR-0002.
- CI: a workflow for pull requests that add a change set runs the gate
  and posts the report.
- Golden tests: a small change-set fixture against `fixtures/world`, one
  for each check failing on purpose, and a world-log fixture whose hash
  chain is verified.

## Unresolved questions

1. **Hosting** of the canonical object store and the world log (an ADR).
2. **Thresholds:** the budgets and tolerances in G7, G8 and the metric
   tolerances. They will be tuned on real contributions; until then, the
   numbers above.
3. **swissSURFACE3D** as the reference for shell and terrain metrics needs
   an ingest ADR and a licence entry. Abroad, other references will be
   needed.
4. **Facade law:** freedom of panorama and trademarks (shop signs, public
   art) differ between countries. This needs a legal review before facades
   carry textures; not an issue for untextured meshes in format 0.1.
5. **Contributor credit:** whether the client should list individual
   contributors per area, beyond the collective credit.
6. **Interiors and interactivity:** the rules that would open these
   layers (separate RFCs).
7. **Reviewers:** when the project moves from one maintainer review to
   two-reviewer approval, and how reviewer rights per layer are granted
   (GOVERNANCE.md).
