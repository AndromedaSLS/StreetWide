//! How world geometry reaches the simulation.
//!
//! The simulation works in a **sim frame**: the region frame of RFC 0001
//! (X east, Y up, Z south) shifted by an integer-millimetre origin near
//! the player, so f32 coordinates stay small and precise. Geometry arrives
//! in integer millimetres in that frame and is converted to f32 metres in
//! exactly one way ([`mm_to_m`]), so every platform builds bit-identical
//! colliders.
//!
//! A [`GeometrySource`] may not have data yet (it is still streaming); it
//! then answers [`Fetch::Pending`] and the tick stalls without changing any
//! state (see [`crate::schedule`]).

use std::fmt;
use std::sync::Arc;

/// Axis-aligned bounds in integer millimetres, `min <= max` per axis.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct BoundsMm {
    pub min: [i64; 3],
    pub max: [i64; 3],
}

impl BoundsMm {
    /// The box `centre +- half` on each axis.
    pub fn around(centre: [i64; 3], half: [i64; 3]) -> Self {
        BoundsMm {
            min: [0, 1, 2].map(|i| centre[i] - half[i]),
            max: [0, 1, 2].map(|i| centre[i] + half[i]),
        }
    }

    /// Closed-interval overlap on all three axes.
    pub fn intersects(&self, other: &BoundsMm) -> bool {
        (0..3).all(|i| self.min[i] <= other.max[i] && other.min[i] <= self.max[i])
    }

    pub fn translated(&self, offset: [i64; 3]) -> Self {
        BoundsMm {
            min: [0, 1, 2].map(|i| self.min[i] + offset[i]),
            max: [0, 1, 2].map(|i| self.max[i] + offset[i]),
        }
    }
}

/// Identifies one placed asset. The derived order (tile, layer, feature) is
/// the order in which the simulation adds and removes colliders.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct AssetKey {
    /// z17 tile `[x, y]`.
    pub tile: [u32; 2],
    pub layer: String,
    pub feature_id: String,
}

/// An asset the simulation collides with.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ColliderAsset {
    pub key: AssetKey,
    /// BLAKE3 of the asset's mesh file (RFC 0001 content id).
    pub content_id: [u8; 32],
    /// Bounds in the sim frame.
    pub bounds_mm: BoundsMm,
}

/// A collision mesh in the sim frame.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ColliderMesh {
    pub vertices_mm: Vec<[i64; 3]>,
    pub triangles: Vec<[u32; 3]>,
}

/// The answer of a [`GeometrySource`]: the data, or "not loaded yet".
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Fetch<T> {
    Ready(T),
    Pending,
}

/// The source could not provide geometry (missing or corrupt data).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GeometryError(pub String);

impl fmt::Display for GeometryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for GeometryError {}

/// Provides collision geometry in the sim frame.
///
/// Implementations must be pure functions of the world they serve: the
/// same question always gets the same answer once it is `Ready`.
pub trait GeometrySource {
    /// Collider assets whose bounds intersect `area`, in any order.
    fn colliders_in(&mut self, area: &BoundsMm)
    -> Result<Fetch<Vec<ColliderAsset>>, GeometryError>;

    /// The mesh of one asset returned by [`Self::colliders_in`].
    fn mesh(&mut self, asset: &ColliderAsset) -> Result<Fetch<Arc<ColliderMesh>>, GeometryError>;
}

/// Metres (f32) of an integer number of millimetres: the f32 nearest to the
/// f64 `mm / 1000`. The same conversion as the RFC 0001 glb profile.
pub fn mm_to_m(mm: i64) -> f32 {
    (mm as f64 / 1000.0) as f32
}

/// Integer millimetres of a sim-frame coordinate, rounded half away from
/// zero.
pub fn m_to_mm(m: f32) -> i64 {
    (f64::from(m) * 1000.0).round() as i64
}

/// The sim-frame origin for a session that starts at `spawn_mm` (region
/// frame): the spawn point's horizontal position at height 0.
pub fn sim_origin_mm(spawn_mm: [i64; 3]) -> [i64; 3] {
    [spawn_mm[0], 0, spawn_mm[2]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_overlap_is_closed() {
        let a = BoundsMm::around([0, 0, 0], [10, 10, 10]);
        let b = BoundsMm::around([20, 0, 0], [10, 10, 10]);
        assert!(a.intersects(&b));
        assert!(!a.intersects(&b.translated([1, 0, 0])));
    }

    #[test]
    fn millimetre_conversion_round_trips() {
        for mm in [-500_000, -1, 0, 1, 410_123, 4_000_000] {
            assert_eq!(m_to_mm(mm_to_m(mm)), mm);
        }
    }
}
