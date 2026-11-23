# Generate a reference page from the JSON Schemas

Labels: `good first issue`, `area: docs`

## Why

The world format and the replay header are defined by the JSON Schemas in
`schemas/`. RFC 0001 explains them in prose, but there is no
field-by-field reference. Contributors writing tools in other languages
have to read raw JSON Schema.

## What to do

1. Write `tools/docs/schema_reference.py`. It reads every
   `schemas/*.schema.json` and writes `docs/schemas.md`, with one section
   per document type:
   - each field with its type, whether it is required, its constraints
     (patterns, ranges, enums) and its `description`;
   - `$ref` resolved to the shared definitions in `common.schema.json`.
2. Add a CI step (in the `python` job) that regenerates the page and fails
   if it differs from the committed one, so the page never goes stale.
3. Link the page from RFC 0001 and the README's repository layout.

## Done when

- `docs/schemas.md` covers the asset manifest, tile index, world manifest
  and replay header.
- Running the script twice gives identical output: sorted, no timestamps.
- A pytest checks the generator on a small inline schema.

## Pointers

- `schemas/`, `tools/worldfmt/schemas.py` (loads the schemas and resolves
  references)
- RFC 0001, ADR-0007
