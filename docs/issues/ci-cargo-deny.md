# Licence and advisory checks with cargo-deny

Labels: `good first issue`, `area: ci`

## Why

The code is `MIT OR Apache-2.0` (ADR-0001). Nothing yet checks that new
Rust dependencies have compatible licences, or that none has a known
security advisory.

## What to do

1. Add a `deny.toml` at the repository root:
   - allow the licences currently used by the dependency tree (run
     `cargo deny list` to see them);
   - deny copyleft licences for code;
   - enable advisories from the RustSec database;
   - ban duplicate versions of `bevy*` and `rapier3d`, which must stay
     pinned (ADR-0005).
2. Add a `deny` job to `.github/workflows/ci.yml` using the official
   `EmbarkStudios/cargo-deny-action`, pinned to a commit SHA like the
   other actions.
3. Mention the check in CONTRIBUTING.md.

## Done when

- `cargo deny check` passes locally and in CI.
- Every allowed licence in `deny.toml` has a short comment saying why it
  is acceptable.
- A deliberately added crate with a disallowed licence makes the job fail.
  Try it in the PR description; don't commit it.

## Pointers

- `.github/workflows/ci.yml`, `Cargo.lock`
- ADR-0001, ADR-0005
