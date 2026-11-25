# RFC 0006: The licence allowlist: gate and validation rules

- **Status:** Draft
- **Authors:** @victorpicaud
- **Created:** 2026-11-25 (comment period until at least 2026-12-02)
- **Contract version affected:** world format `0.1.0` unchanged under the
  recommended option, or `0.2.0` under option A (see "Options
  considered"); contribution contract 0.1, amended in place or bumped to
  `0.2.0`, a choice; gate 0.1.4 → 0.1.5, or the next free version; new:
  the licence list `LICENSES/allowlist.json` and its schema

## Summary

[ADR-0017](../docs/adr/0017-licence-allowlist.md) decided that
StreetWide keeps one machine-readable list of the licences allowed for
world data and for client assets, and of what is forbidden, enforced in
code. It left the gate and validation rules to "a short RFC of their
own". This is that RFC. It proposes:
- **The list:** `LICENSES/allowlist.json`, checked by
  `schemas/allowlist.schema.json`. One entry per licence id says where
  the licence may be used, whether it fits inside an ODbL database
  ([ADR-0019](../docs/adr/0019-odbl-and-the-published-world.md)), what
  it requires, where its text is stored and, for a forbidden entry, why
  (section 1). The first entries are ADR-0017's lists (section 2).
- **Licence rules** for every asset: an asset's licence joins ids with
  `AND` only, each source names one id, and the asset's licence and
  credits cover those of its sources (section 3).
- **G4 and G5 for every new manifest,** in contributions and in baseline
  refreshes, as [RFC 0002](0002-contributions-and-merge-rules.md)'s
  question 14 decided. G4 reads the list instead of its four built-in
  licences and four words (section 4).
- **The ingest** checks each source's licence statement at download, and
  stops when it changes (section 7). **Client assets** are checked in CI
  (section 8).
- **Where the licence rules live.** World format 0.1.0 is frozen, so the
  rules cannot become validation errors without a new format version.
  Recommended, option C: the gate and the ingest enforce them now,
  validation reports them as warnings, and the next format version makes
  them errors. Option A, a format 0.2.0 now, changes every id.

Nothing here is decided. Comments run until at least 2026-12-02; then
the maintainer decides.

## Motivation

- **ADR-0017 asks for it.** Its Decision chose "the gate and validation
  rules written in a short RFC of their own". RFC 0002's Decision names
  this RFC as the follow-up that writes G4's allowlist, and question 14's
  decision, into the G4 and G5 rows.
- **New sources wait for it.** Zurich's trees and lights (CC0,
  [ADR-0015](../docs/adr/0015-zurich-trees-and-lights.md)) and
  swissSURFACE3D ([ADR-0016](../docs/adr/0016-swisssurface3d-heights.md))
  wait for the list, the stored swisstopo terms and this RFC (ADR-0017,
  Consequences). The point ingest is the next work on `main`.
- **Refreshes are unchecked.** The gate skips G4 and G5 for change sets
  authored by `streetwide:ingest`. Only a Python test on the fixture tile,
  and the notices' check of credits (ADR-0019), look at the licences the
  ingest writes.
- **G4 is hard-coded.** Four licences and four words are constants in
  `op_world::gate`. The words would fail a source that cites Google Open
  Buildings (CC BY 4.0), because its name contains "google".
- **The format is frozen.** ADR-0017 wrote its validation rules as an
  amendment of [RFC 0001](0001-world-format.md) section 2, before RFC 0001
  was accepted. Since 2026-11-25, format 0.1.0 is frozen (RFC 0001,
  Decision). A rule that makes some 0.1.0 worlds invalid is a format
  change, and under [RFC 0004](0004-point-features.md)'s accepted
  option A it takes a new `contract_version`, which changes every id.

## Design

Sections 1, 2 and 7 to 10 hold under every option. Sections 3 to 6 say
where each rule runs, under each option of "Options considered". They
are written for the recommended option, C, with what differs under A.

### 1. The list

**The file:** `LICENSES/allowlist.json`, beside the stored licence
texts. JSON with keys sorted, two-space indents and a final newline, so
that a diff shows only real changes; CI checks that form. Like canonical
JSON (RFC 0001 section 5), it has no floats and writes optional values
as `null`. Its **list id** is the BLAKE3 content id of its canonical
form.

```json
{
  "contract_version": "0.1.0",
  "format": "streetwide.licences",
  "licences": [
    {
      "client_assets": true,
      "contributions": true,
      "credit": "required",
      "forbidden": false,
      "id": "CC-BY-4.0",
      "name": "Creative Commons Attribution 4.0 International",
      "notes": null,
      "notice": null,
      "odbl": "fits",
      "reason": null,
      "requires": ["change_note", "licence_link", "no_endorsement"],
      "review": "docs/adr/0017-licence-allowlist.md#world-data",
      "source_urls": [],
      "text": "LICENSES/CC-BY-4.0.txt",
      "text_read": "2026-11-25",
      "text_url": "https://creativecommons.org/licenses/by/4.0/legalcode.en",
      "world_data": true
    },
    {
      "client_assets": false,
      "contributions": false,
      "credit": "none",
      "forbidden": true,
      "id": "LicenseRef-Google-Maps-Platform-Terms",
      "name": "Google Maps Platform Terms of Service",
      "notes": "Photorealistic 3D Tiles, Street View and Google's other map and imagery content",
      "notice": null,
      "odbl": null,
      "reason": "the terms forbid storing the content and creating content from it, including 3D objects traced or derived by hand or machine; not a source, a reference to trace, or a cache",
      "requires": [],
      "review": "docs/adr/0017-licence-allowlist.md#forbidden",
      "source_urls": ["https://maps.googleapis.com/", "https://tile.googleapis.com/"],
      "text": null,
      "text_read": "2026-11-24",
      "text_url": "https://cloud.google.com/maps-platform/terms",
      "world_data": false
    },
    {
      "client_assets": false,
      "contributions": false,
      "credit": "required",
      "forbidden": false,
      "id": "ODbL-1.0",
      "name": "Open Database License 1.0",
      "notes": "OpenStreetMap data stays in its own assets and is fixed upstream (RFC 0001 section 2, RFC 0002 section 3)",
      "notice": null,
      "odbl": "fits",
      "reason": null,
      "requires": ["licence_link", "licence_text", "offer", "share_alike"],
      "review": "docs/adr/0017-licence-allowlist.md#world-data",
      "source_urls": [],
      "text": "LICENSES/ODbL-1.0.txt",
      "text_read": "2026-11-25",
      "text_url": "https://opendatacommons.org/licenses/odbl/1-0/",
      "world_data": true
    }
  ]
}
```

