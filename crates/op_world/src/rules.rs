//! Field-level rules shared by the document types. They mirror the patterns
//! in `schemas/common.schema.json`; the schema tests keep the two in sync.

use crate::cjson::is_control;
use crate::keys::TileKey;

/// Longest text field, in characters.
pub const MAX_TEXT_CHARS: usize = 4096;

/// Non-empty, at most [`MAX_TEXT_CHARS`] characters, no control characters.
pub fn is_text(s: &str) -> bool {
    !s.is_empty() && s.chars().count() <= MAX_TEXT_CHARS && !s.chars().any(is_control)
}

/// `^[a-z][a-z0-9_-]*:[!-~]+$`, at most 256 characters.
pub fn is_feature_id(s: &str) -> bool {
    if s.len() > 256 {
        return false;
    }
    let Some((ns, rest)) = s.split_once(':') else {
        return false;
    };
    let mut ns_chars = ns.chars();
    ns_chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && ns_chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        && !rest.is_empty()
        && rest.chars().all(|c| ('!'..='~').contains(&c))
}

/// One character of a source's parts: `[a-z0-9_-]`.
fn is_source_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-'
}

/// A source of points (RFC 0004 section 2), the id of a point without its
/// last part: `^[a-z][a-z0-9_-]*(:[a-z0-9][a-z0-9_-]*)*$`, such as
/// `stadt-zuerich:baumkataster` or `osm:node`.
pub fn is_source(s: &str) -> bool {
    let mut parts = s.split(':');
    let mut first = parts.next().unwrap_or_default().chars();
    first.next().is_some_and(|c| c.is_ascii_lowercase())
        && first.all(is_source_char)
        && parts.all(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                && chars.all(is_source_char)
        })
}

/// Longest point id, in characters (RFC 0004 section 2).
pub const MAX_POINT_ID_CHARS: usize = 256;

/// The source of a point id (RFC 0004 section 2), or `None` if `id` is not
/// one. A point id is `^[a-z][a-z0-9_-]*(:[a-z0-9][a-z0-9_-]*)*:[!-9;-~]+$`,
/// at most 256 characters: a source, a colon and the source's own local id,
/// printable ASCII without spaces or colons, such as
/// `stadt-zuerich:baumkataster:AA-1656`.
pub fn point_source(id: &str) -> Option<&str> {
    if id.len() > MAX_POINT_ID_CHARS {
        return None;
    }
    let (source, local) = id.rsplit_once(':')?;
    let local_ok = !local.is_empty()
        && local
            .chars()
            .all(|c| ('!'..='9').contains(&c) || (';'..='~').contains(&c));
    (local_ok && is_source(source)).then_some(source)
}

/// The source of the per-tile asset id `tile:17/{x}/{y}/{source}` of tile
/// `tile` (RFC 0004 section 2), or `None` if `feature_id` is not such an id
/// of that tile. Point assets always carry the source.
pub fn per_tile_source(feature_id: &str, tile: TileKey) -> Option<&str> {
    let source = feature_id
        .strip_prefix("tile:")?
        .strip_prefix(tile.to_string().as_str())?
        .strip_prefix('/')?;
    (is_feature_id(feature_id) && is_source(source)).then_some(source)
}

/// Longest rule id, in characters.
pub const MAX_RULE_ID_CHARS: usize = 128;

/// The id of a rule that sets estimated values (RFC 0004 section 3), such
/// as `adr-0015:yaw-from-orientierung:1`: the decision record that defines
/// it, the rule's name and its version, as
/// `^adr-[0-9]{4}:[a-z0-9]+(-[a-z0-9]+)*:[1-9][0-9]*$`, at most 128
/// characters.
pub fn is_rule_id(s: &str) -> bool {
    let mut parts = s.split(':');
    let (Some(adr), Some(name), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    s.len() <= MAX_RULE_ID_CHARS
        && adr
            .strip_prefix("adr-")
            .is_some_and(|n| n.len() == 4 && n.chars().all(|c| c.is_ascii_digit()))
        && name.split('-').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
        && version
            .chars()
            .next()
            .is_some_and(|c| ('1'..='9').contains(&c))
        && version.chars().all(|c| c.is_ascii_digit())
}

/// SPDX-style expression: `^[A-Za-z0-9.+:() -]+$`, at most 256 characters.
pub fn is_license(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 256
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || ".+:() -".contains(c))
}

/// One licence id, as an asset's licence names its sources' licences
/// (ADR-0017): `^[A-Za-z0-9.+:-]+$`, a licence expression with no spaces,
/// parentheses or operators.
pub fn is_license_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 256
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || ".+:-".contains(c))
}

/// The licence ids of an asset's licence, if it joins ids with ` AND `
/// only (ADR-0017), as `CC0-1.0 AND LicenseRef-swisstopo-OGD`; `None`
/// otherwise.
pub fn license_ids(s: &str) -> Option<Vec<&str>> {
    let ids: Vec<&str> = s.split(" AND ").collect();
    ids.iter().all(|id| is_license_id(id)).then_some(ids)
}

/// `^https://[!-~]+$`, at most 2048 characters.
pub fn is_https_url(s: &str) -> bool {
    s.len() <= 2048
        && s.strip_prefix("https://")
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| ('!'..='~').contains(&c)))
}

