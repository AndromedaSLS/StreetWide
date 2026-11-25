# ADR-0020: Plan baseline refreshes by comparing a fresh ingest with the canonical world

- **Status:** Proposed
- **Date:** 2026-11-24
- **Deciders:** @victorpicaud
- **Supersedes / superseded by:** none (proposed together with a
  clarification of
  [RFC 0002](../../rfcs/0002-contributions-and-merge-rules.md) section 7;
  adds a note to [ADR-0010](0010-hosting-the-canonical-world.md);
  proposes two rules for the ingest of
  [ADR-0006](0006-ingest-pipeline-and-data-sources.md), which stays
  unchanged)

## Context

[RFC 0002 section 7](../../rfcs/0002-contributions-and-merge-rules.md#7-baseline-refreshes-and-takedowns)
brings newer source data into the canonical world as change sets
authored by `streetwide:ingest`: a *baseline refresh*. Before the
clarification proposed with this ADR, the section settled two rules. An
asset made by the ingest is replaced "when its content changed", and a
contribution is listed "as a conflict for review". Roadmap M1 asks for
the tool that makes these change sets
([roadmap](../roadmap.md#m1-open-for-contributions)).

**What is in place** (on `main` on 2026-11-24, before the work this ADR
proposes):
- **No tool makes refresh change sets.** The ingest builds whole worlds.
- **No tool can merge a change set that touches a collider.**
  `op_world merge` re-runs only `op_world`'s gate, which reports G9 and
  G10 as `not_run` for any collider change. Such a set does not pass,
  and the merge is refused. This also blocks shell and terrain
  contributions.
- **The world log is kept by hand.** `op_world merge` writes the merge
  record and the check report to files the caller names, and keeps no
  copy of the change set.
- **The ingest overwrites its inputs and its output.** `--refresh`
  queries STAC and Overpass again and overwrites the cached answers, so
  the old inputs cannot be ingested again. The ingest then replaces its
  whole output directory, by default `data/world/<name>`.
- **A removal leaves no trace in the world.** After a facade is merged
  and taken down, the world id is the baseline's again. G2 accepts an
  add to any free key, so only the log shows that a key was removed.
- **Any manifest can name the ingest's generator.** RFC 0002 tells a
  baseline asset by its generator, `streetwide-ingest`, and G5 does not
  check generators.

**What the district shows** (zurich-altstadt, counted on 2026-11-24):
- 1,491 assets in 30 tiles, all made by `streetwide-ingest 0.1.0`, none
  with a parent: 1,377 swissBUILDINGS3D shells, 10 OpenStreetMap
  fallback shells, 30 terrain tiles, 66 OpenStreetMap transport assets
  (30 ribbons without a collider and 36 bridge decks) and 8 water
  assets.
- 68 assets are per tile, with the feature id `tile:<tile>`: the
  terrain, the ribbons and the water.
- 114 assets list swissALTI3D, all with the one item
  `swissalti3d_2026_2683-1247`. 76 of them have colliders.
- A fresh OpenStreetMap snapshot changed 76 manifests (the 10 fallback
  shells and the 66 transport assets), and only in the source's
  `version`. No mesh changed.
- The ingest's `VERSION` is still `0.1.0`, as in its first commit,
  although water has been added since.

**Constraints:**
- The canonical world only moves upward, and a better contribution is
  never overwritten by a newer but worse baseline (RFC 0002 section 7).
- Contracts before features: the rules are written before the tool.
- The tool's output must be the same bytes on every platform, although
  the ingest's output is not (ADR-0006, section 9).
- What RFC 0002 does not settle is drafted as options, not decided
  silently.

## Options considered

Each question has a recommended option. None is decided: the
maintainer, @victorpicaud, chooses. Until then, the recommended options
of sections 1 to 9 are the defaults that the refresh tool and its
workflow follow, and this ADR calls those defaults *Proposed*. Q18
(section 9) is the one exception: there the tool implements a stricter
option than the one recommended. The ingest options of section 10 and
the amendments A1 to A8 are not implemented.

### 1. What the tool compares

| Option | For | Against |
|---|---|---|
| **A. A fresh ingest, built into a directory of its own, against the canonical store and its world log (recommended)** | The current store is the only reliable "before", because `--refresh` overwrites the cached answers. Only outputs compare reliably: deck heights, ribbons, fallback building bases and water levels depend on the height model and on neighbouring ways, not on one source record. Tests need only small synthetic stores, and no network. | A second store on disk, about 48 MB for the district. |
| B. An incremental ingest that writes change sets | No second store. | It ties the ingest to the contribution contract and the world log, and drops the whole-world build that ADR-0006 makes reproducible and checks. |

With A, the tool never runs the ingest, and never writes to either
store or to the log. The maintainer runs the ingest into a directory of
its own. The ingest replaces its output directory, so an ingest into the
canonical store's directory would delete that store (section 5).

### 2. Where the tool lives

| Option | For | Against |
|---|---|---|
| **A. A Python planner, `python -m tools.ingest.changes`, with format helpers in `tools/worldfmt`; the Rust gate verifies every set (recommended)** | It sits next to the ingest, as the roadmap says. It follows the facade tool: Python writes bundles, and `op_world` and `op_replay` judge them. Planning needs no Rust build. A short Python mirror of the gate's candidate builder rebuilds the fixture world as it was before water, byte for byte. | About 1,400 lines of Python mirror Rust rules (`changes.py`, `log.py` and the glb reader in `tools/worldfmt`): the candidate world, the change-set and manifest rules, the change order, the terrain-edge rule, the log chain, the merge-record rules and a reader for the glb profile. They can drift from Rust. `verify` runs every set through the real gate and compares every id, and tests pin ids that Rust made. |
| B. A Rust planner, `op_world refresh` | It reuses the gate's code, so ids agree by construction. | The format crate grows a policy tool for one producer, and every planner change needs a Rust build. |

**The name.** `python -m tools.ingest --refresh` already means "query
the sources again", so the new command is not called "refresh". It is a
module of its own, because the ingest's command line requires one of
`--bbox`, `--tile` or `--world` and has no subcommands. The planner
imports only `tools.worldfmt` and the constants in
`tools/ingest/__init__.py`, so it needs neither numpy, shapely nor
pyproj.

**Two commands:**
- `plan` reads the current store, the new store, the log and an
  optional overrides file (section 6). It writes the change sets and a
  refresh report into an empty output directory, and nothing else.
- `verify` runs every set in order through `op_replay check-change` on
  a scratch copy of the store, and checks every world id the plan
  predicted. It ends with a comparison of the pre-refresh world and the
  final one (Q19).

### 3. Merge tooling in `op_replay`

Merging a change set that touches a collider needs the full gate, G1 to
G10. [ADR-0009](0009-simulation-based-gate-checks.md) puts G9 and G10 in
`op_replay`.

| Option | For | Against |
|---|---|---|
| **A. `op_world`'s merge takes the gate as a parameter, and a new `op_replay merge` passes the full gate (recommended)** | One write path. The merge record's `check_report_id` names the full report. No new crate and no new dependency. Shell and terrain contributions can be merged too. | The merge function gains a parameter. |
| B. `op_world merge` accepts a full report and checks that its G1 to G8 agree with its own run | `op_replay` needs no merge command. | Two reports to reconcile. |
| C. Merging only in `op_replay` | One merge command. | The write step is duplicated. |

With A, `op_replay` also gains two commands:
- **`rerecord`** plays a replay's inputs, with the same seed and spawn,
  on a new world. RFC 0002 moves the replay suite to each new world by
  re-recording. G10 uses only replays recorded on the base world, so
  without this command it is `not_applicable` for every set after the
  first.
- **`compare-worlds`** lists probe points on the earlier ways that are
  newly blocked, and replay waypoints more than 2 m off their earlier
  path. Its findings are for review, so it exits 0 when it finds some.

Both merge commands gain two flags:
- **`--log <dir>`** takes the previous record from the log, checks the
  chain, and appends the new record, the change set, the check report
  and the world after (section 4).
- **`--require-base`** refuses a change set whose `base_world_id` is not
  the store's world. It is opt-in. Every command the refresh report
  prints passes it. Making it mandatory for refreshes is amendment A7,
  which is not adopted.

Both merge commands also hold a lock file in the store, and with `--log`
one in the log, from before they read them until they have written them.
A second merge stops when it finds one. Two merges started from the same
world would otherwise leave the store and the log naming different
worlds. `op_replay merge` refuses a `--replay` recorded in another world
than the store's, which G10 would ignore without a word. These are checks
of the tools, not merge rules: they change nothing about which change
sets may be merged.

[ADR-0002](0002-crate-layout-and-dependency-direction.md) already lets
`op_replay` depend on `op_world`, and `op_world` keeps no workspace
dependencies. Neither changes.

### 4. The local log layout

Until [ADR-0010](0010-hosting-the-canonical-world.md) is decided, the
world log lives in a local directory.

| Option | For | Against |
|---|---|---|
| **A. ADR-0010 option A's layout, with `<hex>` in paths and a `worlds/` directory (recommended)** | It works on Windows. It keeps the world manifests that a local store replaces. | It differs from ADR-0010's text, which carries a note about it. |
| B. ADR-0010's layout as written | Nothing to explain. | `changes/<change-id>/` contains a `:`, which Windows paths do not allow, and earlier world manifests are lost. |

```text
<log>/log/00000000.json          merge records in log order; the first is the baseline
<log>/changes/<hex>/change.json  the change set of every merged or takedown record
<log>/reports/<hex>.json         check reports
<log>/worlds/<hex>.json          the world manifest after each record
```

`<hex>` is the content id without `blake3:`. The tool reads `worlds/`
only to find where a removed asset was, once its manifest has been
erased.

**The log is required.** Without it, a refresh would add back what a
takedown or a contribution removed ("What is in place"). A log exists
as soon as anything has been merged, since every merge names the
previous record. A world that has never had a merge or a takedown first
gets its baseline record from `op_world baseline`, as RFC 0002 asks. A
baseline record written after such a merge would hide a removal, so it
must never be written late.

`plan` checks that every record follows RFC 0002's rules, that the
chain verifies and ends at the current world, and that every change set
is present and hashes to its id.

### 5. The workflow

| Question | Options | Recommended |
|---|---|---|
| Where the canonical store lives | (a) outside `data/world/`, holding a marker file such as `CANONICAL`; (b) anywhere | (a). The ingest replaces its output directory, by default under `data/world/`. It refuses to replace a directory that holds anything besides `world.json` and `objects`, so the marker protects the store. The tool also refuses a `--current` and a `--new` that name the same store. |
| Which platform ingests | (a) the platform of the last baseline; (b) any, once the ingest gives the same bytes on every platform | (a). Across platforms the ingest can move a vertex by 1 mm (ADR-0006), which would look like a change. (b) is out of scope. |
| Other merges during a refresh | (a) a refresh window: no other merge between the sets of one refresh, and if one happens, plan again; (b) rebase the remaining sets by hand | (a). Planning is cheap, and the same inputs give the same bytes. `--require-base` in the printed commands stops a set planned on an older world. |
| The order of sets | (a) visual-only sets first, then by layer and source, decision sets last; (b) collider sets first | (a). Visual-only sets pass `op_world`'s gate alone, and new ribbons merged first keep G9's probes on ways that exist upstream. Q19 records what this misses. |
| Bundle size | (a) a `--max-set-bytes` option, with no limit by default; (b) a fixed limit now | (a). 8000000 matches the 8 MB per pull request that ADR-0010 suggests, once pull requests carry bundles. |

**How sets are made:**
- Changes that must go together form a *unit*: the two halves of a
  move, and removals and additions that overlap (Q7, Q8). A unit is
  never split across sets.
- Each unit has a purpose: its collider (`trimesh` if any version in it
  has one, else `none`), its layer and its source dataset. Purposes are
  ordered `none` before `trimesh`, then by layer and dataset in byte
  order. In the district: ribbons, then water; then OpenStreetMap
  shells, swissBUILDINGS3D shells, terrain and bridge decks.
- Units are packed in that order, and by key within a purpose. A new
  set starts when the purpose changes, or when the next unit would pass
  50 changes or the byte limit. A swissBUILDINGS3D release that changed
  every building in the district would give 28 sets.
- A change to a contribution that the maintainer accepts goes into a
  *decision set* of its own, after the ordinary sets (section 6).
- Each set's `base_world_id` is the world after the sets before it. The
  report lists each set's base and the world it predicts.

**For each set, in order,** the maintainer:
1. runs `op_replay check-change` with the registered replays;
2. reviews the set against the refresh report;
3. merges it with `op_replay merge --require-base --log`, citing the
   report's content id in the reasons;
4. re-records the replays on the new world, and goes on with the next
   set.

### 6. The overrides file

| Option | For | Against |
|---|---|---|
| **A. A JSON file of holds, scoped accepts and removal allowances, each with a reason (recommended)** | Every exception is written down with its reason. The report records the content id of the file's document in canonical JSON, so line endings and layout do not change it, and each merge record cites the report's. An accept names the exact versions it was written for, so it never applies to newer upstream data or to a later contribution. | One more file to keep with each refresh. |
| B. Command-line flags, such as an earlier draft's `--allow-removals` | Quick. | No reason is recorded, and nothing limits a flag to the versions it was meant for. |

With A:
- **A hold** keeps a change out. With a content key, it lasts until the
  upstream content changes, for example "reviewed: the contribution
  stays".
- **An accept** names the current manifest id and the new content key
  (null where a version is absent), and the codes it lifts. It applies
  only while both still match. A key that is both held and accepted is
  an error.
- **An allowance** lifts the removal guards for one ingest group, up to
  a stated number of removals. It also clears the error for a group
  that vanished.
- **Stale entries** are ignored and listed in the report.
- **An accepted change to a contribution** (it replaces or removes one,
  or re-adds what one removed) goes into a decision set of its own,
  with its own review and its own `better` verdict. So it is never
  automatic.

**The hold codes.** Every rule runs on every pending change, and the
report lists every code that applies. Accepts and allowances remove
only the codes they name. A change is emitted when no code is left. A
*conflict* code lists the change under conflicts; the other codes hold
it back.

| Code | Kind | Holds | Lifted by |
|---|---|---|---|
| `takedown` | conflict | any change to a key whose last record is a takedown | nothing (Q18) |
| `contribution` | conflict | a change to a contribution whose upstream is `changed`, `new`, `gone` or `unknown` (Q10) | an accept, in a decision set |
| `origin-unclear` | conflict | a change to an asset whose manifest and log disagree about who made it (Q2) | an accept, in a decision set |
| `removed-by-contribution` | conflict | an add of a key that a contribution removed (Q5) | an accept, in a decision set |
| `dependent-contribution` | conflict | a mesh change, move, rename or removal of a shell that a contributed facade sits on | nothing (A2) |
| `possible-duplicate` | conflict | an add, or a replace whose bounds grow, that newly overlaps, in its layer, a contribution that stays, or a feature that a contribution or a takedown removed | an accept |
| `removed-bounds-unknown` | conflict | every add, and every replace whose bounds grow, in the same layer and tile as a key that a contribution or a takedown removed, when that key's bounds cannot be read | an accept |
| `tile-not-in-world` | held | an add into a tile the world lacks; G2 would fail it | nothing (A5) |
| `terrain-edges` | held | a terrain replace that moves a vertex of its open edges; G8 would fail it | nothing (A1) |
| `height-coupled` | held | every change draped on the same height-model items as a terrain change, when any one of them is held, in both directions; and every change draped on the height model over a contributed terrain that stays (Q17) | an accept |
| `per-tile-removal` | held | any removal of a tile's terrain, ribbons or water (Q4) | an allowance or an accept |
| `mass-removal` | held | removals in an ingest group that no add, move or replace of their unit accounts for, above max(2, ceil(5 % of the group)) (Q4) | an allowance or an accept |
| `unexplained-change` | held | changed content with the same sources, versions and generator: another platform, an ingest change without a `VERSION` bump, or an item republished under the same id | an accept |
| `manual-hold` | held | a hold in the overrides file | removing the hold |
| `unit-held`, `unit-too-large`, `would-empty-tile` | held | another member of the unit is held; the unit has more than 50 changes; the removals would empty a tile, after the whole refresh or after their own set, since each set is merged on its own | nothing |
| `spawn` | held | terrain that rises to within 100 mm of the spawn's feet, or a collider that newly reaches into the player's space at the spawn (300 mm around the feet, from 100 mm below them to 1.8 m above), a collider switched on in a manifest included | nothing (A5) |

A shell under a contributed facade changes only after the facade is
removed or moved by a change set of its own, or once A2 is decided.

### 7. The ingest's `VERSION`

| Option | For | Against |
|---|---|---|
| **A. Bump `tools/ingest`'s `VERSION` whenever its output changes (recommended)** | A changed mesh with unchanged sources and generator then points to noise or republication, and the tool holds it (`unexplained-change`). The tool can refuse a new store made by an older ingest. | One more step in every ingest change. |
| B. No rule | Nothing to remember. | The generator says nothing about the output: water was added without a bump. |

ADR-0006 is accepted and stays unchanged, so this rule is proposed here
(see Consequences).

### 8. RFC 0002's text

| Option | For | Against |
|---|---|---|
| **(a) Clarify section 7 in place, with no version bump (recommended)** | The RFC states the rules the tool applies. The text changes no document and no check, and RFC 0002 asks for a new contract version only when its rules change. RFC 0002 is a Draft and has been clarified in place before. | The section grows. |
| (b) Leave section 7 unchanged, and record the rules here only | The RFC stays as it was. | The tool would apply rules that the RFC does not state. |

The clarification is written into the Draft RFC, marked as proposed
together with this ADR, and the RFC's status is unchanged. If (b) is
chosen, the text comes out again.

### 9. What RFC 0002 leaves open

These are the tool's answers to cases that RFC 0002 did not settle. The
clarification of section 7 states the recommended answers. The
numbering has gaps: Q6 and Q9 belong to amendments A2 and A1
("Amendments flagged for RFC 0002"), Q12 to Q16 are answered in
sections 3 and 5, and Q20 and Q21 are in section 10.

#### Q1. What "content changed" means

RFC 0002 says only "when its content changed". Each manifest records
its sources' versions: the OpenStreetMap snapshot time and the
swisstopo years. They change at every release, even where meshes do
not. swissALTI3D also puts the year into its item ids, which the ingest
records as source ids.

| Option | A fresh OpenStreetMap snapshot (measured on the district) | New swissALTI3D item ids, same heights (computed) |
|---|---|---|
| A. The mesh only (`content_id`) | 0 replaces, but fixes to licences, credits and source ids are missed | 0 |
| B. The whole manifest except `parent_version_id` | 76 replaces, 46 of them on colliders | 114 replaces, 76 of them on colliders |
| **C. The mesh and the manifest, except `parent_version_id`, `generator.version` and each source's `version` (recommended)** | 0 replaces; the 76 are counted as provenance-only | 114 replaces: the source ids changed |
| D. Compare per dataset | the least churn, and the least precise | 0 |

**Default until decided:** C. ADR-0015 already assumes it. In the
district one swissALTI3D item covers everything, so a new year changes
the meshes anyway. In a world that spans several items, terrain,
ribbons, decks and fallback shells list all of them, so one item's new
year changes all those manifests. Q21 (section 10) would narrow the
listings.

#### Q2. Who made an asset

RFC 0002 names only the generator, `streetwide-ingest`, which any
manifest can claim.

| Option | Note |
|---|---|
| (a) The generator alone | A contribution that names the ingest's generator would count as the ingest's. |
| (b) The generator, and no `StreetWide contributions` source | G5 requires that source on every contribution. |
| (c) The log: the last record that touched the key is the baseline or a refresh | It shows who made the last change, which no manifest can prove. |
| **All three together (recommended)** | A disagreement gives `origin-unclear`, a conflict. |

**Default until decided:** all three, with the log required
(section 4).

#### Q3. Adds

| Option | Note |
|---|---|
| **Add every feature that is new upstream, with guards (recommended)** | Held when it brings back a feature that was taken down or that a contribution removed, may duplicate a contribution or a removed feature, falls in a tile the world lacks, or buries the spawn. |
| List every add for review | |
| Never add | The world could never gain a building. |

**Default until decided:** add, with the holds `takedown`,
`removed-by-contribution`, `possible-duplicate`,
`removed-bounds-unknown`, `tile-not-in-world` and `spawn`.

#### Q4. Removals

| Option | Note |
|---|---|
| **Remove every asset made by the ingest that is gone upstream, with guards (recommended)** | RFC 0002's remove row names "a demolished building". |
| List removals only | |
| Never remove | Demolished buildings would stay forever. |

**Default until decided:** remove, with three guards:
- every removal of a per-tile asset is held (`per-tile-removal`). A gap
  in the height model or a short upstream answer would remove a tile's
  whole layer, the gate checks no removal for placement, and G9 cannot
  see a missing floor;
- in each ingest group (a layer and a first source), unpaired removals
  above max(2, ceil(5 % of the group)) are held (`mass-removal`). Each
  add, move or replace in a unit accounts for one of its removals, so a
  rename counts 0, and ten buildings replaced by one new building count
  9;
- a group that vanishes entirely stops the tool with an error, until an
  allowance names it.

A shell under a contributed facade is never removed
(`dependent-contribution`).

#### Q5. Removed keys

Nothing in the world marks a key that was removed, and G2 accepts an
add to any free key.

| Option | Note |
|---|---|
| **K1. Read the log and its change sets (recommended)** | No contract change. It needs the log, which section 4 makes required. |
| K2. Tombstones in the world format | An RFC 0001 amendment. |
| K3. Removed keys in the merge record | An RFC 0002 amendment. |
| K4. A maintainer's deny list | Manual: the holds in the overrides file. |

**Default until decided:** K1, with K4 as the fallback.

#### Q7. Moves

A building moves to another tile when the centroid of its footprint
crosses a tile line, and a bridge deck when its middle point does.

| Option | Note |
|---|---|
| **A removal and an add in one unit, so in one set; the lineage starts again (recommended)** | No contract change: an add has no parent. |
| A replace across tiles | An RFC 0002 amendment. |
| The ingest keeps a feature's earlier tile | An RFC 0001 amendment. |

**Default until decided:** a removal and an add in one unit. Stable
entity ids are planned for M3.

#### Q8. Renames, splits and merges

An OpenStreetMap way that is split turns one deck into several. A
fallback building is dropped once swissBUILDINGS3D covers it, often
with several buildings. swisstopo may give a building a new id: nothing
documents whether its ids survive releases.

| Option | Note |
|---|---|
| **Units: removals and adds or replaces that overlap, emitted or held whole (recommended)** | The world never shows both versions, or neither. A hold on any member holds the unit, and a unit over 50 changes is held. |
| Each change on its own | A held removal next to an emitted add leaves a duplicate. |
| Hold every removal that overlaps an add | |

**Default until decided:** units. Two assets overlap when their
horizontal boxes share a positive area that is at least half the area
of the smaller box, in integer millimetres. A box with no area overlaps
nothing.

#### Q10. Which contributions are conflicts

The tool compares the new data with the ingest version that the
contribution replaced. It finds that version by following
`parent_version_id` back to a manifest made by the ingest.

| Upstream | Meaning | Result |
|---|---|---|
| `changed` | the new data differs from that version, a move included | conflict |
| `new` | upstream gained a feature with this key, and the contribution replaced no ingest version | conflict |
| `gone` | upstream lost the feature | conflict |
| `unknown` | a manifest in the lineage cannot be read | conflict |
| `unchanged` | the new data equals that version | listed only |
| `none` | no ingest version, and none upstream, as for a facade | listed only |

| Option | Note |
|---|---|
| **(a) Only the four conflict statuses above (recommended)** | Unchanged upstream data raises nothing to decide. |
| (b) Every contribution whose key the new ingest also has | |

**Default until decided:** (a). Either way the contribution is not
changed: RFC 0002 lists it "as a conflict for review".

#### Q11. Where conflicts are listed

No contract document has a field for them, and change sets and check
reports refuse unknown fields.

| Option | Note |
|---|---|
| **The refresh report, outside the change sets (recommended)** | No contract change. Each set's rationale gives the counts. |
| The change set's rationale | A single line of at most 4096 characters. |
| A new field | A contract change. |

**Default until decided:** the report. `refresh-report.json` is
canonical JSON, so a merge record's reasons can cite its content id,
and `refresh-report.md` says the same for people. Neither is a contract
document, and neither goes into a bundle.

#### Q17. Draped assets over contributed terrain

If a terrain tile is a contribution that stays, the new store's
ribbons, decks, fallback shells and water over it are draped on the new
height model, not on the contribution. That is so whether or not
upstream changes the terrain under the contribution.

| Option | Note |
|---|---|
| **Hold every change draped on the height model whose bounds overlap that terrain (`height-coupled`), and with it the height component it belongs to, liftable by an accept (recommended)** | Nothing floats or sinks unseen. |
| Let them through | |

**Default until decided:** hold. Terrain contributions can be merged
with `op_replay merge` (section 3), so the case can arise from now on.

#### Q18. How far a takedown reaches

RFC 0002 lets a takedown "remove or revert" an asset, and says nothing
about refreshes.

| Option | Note |
|---|---|
| (a) Every takedown key stays frozen | What the tool implements until this is decided. |
| **(b) Removals stay frozen; a revert may be lifted by an accept with a reason, in a decision set (recommended)** | A reverted asset can follow upstream again. |
| (c) Every takedown may be lifted by an accept with a reason | |

**Default until decided:** (a), not the recommended (b): nothing lifts
`takedown`. Adding back a key that a takedown removed reverses the
takedown, so it stays a maintainer's change set of its own, never part
of a refresh.

#### Q19. G9 and G10 on a refresh split into sets

G9 takes its probe points from the base world's ribbons, and G10
compares the base world with the candidate only. With visual-only sets
first, a later set's new building over a way removed upstream is never
probed. G10's 2 m tolerance applies per set, so drift can add up over
many sets.

| Option | Note |
|---|---|
| **Per set, visual-only sets first, plus `compare-worlds` between the pre-refresh world and the final one, whose findings are reviewed (recommended)** | The contract works per set; the comparison covers the whole refresh. |
| Collider sets first, so that G9 probes the old ways | A building over a way removed upstream then fails, until A4. |
| Per set only | |

**Default until decided:** the first. `verify` runs the comparison
unless it is told not to.

#### Q22. Who approves a refresh

RFC 0002 says the approving review "may not be the change set's
author". A refresh's author is `streetwide:ingest`, and today there is
one maintainer.

| Option | Note |
|---|---|
| **(a) The maintainer who planned the refresh may approve it while there is one maintainer, naming the planner in the record's reasons (recommended)** | |
| (b) A second reviewer | |
| (c) No refreshes until there are reviewers | It blocks M1. |

**Default until decided:** (a), revisited when
[GOVERNANCE.md](../../GOVERNANCE.md) adds reviewers (RFC 0002,
question 7). No code checks who reviews.

### 10. Ingest options left open

These change the ingest, not the tool, and none is part of this
proposal. Each would change manifest bytes once and need a `VERSION`
bump (section 7).

| Option | What it changes | Choices | Recommended |
|---|---|---|---|
| Q20. Checksums in source versions | A version such as `2026 sha256:<hex>`, and a derived-buildings cache keyed by checksum. Today swisstopo can republish an item under the same id. The ingest then refuses the cached file, whose checksum no longer matches, and after a new download the tool holds the change as `unexplained-change`. A republished building sheet is never parsed again, because that cache is keyed by file name. | now; later | Now. Rule C ignores versions, so a refresh sees no churn from it. |
| Q21. Narrower height-model listings | Each asset lists only the swissALTI3D items its heights came from, and a deck those of every anchor on its bridge. Today terrain, ribbons, decks and fallback shells list every item of the area. | before the first world that spans several items; live with manifest-only replaces | Before the first such world. It changes nothing in the district, where one item covers everything. |
| Refreshing one source at a time | An ingest option such as `--refresh osm`. Today `--refresh` queries every source again, so a new swissALTI3D year holds every OpenStreetMap change draped on the height model until A1. | add it; do without | Add it before the first new swissALTI3D year. |

## Decision

*Proposed:* the recommended option in each section above. **Not
decided:** the maintainer, @victorpicaud, decides, and the decisions
and their reasons are recorded here when made.

Until then, the recommended options of sections 1 to 9 are the
defaults, except Q18, where the tool implements (a). With these
defaults, the tool:
- never runs the ingest, and never writes to either store or to the
  log;
- never replaces or removes a contribution, and never re-adds a key
  that a contribution removed, unless the maintainer accepts that exact
  change with a written reason. Such a change goes into a decision set
  of its own;
- never changes a key whose last change was a takedown;
- never changes the mesh, the tile or the id of a shell that a
  contributed facade sits on, whatever the overrides say;
- holds back and lists any change that RFC 0002 does not settle,
  instead of deciding it.

RFC 0002 section 7 carries these rules as a clarification proposed
together with this ADR (section 8). The RFC stays a Draft.

### Amendments flagged for RFC 0002

Each of these would change a gate rule, a merge rule or the contract,
so none is part of the clarification. None is decided here, and the
tool assumes none of them. They are questions 9 to 16 of
[RFC 0002's unresolved questions](../../rfcs/0002-contributions-and-merge-rules.md#unresolved-questions).

- **A1. Terrain edges (Q9).** G8 would require a replaced terrain to
  meet the candidate world's neighbouring terrain bit for bit, keep the
  world's outer edge, and put terrain tiles that share a changed edge
  into one change set. Today G8 fails any terrain whose open edges
  move, even when the neighbours move the same way. Ingest terrain
  shares its edge vertices with its neighbours, so practically every
  new swissALTI3D year fails. Until then the tool holds such terrain
  and its height component: in the district, a new year holds all 114
  assets that list the item. Recommended: hold now, and amend G8 before
  the first height-model refresh. The other options were stitching the
  old edges in the ingest, which freezes stale heights along every tile
  edge, and `review` instead of `fail`, the weakest.
- **A2. Dependents of a shell (Q6).** G8 would re-check contributed
  dependents when their shell changes: `review` for a refresh, `fail`
  for a contribution. Today G8 checks a facade only when the facade
  itself changes. Until then the tool holds the shell, and nothing
  lifts the hold. Recommended: hold now, amend later. The tool could
  also change the shell anyway, mirror G8's facade rule in Python, or
  replace the facade unchanged under the ingest's name; none of these
  is recommended.
- **A3. No 50-change cap for `streetwide:ingest`.** A swissBUILDINGS3D
  release that changed every building would be 28 sets to review in the
  district. Until then the tool splits refreshes by purpose and at 50
  changes. Recommended: split for M1 and
  M2, and lift the cap for the ingest before M3, as
  [RFC 0004](../../rfcs/0004-point-features.md) recommends.
- **A4. G9 gives `review` instead of `fail` when the blocking asset was
  made by the ingest.** A refreshed building can stand over a way that
  OpenStreetMap still maps. Until then a set that fails this way is
  planned again with the culprit held. Recommended: wait for the first
  real refresh; `compare-worlds` will show whether it happens.
- **A5. Change sets may change the spawn, and add or remove tiles.**
  The ingest puts the spawn 1 m above the height model, and coverage
  can change upstream, but no change set can follow. Until then the
  tool holds what would bury the spawn, and adds into tiles the world
  lacks. Recommended: keep holding, and amend when a world's coverage
  or the ground at its spawn must change.
- **A6. G4 and G5 also run on refreshes.** The gate skips them for
  refreshes, which RFC 0002's G4 and G5 rows do not say. Until then
  refreshes skip them. Recommended: follow
  [ADR-0017](0017-licence-allowlist.md)'s proposal.
- **A7. A change set authored by `streetwide:ingest` merges only on the
  world named in its `base_world_id`.** If the canonical world has
  moved, the refresh is planned again. Without it, a facade merged
  between two sets can end up on a shell that the next set replaces,
  and nothing re-checks it. It narrows RFC 0002 section 2, which lets a
  change set whose parents are still current merge on a newer world. It
  needs the merge commands, not the gate. Until then `--require-base`
  is opt-in, and a merge that forgets it stays possible. Recommended:
  adopt it with the clarification. The other options were keeping the
  flag opt-in, and making G2 fail such a set, which is a gate rule.
- **A8. Holes in the ground.** G8 would give `review` for every terrain
  removal, and G9 would also flag a probe point that had ground below
  it in the base world (a downward ray within 2 m) and has none in the
  candidate. Today removals skip G7 and G8, and G9 casts no ray
  downward, so only the tool's `per-tile-removal` hold stops a hole in
  the ground. Recommended: amend together with A1, since both touch
  G8's terrain rules.

How the contract changes for these is open too: amend RFC 0002 in place
while it is a Draft and raise `GATE_VERSION`, as water did
(recommended), or raise the contribution contract version.

### Left open by this ADR

- The amendments above, and the ingest options of section 10.
- Hosting, submissions and a merge bot (ADR-0010).
- Running or scheduling the ingest, stable entity ids across sources,
  and carrying contributions across renames (M3).

## Consequences

- **Contracts:** no format, schema, gate rule, `GATE_VERSION` or
  contract version changes. RFC 0002 section 7 gains the proposed
  clarification, and its unresolved questions gain A1 to A8. The
  50-change cap stays.
- **ADR-0010** carries a note on the local log layout (section 4). The
  log repository of its option A would need the same `<hex>` paths to
  be cloned on Windows.
- **The ingest.** ADR-0006 is accepted, so it is not edited. This ADR
  proposes two consequences for it:
  - bump `tools/ingest`'s `VERSION` whenever its output changes, since
    `unexplained-change` and the tool's generator checks rely on it
    (section 7);
  - ingest a refresh on the platform of the last baseline (section 5).

  Q20, Q21 and refreshing one source at a time would each change the
  ingest, with a `VERSION` bump, if chosen.
- **Work that follows:**
  - in `tools/worldfmt`, mirrors of the candidate world, the change-set
    rules and the log rules, and a strict reader for the glb profile,
    tested against ids that Rust made;
  - in Rust, the merge with a gate parameter; `op_replay merge`,
    `rerecord` and `compare-worlds`; `--require-base` and `--log`;
  - in `tools/ingest/changes`, `plan` and `verify`, tested on synthetic
    stores, on the committed fixtures, through the real gate, and on
    local stores where they exist;
  - a maintainer's guide to refreshes, and notes for contributors: a
    refresh never replaces their work, and the shell under a facade can
    be held.
- **Easier:** shell and terrain contributions can be merged once
  `op_replay merge` exists. With `--log`, every merge keeps the log
  complete, facade contributions included. Anyone with the same inputs
  can plan the refresh again and get the same bytes.
- **Harder:**
  - one review per set: about 28 for a buildings release, until A3;
  - terrain waits for A1. A new swissALTI3D year holds every change
    draped on it, OpenStreetMap edits included, unless the ingest can
    refresh one source at a time;
  - the Python mirror of the gate can drift from Rust. `verify`
    compares every predicted id with the gate's.
- **Risks that remain:**
  - a baseline record written after a removal-only merge hides the
    removal;
  - a merge between sets without `--require-base` can put a facade on
    a shell that the next set replaces (A7);
  - anyone can write `streetwide:ingest` as the author. That turns G3
    into `review` and skips G4 and G5, so the submission workflow
    should take such bundles from maintainers only (ADR-0010);
  - if swissBUILDINGS3D ids change between releases, a buildings
    release becomes removals and additions. Units keep each rename
    whole, but lineage breaks, and every facade on those buildings
    becomes a conflict. Check the next release before relying on
    building refreshes.
- **Revisit when:** A1 to A8 are decided; ADR-0010 is decided; a second
  reviewer joins (Q22); a world spans several swissALTI3D items (Q21);
  stable entity ids arrive (M3).