The dates and links in this example are placeholders: each is set when
its text is stored and read.

**Fields of an entry.** Every field is present, `null` where it does not
apply.

| Field | Value | Meaning |
|---|---|---|
| `id` | an SPDX id, or `LicenseRef-` with the publisher and the name of the terms | One licence, never an expression. Entries are sorted by `id` in byte order, each id once. An id once used is never renamed (ADR-0017). |
| `name` | text | The full name, as notices and the client's credits page show it. |
| `forbidden` | boolean | `true`: refused everywhere, with `reason`. |
| `reason` | text, or `null` | Why it is forbidden. G4, the ingest and CI quote it. `null` for an allowed entry. |
| `world_data` | boolean | Allowed for world assets and their sources. |
| `client_assets` | boolean | Allowed for files and data the client ships or downloads. |
| `contributions` | boolean | Contributed assets may build on it. Only with `world_data`. |
| `odbl` | `"fits"`, `"to_check"`, or `null` | Whether it fits inside an ODbL database (ODbL 4.4, ADR-0019). Required with `world_data`, `null` otherwise. |
| `credit` | `"required"`, `"suggested"` or `"none"` | Whether a credit must be given. StreetWide gives a suggested credit anyway (ADR-0017). |
| `requires` | sorted list | Other obligations, from a fixed set: `change_note`, `licence_link`, `licence_text`, `liability_notice`, `no_endorsement`, `notice`, `offer`, `share_alike`, `update_date`. |
| `notice` | text, or `null` | Fixed text that must travel with the data, such as the Copernicus DEM's notice. Required when `requires` has `notice`. |
| `notes` | text, or `null` | For people: limits on use. G4 quotes it when it refuses a use. |
| `text` | `LICENSES/<id>.txt`, or `null` | The stored text. Required for an allowed entry, `null` for a forbidden one. |
| `text_url` | https URL | Where the text or the terms were read. |
| `text_read` | `YYYY-MM-DD` | The date they were read. |
| `review` | text | Where the entry was reviewed: an ADR section, or the pull request. |
| `source_urls` | sorted list of https URL prefixes | Forbidden entries only: a source whose `url` starts with one of them fails, whatever licence it declares. Empty for an allowed entry. |

