# ADR-0017: Keep an allowlist of licences for world data and client assets

- **Status:** Accepted (2026-11-25)
- **Date:** 2026-11-24
- **Deciders:** @victorpicaud
- **Supersedes / superseded by:** none (amends [ADR-0001](0001-licensing.md),
  which stays in force)

## Context

[ADR-0001](0001-licensing.md) chose the licences of StreetWide's code and
contributed content, and of its first two data sources. It ends with a
rule: any new data source needs a licence review and an entry in
ATTRIBUTION.md before its data can be ingested. It does not say what a
review may accept. This ADR amends ADR-0001 with that answer. ADR-0001 is
accepted, so it is not edited: it stays in force, and the two are read
together.

The roadmap counts this ADR among the decisions M1 opens with, and as
blocking new sources in M2
([section 8](../roadmap.md#8-decisions-waiting)). M2 brings new data
sources (Zurich's tree register and lighting data, CC0,
[ADR-0015](0015-zurich-trees-and-lights.md); swissSURFACE3D,
[ADR-0016](0016-swisssurface3d-heights.md)) and the first client assets:
sounds, character models and animations, which no licence rule covers.
Later milestones add sources country by country (M6, M7).

**What is in place** (the code on 2026-11-24):
- **G4** lets contributed assets build on four licences, written as
  constants in `op_world`: `CC-BY-SA-4.0`, `CC-BY-4.0`, `CC0-1.0` and
  `LicenseRef-swisstopo-OGD`. It treats a source as forbidden if its name
  or URL contains `google`, `apple`, `bing` or `here.com`.
- **Baseline refreshes skip G4 and G5.** The ingest's data reaches the
  canonical world only as refreshes
  ([RFC 0002](../../rfcs/0002-contributions-and-merge-rules.md)
  section 7), so no check looks at its licences.
- **Validation** checks that a licence is well formed and that its copies
  agree, not that it covers the asset's sources. G4 checks that for
  contributions; for the ingest's assets only a Python test over the
  fixture tile does, although [RFC 0001](../../rfcs/0001-world-format.md)
  lists "licence rules" among validation's checks.
- **The ingest** writes licences from constants in its code and never
  reads what the publisher states.
- **`LICENSES/`** holds CC BY-SA 4.0 and the ODbL. The swisstopo terms are
  not stored, and ADR-0001 and ATTRIBUTION.md link two different swisstopo
  pages.

**What the research found** (2026-11-24, from the publishers' own pages;
no lawyer has reviewed it):
- **Some of the most tempting data is non-commercial or no-derivatives:**
  FABDEM and FathomDEM, EOX's cloudless Sentinel-2 mosaics,
  GlobalBuildingAtlas's LoD1 models and heights, and the LAFAN1 and Bandai
  Namco motion data. Anyone may sell copies of StreetWide's content
  (CC BY-SA 4.0), which NC forbids; ND forbids sharing adapted data.
- **Licences change between releases.** Microsoft's building footprints
  moved from the ODbL to CDLA-Permissive-2.0 on 2026-03-11. Quaternius now
  uses its own asset licence (v1.0, dated 2026-08-28) instead of CC0,
  though some of its pack pages still say CC0.
- **Labels disagree or mislead.** The canton of Zurich's tree crowns are
  CC0 on the city's portal but have no licence id on opendata.swiss. The
  city's tree inventory is labelled CC0 but contains swisstopo points. A
  vegetation height model first noted as CC BY 4.0 is ODbL in its
  publisher's own record.
- **Many terms have no SPDX id:** swisstopo, Copernicus, PLATEAU (Japan),
  Estonia's Land Board, New York City, US federal data, the canton of
  Zurich.
- **Obligations go beyond a credit.** CC BY 4.0 and DL-DE-BY-2.0 require
  changes to be indicated; CDLA-Permissive-2.0 requires its text to go
  with the data; the Copernicus DEM requires a liability sentence that
  every later redistributor passes on.
- **Some royalty-free assets may not be passed on as assets:** Sonniss,
  Quaternius's licence and, with low confidence, Mixamo. StreetWide's
  assets are files anyone can download
  ([ADR-0010](0010-hosting-the-canonical-world.md)).

**Constraints:** every asset carries provenance and licence, and the
canonical world only moves upward, so a licence error must be caught
before it merges, whether it comes from a contribution or a baseline
refresh. A failed check must say why and how to fix it (roadmap, M1).

## Options considered

### A. An allowlist in the repository, enforced in code (recommended)

One machine-readable list: the licences allowed for world data and for
client assets, what each requires, and what is forbidden. The gate,
validation and the ingest enforce it.

- Pros:
  - one answer for the gate, the ingest, contributors and reviewers, with
    each licence's credits and notices written once;
  - baseline refreshes get checked, which nothing does today;
  - a licence change at a source stops the ingest instead of slipping
    through;
  - contributors see in advance what they may use, and a failed check
    says why.
- Cons:
  - code and tests in Rust and Python, which must agree;
  - each new licence waits for a review in a pull request;
  - code sees only what manifests declare: tracing from a forbidden source
    without saying so passes, as under every option;
  - the entries are readings by non-lawyers, and code makes them look more
    certain than they are.

### B. A written policy only

The same list as a page of documentation; reviewers apply it.

- Pros: quick to write; no code; easy to bend for odd cases.
- Cons:
  - the ingest's data stays unchecked, because refreshes skip G4 and G5;
  - it rests on reviewers' memory, and today there is one reviewer;
  - the page and the gate's constants drift apart;
  - the word filter stays. It would fail a contribution that cites Google
    Open Buildings (CC BY 4.0), because its name contains "google".

### C. Case by case, in each source's ADR

No general list. Each source's ADR reviews its licence, as ADR-0006 and
ADR-0014 did.

- Pros: each review sees its source in full; nothing to maintain up front.
- Cons:
  - client assets get no ADRs, so thousands of sounds and clips would have
    no rule;
  - the same licence is reviewed again for each source: CC BY 4.0 alone
    covers 3DBAG and the buildings of Bavaria, Vienna and Helsinki;
  - contributors cannot know in advance what they may use, and an
    accepted ADR cannot be edited when a source changes its licence;
  - nothing is checked in code.

## Decision

*Accepted on 2026-11-25 by @victorpicaud:* option A, an allowlist in the
repository, enforced in code, with the choices recommended within it:
- the lists for world data and client assets below, without CC BY-SA 4.0
  for client assets;
- additions by pull request under a delegation in the RFC that amends G4
  ("Changing the list", (i));
- the gate and validation rules written in a short RFC of their own
  ("Where the gate and validation changes are written");
- CMU's motion capture left out for now, with CMU asked in parallel
  ("Left open by this ADR").

The rest of this section describes option A.

### What a licence must allow

The rules below are option A as accepted. A licence joins a list only if
it allows:
1. use for any purpose, including commercial use;
2. adaptation, and sharing the result as files anyone can download;
3. obligations StreetWide can meet for every later user, without asking or
   notifying the licensor.

Only the versions listed are allowed: another version is another licence.
Whatever is not listed as allowed is refused until reviewed.

### World data

| Licence | Examples | Requires |
|---|---|---|
| `CC0-1.0` | Zurich's tree register and lighting data (ADR-0015); AHN (Netherlands); Luxembourg's 3D buildings | nothing; the publisher's suggested credit is given anyway, e.g. "Quelle: Stadt Zürich" |
| `CC-BY-4.0` | 3DBAG; the buildings of Bavaria, Vienna and Helsinki; Google Open Buildings, under its CC BY 4.0 option; ESA WorldCover | the publisher's credit, a link to the licence, a note of changes |
| `CC-BY-SA-4.0` | contributed content (ADR-0001) | as CC BY 4.0; adaptations stay CC BY-SA 4.0 |
| `ODbL-1.0` | OpenStreetMap; Overture | the source's credit (for OSM, "© OpenStreetMap contributors") and a notice, with a link, that the data is under the ODbL; the derived database, or how it was made, offered free (ODbL 4.6). Kept in its own assets (RFC 0001 section 2); where it may mix with other sources is for ADR-0019 |
| `LicenseRef-swisstopo-OGD` | swissBUILDINGS3D, swissALTI3D, swissSURFACE3D (ADR-0016), SWISSIMAGE | "© swisstopo" |
| `OGL-UK-3.0` | the Environment Agency's LIDAR (England) | the provider's attribution statement, or "Contains public sector information licensed under the Open Government Licence v3.0"; a link to the licence where possible |
| `etalab-2.0` | IGN's LiDAR HD and BD ORTHO (France) | the producer and the date of the last update |
| `DL-DE-ZERO-2.0` | the LoD2 buildings of North Rhine-Westphalia and Berlin; NRW's 1 m terrain | nothing |
| `DL-DE-BY-2.0` | the LoD2 buildings of Hamburg and Baden-Württemberg | the provider, the licence with a link and a note of changes, e.g. "Datengrundlage: LGL, www.lgl-bw.de" with "dl-de/by-2-0" |
| `CDLA-Permissive-2.0` | Microsoft's building footprints, releases since 2026-03-11 | the agreement's text with the data |
| `LicenseRef-PublicDomain-USGov` | USGS 3DEP ("free of charge and without use restrictions") and other US federal data in the public domain | nothing; a citation where the agency asks for one |
| `LicenseRef-Copernicus-DEM-GLO-30-F`, `LicenseRef-Copernicus-DEM-GLO-90-F` | the Copernicus DEM, public instances | the DLR and Airbus notice, in its own form for adapted data; the liability sentence in StreetWide's licence notice; the same obligations for every later user; no implied endorsement |
| `LicenseRef-Copernicus-Sentinel` | Sentinel-2 imagery | "Copernicus Sentinel data [year]", or "Contains modified Copernicus Sentinel data [year]" when modified; users waive claims for damages |

**Contributed assets** may build on every entry except the ODbL, whose
data stays in its own assets and is fixed upstream (RFC 0002 section 3),
and the Copernicus entries. Their terms bind every later user (a liability
sentence, a waiver of claims) and need a closer review before they meet
CC BY-SA content.

### Client assets

Sounds, character models and animations that the client ships or
downloads:

| Licence | Examples | Requires |
|---|---|---|
| `CC0-1.0` | Poly Haven and ambientCG materials and props; Kenney; MakeHuman's assets; Freesound sounds under CC0 | nothing |
| `CC-BY-4.0` | the 100STYLE motion data; Freesound sounds under CC BY 4.0 | the author's credit as asked (e.g. "The 100STYLE Dataset - Ian Mason"), a link to the licence, a note of changes |

CC BY-SA 4.0 is **not proposed** for client assets. It would be lawful,
but the client's code is MIT OR Apache-2.0, chosen in ADR-0001 so the
engine is easy to reuse, and share-alike assets would make the client less
so. The maintainer may add it. *Decided 2026-11-25:* it is not on the
client list.

### Forbidden

| What | Terms | Why |
|---|---|---|
| Non-commercial data: FABDEM, FathomDEM, EOX's cloudless mosaics, GlobalBuildingAtlas's polygons, LoD1 models and heights (its ODbL subset is ordinary ODbL data), Bandai Namco's motion data | CC BY-NC-SA 4.0, CC BY-NC 4.0 | anyone may sell StreetWide's content |
| No-derivatives data: LAFAN1; some of Bavaria's open-data products | CC BY-NC-ND 4.0, CC BY-ND 4.0 | assets are adapted |
| Sonniss's GDC audio bundle | royalty-free, but the sounds may not be supplied "as sound effects to any other person" | StreetWide's assets are downloadable files |
| Quaternius's assets under its Asset License v1.0 | "You may not extract, repackage, sublicense, sell, or otherwise redistribute the Assets" | as above. Packs downloaded under CC0, with a dated copy of that licence, stay CC0 |
| Mixamo characters and animations | Adobe's terms: the raw files may not be distributed (low confidence: a 2022 community post; Adobe's own page could not be read) | as above |
| Google Photorealistic 3D Tiles, and other proprietary map and imagery content such as Street View, Apple's and Bing's imagery and 3D, and Flight Simulator's photogrammetry | the terms forbid storing the content and creating content from it, including 3D objects "extracted, traced, or otherwise derived by hand or machine" | not as a source, a reference to trace, or a cache |
| Restricted instances of the Copernicus DEM (EEA-10, GLO-30-R) | not licensed to the public | not open |

The rule is the licence, not the name: every NC or ND licence is refused,
listed here or not. The gate sees only what is declared: a contributor
who models a facade from Google's 3D Tiles, and lists only their own work,
passes every check. The defences are the DCO, review and the contributor
guide. Forbidden inputs stay forbidden when an AI tool processes them
(RFC 0002 section 8).

### Terms with no SPDX id

- **An id:** `LicenseRef-`, the publisher and the name of the terms. A
  public-domain statement counts as terms.
- **A stored copy** in `LICENSES/<id>.txt`, with the URL and the date it
  was read, so an id means one text. `LicenseRef-swisstopo-OGD` has none
  today.
- **Updates** by pull request when the publisher changes the terms; a
  change that matters gets a new id.
- **Existing ids keep their names:** renaming one would change the bytes,
  and so the ids, of every manifest that carries it.

### What a review checks

- **The licence of the release used,** in the publisher's own record:
  EnviDat's record for the forest inventory's stereo vegetation height
  model says ODbL, not CC BY 4.0 as first noted.
- **What the data is made from,** not only its label: the city's tree
  inventory holds swisstopo points.
- **Terms of use beyond the licence,** which the tools follow: GEDTM30's
  record says "Use for testing purposes only"; Freesound's API terms
  forbid copying its database; Poly Haven's site terms forbid scraping.
- **Privacy:** an allowed licence does not make data private by design.

Data whose terms allow it, used only to check the ingest and never copied
into the world (like the cadastral survey in ADR-0014), needs no entry.

### Enforcement

**The list:** `LICENSES/allowlist.json`, next to the texts, checked by a
schema in `schemas/`, and read by `op_world` and the Python tools with the
JSON parsers they already have. Each allowed entry gives the id and the
path of its text; where it may be used (world data, client assets or
both); whether contributed assets may build on it; the credit, notices and
change notes it requires, with any fixed text such as the Copernicus
notices; and a link to its review. Each forbidden entry says why, so the
gate can give the reason.

**The gate:**
- G4 reads the list instead of its built-in four licences and four words;
  its other rules stay. Every id in an asset's licence and in its sources'
  licences must be allowed for world data, and in a contributed asset also
  for contributions to build on.
- A forbidden licence or source fails with the list's reason; an unlisted
  id fails as "not reviewed yet", with how to ask for a review.
- G4's list check also runs on baseline refreshes. Their licences and
  credits are covered by validation (below), which G1 runs on every
  change set.
- The gate embeds the list when it is built, so one gate version means one
  list and a re-run gives the same report. Changing the list raises
  `GATE_VERSION`. These change RFC 0002's G4, through an RFC (see
  "Changing the list").

