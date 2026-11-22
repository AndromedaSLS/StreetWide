//! Tile and region keys (RFC 0001, section 1): Web Mercator `z/x/y` cells at
//! zoom 17 (tiles) and zoom 12 (regions).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{Error, Result};

/// Zoom level of tiles.
pub const TILE_ZOOM: u32 = 17;
/// Zoom level of regions.
pub const REGION_ZOOM: u32 = 12;

macro_rules! cell_key {
    ($(#[$doc:meta])* $name:ident, $zoom:expr) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name {
            pub x: u32,
            pub y: u32,
        }

        impl $name {
            /// Number of cells along each axis at this zoom.
            pub const SIZE: u32 = 1 << $zoom;

            pub fn new(x: u32, y: u32) -> Result<Self> {
                if x < Self::SIZE && y < Self::SIZE {
                    Ok(Self { x, y })
                } else {
                    Err(Error::Invalid(format!(
                        "cell {}/{x}/{y} is out of range (x, y < {})",
                        $zoom,
                        Self::SIZE
                    )))
                }
            }

            /// Parses `z/x/y`; rejects leading zeros, signs and other zooms.
            pub fn parse(s: &str) -> Result<Self> {
                let invalid = || Error::Invalid(format!("{s:?} is not a zoom-{} key", $zoom));
                let mut parts = s.split('/');
                let (Some(z), Some(x), Some(y), None) =
                    (parts.next(), parts.next(), parts.next(), parts.next())
                else {
                    return Err(invalid());
                };
                if z != $zoom.to_string() {
                    return Err(invalid());
                }
                let x: u32 = x.parse().map_err(|_| invalid())?;
                let y: u32 = y.parse().map_err(|_| invalid())?;
                let key = Self::new(x, y)?;
                if key.to_string() != s {
                    return Err(invalid());
                }
                Ok(key)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}/{}/{}", $zoom, self.x, self.y)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(self, f)
            }
        }

        impl FromStr for $name {
            type Err = Error;
            fn from_str(s: &str) -> Result<Self> {
                Self::parse(s)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(
                deserializer: D,
            ) -> std::result::Result<Self, D::Error> {
                let s = String::deserialize(deserializer)?;
                Self::parse(&s).map_err(serde::de::Error::custom)
            }
        }
    };
}

cell_key!(
    /// A zoom-17 tile. Ordered by `x`, then `y`.
    TileKey,
    TILE_ZOOM
);
cell_key!(
    /// A zoom-12 region.
    RegionKey,
    REGION_ZOOM
);

impl TileKey {
    /// The region containing this tile.
    pub fn region(&self) -> RegionKey {
        let shift = TILE_ZOOM - REGION_ZOOM;
        RegionKey {
            x: self.x >> shift,
            y: self.y >> shift,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_region() {
        let t = TileKey::parse("17/68646/45908").unwrap();
        assert_eq!((t.x, t.y), (68646, 45908));
        assert_eq!(t.region().to_string(), "12/2145/1434");
        for bad in [
            "17/068646/1",
            "16/1/1",
            "17/1",
            "17/1/1/1",
            "17/+1/1",
            "17/131072/0",
            "17/-1/0",
        ] {
            assert!(TileKey::parse(bad).is_err(), "{bad} should be rejected");
        }
        assert!(RegionKey::parse("12/4095/0").is_ok());
        assert!(RegionKey::parse("12/4096/0").is_err());
    }
}