/// `^[a-z0-9]+(-[a-z0-9]+)*$`, at most 64 characters.
pub fn is_slug(s: &str) -> bool {
    s.len() <= 64
        && !s.is_empty()
        && s.split('-').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

/// Strictly increasing (sorted, no duplicates).
pub fn is_sorted_unique<T: Ord>(items: &[T]) -> bool {
    items.windows(2).all(|w| w[0] < w[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_ids() {
        for ok in [
            "osm:way:123",
            "swisstopo:swissbuildings3d:{ABC-1}",
            "tile:17/1/2",
            "streetwide:x",
        ] {
            assert!(is_feature_id(ok), "{ok}");
        }
        for bad in ["", "osm", "Osm:way:1", "osm:", "osm:way 1", "1osm:x", ":x"] {
            assert!(!is_feature_id(bad), "{bad}");
        }
    }

    #[test]
    fn point_ids_sources_and_per_tile_ids() {
        for (ok, source) in [
            (
                "stadt-zuerich:baumkataster:AA-1656",
                "stadt-zuerich:baumkataster",
            ),
            (
                "stadt-zuerich:beleuchtung:LEU105227",
                "stadt-zuerich:beleuchtung",
            ),
            ("osm:node:123", "osm:node"),
            ("streetwide:0-a", "streetwide"),
            ("a:1b:\"x\\;", "a:1b"),
        ] {
            assert_eq!(point_source(ok), Some(source), "{ok}");
        }
        let long = format!("a:{}", "x".repeat(254));
        assert_eq!(point_source(&long), Some("a"));
        for bad in [
            "",
            "osm",
            "osm:",
            ":1",
            "Osm:node:1",
            "osm::1",
            "osm:Node:1",
            "osm:node:1 2",
            "osm:node:\u{e9}",
            "1osm:node:1",
            "osm:_node:1",
            &format!("a:{}", "x".repeat(255)),
        ] {
            assert_eq!(point_source(bad), None, "{bad}");
        }
        let tile = TileKey::parse("17/68646/45903").unwrap();
        assert_eq!(
            per_tile_source("tile:17/68646/45903/stadt-zuerich:baumkataster", tile),
            Some("stadt-zuerich:baumkataster")
        );
        for bad in [
            "tile:17/68646/45903",
            "tile:17/68646/45903/",
            "tile:17/68646/45904/osm:node",
            "tile:17/068646/45903/osm:node",
            "tile:17/68646/45903/osm:node:",
            "tile:17/68646/45903/Osm",
            "tile:17/68646/45903osm",
        ] {
            assert_eq!(per_tile_source(bad, tile), None, "{bad}");
        }
    }

    #[test]
    fn rule_ids() {
        for ok in [
            "adr-0015:yaw-from-orientierung:1",
            "adr-0016:a:10",
            "adr-9999:x2-y:7",
        ] {
            assert!(is_rule_id(ok), "{ok}");
        }
        for bad in [
            "",
            "adr-0015:yaw:0",
            "adr-0015:yaw:01",
            "adr-015:yaw:1",
            "adr-00151:yaw:1",
            "rfc-0004:yaw:1",
            "adr-0015:Yaw:1",
            "adr-0015:yaw--x:1",
            "adr-0015:-yaw:1",
            "adr-0015::1",
            "adr-0015:yaw",
            "adr-0015:yaw:1:2",
            &format!("adr-0015:{}:1", "x".repeat(118)),
        ] {
            assert!(!is_rule_id(bad), "{bad}");
        }
        assert!(is_rule_id(&format!("adr-0015:{}:1", "x".repeat(117))));
    }

    #[test]
    fn licenses_urls_slugs() {
        assert!(is_license("CC-BY-SA-4.0"));
        assert!(is_license("LicenseRef-swisstopo-OGD"));
        assert!(is_license("MIT OR Apache-2.0"));
        assert!(!is_license("CC BY-SA ©"));
        for id in [
            "CC0-1.0",
            "LicenseRef-swisstopo-OGD",
            "GPL-2.0+",
            "DocumentRef-a:LicenseRef-b",
        ] {
            assert!(is_license_id(id), "{id}");
        }
        for not_id in [
            "",
            "MIT OR Apache-2.0",
            "CC0-1.0 AND ODbL-1.0",
            "(MIT)",
            "CC0 1.0",
            "©",
        ] {
            assert!(!is_license_id(not_id), "{not_id}");
        }
        assert_eq!(
            license_ids("CC0-1.0 AND LicenseRef-swisstopo-OGD"),
            Some(vec!["CC0-1.0", "LicenseRef-swisstopo-OGD"])
        );
        assert_eq!(license_ids("ODbL-1.0"), Some(vec!["ODbL-1.0"]));
        for not_and in [
            "MIT OR Apache-2.0",
            "(MIT AND CC0-1.0)",
            "MIT  AND CC0-1.0",
            "MIT AND",
            "MIT and CC0-1.0",
        ] {
            assert_eq!(license_ids(not_and), None, "{not_and}");
        }
        assert!(is_https_url("https://www.openstreetmap.org/copyright"));
        assert!(!is_https_url("http://example.org"));
        assert!(!is_https_url("https://"));
        assert!(is_slug("zurich-altstadt"));
        assert!(!is_slug("Zurich"));
        assert!(!is_slug("a--b"));
    }
}
