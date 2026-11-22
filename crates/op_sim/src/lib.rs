//! StreetWide deterministic simulation.
//!
//! Runs the world at a fixed 60 Hz timestep on top of `op_kernel`: a player
//! capsule in Rapier (with `enhanced-determinism`) colliding against building
//! shells and terrain. Implemented in stage 4.
//!
//! `op_sim` depends only on `op_kernel` among workspace crates and never on
//! rendering (ADR-0002). Geometry arrives through kernel-defined types.