**Validation** (`op_world validate` and `tools/worldfmt`), for any world:
- every id in a source's licence appears in the asset's licence, and every
  source's credit in the asset's attribution;
- an asset's licence joins ids with `AND` only, and each source names the
  one licence it is used under (Google Open Buildings: CC BY 4.0 or the
  ODbL, chosen at ingest).

G1 validates every candidate world, so these rules reach baseline
refreshes too. As format rules they amend RFC 0001 section 2, in the same
RFC as G4. The list stays out of validation, so a private world built for
research may hold other data; only the canonical world is held to the
list, by the gate and the ingest.

**The ingest:**
- Each source definition names the licence it expects, where the
  publisher states it and in what words: a STAC `license` field, a CKAN
  `license_id` (`cc-zero` for the city's data), the copyright note in
  every Overpass answer (it names the ODbL), a LICENSE file, or a terms
  page.
- At download it reads that statement for the release it pins, stops if
  the licence differs from what it expects or is not allowed, and writes
  it into each source entry, as it writes the release into `version`.
- The statement is cached with the download, so a re-run from cache gives
  the same world id (ADR-0006); a fresh download checks again.
- A licence stated only on a terms page is checked by a person. The source
  definition records the date, and a newer release waits for another
  check.

**Client assets:** each has a manifest with its source URL, author,
licence, credit, download date and the licence as it came with the
download. CI checks every manifest against the client list, and the
client's credits show them. The assets are downloaded and checked by hash,
not committed; their manifest format comes with the first one.

**Notices:** the published world carries the text and notices of every
licence its assets use, as the ODbL, CDLA-Permissive-2.0 and the
Copernicus DEM require. [ADR-0019](0019-odbl-and-the-published-world.md)
decides how the ODbL notices travel with a download and appear in the
client; the others can follow the same path, with their text from the
list.

### Changing the list

[GOVERNANCE.md](../../GOVERNANCE.md) defines the merge criteria by RFC,
and changing them needs an RFC open for comment for at least 7 days. Once
G4 reads the list, its entries decide whether a change merges. Two ways
to meet that rule:
- **(i) Delegation (recommended; decided 2026-11-25).** The RFC that
  amends G4 says that G4 allows the list's entries, changed as this
  section describes. Accepting it authorises additions that meet the
  three rules by pull request: they
  apply the criterion rather than change it, like everyday changes within
  a contract. For: a licence can be added in days, which M6 and M7 need
  country by country. Against: an entry gets a maintainer's review, not
  7 days of comment.
- **(ii) An RFC amendment for every change,** with 7 days of comment. For:
  every entry is open to comment. Against: every new licence waits at
  least a week, for a one-line change.

Under either:
- **Adding a licence:** a pull request with the entry, the stored text and
  a short review against the three rules; a maintainer approves. A licence
  that breaks a rule changes the rules: a new ADR and an RFC amendment.
- **A wrong entry** is removed, and assets under it are taken down
  (RFC 0002 section 7 allows that for "a licence or copyright problem").
- **A source that changes its licence:** the change applies to later
  releases. Assets keep the licence of the release they came from, which
  their manifests record, if the old terms allow it: Microsoft's
  footprints from before 2026-03-11 stay ODbL.

**Where the gate and validation changes are written.** G4 reading the
list changes RFC 0002, and the validation rules change RFC 0001
section 2. Both are Drafts, but their comment periods can end within a
week (RFC 0002's runs until at least 2026-11-30), so changes written into
them now would get less than 7 days of comment.
- **A short RFC of their own (recommended; decided 2026-11-25):** a full
  comment period, and no wait for questions about point features.
- **RFC 0004,** whose comment period runs until at least 2026-12-01: one
  RFC fewer, but these rules cover every layer, and RFC 0004 waits for
  the pedestrian ADR.

### Left open by this ADR

- **CMU motion capture.** Its terms call the data free for all uses, even
  in commercially sold products, but say "you may not resell this data
  directly, even in converted form", and anyone may resell StreetWide's
  content. Options:
  - leave it out, and use 100STYLE (CC BY 4.0) for locomotion;
  - allow it for client assets only, under `LicenseRef-CMU-Mocap`;
  - ask CMU whether converted clips in a free, open-source client count as
    reselling.

  Recommended: the first now, and the third in parallel. The terms were
  read over plain HTTP, because the site's HTTPS certificate failed to
  verify. *Decided 2026-11-25:* the first now, and the third in
  parallel. CMU's data stays out of the client, 100STYLE serves
  locomotion, and the maintainer asks CMU.
- **JAXA's AW3D30.** Commercial use needs notice to JAXA in advance, which
  breaks rule 3 unless one notice covers every later user. That these
  terms govern AW3D30 is inferred from Google Earth Engine's catalogue;
  JAXA's own pages returned errors. Not needed before M7.
- **The canton of Zurich's open data.** Its terms name no licence and ask
  for no credit, its terms page could not be read by a script, and the
  city's portal relabels some of it CC0. Using it in the world needs
  `LicenseRef-ZH-OGD` with the terms read, or the canton's written
  confirmation.
- **An ODbL world.** ADR-0019 recommends offering the whole world under
  the ODbL. If that is chosen, every world-data entry must also fit inside
  an ODbL database (ODbL 4.4), and the list records that per entry.
  ADR-0019 reads CC0 as fitting, CC BY 4.0 as appearing to fit, and
  swisstopo and Copernicus as still to check. The stored swisstopo terms
  should include swisstopo's clarification of 14 September 2021 on
  central attribution; so far only the OpenStreetMap wiki's record of it
  has been read. *Settled 2026-11-25:* ADR-0019 was accepted with option
  A, so each world-data entry records whether it fits inside an ODbL
  database. The swisstopo and Copernicus entries say so only once they
  are checked; for swisstopo, ADR-0019 asks for its clarification to be
  quoted, or swisstopo asked, and the FSDI terms checked, before
  publishing.
- **Training the compiler:** whether data that may not enter the world may
  train models whose output does is for the compiler's ADR (M6).

## Consequences

- **ADR-0001** stays accepted and unchanged; its licence review for new
  sources becomes a review against this list. The relation is recorded in
  this ADR's header, as ADR-0012 and ADR-0013 record that they extend
  ADR-0008; the index's statuses do not change.
- **New M2 sources** (ADR-0015, ADR-0016) and the point assets of
  [RFC 0004](../../rfcs/0004-point-features.md) wait for this ADR's
  acceptance, the list with the stored swisstopo terms, the RFC with the
  validation rules and each source's ATTRIBUTION.md entry. The check at
  download can come with the first new source.
- **Existing data:** the 48 asset manifests in the committed fixtures, and
  every manifest of a local build of the district, pass the validation
  rules (checked on 2026-11-24), and their licences are all on the list.
  No manifest, world id, replay or simulation result changes. Only the
  facade sample's check report is regenerated, for the new gate version.
- **Work after acceptance:**
  - in `op_world`, the list and the G4 and validation changes;
  - in `tools/worldfmt` and `tools/ingest`, the same rules and the check
    at download, with Rust and Python agreement tests;
  - tests that fail on purpose: an NC source, an unknown id, a missing
    credit, a refresh with an uncovered licence.
- **Documents to update:** the RFC that amends RFC 0001 section 2 and
  RFC 0002's G4; ATTRIBUTION.md (one swisstopo terms page);
  CONTRIBUTING.md and the facade guide (what contributors may use); the
  roadmap's licence risk.
- **Harder:** every new licence waits for a review, and some well-known
  data and assets are out (FABDEM, LAFAN1, Mixamo).
- **Not legal advice:** the readings here are the project's own. A review
  by someone experienced in open-data licensing before the world is
  announced (M1) would lower the risk.
- **Revisit** when a source the project needs fails the rules (AW3D30,
  CMU), or if a reading here proves wrong. ADR-0019, decided on
  2026-11-25, is taken in under "An ODbL world".
