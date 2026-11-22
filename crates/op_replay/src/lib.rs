//! StreetWide replays.
//!
//! The `.oprep` format: a header naming the world and seed, the per-tick
//! player inputs, and a trail of state hashes. Playing a replay back must
//! reproduce every hash bit for bit. Implemented in stage 4.
