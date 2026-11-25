//! Field-level rules shared by the document types. They mirror the patterns
//! in `schemas/common.schema.json`; the schema tests keep the two in sync.

use crate::cjson::is_control;

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

/// SPDX-style expression: `^[A-Za-z0-9.+:() -]+$`, at most 256 characters.
pub fn is_license(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 256
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || ".+:() -".contains(c))
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
    fn licenses_urls_slugs() {
        assert!(is_license("CC-BY-SA-4.0"));
        assert!(is_license("LicenseRef-swisstopo-OGD"));
        assert!(is_license("MIT OR Apache-2.0"));
        assert!(!is_license("CC BY-SA ©"));
        assert!(is_https_url("https://www.openstreetmap.org/copyright"));
        assert!(!is_https_url("http://example.org"));
        assert!(!is_https_url("https://"));
        assert!(is_slug("zurich-altstadt"));
        assert!(!is_slug("Zurich"));
        assert!(!is_slug("a--b"));
    }
}
