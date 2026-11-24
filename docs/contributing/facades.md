# Contributing a facade

StreetWide's buildings come from swisstopo's 3D data: accurate walls and
roofs, but plain. A **facade** adds what you see from the street (window
frames, doors, cornices, balconies, shop fronts) as a separate layer on
top of a building. Facades are the first thing anyone can contribute
([RFC 0002](../../rfcs/0002-contributions-and-merge-rules.md)).

This guide takes you from picking a building to a contribution that passes
StreetWide's automatic checks, and shows you how to see it in the game.

> **Status:** you can do everything below today, on your own machine.
> Submitting to the shared world opens once the world repository exists
> ([ADR-0010](../adr/0010-hosting-the-canonical-world.md)).

## What makes a good facade

The automatic checks enforce these rules:

| Rule | Why |
|---|---|
| Stay within **0.5 m of the building's walls**, and no higher than its roof | A facade belongs to its building |
| At most **50,000 triangles** and **4 MB** per building | Everyone has to download and draw it |
| **Your own work**, or open sources you name | The world is CC BY-SA 4.0 |
| **No Google, Apple or Bing imagery** (Street View, aerial images) and no copyrighted plans | Their terms forbid it |
| Show only what is **visible from public ground** | Privacy: StreetWide never models people's homes from the inside |

A reviewer then checks what software can't:
- **Matches the real building:** openings, proportions, storeys, roof
  edge.
- **Fits its neighbours:** about the same level of detail as the buildings
  around it.
- **No people, number plates or names:** nothing on doorbells,
  letterboxes or name plates.

Facades are visual only in this first version: you walk through them,
not into them, so they can never block a street.

## What you need

- StreetWide built as in the README's
  [Getting started](../../README.md#getting-started), steps 1 to 3. You
  need the Zurich district in `data/world/zurich-altstadt`, or you can try
  everything on the small test tile in `fixtures/world`.
- A 3D editor that exports glTF binary files (`.glb`). This guide uses
  [Blender](https://www.blender.org) (free).
- Your own photos or notes of the building, taken from the street.

On Windows, write `.venv\Scripts\python` where this guide says
`.venv/bin/python`.

## 1. Pick a building and get its reference

**In the game.** Walk or fly to the building, press **I** and point the
crosshair at it:
- **amber** means it has no facade yet: it is yours to model;
- **blue** means someone has contributed one. A better facade can still
  replace it.

The panel shows the building's id (`swisstopo:swissbuildings3d:…`), who
made it and its licence. Click (once more if the first click captured the
mouse): the terminal you started the client from prints the same, and the
exact command for this step with your world's path. Run it in the
repository's folder; if you started the client elsewhere, it says where
that is. On Windows the client prints the PowerShell form, or the Git Bash
form if you started it from Git Bash. For example:

```sh
.venv/bin/python -m tools.contrib reference --world 'data/world/zurich-altstadt' --building 'swisstopo:swissbuildings3d:000f609c-4003-495f-9426-b43e3d770167'
```

**From coordinates.** Find the building's coordinates, for example on
[openstreetmap.org](https://www.openstreetmap.org): right-click it and
choose "Show address". Then:

```sh
.venv/bin/python -m tools.contrib reference --world data/world/zurich-altstadt --at 8.54397,47.37017
```

Either way, this prints the building's id and writes
`contrib/<id>-shell.glb`: the building's walls and roof, exactly where they
are in the world.

A few buildings come from OpenStreetMap, where swissBUILDINGS3D has none;
the panel says so. If their outline or height is wrong, fix it in
OpenStreetMap. Model their facade from your own observations: their shell
is under the ODbL, so use it only to place your facade, and don't copy its
geometry (in step 4, never `--derived-from-shell`).

## 2. Model the facade

1. In Blender: **File → Import → glTF 2.0**, and open the reference file.
2. **Don't move, rotate or scale the reference.** Its position is the
   building's real position. Model your facade around it.
3. Model the facade elements as separate objects in front of the walls,
   within 0.5 m of them. Typical elements are window frames and sills,
   doors, cornices, pilasters and balconies.
4. **Materials:** a material whose name contains `wall`, `roof`, `ground`,
   `path` or `bridge` gets that StreetWide colour; anything else becomes
   `wall`. Textures are not used in this version.
5. Keep it light: a typical old-town facade needs a few thousand triangles.

## 3. Export your facade

1. Select **only your facade objects**, not the reference.
2. **File → Export → glTF 2.0**, with:
   - format: *glTF Binary (.glb)*;
   - include: *Limit to → Selected Objects*;
   - transform: *+Y Up* (the default);
   - mesh: *Apply Modifiers*.

## 4. Turn it into a contribution

```sh
.venv/bin/python -m tools.contrib facade \
  --world data/world/zurich-altstadt \
  --building swisstopo:swissbuildings3d:<id> \
  --model my-facade.glb \
  --author github:<your-login> \
  --title "Facade of <street> <number>" \
  --rationale "Modelled from my own photos taken on <date>." \
  --out my-facade
```

- Add `--derived-from-shell` if you copied geometry from the reference
  itself. Your facade then also carries swisstopo's credit and terms.
- Add `--fictional` if the facade is invented rather than observed.

The tool writes a **bundle**: the folder `my-facade/`, holding
`change.json` (your change set) and your mesh and its manifest in
`objects/`. Your facade is licensed CC BY-SA 4.0 and credited to "The
StreetWide contributors". The manifest names your handle as the source.

## 5. Run the checks

```sh
cargo run -p op_replay -- check-change data/world/zurich-altstadt my-facade
```

You want every line to read `pass`, `not applicable` or `review`:
- **G6 privacy** always says `review`: a person looks at every facade.
- **G8 placement** fails if part of your facade is more than 0.5 m from
  the walls, which usually means the reference was moved in the editor.
- **G7 budgets** fails if the model is too heavy.

The command ends with "passes the gate" when the bundle is ready.

## 6. See it in the game

Merge your bundle into a private copy of the world and walk to it:

```sh
cp -r data/world/zurich-altstadt preview-world
cargo run -p op_world -- baseline preview-world --record preview-0.json --reason "local preview"
cargo run -p op_world -- merge preview-world my-facade --previous preview-0.json \
  --record preview-1.json --report preview-1-report.json \
  --reason "local preview" --review "github:<your-login>=better:local preview"
cargo run --release -p op_client -- --world preview-world
```

(On Windows PowerShell, `Copy-Item -Recurse data\world\zurich-altstadt preview-world`.)

Press I and point at your building: it turns blue, and the panel names you
as the author of its facade, with its licence and credits.

The review you give yourself here only lets the local merge through. In
the shared world, reviewers decide.

## 7. Submit

Submission opens when the canonical world repository exists
([ADR-0010](../adr/0010-hosting-the-canonical-world.md)). You will then
open a pull request with your bundle, and the gate will run on it
automatically.

Questions and early facades are welcome in the issue tracker.