**The schema**, `schemas/allowlist.schema.json`:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "urn:streetwide:schema:allowlist:0.1",
  "title": "StreetWide licence list (LICENSES/allowlist.json)",
  "type": "object",
  "additionalProperties": false,
  "required": ["contract_version", "format", "licences"],
  "properties": {
    "format": { "const": "streetwide.licences" },
    "contract_version": { "const": "0.1.0" },
    "licences": { "type": "array", "minItems": 1, "items": { "$ref": "#/$defs/entry" } }
  },
  "$defs": {
    "text": { "$ref": "urn:streetwide:schema:common:0.1#/$defs/text" },
    "textOrNull": { "oneOf": [{ "$ref": "#/$defs/text" }, { "type": "null" }] },
    "id": { "type": "string", "maxLength": 128, "pattern": "^[A-Za-z0-9][A-Za-z0-9.-]*$" },
    "url": { "type": "string", "maxLength": 2048, "pattern": "^https://[!-~]+$" },
    "date": { "type": "string", "pattern": "^[0-9]{4}-[0-9]{2}-[0-9]{2}$" },
    "entry": {
      "type": "object",
      "additionalProperties": false,
      "required": [
        "client_assets", "contributions", "credit", "forbidden", "id", "name", "notes",
        "notice", "odbl", "reason", "requires", "review", "source_urls", "text",
        "text_read", "text_url", "world_data"
      ],
      "properties": {
        "id": { "$ref": "#/$defs/id" },
        "name": { "$ref": "#/$defs/text" },
        "forbidden": { "type": "boolean" },
        "reason": { "$ref": "#/$defs/textOrNull" },
        "world_data": { "type": "boolean" },
        "client_assets": { "type": "boolean" },
        "contributions": { "type": "boolean" },
        "odbl": { "enum": ["fits", "to_check", null] },
        "credit": { "enum": ["required", "suggested", "none"] },
        "requires": {
          "type": "array",
          "uniqueItems": true,
          "items": { "enum": [
            "change_note", "licence_link", "licence_text", "liability_notice",
            "no_endorsement", "notice", "offer", "share_alike", "update_date"
          ] }
        },
        "notice": { "$ref": "#/$defs/textOrNull" },
        "notes": { "$ref": "#/$defs/textOrNull" },
        "text": { "oneOf": [
          { "type": "string", "pattern": "^LICENSES/[A-Za-z0-9][A-Za-z0-9.-]*\\.txt$" },
          { "type": "null" }
        ] },
        "text_url": { "$ref": "#/$defs/url" },
        "text_read": { "$ref": "#/$defs/date" },
        "review": { "$ref": "#/$defs/text" },
        "source_urls": { "type": "array", "uniqueItems": true, "items": { "$ref": "#/$defs/url" } }
      },
      "allOf": [
        {
          "if": { "properties": { "forbidden": { "const": true } } },
          "then": { "properties": {
            "reason": { "$ref": "#/$defs/text" }, "text": { "type": "null" },
            "world_data": { "const": false }, "client_assets": { "const": false }
          } },
          "else": {
            "properties": {
              "reason": { "type": "null" }, "text": { "type": "string" },
              "source_urls": { "maxItems": 0 }
            },
            "anyOf": [
              { "properties": { "world_data": { "const": true } } },
              { "properties": { "client_assets": { "const": true } } }
            ]
          }
        },
        {
          "if": { "properties": { "world_data": { "const": true } } },
          "then": { "properties": { "odbl": { "enum": ["fits", "to_check"] } } },
          "else": { "properties": { "odbl": { "const": null }, "contributions": { "const": false } } }
        },
        {
          "if": { "properties": { "requires": { "contains": { "const": "notice" } } } },
          "then": { "properties": { "notice": { "$ref": "#/$defs/text" } } },
          "else": { "properties": { "notice": { "type": "null" } } }
        }
      ]
    }
  }
}
```

**Rules the schema cannot express,** checked by `op_world` and
`tools/worldfmt` with the same messages: ids sorted and unique; `text`
is `LICENSES/<id>.txt` and the file exists, in UTF-8 with LF line
endings; `requires` and `source_urls` sorted.

**One gate version, one list.** The gate embeds the file when it is
built, so a re-run gives the same report. A test pins the list id next
to `GATE_VERSION`: a list changed without raising `GATE_VERSION` fails
CI. The Python tools read the same file, and agreement tests check that
both languages read the same entries and compute the same list id.

**Its contract.** G4 reads the list, so its format is part of the
contribution contract, and its `contract_version` is that contract's.
Its schema changes by RFC; its entries change by pull request
(section 10).

### 2. The first entries

The first file holds every entry ADR-0017 accepted, each with its stored
text (see "Smaller choices"). `odbl` follows ADR-0019's readings:
CC0 fits, CC BY 4.0 appears to fit with its credit kept, and swisstopo
and Copernicus are still to check. Entries no one has read for the ODbL
yet start as `to_check`.

**Allowed:**

| Id | World data | Client assets | Contributions build on it | `odbl` | `credit` | `requires` | Text | In use, or planned |
|---|---|---|---|---|---|---|---|---|
| `CC0-1.0` | yes | yes | yes | `fits` | `suggested` | none | new | the City of Zurich's tree register and lighting data (ADR-0015), from M2 |
| `CC-BY-4.0` | yes | yes | yes | `fits` | `required` | `change_note`, `licence_link`, `no_endorsement` | new | MeteoSwiss's leaf dates, for the client's trees (ADR-0022) |
| `CC-BY-SA-4.0` | yes | no | yes | `to_check` (ADR-0019, question 3) | `required` | `change_note`, `licence_link`, `no_endorsement`, `share_alike` | stored | contributions |
| `ODbL-1.0` | yes | no | no | `fits` | `required` | `licence_link`, `licence_text`, `offer`, `share_alike` | stored | OpenStreetMap |
| `LicenseRef-swisstopo-OGD` | yes | no | yes | `to_check` (ADR-0019) | `required` | none | new: the terms page, and swisstopo's clarification of 14 September 2021 once read | swissBUILDINGS3D, swissALTI3D; swissSURFACE3D from M2 |
| `OGL-UK-3.0` | yes | no | yes | `to_check` | `required` | `licence_link` | new | not yet |
| `etalab-2.0` | yes | no | yes | `to_check` | `required` | `update_date` | new | not yet |
| `DL-DE-ZERO-2.0` | yes | no | yes | `to_check` | `none` | none | new | not yet |
| `DL-DE-BY-2.0` | yes | no | yes | `to_check` | `required` | `change_note`, `licence_link` | new | not yet |
| `CDLA-Permissive-2.0` | yes | no | yes | `to_check` | `none` | `licence_text` | new | not yet |
| `LicenseRef-PublicDomain-USGov` | yes | no | yes | `to_check` | `suggested` | none | new: the agencies' statements | not yet |
| `LicenseRef-Copernicus-DEM-GLO-30-F` | yes | no | no | `to_check` | `required` | `liability_notice`, `no_endorsement`, `notice` | new | not yet (M7) |
| `LicenseRef-Copernicus-DEM-GLO-90-F` | yes | no | no | `to_check` | `required` | `liability_notice`, `no_endorsement`, `notice` | new | not yet (M7) |
| `LicenseRef-Copernicus-Sentinel` | yes | no | no | `to_check` | `required` | `change_note`, `liability_notice`, `notice` | new | not yet |

The ODbL and Copernicus entries carry `notes` saying why contributions
may not build on them (ADR-0017, "World data"). CC BY-SA 4.0 is not on
the client list, as decided on 2026-11-25.

**Forbidden** (`reason` from ADR-0017's table, "Forbidden"):

| Id | `source_urls` | Reason, in short |
|---|---|---|
| `CC-BY-NC-4.0`, `CC-BY-NC-SA-4.0` | none | non-commercial: anyone may sell StreetWide's content |
| `CC-BY-NC-ND-4.0`, `CC-BY-ND-4.0` | none | no derivatives: assets are adapted |
| `LicenseRef-Sonniss-GDC-Bundle` | none | the sounds may not be supplied as sound effects to others; StreetWide's assets are downloadable files |
| `LicenseRef-Quaternius-Asset-License-1.0` | none | no redistribution; packs downloaded under CC0, with a dated copy of that licence, stay CC0 |
| `LicenseRef-Adobe-Mixamo` | none | the raw files may not be distributed (low confidence, ADR-0017) |
| `LicenseRef-Google-Maps-Platform-Terms` | `https://maps.googleapis.com/`, `https://tile.googleapis.com/` | not a source, a reference to trace, or a cache |
| `LicenseRef-Apple-Maps-Terms`, `LicenseRef-Microsoft-Bing-Maps-Terms`, `LicenseRef-Microsoft-Flight-Simulator` | written with the file | as Google's |
| `LicenseRef-Copernicus-DEM-Restricted` | written with the file | EEA-10 and GLO-30-R are not licensed to the public |

