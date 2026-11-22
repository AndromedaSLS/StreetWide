//! Content ids: `blake3:` + 64 lowercase hex digits of a file's BLAKE3 hash.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{Error, Result};

/// BLAKE3-256 hash of a file's exact bytes (RFC 0001, section 5).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentId([u8; 32]);

impl ContentId {
    /// The string prefix of every content id.
    pub const PREFIX: &'static str = "blake3:";

    /// Hashes `bytes`.
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The 64 lowercase hex digits, without prefix.
    pub fn hex(&self) -> String {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut s = String::with_capacity(64);
        for b in self.0 {
            s.push(char::from(DIGITS[usize::from(b >> 4)]));
            s.push(char::from(DIGITS[usize::from(b & 0x0f)]));
        }
        s
    }

    /// Parses `blake3:<64 lowercase hex digits>`.
    pub fn parse(s: &str) -> Result<Self> {
        let invalid = || Error::Invalid(format!("{s:?} is not a content id (blake3:<64 hex>)"));
        let hex = s.strip_prefix(Self::PREFIX).ok_or_else(invalid)?;
        if hex.len() != 64 {
            return Err(invalid());
        }
        let mut out = [0u8; 32];
        let (pairs, _) = hex.as_bytes().as_chunks::<2>();
        for (i, pair) in pairs.iter().enumerate() {
            let hi = hex_value(pair[0]).ok_or_else(invalid)?;
            let lo = hex_value(pair[1]).ok_or_else(invalid)?;
            out[i] = (hi << 4) | lo;
        }
        Ok(Self(out))
    }
}

fn hex_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

impl fmt::Display for ContentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", Self::PREFIX, self.hex())
    }
}

impl fmt::Debug for ContentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl FromStr for ContentId {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Self::parse(s)
    }
}

impl Serialize for ContentId {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ContentId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vector() {
        // BLAKE3 of the empty input (from the BLAKE3 specification).
        assert_eq!(
            ContentId::of(b"").to_string(),
            "blake3:af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
    }

    #[test]
    fn parse_round_trip_and_rejects() {
        let id = ContentId::of(b"streetwide");
        assert_eq!(ContentId::parse(&id.to_string()), Ok(id));
        let upper = id.to_string().to_uppercase().replace("BLAKE3", "blake3");
        assert!(ContentId::parse(&upper).is_err());
        assert!(ContentId::parse(&id.hex()).is_err());
        assert!(ContentId::parse("blake3:abc").is_err());
    }
}
