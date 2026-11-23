//! State hashing: BLAKE3 over an explicit, versioned byte encoding of the
//! simulation state. Floats are hashed by their bit patterns, so two states
//! hash equal only if they are bit-identical.

use std::fmt;

/// A 256-bit state hash.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StateHash(pub [u8; 32]);

impl StateHash {
    pub fn hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Parses 64 lowercase hex digits.
    pub fn from_hex(s: &str) -> Option<StateHash> {
        let bytes = s.as_bytes();
        if bytes.len() != 64 {
            return None;
        }
        let digit = |c: u8| match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            _ => None,
        };
        let mut out = [0u8; 32];
        for (o, pair) in out.iter_mut().zip(bytes.as_chunks::<2>().0) {
            *o = digit(pair[0])? << 4 | digit(pair[1])?;
        }
        Some(StateHash(out))
    }
}

impl fmt::Display for StateHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex())
    }
}

impl fmt::Debug for StateHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "StateHash({})", self.hex())
    }
}

/// Feeds typed values into BLAKE3. Every value has a fixed-size
/// little-endian encoding; strings and byte slices are length-prefixed, so
/// different sequences of values never produce the same byte stream.
pub struct StateHasher(blake3::Hasher);

impl StateHasher {
    /// Starts a hash in a named domain (e.g. `"streetwide.state.v1"`).
    pub fn new(domain: &str) -> Self {
        let mut h = StateHasher(blake3::Hasher::new());
        h.str(domain);
        h
    }

    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.0.update(&[v]);
        self
    }

    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.u8(u8::from(v))
    }

    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.0.update(&v.to_le_bytes());
        self
    }

    pub fn i16(&mut self, v: i16) -> &mut Self {
        self.0.update(&v.to_le_bytes());
        self
    }

    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.0.update(&v.to_le_bytes());
        self
    }

    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.0.update(&v.to_le_bytes());
        self
    }

    pub fn i64(&mut self, v: i64) -> &mut Self {
        self.0.update(&v.to_le_bytes());
        self
    }

    /// Hashes the bit pattern, so `0.0` and `-0.0` differ, as do NaNs.
    pub fn f32(&mut self, v: f32) -> &mut Self {
        self.u32(v.to_bits())
    }

    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.u64(v.len() as u64);
        self.0.update(v);
        self
    }

    pub fn str(&mut self, v: &str) -> &mut Self {
        self.bytes(v.as_bytes())
    }

    pub fn finish(&self) -> StateHash {
        StateHash(*self.0.finalize().as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_is_fixed() {
        let mut h = StateHasher::new("test");
        h.u64(7).f32(-0.0).str("a").bool(true);
        let mut expected = blake3::Hasher::new();
        expected.update(&4u64.to_le_bytes());
        expected.update(b"test");
        expected.update(&7u64.to_le_bytes());
        expected.update(&0x8000_0000u32.to_le_bytes());
        expected.update(&1u64.to_le_bytes());
        expected.update(b"a");
        expected.update(&[1]);
        assert_eq!(h.finish().0, *expected.finalize().as_bytes());
    }

    #[test]
    fn length_prefix_separates_strings() {
        let a = StateHasher::new("d").str("ab").str("c").finish();
        let b = StateHasher::new("d").str("a").str("bc").finish();
        assert_ne!(a, b);
    }

    #[test]
    fn hex_round_trips() {
        let h = StateHasher::new("x").finish();
        assert_eq!(StateHash::from_hex(&h.hex()), Some(h));
        assert_eq!(StateHash::from_hex("AB"), None);
    }
}