The rule is the licence, not the name: every NC or ND licence is
refused, listed or not. An unlisted id already fails as "not reviewed
yet"; the forbidden entries give the better reason for the ones people
are likely to try.

**The sources in use or planned for M2, and where they state their
licence:**

| Source | Statement, as read | Id |
|---|---|---|
| swissBUILDINGS3D 3.0, swissALTI3D | STAC collection `license`: `"proprietary"`, with no licence link (read on 2026-11-25); the terms page | `LicenseRef-swisstopo-OGD` |
| OpenStreetMap | Overpass `osm3s.copyright`: "The data is made available under ODbL." | `ODbL-1.0` |
| City of Zurich, tree register and lighting | CKAN `license_id`: `cc-zero` (ADR-0017) | `CC0-1.0` |
| StreetWide contributions | the DCO and CONTRIBUTING.md | `CC-BY-SA-4.0` |
| MeteoSwiss's phenology (leaf dates, client) | STAC collection `license`: `"CC-BY"`, with no version; the terms of use: the open data "are published accordingly under the Creative Commons Licence CC BY 4.0" (both read on 2026-11-25) | `CC-BY-4.0` |

MeteoSwiss's terms also ask for the credit "Source: MeteoSwiss" and say
the credit must not suggest that MeteoSwiss supports the user. Reading
its unversioned "CC-BY" as `CC-BY-4.0` settles ADR-0022's open question
6, which waits for a review under ADR-0017 (see "Smaller choices").

### 3. The licence rules

**Licence rules** (they hold for any world, whatever its sources):
- **L1, `AND` only:** an asset's `provenance.license` is one licence id,
  or ids joined by ` AND `. No `OR`, `WITH`, `+` or parentheses. An id
  is letters, digits, `.` and `-`.
- **L2, one licence per source:** each source's `license` is one id, the
  licence it is used under. A source offered under a choice, such as
  Google Open Buildings (CC BY 4.0 or the ODbL), names the one chosen at
  ingest.
- **L3, coverage:** every source's id appears in the asset's licence,
  and the asset's licence has no other id (see "Smaller choices").
- **L4, credits:** every source's `attribution` appears in the asset's
  `provenance.attribution`.

**List rules** (only the canonical world is held to them; ADR-0017 keeps
the list out of validation, so a private world built for research may
hold other data):
- **P1, listed for its use:** every id of an asset's licence and of its
  sources' licences is an allowed entry with `world_data: true`.
- **P2, forbidden sources:** no source's `url` starts with a forbidden
  entry's `source_urls`.
- **P3, contributions:** in a contributed asset, every id has
  `contributions: true`, and the `StreetWide contributions` source is
  licensed `CC-BY-SA-4.0`.

**Where each rule runs:**

| What is checked | A: format 0.2.0 | B: gate and ingest | C: B plus warnings (recommended) |
|---|---|---|---|
| L1 to L4, any world | `op_world validate`: errors, in 0.2.0 worlds | not checked | `op_world validate`: warnings |
| L1 to L4, new manifests in a change set | G1, through validation | G4 (L1 to L3) and G5 (L4) | G4 (L1 to L3) and G5 (L4) |
| P1 to P3, new manifests in a change set | G4 | G4 | G4 |
| L1 to L4, P1 and P2, what the ingest writes | the ingest, before it writes anything | same | same |
| L1 to L4, P1 and P2, a whole world | `op_world licences` | same | same |
| Client assets against the list | CI | CI | CI |

`op_world licences <world>` is a new command, with a twin in
`tools/worldfmt`. It checks every asset of a world against the rules and
the list, and exits with an error if any fails. CI runs it on the
fixtures; the ingest's tests run it on the worlds they write; publishing
(M1) runs it on the canonical world.

### 4. G4 and G5 (amends RFC 0002 section 4)

The G4 and G5 rows become:

| # | Check | Rule |
|---|---|---|
| G4 | Licence | Every new manifest, in a contribution or a baseline refresh, against `LICENSES/allowlist.json` as built into the gate. **Listed:** every licence id in `provenance.license` and in each source's `license` is an allowed entry for world data (P1). A forbidden id fails with the entry's reason; an id with no entry fails as not reviewed yet. **Forbidden sources:** no source's `url` starts with a forbidden entry's `source_urls` (P2). **Form and coverage:** the licence joins ids with `AND` only, each source names one id, and the licence holds exactly its sources' ids (L1 to L3). **Contributed assets** also: every id allows contributions to build on it, and the `StreetWide contributions` source is licensed `CC-BY-SA-4.0` (P3). So ODbL-derived data is never combined with CC BY-SA content (ADR-0001). **The list** changes as section 10 of RFC 0006 describes; each change raises `GATE_VERSION`. |
| G5 | Provenance | Every new manifest, in a contribution or a baseline refresh: each source's credit appears in the attribution (L4). **Contributed assets** also: a source entry `StreetWide contributions` whose `source_ids` contain the author's handle, plus every upstream dataset used; a `generator` naming the tool and version (including AI tools, see section 8); `The StreetWide contributors` in the attribution. |

