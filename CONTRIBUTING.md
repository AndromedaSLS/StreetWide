# Contributing to StreetWide

Thank you for helping build an open world that only gets better. This guide
explains how contributions are made, checked and merged.

## Ground rules

1. **The canonical world only moves upward.** A change is merged only if it
   passes every check and is better than what it replaces.
2. **The simulation is deterministic.** See
   [Determinism rules](#determinism-rules). A determinism test is never
   weakened or skipped to make it pass; we find the cause.
3. **Contracts before features.** Changes to the world format, module
   interfaces or replay format start with an RFC (see
   [Decisions](#decisions-adrs-and-rfcs)).
4. **Every asset carries provenance and licence** in its manifest.
5. **Private by design.** Interiors are fictional. Never model, describe or
   reconstruct a real private home.
6. **No large data in Git.** Source data is downloaded and cached by
   `tools/ingest`; only small test fixtures are committed.

## Sign your work: Developer Certificate of Origin

We use the DCO instead of a contributor licence agreement. You keep the
copyright of your contribution; by signing off, you certify the following:

```
Developer Certificate of Origin
Version 1.1

Copyright (C) 2004, 2006 The Linux Foundation and its contributors.

Everyone is permitted to copy and distribute verbatim copies of this
license document, but changing it is not allowed.


Developer's Certificate of Origin 1.1

By making a contribution to this project, I certify that:

(a) The contribution was created in whole or in part by me and I
    have the right to submit it under the open source license
    indicated in the file; or

(b) The contribution is based upon previous work that, to the best
    of my knowledge, is covered under an appropriate open source
    license and I have the right under that license to submit that
    work with modifications, whether created in whole or in part
    by me, under the same open source license (unless I am
    permitted to submit under a different license), as indicated
    in the file; or

(c) The contribution was provided directly to me by some other
    person who certified (a), (b) or (c) and I have not modified
    it.

(d) I understand and agree that this project and the contribution
    are public and that a record of the contribution (including all
    personal information I submit with it, including my sign-off) is
    maintained indefinitely and may be redistributed consistent with
    this project or the open source license(s) involved.
```

Sign off every commit with `git commit -s`. This adds a line like:

```
Signed-off-by: Your Name <you@example.org>
```

The email must match the commit's author email. CI rejects pull requests
with unsigned commits. To fix a branch, run
`git rebase --signoff origin/main` and force-push.

## Before you open a pull request

Run the same checks as CI:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python -m pip install -r tools/requirements.txt   # once, ideally in a venv
python tools/ci/check_deps.py          # dependency direction (needs Python 3.11+)
python -m pytest                       # tests for the Python tools
```

If you change a `.glb` fixture, also run the Khronos glTF validator:
`cd tools/ci/gltf && npm ci --ignore-scripts && npm run check`.

Keep pull requests focused, explain *why* in the description, and update
[docs/progress.md](docs/progress.md) when you complete a milestone.

## Architecture rules

- Crates: `op_kernel` (module API, schedule), `op_sim` (deterministic
  simulation), `op_replay`, `op_world` (format and hashing), `op_stream`,
  `op_client` (Bevy app).
- **Dependencies point inward.** `op_sim` depends only on `op_kernel`.
  Nothing depends on `op_client`. The simulation never depends on
  rendering. The allowed edges live in
  [tools/ci/deps-policy.toml](tools/ci/deps-policy.toml) and are enforced in
  CI ([ADR-0002](docs/adr/0002-crate-layout-and-dependency-direction.md)).
- Bevy and Rapier versions are pinned in the workspace `Cargo.toml`
  ([ADR-0005](docs/adr/0005-toolchain-and-engine-pinning.md)).

## Determinism rules

These apply to all simulation code (everything except `op_client`'s
rendering):

- Fixed 60 Hz timestep. No wall-clock time.
- Randomness only from the seeded RNG whose seed is stored in the replay.
- No iteration over `HashMap`/`HashSet`; use `BTreeMap` or `IndexMap`.
- No platform trig or transcendental functions (`sin`, `cos`, `atan2`,
  `exp`, `powf`, ...); use the deterministic math helpers in `op_kernel`.

Most of these are enforced by `clippy.toml`. Don't silence those lints with
`#[allow]`; if you need an exception, open an issue.

## Decisions: ADRs and RFCs

- **ADR** (`docs/adr/`): records one foundational decision (a technology, a
  rule, a boundary) with the options considered. Copy
  [the template](docs/adr/0000-template.md).
- **RFC** (`rfcs/`): proposes a change to a contract other people build
  against: the world format, module interfaces, the replay format, the merge
  rules. Copy [the template](rfcs/0000-template.md).

Anything not covered by an existing ADR or RFC is drafted as options first,
not decided silently in code.

## Contributing world content

World content (meshes, textures, manifests) is licensed under
**CC BY-SA 4.0**. Layers derived from OpenStreetMap are **ODbL 1.0** and
must stay in separate assets. Every asset needs a manifest with its source
datasets, licence and attribution. If you add a new data source, it must have
an open licence compatible with the above, and you must credit it in
[ATTRIBUTION.md](ATTRIBUTION.md).

Interiors must be fictional: do not use floor plans, photos or descriptions
of real private homes.

## Licensing of code

Code is dual-licensed under MIT OR Apache-2.0. Unless you explicitly state
otherwise, any contribution you intentionally submit for inclusion is
licensed the same way, without additional terms or conditions.

## Conduct

Everyone participating is expected to follow the
[Code of Conduct](CODE_OF_CONDUCT.md).
