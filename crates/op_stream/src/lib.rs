//! StreetWide tile store and streaming.
//!
//! Loads content-addressed world tiles from disk for the simulation
//! (synchronously, in a deterministic order) and prefetches them for the
//! client. Implemented in stage 5.
