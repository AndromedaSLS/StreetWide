# Baseline refreshes: a guide for maintainers

A **baseline refresh** brings newer open data into the canonical world: a
new swisstopo release, or a newer OpenStreetMap snapshot. It arrives as
change sets authored by `streetwide:ingest`, checked by the same gate as
contributions
([RFC 0002 section 7](../../rfcs/0002-contributions-and-merge-rules.md#7-baseline-refreshes-and-takedowns)).

Two commands do the work:
- `python -m tools.ingest.changes plan` compares a fresh ingest with the
  canonical world and its world log. It writes the change sets and a
  refresh report.
- `python -m tools.ingest.changes verify` runs every set through the real
  gate, in order, on a copy of the canonical world, and checks every
  world id the plan predicted.

Neither command runs the ingest, and neither writes to the canonical
store or to its log. You merge the sets yourself, one at a time.

> **Status:** the tools work locally, and the rules they apply are those
> of [ADR-0020](../adr/0020-baseline-refreshes.md), accepted on
> 2026-11-25 with its recommended options. RFC 0002's amendments A1 to
> A8 were decided the same day. A7 is built: a refresh set merges only
> on its base world. Until each of the others is implemented, the tools
> keep the holds described here. See
> [Decisions and what is still to build](#decisions-and-what-is-still-to-build).

On Windows, write `.venv\Scripts\python` where this guide says
`.venv/bin/python`. Keep the plan and scratch directories short, such as
`C:\sw\plan`. Object paths inside them are about 100 characters long, and
Windows refuses paths over 260 characters unless long paths are enabled.

## 1. Where the canonical store and its log live

The **canonical store** is a world store (`world.json` and `objects/`)
that holds the canonical world. Until the hosting that
[ADR-0010](../adr/0010-hosting-the-canonical-world.md) chose is set up,
it exists only on the maintainer's disk.
- Keep it outside `data/world/`. The ingest writes to `data/world/<name>`
  by default, and it replaces that directory as a whole.
- Never give its path to the ingest's `--out`.
- Put an empty file named `CANONICAL` in it. The ingest refuses to
  replace a directory that holds anything besides `world.json`,
  `objects/` and the licence notices it writes, so an `--out` that names
  the store by mistake fails instead of deleting it. `op_world`, `plan`
  and `verify` ignore the file. ADR-0020 proposes this marker.
- Keep the licence notices beside its `world.json`: `NOTICE.txt`,
  `CREDITS.txt`, `ODbL-1.0.txt` and `CC-BY-SA-4.0.txt`
  ([ADR-0019](../adr/0019-odbl-and-the-published-world.md)). The ingest
  writes them with every world; a store made before it did gets them
  by copying them from a new build. They are not part of the world, so
  no id covers them, `plan` and `verify` ignore them, and merges leave
  them as they are. When a new ingest changes them (a new source), copy
  them from its build into the store.

The **world log** is a directory next to the store. It holds every merge
record, with the change set, the check report and the world that go with
each one:

```
<log>/log/00000000.json          merge records in order; the first is the baseline
<log>/changes/<hex>/change.json  the change set of each merge or takedown
<log>/reports/<hex>.json         its check report
<log>/worlds/<hex>.json          the world after each record
```

`<hex>` is a content id without its `blake3:` prefix, because Windows
paths cannot hold a `:`. This layout is a note proposed with ADR-0020 in
ADR-0010. The merge commands keep it when you pass `--log <log>`, and the
planner reads it.

In this guide, `<canonical>` is the store, `<log>` its log, `<plan>` a
plan's directory, and `<suite>` each replay registered for the world.

## 2. Before the first merge: the baseline record

A world log starts with a baseline record. Write it once, before the
first merge or takedown:

```sh
cargo run -p op_world -- baseline <canonical> --log <log> --reason "Baseline of zurich-altstadt, built by tools/ingest 0.1.0"
```

From then on, merge every change set with `--log <log>`, contributions
included, so that the log stays complete.

Write the baseline only for a world that has never had a merge or a
takedown. A merge that only removes something leaves no trace in the
world. A baseline written after it would hide the removal, and a later
refresh would add the feature back.

`plan` refuses to run without a log. The log must verify, and it must end
at the canonical world.

## 3. Ingest into a separate directory

```sh
.venv/bin/python -m tools.ingest --world zurich-altstadt --refresh --out data/refresh/zurich-altstadt-new
```

- **A directory of its own.** The ingest replaces its output directory,
  so give it a new one. The canonical store is the only record of the
  world before the refresh: `--refresh` queries STAC and Overpass again
  and overwrites the cached answers, so the old inputs cannot be
  ingested again.
- **The same platform as the last baseline.** On another operating
  system the ingest can move a vertex by 1 mm
  ([ADR-0006](../adr/0006-ingest-pipeline-and-data-sources.md)). The
  planner would see a change with the same sources and hold it back as
  `unexplained-change`. ADR-0020 proposes this rule.
- **The current ingest.** `plan` refuses a new store made by several
  versions of the ingest, or by an older version than the one that made
  current assets. ADR-0020 also proposes to bump `tools/ingest`'s
  `VERSION` whenever its output changes, since this check and
  `unexplained-change` rely on it.

## 4. Plan

```sh
.venv/bin/python -m tools.ingest.changes plan --current <canonical> --new data/refresh/zurich-altstadt-new --log <log> --out <plan>
```

`<plan>` must not exist, or must be empty. Afterwards it holds:

```
<plan>/refresh-report.json      the report, in canonical JSON
<plan>/refresh-report.md        the same report for people
<plan>/sets/001/change.json     a change set by streetwide:ingest, at most 50 changes
<plan>/sets/001/objects/...     the manifests and meshes the canonical store lacks
<plan>/sets/002/...
```

- **Order.** Sets without colliders come first, then sets with
  colliders, each group by layer and source dataset. Decision sets come
  last (section 6). Merge them in this order.
- **Base worlds.** Each set names the world it was planned on, its
  `base_world_id`, and merges only on that world (section 8). The report
  lists each set's base and the world it produces.
- **Nothing to do.** When nothing changed, `plan` prints `nothing to
  refresh` and writes only the report.
- **Exit codes.** 0 when the plan is written, conflicts included; 2 for a
  usage or input error, with the reason; 1 when one of the planner's own
  checks fails, which is a bug in the planner. Then nothing is written.
  A file that cannot be written, such as a path too long for Windows,
  gives 2, and the files written so far are removed again.
- **`--max-set-bytes N`** also splits sets by bundle size. Use 8000000 if
  bundles will go through pull requests (ADR-0010).

## 5. Read the report

Open `refresh-report.md`. In order, it lists:
1. **Conflicts:** changes that touch a contribution, a takedown or what a
   contribution removed. None of them is made.
2. **Held back:** changes the gate would refuse, or that would leave the
   world inconsistent. Each entry lists every reason, not only the first.
3. **Contributions** that the refresh concerns. A refresh never replaces
   or removes a contribution on its own. It lists it here instead.
4. **World-level differences:** the spawn, which stays where it is, the
   ground under it, and tiles found in one store only.
5. **Stale overrides**, the **change sets**, the **commands** that check
   and merge each set, and an **overrides template**.

Note the report's content id. `plan` prints it, and the report shows it
at the top. Each merge cites it in its reasons.

## 6. Write overrides, and plan again

Most refreshes need no overrides. To change what the report holds, write
an overrides file and plan again with `--overrides <file>`. Every entry
needs a reason.

```json
{
  "overrides_version": "0.1.0",
  "hold": [{"layer": "shell", "feature_id": "osm:way:123", "reason": "..."}],
  "accept": [{"layer": "shell", "feature_id": "swisstopo:swissbuildings3d:...",
              "current_manifest_id": "blake3:...", "content_key": "blake3:...",
              "lifts": ["unexplained-change"], "reason": "..."}],
  "allow_removals": [{"layer": "water", "dataset": "OpenStreetMap", "at_most": 8, "reason": "..."}]
}
```

- A **hold** keeps a change out. With a `content_key`, it lasts until the
  upstream content changes again.
- An **accept** lifts the codes it names, and only while both versions
  still match: the current manifest id and the new content key, null
  where a version is absent. Copy them from the report, or from the
  template at its end, which leaves each reason empty. An empty reason is
  refused, so the template changes nothing until you write one.
- An **allowance** lets one ingest group (a layer and a source dataset)
  lose up to `at_most` assets.
- An entry that no longer matches is ignored and listed under stale
  overrides. A key that is both held and accepted is an error.

An accepted change to a contribution goes into a **decision set** of its
own: one that replaces or removes a contribution, or adds back what a
contribution removed. A decision set gets its own review and its own
`better` verdict.

The report records the overrides file's id: the content id of the
document in canonical JSON. Line endings, a byte order mark, spacing and
the order of fields do not change it, so the same overrides give the
same plan on every platform and in every editor.

## 7. Verify

```sh
.venv/bin/python -m tools.ingest.changes verify --current <canonical> --plan <plan> --scratch <scratch> --replay <suite> --report verify.json
```

`<scratch>` must not exist, or must be empty. `verify`:
1. **builds a work store** in `<scratch>/work`. Its `world.json` is a
   copy, and its objects are hard links to the canonical store's, or
   copies where a link fails. Objects are only ever added, so nothing is
   written to the canonical store;
2. **checks each set in order** with `op_replay check-change`, the full
   gate (G1 to G10). The gate must have run on the set's base world,
   built the world the plan predicts, and passed. G3 must be `review`
   and G4 and G5 `not_applicable`, as for every refresh. Then `verify`
   applies the set to the work store, checks the work world's id, and
   re-records the replays on the new world;
3. **checks the end:** the final world id, `op_world validate`, and a
   comparison of the world before the refresh with the final world
   (`op_replay compare-worlds`).

It exits with 0 when every set passes and every id matches. It exits
with 1 when a set fails the gate, an id differs, the final world does
not validate, or the plan made again differs (`--replan`). It exits with
2 for a usage or input error.

- **`--replay <suite>`**, repeated, names the replays registered for the
  canonical world. Each must have been recorded in that world, or G10
  would ignore it. Leave it out if the world has none.
- **The comparison** lists probe points on the old ways that are newly
  blocked, and replay waypoints more than 2 m off their old path. The
  gate judges each set against its own base only, so this is the one
  check of the refresh as a whole (ADR-0020, Q19). Its findings are for
  review and do not change the exit code. `--no-compare` skips it.
- **`--replan --new <new store> --log <log>`**, with `--overrides <file>`
  if the plan used one, plans again into `<scratch>/replan` and compares
  every file, byte for byte.
- **`--report <file>`** writes a summary in canonical JSON. It names no
  local path, so the same inputs give the same bytes. It must not be
  inside `<scratch>`, whose work store links the canonical objects.
- **`--op-replay` and `--op-world`** replace the default commands,
  `cargo run -q -p op_replay --locked --` and the same for `op_world`,
  for example with binaries you built.

The gate validates the whole candidate world for each set: expect
seconds to tens of seconds per set in the district, and a minute or two
for the comparison.

## 8. Merge in order

Merge the sets one at a time, in the report's order. Its section
"Checking and merging, in order" prints the commands for each set, with
placeholders. For set 1 of 3:

```sh
cargo run -p op_replay -- check-change <canonical> <plan>/sets/001 --replay <suite> --report gate-001.json
cargo run -p op_replay -- merge <canonical> <plan>/sets/001 --require-base --log <log> --reason "Baseline refresh zurich-altstadt, set 1 of 3, refresh report blake3:...; planned by <handle>" --review <handle>=better:<notes> --replay <suite>
cargo run -p op_replay -- rerecord <suite> --world <canonical> --out <suite>
```

- **`op_replay merge`** runs the full gate, G1 to G10. It is the only
  merge command that accepts a set that touches a collider. Sets without
  colliders also pass `op_world merge`, but `op_replay merge` takes them
  all.
- **A set merges only on its base world.** Both merge commands refuse a
  change set authored by `streetwide:ingest` whose `base_world_id` is
  not the world in the store: "the store holds world ..., but this
  change set was planned on ...; plan the refresh again". They do so
  with or without `--require-base` (RFC 0002 section 7, amendment A7).
  The flag applies the same rule to any change set, and the printed
  commands still pass it, as ADR-0020's workflow does.
- **`--log <log>`** appends the merge record, the change set, the gate's
  report and the new world to the log. The merge checks that the log can
  take them before it writes the store: a log that cannot (a link inside
  it, a file that holds other bytes where the merge writes) is refused
  with nothing written. No path may be empty: `--log ""`, which an unset
  variable gives, is refused.
- **One merge at a time.** A merge takes a lock file, `merge.lock` in
  the store and `log.lock` in the log, before it reads either, and keeps
  them until it has written the store and the record. A second merge
  that finds one stops before it writes anything. If a merge was
  interrupted and left a lock behind, check that no merge is running,
  then delete the file.
- **A merge that stopped between the store and the log.** If a merge is
  killed after it wrote the store and before it appended its record,
  the store is one world ahead of the log, and every later merge refuses
  with "the store holds world ..., but the log's last record ends at
  ...", followed by what to do. Put the store back as it says: copy the
  log's `worlds/<hex>.json` of the log's last world over the store's
  `world.json` (the objects the stopped merge copied in can stay). Then
  merge the set again. A write into the log that fails during a merge
  (a full disk) says the same, with the same advice.
- **Inputs from elsewhere.** Plans, logs, bundles and replays may come
  from someone else. A link, a FIFO or a file over 64 MiB where a file
  should be (a replay may be larger) makes every command stop with exit
  code 2 and one line saying what it refused, as does a link at or under
  a set's `objects/` or anywhere inside a log. A JSON document over the
  limit for its kind (RFC 0001 section 7) is refused too, before it is
  parsed: 1 MiB for an asset manifest or a merge record, 8 MiB for a
  change set, 16 MiB for a tile index or the gate's report.
- **Review.** The gate leaves every change of a refresh to review (G3),
  and a merge needs a review that judges the set `better`. RFC 0002 says
  that the approving reviewer may not be the change set's author.
  ADR-0020's Q22 lets the maintainer who planned the refresh approve it
  while there is one maintainer, naming the planner in `--reason`.
- **Replays.** G10 uses only replays recorded in the set's base world.
  After each merge, re-record the suite on the new world with
  `op_replay rerecord`. `op_replay merge` refuses a `--replay` recorded
  in another world, rather than skipping G10 without a word.

## 9. The refresh window, and planning again

No other merge happens between the sets of one refresh (ADR-0020). If
one does, the canonical world no longer matches the next set's base, and
the merge commands refuse that set, with or without `--require-base`.

Then plan again, from the same new store and the same overrides, against
the canonical world as it is now and its log. The same inputs give the
same bytes, so planning again is cheap and safe. Plan again also when:
- `verify` says that the plan was made on another world than
  `--current`;
- you change the overrides file;
- a set fails the gate, once you have held the culprit.

## 10. What to do for each code

**Conflicts:**

| Code | It means | What to do |
|---|---|---|
| `takedown` | The key's last change was a takedown that removed it. | Nothing, in a refresh. Adding it back is a maintainer's own change set (Q18). |
| `takedown-revert` | The key's last change was a takedown that reverted it to an earlier version. | If upstream's version is right again, accept it with a reason: the change goes into a decision set of its own, reviewed on its own (Q18 (b)). |
| `contribution` | A contribution holds the key, and upstream changed, gained or lost the feature since the version the contribution replaced. | Review it. The contribution stays either way. To record that you reviewed it, hold the key with its `content_key`: the entry then also shows `manual-hold`, until upstream changes again. To take the new data, accept it with a reason: it goes into a decision set. |
| `origin-unclear` | The manifest and the world log disagree about who made the asset. | Check the log. Accept the change with a reason only once you know: it goes into a decision set. |
| `removed-by-contribution` | A contribution removed this key, and upstream still has the feature. | Usually, leave it out. If the removal was wrong, accept the add with a reason: it goes into a decision set. |
| `dependent-contribution` | A contributed facade sits on this shell, and the refresh would change the shell's mesh, move it, rename it or remove it. | Nothing lifts this. Ask the facade's author to move or remove the facade with a change set of their own, or wait for amendment A2. The next refresh then changes the shell. |
| `possible-duplicate` | A new feature, or a feature whose bounds grow, newly overlaps a contribution or a feature that a contribution removed, in the same layer (by at least half of the smaller one's area), or touches at all a feature that a takedown removed. A feature that upstream merged into its neighbour would come back inside it. | Check that it is not the same feature coming back, under a new id or inside another one. If it is not, accept it with a reason. |
| `removed-bounds-unknown` | A contribution or a takedown removed a feature in this tile, and that feature's bounds can no longer be read. Every add, and every replace whose bounds grow, in that layer and tile is held. | Check the change against that removal. If it is another feature, accept it with a reason. |

**Held back:**

| Code | It means | What to do |
|---|---|---|
| `tile-not-in-world` | The add is in a tile the world lacks. | Nothing: change sets cannot add tiles (amendment A5). |
| `terrain-edges` | The new terrain moves vertices on the tile's open edges, which G8 fails. | Nothing, until amendment A1. Everything draped on the same height-model items waits with it (`height-coupled`). |
| `height-coupled` | The change is draped on height-model items that a held change uses, or its new mesh is draped on the height model over a contributed terrain (Q17), so it would float or sink. That holds even when you accept the terrain's own change: its decision set merges last, and only if judged better, so the draped changes wait for the plan made after it. | Wait, or accept it with a reason if its heights do not matter. |
| `per-tile-removal` | The change removes a tile's whole terrain, ribbons or water. | Check upstream. If the data is really gone, add an `allow_removals` entry for its group, or accept it. The terrain under the spawn stays held all the same (`spawn`). |
| `mass-removal` | An ingest group loses more than max(2, 5 %) of its assets without anything replacing them. Each add, move or replace in a unit accounts for one of its removals: a rename counts 0, and ten buildings replaced by one new building count 9. | Check upstream for a truncated or vandalised answer. Then add an `allow_removals` entry, or accept. When a whole group vanishes, `plan` stops with an error until an allowance names the group. |
| `unexplained-change` | The content changed, but the sources, their versions and the generator did not. | Find the cause: another platform, an ingest change without a `VERSION` bump, or an item republished under the same id. Ingest again on the right platform, or accept with a reason once it is explained. |
| `manual-hold` | Your overrides file holds it. | Remove the hold to let it through. |
| `unit-held` | Another change of its unit is held. A move, a split or a merge goes whole. | Deal with the held member. |
| `unit-too-large` | Its unit has more than 50 changes. | Nothing: a change set holds at most 50 (amendment A3). |
| `would-empty-tile` | The removals would leave a tile empty, which G1 fails. Sets are merged one after another, so this also holds a removal whose set empties a tile that only a later set fills again. | Nothing: change sets cannot remove tiles (amendment A5). When a later set fills the tile, plan again once this refresh is merged: the removal then goes through. |
| `spawn` | The change would bury the spawn or take the ground from under it, or puts a collider where the player stands at the spawn: 300 mm around the feet, from 100 mm below them to 1.8 m above. The ground is measured under the feet, and meshes triangle by triangle: a building with a courtyard around the spawn is not held, one that encloses the spawn is. A collider switched on in a manifest alone counts. | Nothing: change sets cannot move the spawn (amendment A5). |

**When `verify` fails:**
- **A set fails the gate.** `verify` prints each failing check with the
  gate's details, and goes on with the next sets. For example, G9 fails
  when a building stands over a way that OpenStreetMap still maps. Hold
  the culprit and plan again, and fix the data upstream if it is wrong.
  Amendment A4 would make this a review instead.
- **An id differs.** The planner's Python mirror of the gate disagrees
  with Rust. Do not merge. Report it as a bug, with the plan and
  `verify`'s output.
- **The final world does not validate.** Also a bug: report it.

## Decisions and what is still to build

ADR-0020 was accepted on 2026-11-25 with the recommended option of each
choice, and RFC 0002 section 7 states these rules (RFC 0002 was accepted
the same day). The decisions that shape a refresh:
- **What changed** (Q1): the mesh and the manifest, except
  `parent_version_id`, the generator's version and each source's
  version. Source ids count, so a release that renames its items changes
  every asset that lists them.
- **Who made an asset** (Q2): the generator, no `StreetWide
  contributions` source, and the world log must all say the ingest.
- **Adds, removals, moves and splits** (Q3, Q4, Q7, Q8): emitted with the
  holds above. A move is a removal and an add in one set, and its lineage
  starts again. Overlapping removals and adds go together.
- **Contributions** (Q10, Q11): a conflict only when upstream changed,
  gained or lost the feature. Conflicts are listed in the report, never
  in the change sets.
- **The shell under a facade** (Q6) and **terrain whose edges move**
  (Q9) are held, until amendments A2 and A1.
- **Draped assets over contributed terrain** (Q17) are held while that
  terrain is in the world, whether or not upstream changes it and even
  when its own change is accepted, and an accept can lift that.
- **Takedowns** (Q18, decided (b)): a removal by a takedown freezes its
  key for good; a revert can be lifted by an accept with a reason, into
  a decision set of its own.
- **A refresh in several sets** (Q19): each set is judged on its own,
  and `verify` compares the whole refresh for review.
- **Who approves** (Q22): the maintainer who planned it, while there is
  one maintainer.

The amendments A1 to A8 (RFC 0002, questions 9 to 16) were decided on
2026-11-25 with their recommendations. A7 is built: the merge commands
refuse a refresh set on any world but its base, with or without
`--require-base` (section 8). It is a merge rule, so the gate and its
version are unchanged. The others are not built yet: until each is, the
tools keep the holds above, the gate is unchanged and the 50-change cap
stays. A1 and A8 come before the first height-model refresh, A3 before
M3. The ingest options Q20 and Q21, and refreshing one source at a time,
were decided too (ADR-0020, section 10) and are still to be built.
