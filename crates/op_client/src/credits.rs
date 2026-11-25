//! The credits page (C): each source of the world's data, its licence and
//! a link, and the licence the world is offered under (ADR-0019, "Notices
//! in the client"). The credit line, always on screen, and the help panel
//! (F1) carry the short form.
//!
//! The sources are those [ATTRIBUTION.md] credits, matched by the credits
//! the world manifest lists (`attribution`, the union of every asset's
//! credits). A credit the client does not know is listed too, with where
//! its licence is: the manifests of the assets it credits, which inspect
//! mode (I) shows.
//!
//! [ATTRIBUTION.md]: https://github.com/AndromedaSLS/StreetWide/blob/main/ATTRIBUTION.md

/// A source of world data, as ATTRIBUTION.md credits it.
struct Source {
    /// The credit its assets carry, as the world manifest lists it.
    credit: &'static str,
    /// What it is, and what the world takes from it.
    what: &'static str,
    /// Its licence ...
    licence: &'static str,
    /// ... and where to read it.
    link: &'static str,
}

const SWISSTOPO_TERMS: &str = "swisstopo.admin.ch/en/terms-of-use-free-geodata-and-geoservices";

/// Every source ATTRIBUTION.md credits, in its order.
const SOURCES: [Source; 4] = [
    Source {
        credit: "© swisstopo",
        what: "swissBUILDINGS3D 3.0 Beta, by swisstopo: buildings",
        licence: "swisstopo terms for free geodata",
        link: SWISSTOPO_TERMS,
    },
    Source {
        credit: "© swisstopo",
        what: "swissALTI3D, by swisstopo: terrain, and the heights of streets, bridges and water",
        licence: "swisstopo terms for free geodata",
        link: SWISSTOPO_TERMS,
    },
    Source {
        credit: "© OpenStreetMap contributors",
        what: "OpenStreetMap: streets, paths, bridges, water and a few buildings",
        licence: "Open Database License (ODbL) 1.0",
        link: "openstreetmap.org/copyright",
    },
    Source {
        credit: "The StreetWide contributors",
        what: "StreetWide contributions: what players added, such as facades",
        licence: "CC BY-SA 4.0",
        link: "creativecommons.org/licenses/by-sa/4.0",
    },
];

/// The ODbL's own page.
const ODBL: &str = "opendatacommons.org/licenses/odbl/1-0";

/// The credits page for a world whose manifest lists `attribution`.
pub fn credits_text(attribution: &[String]) -> String {
    let mut lines = vec!["CREDITS AND LICENCES · C: close · F1: help".to_string()];
    // A world with OpenStreetMap data is offered as a whole under the
    // ODbL (ADR-0019, option A).
    if attribution.iter().any(|a| a.contains("OpenStreetMap")) {
        lines.push(format!(
            "The world is offered under the Open Database License (ODbL) 1.0: {ODBL}."
        ));
        lines.push("Inside it, each asset keeps its sources' licence and credits;".into());
    } else {
        lines.push("Each asset keeps its sources' licence and credits;".into());
    }
    lines.push("press I and point at one to see them.".into());
    for s in SOURCES
        .iter()
        .filter(|s| attribution.iter().any(|a| a == s.credit))
    {
        lines.push(s.what.into());
        lines.push(format!("        {} · {} · {}", s.credit, s.licence, s.link));
    }
    for credit in attribution
        .iter()
        .filter(|a| !SOURCES.iter().any(|s| s.credit == a.as_str()))
    {
        lines.push(credit.clone());
        lines.push("        its licence: in the manifest of each asset it credits (I)".into());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn repo() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// The fixture world's page names the ODbL, and lists each source with
    /// its credit, licence and link.
    #[test]
    fn the_page_lists_each_source_with_its_licence_and_a_link() {
        let store = op_world::Store::new(repo().join("fixtures/world"));
        let (_, world) = store.read_world().unwrap();
        let text = credits_text(&world.attribution);
        let expected = [
            "CREDITS AND LICENCES · C: close · F1: help",
            "The world is offered under the Open Database License (ODbL) 1.0: opendatacommons.org/licenses/odbl/1-0.",
            "Inside it, each asset keeps its sources' licence and credits;",
            "press I and point at one to see them.",
            "swissBUILDINGS3D 3.0 Beta, by swisstopo: buildings",
            "        © swisstopo · swisstopo terms for free geodata · swisstopo.admin.ch/en/terms-of-use-free-geodata-and-geoservices",
            "swissALTI3D, by swisstopo: terrain, and the heights of streets, bridges and water",
            "        © swisstopo · swisstopo terms for free geodata · swisstopo.admin.ch/en/terms-of-use-free-geodata-and-geoservices",
            "OpenStreetMap: streets, paths, bridges, water and a few buildings",
            "        © OpenStreetMap contributors · Open Database License (ODbL) 1.0 · openstreetmap.org/copyright",
        ];
        assert_eq!(text, expected.join("\n"));
    }

    /// Contributions are credited once merged; a credit the client does not
    /// know is listed with where its licence is; a world without
    /// OpenStreetMap data is not said to be under the ODbL.
    #[test]
    fn contributions_and_unknown_credits_are_listed() {
        let text = credits_text(&["The StreetWide contributors".into(), "© Somewhere".into()]);
        assert!(!text.contains("ODbL"), "{text}");
        assert!(
            text.contains(
                "StreetWide contributions: what players added, such as facades\n        The StreetWide contributors · CC BY-SA 4.0 · creativecommons.org/licenses/by-sa/4.0\n"
            ),
            "{text}"
        );
        assert!(
            text.ends_with(
                "© Somewhere\n        its licence: in the manifest of each asset it credits (I)"
            ),
            "{text}"
        );
    }

    /// The page credits what ATTRIBUTION.md credits, with its links.
    #[test]
    fn the_sources_are_those_of_attribution_md() {
        let attribution = std::fs::read_to_string(repo().join("ATTRIBUTION.md")).unwrap();
        for s in &SOURCES {
            let name = s.what.split([',', ':']).next().unwrap();
            for text in [name, s.credit, s.link] {
                assert!(attribution.contains(text), "{text}");
            }
        }
        assert!(attribution.contains(ODBL));
    }
}
