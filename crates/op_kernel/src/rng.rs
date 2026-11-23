//! The simulation's only source of randomness: xoshiro256** seeded through
//! SplitMix64 (both by Blackman and Vigna). Integer-only, so identical on
//! every platform. The seed is stored in every replay.

use crate::hash::StateHasher;

/// SplitMix64, used to expand a 64-bit seed into the xoshiro state.
#[derive(Clone, Debug)]
pub struct SplitMix64(pub u64);

impl SplitMix64 {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

/// xoshiro256**.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Rng {
    s: [u64; 4],
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut sm = SplitMix64(seed);
        Rng {
            s: [sm.next_u64(), sm.next_u64(), sm.next_u64(), sm.next_u64()],
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        let s = &mut self.s;
        let result = s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        result
    }

    /// Uniform in [0, 1), with 24 random bits (exact in f32).
    pub fn next_unit_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 * (1.0 / 16_777_216.0)
    }

    pub fn hash_state(&self, h: &mut StateHasher) {
        for v in self.s {
            h.u64(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix_matches_the_reference() {
        // First output of the reference splitmix64.c for seed 0.
        assert_eq!(SplitMix64(0).next_u64(), 0xe220_a839_7b1d_cdaf);
    }

    #[test]
    fn sequence_is_pinned() {
        // Recorded when the generator was introduced; integer-only code, so
        // it must hold on every platform.
        let mut r = Rng::new(42);
        let mut h = StateHasher::new("rng");
        for _ in 0..1000 {
            h.u64(r.next_u64());
        }
        let u = r.next_unit_f32();
        assert!((0.0..1.0).contains(&u));
        assert_eq!(h.finish().hex()[..16], *PINNED_PREFIX);
    }

    const PINNED_PREFIX: &str = "15d0a7e293deeb98";
}
