# ADR-0002: Crate layout and dependency direction

- **Status:** Accepted (2026-11-23, stage 1 checkpoint)
- **Date:** 2026-11-22
- **Deciders:** @victorpicaud

## Context

CLAUDE.md fixes six crates and the rule that dependencies point inward:
`op_sim` depends only on `op_kernel`, nothing depends on `op_client`, and the
simulation never depends on rendering. Bevy must stay behind `op_kernel` and
`op_client`.

One open question was whether `op_kernel` itself should be built on Bevy's
ECS (`bevy_ecs`) for its schedule, or be engine-free.

## Options considered

### A. Engine-free kernel; Bevy only in `op_client`

`op_kernel` defines its own minimal module trait and a fixed-order schedule.
`op_client` runs the kernel inside Bevy's `FixedUpdate`.

- Pros: we control iteration and execution order completely, which matters
  for determinism. Bevy upgrades (roughly every four months, usually
  breaking) never touch simulation code. Determinism checks and replay runs
  build without compiling Bevy, so CI is fast on all platforms.
- Cons: we write and maintain a small scheduler; simulation modules cannot
  reuse Bevy ECS idioms.

### B. Kernel built on `bevy_ecs` (single-threaded executor)

- Pros: one ECS for simulation and client; familiar to Bevy users.
- Cons: query iteration order and schedule behaviour are implementation
  details that can change between Bevy versions, so every upgrade risks
  determinism; `op_sim` pulls in `bevy_ecs`; slower CI.

## Decision

**Option A.** The allowed workspace dependency edges are:

```
op_kernel  → (none)
op_sim     → op_kernel                          (+ rapier3d)
op_world   → (none)
op_stream  → op_kernel, op_world
op_replay  → op_kernel, op_sim, op_world, op_stream
op_client  → op_kernel, op_sim, op_world, op_stream, op_replay (+ bevy)
```

These edges apply to normal, build and dev dependencies. No crate other than
`op_client` may have Bevy, wgpu, winit, naga or gilrs anywhere in its
dependency closure.

## Consequences

- The policy lives in `tools/ci/deps-policy.toml`; `tools/ci/check_deps.py`
  enforces it in CI from `cargo metadata`. A new crate fails CI until it is
  added to the policy, which needs an update to this ADR.
- Geometry reaches `op_sim` through types defined in `op_kernel`, not
  through `op_world`; `op_replay` and `op_client` do the loading.
- `op_client` owns all rendering, input capture and interpolation; it may be
  nondeterministic, and its `clippy.toml` relaxes the determinism bans.
