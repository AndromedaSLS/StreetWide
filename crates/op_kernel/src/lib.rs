//! StreetWide kernel.
//!
//! The kernel owns the contracts every simulation module builds on: the module
//! API, the fixed 60 Hz schedule, deterministic math helpers, the seeded RNG
//! and per-tick state hashing. It is implemented in stage 4.
//!
//! Rules (CLAUDE.md, ADR-0002, ADR-0003):
//! - no engine or rendering dependencies (Bevy lives only in `op_client`);
//! - no wall-clock time, no unseeded randomness, no iteration over unordered
//!   maps, no platform trig/transcendental functions.