- **Under option A,** L1 to L4 are validation rules, which G1 runs on
  every change set. G4 and G5 then leave them to G1, so that one problem
  is reported once.
- **Refreshes** get `pass` or `fail` from G4 and G5, never `review`: a
  licence is not a matter of judgement. The ingest's own check (section
  7) means a refresh should never fail them; G4 and G5 are the backstop.
- **Removals** are not checked: only new manifests are.
- **The rows' other rules stay.** "Contributed geometry: licensed
  `CC-BY-SA-4.0`" and "reused geometry lists its source and states the
  combined licence" follow from L3 and P3.

This writes question 14's decision (ADR-0020's A6) into the G4 and G5
rows, as RFC 0002's Decision asks. Under options B and C it departs from
that decision's wording, which puts the coverage check in validation:
see "Options considered".

### 5. Messages

`{what}` is the gate's usual prefix: tile, layer and feature id.

| Rule | Gate (G4, G5) | Validation (warning under C, error under A) |
|---|---|---|
| P1, no entry | `{what}: licence {id} is not on the licence list yet (LICENSES/allowlist.json); to use it, open a pull request that adds it, with its text and a review against ADR-0017's three rules` | none |
| P1, forbidden | `{what}: licence {id} is forbidden: {reason}` | none |
| P1, client only | `{what}: licence {id} is allowed for client assets only, not in the world` | none |
| P2 | `{what}: source "{dataset}" ({url}) is forbidden: {reason}` | none |
| P3 | `{what}: contributed assets may not build on {id}: {notes}`; `{what}: the "StreetWide contributions" source must be licensed CC-BY-SA-4.0` | none |
| L1 | `{what}: the licence "{license}" may join licence ids with AND only` | `provenance.license "{license}" may join licence ids with AND only` |
| L2 | `{what}: source "{dataset}" must name one licence id, the one it is used under, not "{license}"` | `provenance.sources[{i}].license "{license}" must be one licence id` |
| L3 | `{what}: the licence must be its sources' licence ids joined by AND ({ids})` | `provenance.sources[{i}] is licensed {id}, which provenance.license "{license}" does not include` |
| L4 | `{what}: attribution must include "{credit}"` (G5's message today) | `provenance.sources[{i}].attribution "{credit}" is missing from provenance.attribution` |

Under C, `op_world validate` prints each warning as `warning: {tile}
{layer} {feature_id}: {message}` and keeps its exit code; the world
stays valid. `op_world licences` prints the same messages as errors.
Rust and Python give the same texts, checked by the agreement tests.

### 6. Validation

**Under option A** (amends RFC 0001 sections 2 and 3, as format
`0.2.0`): section 2's "Licence separation" gains L1 to L4, as rules
every asset must meet. `contract_version` becomes `"0.2.0"` in every
manifest, tile index, world manifest and points document, and in the
schemas' `contractVersion`. Validation reports L1 to L4 as errors.
Readers either read both versions, applying L1 to L4 to 0.2.0 only, or
read 0.2.0 alone; that choice is A's own (see "Options considered").

**Under option C:** RFC 0001 is not amended. `op_world validate` and
`tools/worldfmt` report L1 to L4 as warnings, which change no world's
validity, no id and no G1 outcome. RFC 0001 already lists "licence
rules" among the checks of `op_world validate` (its "Tests and checks");
today it checks only that licences are well formed and that their
copies agree. The next RFC that changes the world format for any reason
makes L1 to L4 errors in its new version. Accepting C records that
commitment here.

**Under option B:** validation does not change.

Under every option, validation never reads the list (ADR-0017).

### 7. The ingest: the licence at download

Each source definition in `tools/ingest` names the licence it expects,
where the publisher states it, and in what words:

```python
LicenceCheck(
    licence="LicenseRef-swisstopo-OGD",   # an allowed world-data entry
    where="stac_collection",              # stac_collection, ckan_package,
                                          # overpass_copyright, licence_file
                                          # or terms_page
    url="https://data.geo.admin.ch/api/stac/v1/collections/ch.swisstopo.swissalti3d",
    expected="proprietary",               # the publisher's words, exactly
    terms_url="https://www.swisstopo.admin.ch/en/terms-of-use-free-geodata-and-geoservices",
    terms_checked="2026-11-25",           # when a person last read the terms
    release_checked="2026",               # the newest release they covered
)
```

- **At download** the ingest reads the statement for the release it
  pins. It stops, before writing anything, if the words differ from
  `expected`, or if `licence` is not an allowed world-data entry.
- **Where the words name no licence** (swisstopo's STAC says
  `proprietary`), or where the licence is stated only on a terms page, a
  person reads the terms. The definition records the date and the
  newest release checked; a newer release stops the ingest until a
  person checks again.
- **The statement is cached with the download,** next to the data in the
  cache, so a re-run from cache gives the same world id (ADR-0006). A
  fresh download, or `--refresh`, checks again.
- **What the world records:** the checked id, in each source's `license`,
  as today. No field is added, so no manifest changes.
- **Before writing,** the ingest checks L1 to L4, P1 and P2 on every
  asset, as its notices already check credits (ADR-0019), and refuses to
  write a world that fails.
- **Data used only to check the ingest** and never copied into the world
  needs no entry (ADR-0017).

Messages:
- `licence check failed for {dataset}: the {where} at {url} says
  "{found}", expected "{expected}" ({licence}); review the terms, then
  update the source definition and, if needed, LICENSES/allowlist.json`
- `licence check: {dataset} release {release} is newer than the release
  whose terms were checked ({release_checked}, on {terms_checked}); read
  {terms_url} again and update the source definition`
- `licence check: {dataset}: licence {id} is not allowed for world data
  (LICENSES/allowlist.json)`, or `... is forbidden: {reason}`

This is the same under every option, except that under A the ingest
writes format 0.2.0.

### 8. Client assets

Sounds, models, animations, textures and data tables that the client
ships or downloads, made by others. Code, and the licences of code
dependencies, are outside the list.
- **Today** the client ships no asset files (ADR-0022 generates meshes
  and textures in code). Its first outside input will be MeteoSwiss's
  leaf dates, a handful of medians in code, when it draws trees
  (ADR-0022). The client's credits page lists each source. The first
  asset files are likely M2's sounds (roadmap, M2).
- **CI** checks each client asset's licence against the list: an allowed
  entry with `client_assets: true`. A forbidden id fails with its reason;
  an unlisted id fails as not reviewed yet. Message: `client asset
  {name}: licence {id} is not allowed for client assets
  (LICENSES/allowlist.json)`.
- **Where the licence is read:** each asset's manifest, whose format
  comes with the first asset file (ADR-0017: source URL, author, licence,
  credit, download date, the licence as it came with the download, and
  a hash). For data in code, such as the leaf dates, the client's credits
  table gains the licence id, and CI checks that.

This is the same under every option: client assets are not world data.

### 9. Notices from the list

The notices beside every world (`NOTICE.txt`, `CREDITS.txt` and the
licence texts, ADR-0019) and the client's credits page take each
licence's name, link and stored text from the list instead of constants
in `tools/ingest/notices.py`. A licence whose entry `requires`
`licence_text` has its text copied beside the world; one with `notice`
has its fixed notice in `NOTICE.txt`. No world id changes: the notices
are not part of the world.

The publishing step (M1, with the hosting of
[ADR-0010](../docs/adr/0010-hosting-the-canonical-world.md)) refuses a
world that uses an entry whose `odbl` is `to_check` (see "Smaller
choices"). Today that is swisstopo, as ADR-0019 already requires before
publishing, and CC BY-SA 4.0 once a contribution is merged.

### 10. Changing the list

As ADR-0017 decided ("Changing the list", option (i), the delegation),
G4 allows the entries of the list built into the gate, changed as
follows. Accepting this RFC authorises these changes by pull request,
without an RFC:
- **Adding a licence** that meets ADR-0017's three rules ("What a
  licence must allow"): the entry, the stored text, and a short review
  against the rules and the checks of "What a review checks". A
  maintainer approves.
- **A wrong entry** is removed, and assets under it are taken down
  (RFC 0002 section 7, "a licence or copyright problem").
- **An `odbl` reading** moves from `to_check` to `fits` with the review
  that settles it, quoted in the pull request.
- **A source that changes its licence:** the change applies to later
  releases. Assets keep the licence of the release they came from, as
  their manifests record, if the old terms allow it.
- **Each change raises `GATE_VERSION`,** and the pinned list id with it.

A licence that breaks one of the three rules changes the rules: a new
ADR and an amendment of this RFC. So does a new kind in `requires`, or
any change to the schema.

## Options considered

### Where the licence rules live

#### A. World format 0.2.0, with L1 to L4 in validation

- **What changes:** RFC 0001 sections 2 and 3 gain L1 to L4 as format
  rules; `contract_version` becomes `0.2.0` in every manifest, tile
  index, world manifest and points document; `op_world`,
  `tools/worldfmt`, the ingest, the schemas and `op_stream` move to it.
  Readers either keep reading 0.1.0, without L1 to L4, or drop it. Only
  before the first publication can they drop it.
- **Ids:** every manifest id, tile id, points content id and world id
  changes. Meshes keep their bytes and content ids.
- **Fixtures and replays:** new golden ids in `fixtures/format`
  (`expected.json`, world `fdac2cbe…`, both points ids); `fixtures/world`
  re-ingested; the facade sample's change set and report remade; the
  refresh planner's pinned change and report ids remade; `sample.oprep`
  recorded again. Its checkpoints do not change, since the simulation
  hashes mesh content ids, but its header names the world id.
- **The canonical world:** none is published yet. Before M1's
  publication, 0.2.0 costs a re-ingest. After it, every asset is a
  content change (ADR-0020's definition), so a refresh replaces all
  1,491 of the district's assets with `parent_version_id` pointing at
  their 0.1.0 versions: at least 30 change sets while the 50-change cap
  holds. Readers then read 0.1.0 forever, since replays and the world
  log name old worlds.
- **Contribution contract:** unaffected by the format version.
  **GATE_VERSION:** raised once, for G1's new errors and G4's list.
- **Ingest:** writes 0.2.0, and `tools.ingest`'s `VERSION` rises, as
  its output changes (ADR-0020). **Client assets:** unaffected.
- **Refreshes (A6):** L1 to L4 through G1, P1 to P3 through G4, as
  question 14's text says. **The list's format:** unaffected; the list
  stays out of validation, as ADR-0017 decided. A variant with the list
  itself in validation would go against that Decision, and is not
  proposed.
- **What could ride with it:** ADR-0019's licence field in the world
  manifest, its "an asset's licence is that of its contents", RFC 0004's
  explicit content key. Each needs its own design, which would delay
  the list.
- Pros:
  - every world, private ones included, is held to L1 to L4 by the same
    validator;
  - question 14 and ADR-0017's "Enforcement" hold as written;
  - an older reader fails cleanly, with "unknown contract version";
  - cheapest now, before anything is published.
- Cons:
  - every id changes for rules that no stored world breaks: the 50 asset
    manifests in `fixtures/` and the district's 1,491 pass L1 to L4 and
    P1 (checked on 2026-11-25);
  - readers learn two versions, or the project makes a hard cut;
  - it spends 0.2.0 on one rule, while the roadmap plans a textures RFC
    in M2, which RFC 0001 expects to come with a profile version bump
    (RFC 0004, "Compatibility and migration");
  - new sources wait for the migration as well as the list.

#### B. The gate and the ingest only

- **What changes:** RFC 0001 does not change. G4 and G5 carry L1 to L4
  for every new manifest; the ingest refuses to write a world that fails
  them; `op_world licences` checks whole worlds, in CI and before
  publishing.
- **Ids, fixtures and replays:** none change, except the reports that
  name `GATE_VERSION` (see "Compatibility and migration").
- **Contribution contract:** see "Smaller choices". **GATE_VERSION:**
  0.1.5, or the next free version.
- **Ingest, client assets, the list's format:** as in A.
- **Refreshes (A6):** G4 and G5 run on them, as question 14 decided, but
  the coverage check sits in G4 and G5 instead of validation. That
  departs from the decision's wording ("validation, which G1 runs on
  every change set, checks..."), which the maintainer would accept with
  this RFC. The outcome for the canonical world is the same: every asset
  enters it either in the first baseline, which `op_world licences`
  checks, or through a change set, which G4 and G5 check.
- Pros:
  - no id changes; the list can land with the point ingest;
  - contributors see licence problems where they look, in the gate's
    report;
  - every stored world passes already, so nothing breaks.
- Cons:
  - `op_world validate` calls a world valid when its licences do not
    cover its sources;
  - question 14's wording is not met as written;
  - the rules live in the gate now and move to validation later.

#### C. B, plus validation warnings until the next format version (recommended)

- **What changes:** as B, and `op_world validate` and `tools/worldfmt`
  report L1 to L4 as warnings. The next RFC that changes the world format
  makes them errors.
- **Ids, contract versions, GATE_VERSION, ingest, client assets, the
  list's format, refreshes:** as B. Warnings change no G1 outcome and no
  report.
- Pros:
  - B's pros;
  - every world, private or canonical, shows its licence problems where
    people already look;
  - the move to errors is written down now, so 0.2.0 carries it at no
    extra cost, whenever it comes.
- Cons:
  - a warning level is new in both validators, and Rust and Python must
    agree on it;
  - warnings are easy to ignore, so `op_world licences` is still needed
    for the canonical world;
  - question 14's wording, as under B.

#### D. Amend 0.1.0 in place, as RFC 0003 to RFC 0005 did

- Pros: question 14 and ADR-0017 as written; no id changes; every stored
  world passes.
- Cons:
  - it reverses RFC 0001's Decision, which froze 0.1.0 on 2026-11-25;
  - two readers of 0.1.0, one older, would disagree about the same
    world;
  - after one exception, the freeze means little.

| | A | B | C | D |
|---|---|---|---|---|
| World format | 0.2.0 | 0.1.0 | 0.1.0 | 0.1.0, changed |
| Ids that change | every manifest, tile, points and world id | none | none | none |
| Replays | header recorded again | none | none | none |
| L1 to L4 on any world | errors | none | warnings | errors |
| New content held to L1 to L4 and the list | G1 and G4 | G4 and G5 | G4 and G5 | G1 and G4 |
| Question 14's wording | met | departs | departs | met |
| RFC 0001's freeze | kept | kept | kept | broken |

**Recommended: C.** It holds every new asset to the rules and the list
now, contributions and refreshes alike, and changes no id. Every stored
world already passes. The format gains the rules at its next version,
which the textures RFC planned for M2, or ADR-0019's licence field in
the world manifest, is likely to bring. If the maintainer expects that
version before the first publication anyway, A's cost falls, and the
rules can join it then: C leads there without changing anything twice.

### Smaller choices

- **The contribution contract version.**
  - **Amend 0.1.0 in place (recommended).** Question 14 was decided in
    place, and RFC 0002's Decision names this RFC as the follow-up that
    writes it. For contributions, the new G4 refuses nothing the old one
    accepted, except sources under a forbidden `source_urls` prefix that
    the four words missed: it allows more licences and drops the words.
    The new failures are for refreshes, which the ingest writes.
    `GATE_VERSION` records the change, and a merge applies the rules
    current at merge time anyway (RFC 0002, "Compatibility and
    migration").
  - **Bump to 0.2.0,** as RFC 0002's "Compatibility and migration" says
    for rule changes after its acceptance. Change sets and merge records
    then carry 0.2.0: the facade sample's change id and the planner's
    pinned ids change, and readers of a world log must read 0.1.0
    records too. No outside contribution exists for it to protect.
- **What `to_check` means for the ODbL.**
  - **Merging allowed, publishing refused (recommended).** ADR-0019
    already makes publishing wait for swisstopo's confirmation and for
    the contributors' question. The publishing step refuses a world with
    a `to_check` licence; merges and refreshes go on.
  - G4 gives `review` for each such asset: every swisstopo asset of
    every refresh, and every facade, would need a reviewer's note.
  - G4 fails it: no swisstopo refresh merges until swisstopo confirms.
- **The first file.**
  - **Every entry ADR-0017 accepted, with its text (recommended).** The
    lists were decided, and contributors see in advance what they may
    use. About 12 texts to store; the Copernicus terms come as PDFs and
    are stored as text with their URL and date.
  - Only the entries in use (CC0, CC BY 4.0, CC BY-SA 4.0, the ODbL and
    swisstopo); the others by pull request when a source needs them.
    Less to store now, but decided entries would be missing.
- **MeteoSwiss's "CC-BY".**
  - **`CC-BY-4.0` (recommended):** the publisher's own terms page names
    CC BY 4.0. The source definition expects `"CC-BY"` in STAC and
    records the terms check.
  - `LicenseRef-MeteoSwiss-OGD`, with its terms stored: exact, but a
    second entry for the same terms.
  - Ask MeteoSwiss first.
- **Forbidden sources.**
  - **URL prefixes on forbidden entries (recommended):** precise; Google
    Open Buildings, hosted elsewhere, passes.
  - Words, as today: fail Google Open Buildings.
  - None: licence ids only, so a forbidden source declared under an
    allowed licence passes.
- **Extra ids in an asset's licence.**
  - **Refused (recommended):** the licence holds exactly its sources'
    ids, as G4 already requires of contributions. An extra `ODbL-1.0`
    would close an asset to contributions (G3) for no reason. No stored
    manifest has one.
  - Allowed, as ADR-0017's wording ("every id in a source's licence
    appears in the asset's licence") permits.

## Compatibility and migration

- **Existing data** (counted on 2026-11-25 with a script over the
  manifests): the 50 asset manifests in `fixtures/` and the 1,491 of a
  local build of the district pass L1 to L4, and every id they use is on
  the list (P1). Their licences: `LicenseRef-swisstopo-OGD`,
  `ODbL-1.0 AND LicenseRef-swisstopo-OGD`, `CC-BY-SA-4.0` and
  `CC-BY-SA-4.0 AND LicenseRef-swisstopo-OGD`.
- **Under B and C:** no manifest, mesh, tile index, world id or replay
  changes. What changes:
  - the facade sample's report, in `gate_version` only (its G4 and G5
    still pass);
  - the refresh planner's pinned report ids, and the gate tests on
    refreshes: G4 and G5 go from `not_applicable` to `pass`;
  - `tools/ingest/notices.py` takes names and links from the list
    (section 9); `CREDITS.txt` may change wording, with no id;
  - `tools.ingest`'s `VERSION` stays, as no manifest changes.
- **Under A:** as listed under option A, with the world format at 0.2.0.
- **The contribution contract:** unchanged in place; if bumped, the
  change ids and the world log's reader change as "Smaller choices"
  says.
- **The canonical world** is not published yet. Under every option,
  `op_world licences` checks the first baseline before its log record is
  written.
- **New sources:** each waits for its entry (with stored text), its
  source definition's licence check and its ATTRIBUTION.md row, as
  ADR-0017 says.

## Determinism, provenance and privacy

- **Determinism:** the gate embeds the list, so its report depends only
  on the change set, the world and `GATE_VERSION`; no check reads the
  network. The ingest reads licence statements only at download, and
  caches them, so a re-run from cache gives the same world id. The
  simulation never sees licences: `SIM_CONTRACT_VERSION` stays.
- **Provenance:** every asset's licence and credits cover its sources',
  on refreshes too. Every licence the world uses has one entry, one
  stored text and one review.
- **Privacy:** an allowed licence does not make data private by design
  (ADR-0017); RFC 0002's G6 and each source's ADR still apply. The list
  holds no personal data, and reviews name people by public handle.
- **Not legal advice:** the entries are the project's own readings
  (ADR-0017).

## Tests and checks

- **The list:** the schema validates `LICENSES/allowlist.json`; Rust and
  Python read the same entries and compute the same list id; every
  allowed entry's text exists, in UTF-8 with LF; the file's form; the
  list id pinned with `GATE_VERSION`.
- **Schema mutations,** refused by the schema and by both readers: an
  expression as an id, unsorted or repeated ids, a forbidden entry with a
  text or without a reason, an allowed entry with `source_urls`,
  `contributions` without `world_data`, world data without `odbl`, a
  `notice` without `requires: ["notice"]`, an unknown obligation.
- **The gate, failing on purpose** (ADR-0017's list, and more): an NC
  source; an unknown id; a client-only id in the world; a forbidden
  source URL; ODbL in a contribution; a missing credit; an `OR`
  expression; a source with two ids; an extra id; a refresh with an
  uncovered licence. And passing: a refresh of the fixture tile; a
  contribution citing Google Open Buildings under CC BY 4.0.
- **Validation (C):** each of L1 to L4 gives its warning, the same in
  both languages, and the world stays valid with the same id. **(A):**
  the same mutations as errors, and new golden ids.
- **`op_world licences`:** the fixtures pass; each failure above fails
  it.
- **The ingest:** with recorded statements, an expected statement passes;
  a changed one stops with nothing written; a newer release past the
  terms check stops; a run from cache gives the same world id.
- **Notices and client:** `CREDITS.txt` and the client's credits page
  take their names and links from the list; every client credit's
  licence is a client entry.
- **No new crate or Python package.**

## Unresolved questions

None of these has a recommendation.

- **Refresh manifests in G5:** whether G5 should also require a
  refresh's manifests to name the ingest as `generator` and to list no
  `StreetWide contributions` source, matching ADR-0020's definition of
  an asset made by the ingest.
- **The list id in check reports:** a report could name the list id it
  used, not only `GATE_VERSION`. That changes the report's format, so
  the contribution contract.
- **Fixed notices per asset:** whether a licence with a `notice`
  (Copernicus) also puts it in each asset's attribution, or only in the
  world's notices, as ADR-0019's path suggests. Needed before M7.
- **Code dependencies:** whether crates and Python packages get a
  licence check of their own; they are outside this list.
- **Left open by ADR-0017** and unchanged here: CMU's motion capture,
  JAXA's AW3D30, the canton of Zurich's open data, and training data for
  the compiler.
