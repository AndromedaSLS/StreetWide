//! Inspect mode (I): what is under the crosshair, who made it, under which
//! licence, and how to improve it. The roadmap's M1 ("Newcomers") asks the
//! client to mark baseline buildings as improvable and link to the facade
//! guide.
//!
//! In inspect mode a crosshair marks the centre of the screen. What it is
//! on is highlighted as a whole: a building (its shell and its facade
//! together), or one asset (a tile's terrain, its streets and paths, its
//! water, or a bridge deck). Amber is a building without contributions,
//! which is improvable; blue a building with a contributed facade or shell,
//! or any contributed asset; white the rest. A panel (`hud`) names it, says
//! whether it is baseline or a contribution and by whom, with its licence
//! and credits; for a building it links the facade guide and gives the
//! facade tool's first command. A click prints the same to the terminal
//! (stdout, which nothing else writes to: Bevy logs to stderr), where the
//! command can be copied.
//!
//! - **Walk and fly views.** The ray runs from the camera through the
//!   centre of the screen, never from the mouse pointer, as far as the
//!   camera draws (its far plane): flying high, the city below.
//! - **Map view: paused.** There the pointer and the click belong to travel
//!   (ADR-0013), and from 80 m up a building is a few pixels. Inspect
//!   resumes on leaving the map.
//! - **Replays: works.** In walk view the crosshair follows the recorded
//!   gaze; V flies freely.
//!
//! Baseline or contribution follows RFC 0002: an asset made by the ingest
//! has the generator `streetwide-ingest` (section 7); a contribution lists a
//! "StreetWide contributions" source naming its author (check G5), whatever
//! its generator says.
//!
//! Rendering only: inspect reads the scene's meshes (`pick`) and the
//! world's manifests. It never touches the simulation's input or state, and
//! changes nothing in the world.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use bevy::input::keyboard::Key;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use op_kernel::AssetKey;
use op_world::gate::CONTRIBUTIONS_DATASET;
use op_world::glb::Material;
use op_world::{AssetManifest, Collider, ContentId, Layer, ObjectKind, Store, TileKey};

use crate::materials::{Looks, MaterialSet};
use crate::pick::{PickIndex, PickStats};
use crate::scene::{PlayerCamera, SceneSystems, Shown, asset_key};
use crate::session::Session;
use crate::travel::{Travel, View, letter};

/// `provenance.generator.name` of every asset the ingest makes (RFC 0002
/// section 7; `GENERATOR` in `tools/ingest`).
pub const INGEST_GENERATOR: &str = "streetwide-ingest";
/// The facade guide, in the repository and online.
pub const FACADE_GUIDE: &str = "docs/contributing/facades.md";
pub const GUIDE_URL: &str =
    "https://github.com/AndromedaSLS/StreetWide/blob/main/docs/contributing/facades.md";
/// Where the facade tool's command runs: its `.venv` and `-m tools.contrib`
/// are relative to the repository's folder.
const REPOSITORY: &str = "the StreetWide repository folder";
/// How long the panel says it printed (s).
const PRINTED_S: f32 = 3.0;
/// How far a highlighted surface's own colour is tinted towards its class
/// colour. Tinting keeps its light, shade and texture, so it still reads
/// as what it is; a strong glow would flatten a plain wall into a patch
/// of the sky's colour.
const TINT: f32 = 0.75;
/// How much of its class colour a highlighted surface gives off, so it
/// shows at night too: buildings, and other assets (a whole tile's terrain
/// or streets).
const GLOW_BUILDING: f32 = 0.2;
const GLOW_ASSET: f32 = 0.15;

/// What the crosshair is on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A building: its shell and its facade, as one.
    Building { tile: usize, feature_id: String },
    /// Any other asset. `tile` indexes the world's tiles.
    Asset {
        tile: usize,
        layer: Layer,
        feature_id: String,
    },
}

impl Target {
    pub fn of(tile: usize, layer: Layer, feature_id: &str) -> Target {
        match layer {
            Layer::Shell | Layer::Facade => Target::Building {
                tile,
                feature_id: feature_id.into(),
            },
            _ => Target::Asset {
                tile,
                layer,
                feature_id: feature_id.into(),
            },
        }
    }

    /// The scene's keys of the assets it is made of, in the tile `key`.
    pub fn keys(&self, key: TileKey) -> Vec<AssetKey> {
        match self {
            Target::Building { feature_id, .. } => vec![
                asset_key(key, Layer::Facade, feature_id),
                asset_key(key, Layer::Shell, feature_id),
            ],
            Target::Asset {
                layer, feature_id, ..
            } => vec![asset_key(key, *layer, feature_id)],
        }
    }
}

/// Who made an asset version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The ingest, from open data.
    Baseline { version: String },
    /// Contributors, by public handle.
    Contribution {
        authors: Vec<String>,
        generator: String,
        version: String,
    },
    /// Another tool, naming no contributor.
    Other { generator: String, version: String },
}

/// Who made an asset version, from its manifest (RFC 0002 sections 4 and
/// 7). A "StreetWide contributions" source makes it a contribution whatever
/// its generator claims.
pub fn origin(m: &AssetManifest) -> Origin {
    let g = &m.provenance.generator;
    let mut contributions = m
        .provenance
        .sources
        .iter()
        .filter(|s| s.dataset == CONTRIBUTIONS_DATASET)
        .peekable();
    if contributions.peek().is_some() {
        let mut authors: Vec<String> = contributions
            .flat_map(|s| s.source_ids.iter().cloned())
            .collect();
        authors.dedup();
        Origin::Contribution {
            authors,
            generator: g.name.clone(),
            version: g.version.clone(),
        }
    } else if g.name == INGEST_GENERATOR {
        Origin::Baseline {
            version: g.version.clone(),
        }
    } else {
        Origin::Other {
            generator: g.name.clone(),
            version: g.version.clone(),
        }
    }
}

/// How a target is highlighted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// A building without contributions: improvable.
    Improvable,
    /// A building with a contributed part, or a contributed asset.
    Contributed,
    /// Anything else.
    Neutral,
}

impl Class {
    /// Amber, blue and white (sRGB): told apart with any colour vision.
    pub fn color(self) -> Color {
        match self {
            Class::Improvable => Color::srgb(1.0, 0.62, 0.10),
            Class::Contributed => Color::srgb(0.30, 0.68, 1.0),
            Class::Neutral => Color::srgb(0.92, 0.92, 0.92),
        }
    }

    /// The light a highlighted surface gives off (linear). It is not scaled
    /// by the camera's exposure, so it shows by day and by night.
    pub fn glow(self, building: bool) -> LinearRgba {
        let c = self.color().to_linear();
        let k = if building { GLOW_BUILDING } else { GLOW_ASSET };
        LinearRgba::rgb(c.red * k, c.green * k, c.blue * k)
    }

    /// What a highlighted surface's colour is multiplied by (linear): its
    /// class colour, part way; white (no change) for neutral assets.
    pub fn tint(self) -> LinearRgba {
        if self == Class::Neutral {
            return LinearRgba::WHITE;
        }
        let c = self.color().to_linear();
        let mix = |v: f32| 1.0 - TINT + TINT * v;
        LinearRgba::rgb(mix(c.red), mix(c.green), mix(c.blue))
    }
}

/// What kind of thing a target is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A swissBUILDINGS3D building.
    SwissBuilding,
    /// An OpenStreetMap footprint, where swissBUILDINGS3D has no building.
    OsmBuilding,
    /// Any other building.
    Building,
    Terrain,
    Street,
    Path,
    Bridge,
    Water,
    Other(Layer),
}

impl Kind {
    pub fn is_building(self) -> bool {
        matches!(
            self,
            Kind::SwissBuilding | Kind::OsmBuilding | Kind::Building
        )
    }

    /// In a few words, lower case.
    fn noun(self) -> &'static str {
        match self {
            Kind::SwissBuilding | Kind::OsmBuilding | Kind::Building => "building",
            Kind::Terrain => "terrain",
            Kind::Street => "street",
            Kind::Path => "path",
            Kind::Bridge => "bridge",
            Kind::Water => "water",
            Kind::Other(layer) => layer.as_str(),
        }
    }
}

/// How a target can be improved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Improve {
    /// A building without a facade: model one.
    Facade,
    /// A building with a facade: a better one can replace it.
    BetterFacade,
    /// Closed to contributions (RFC 0002 section 3): fix it in
    /// OpenStreetMap, and the next baseline refresh brings the fix in.
    Upstream,
    /// Open to contributions with measured evidence, with no tool yet.
    NoTool,
    /// Not open to contributions yet.
    Nothing,
}

/// One asset of a target, as its manifest and tile index describe it.
#[derive(Clone, Debug)]
pub struct Part {
    pub layer: Layer,
    pub manifest_id: ContentId,
    /// `Err` if the manifest could not be read.
    pub origin: Result<Origin, String>,
    /// From the tile index, as the gate reads it.
    pub license: String,
    pub credits: Vec<String>,
    /// The upstream datasets, in manifest order.
    pub sources: Vec<String>,
    /// The OpenStreetMap elements it comes from (`way/1`).
    pub osm: Vec<String>,
    pub fictional: bool,
}

impl Part {
    pub fn is_contribution(&self) -> bool {
        matches!(self.origin, Ok(Origin::Contribution { .. }))
    }
}

/// Everything the panel and the terminal say about a target.
#[derive(Clone, Debug)]
pub struct Description {
    pub kind: Kind,
    pub class: Class,
    pub tile: TileKey,
    pub feature_id: String,
    /// Facade first, then shell; one part for any other asset.
    pub parts: Vec<Part>,
    pub improve: Improve,
    /// The OpenStreetMap element it comes from, if one.
    pub osm_url: Option<String>,
}

/// Reads a manifest, by id.
pub type ManifestFn<'a> = &'a mut dyn FnMut(&ContentId) -> Result<Arc<AssetManifest>, String>;

/// The datasets a manifest names, without the contributions source:
/// OpenStreetMap with its element, or how many.
fn source_names(m: &AssetManifest) -> Vec<String> {
    m.provenance
        .sources
        .iter()
        .filter(|s| s.dataset != CONTRIBUTIONS_DATASET)
        .map(|s| match (s.dataset.as_str(), s.source_ids.as_slice()) {
            ("OpenStreetMap", [one]) => format!("OpenStreetMap {}", one.replace('/', " ")),
            ("OpenStreetMap", []) => "OpenStreetMap".into(),
            ("OpenStreetMap", many) => {
                let ways = many.iter().all(|i| i.starts_with("way/"));
                let what = if ways { "ways" } else { "elements" };
                format!("OpenStreetMap ({} {what})", many.len())
            }
            (dataset, _) => dataset.to_string(),
        })
        .collect()
}

fn part(asset: &op_world::TileAsset, manifest: ManifestFn) -> Part {
    let m = manifest(&asset.manifest_id);
    let read = m.as_deref().ok();
    Part {
        layer: asset.layer,
        manifest_id: asset.manifest_id,
        origin: m.as_deref().map(origin).map_err(Clone::clone),
        license: asset.license.clone(),
        credits: read
            .map(|m| m.provenance.attribution.clone())
            .unwrap_or_default(),
        sources: read.map(source_names).unwrap_or_default(),
        osm: read
            .into_iter()
            .flat_map(|m| &m.provenance.sources)
            .filter(|s| s.dataset == "OpenStreetMap")
            .flat_map(|s| s.source_ids.iter().cloned())
            .collect(),
        fictional: read.is_some_and(|m| m.fictional),
    }
}

/// The OpenStreetMap page of the element an asset comes from: its feature
/// id (`osm:way:1`), else its only OpenStreetMap source.
fn osm_url(feature_id: &str, osm: &[String]) -> Option<String> {
    let element = |kind: &str, id: &str| {
        matches!(kind, "node" | "way" | "relation")
            .then(|| format!("https://www.openstreetmap.org/{kind}/{id}"))
    };
    if let Some((kind, id)) = feature_id
        .strip_prefix("osm:")
        .and_then(|r| r.split_once(':'))
    {
        return element(kind, id);
    }
    match osm {
        [one] => one.split_once('/').and_then(|(kind, id)| element(kind, id)),
        _ => None,
    }
}

/// Describes a target of `tile`. `material` is that of the surface hit,
/// which tells streets from paths within a tile's ribbons.
pub fn describe(
    tile: &op_stream::Tile,
    target: &Target,
    material: Option<Material>,
    manifest: ManifestFn,
) -> Description {
    let find = |layer: Layer, id: &str| {
        tile.assets
            .iter()
            .find(|a| a.layer == layer && a.feature_id == id)
    };
    match target {
        Target::Building { feature_id, .. } => {
            let (facade, shell) = (
                find(Layer::Facade, feature_id),
                find(Layer::Shell, feature_id),
            );
            let parts: Vec<Part> = [facade, shell]
                .into_iter()
                .flatten()
                .map(|a| part(a, &mut *manifest))
                .collect();
            // The gate's own test for a shell from OpenStreetMap (G3).
            let kind = match shell {
                Some(s) if s.license.contains("ODbL-1.0") => Kind::OsmBuilding,
                _ if feature_id.starts_with("swisstopo:") => Kind::SwissBuilding,
                _ => Kind::Building,
            };
            let osm = parts.iter().flat_map(|p| p.osm.clone()).collect::<Vec<_>>();
            Description {
                kind,
                class: if parts.iter().any(Part::is_contribution) {
                    Class::Contributed
                } else {
                    Class::Improvable
                },
                tile: tile.key,
                feature_id: feature_id.clone(),
                improve: if facade.is_some() {
                    Improve::BetterFacade
                } else {
                    Improve::Facade
                },
                osm_url: (kind == Kind::OsmBuilding)
                    .then(|| osm_url(feature_id, &osm))
                    .flatten(),
                parts,
            }
        }
        Target::Asset {
            layer, feature_id, ..
        } => {
            let asset = find(*layer, feature_id);
            let parts: Vec<Part> = asset.into_iter().map(|a| part(a, &mut *manifest)).collect();
            let kind = match layer {
                Layer::Terrain => Kind::Terrain,
                Layer::Water => Kind::Water,
                Layer::Transport => match material {
                    Some(Material::Road) => Kind::Street,
                    Some(Material::Path) => Kind::Path,
                    Some(Material::Bridge) => Kind::Bridge,
                    // Decks are what the player can stand on; ribbons are
                    // visual only.
                    _ if asset.is_some_and(|a| a.collider == Collider::Trimesh) => Kind::Bridge,
                    _ => Kind::Street,
                },
                other => Kind::Other(*other),
            };
            let improve = match layer {
                Layer::Transport | Layer::Water => Improve::Upstream,
                Layer::Terrain => Improve::NoTool,
                _ => Improve::Nothing,
            };
            Description {
                kind,
                class: if parts.iter().any(Part::is_contribution) {
                    Class::Contributed
                } else {
                    Class::Neutral
                },
                tile: tile.key,
                feature_id: feature_id.clone(),
                osm_url: osm_url(feature_id, parts.first().map_or(&[], |p| &p.osm)),
                improve,
                parts,
            }
        }
    }
}

/// The shell a printed command is written for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names, reason = "PowerShell is that shell's name")]
pub enum Shell {
    /// sh, bash or zsh on Linux and macOS.
    Sh,
    /// Git Bash (MSYS) on Windows.
    GitBash,
    PowerShell,
}

impl Shell {
    /// The README's shells: PowerShell on Windows, sh elsewhere; Git Bash,
    /// which sets `MSYSTEM`, too.
    pub fn detect(windows: bool, msystem: Option<&str>) -> Shell {
        match (windows, msystem) {
            (false, _) => Shell::Sh,
            (true, Some(m)) if !m.is_empty() => Shell::GitBash,
            (true, _) => Shell::PowerShell,
        }
    }

    /// The shell the client was started from.
    pub fn host() -> Shell {
        Shell::detect(cfg!(windows), std::env::var("MSYSTEM").ok().as_deref())
    }

    /// The repository's Python (README, Getting started, step 3).
    pub fn python(self) -> &'static str {
        match self {
            Shell::Sh => ".venv/bin/python",
            Shell::GitBash => ".venv/Scripts/python",
            Shell::PowerShell => r".venv\Scripts\python",
        }
    }

    /// The Python's file, to see whether it is there.
    fn python_file(self) -> &'static str {
        match self {
            Shell::Sh => ".venv/bin/python",
            Shell::GitBash | Shell::PowerShell => ".venv/Scripts/python.exe",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Shell::Sh => "a terminal",
            Shell::GitBash => "Git Bash",
            Shell::PowerShell => "PowerShell",
        }
    }
}

/// One argument, quoted for `shell`: single quotes, which keep `$`,
/// backticks, spaces and backslashes as they are. PowerShell also ends a
/// single-quoted string at the typographic single quotes (U+2018 to
/// U+201B), so they are doubled like `'`.
pub fn quote(shell: Shell, arg: &str) -> String {
    match shell {
        Shell::Sh | Shell::GitBash => format!("'{}'", arg.replace('\'', r"'\''")),
        Shell::PowerShell => {
            let mut s = String::with_capacity(arg.len() + 2);
            s.push('\'');
            for c in arg.chars() {
                if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
                    s.push(c);
                }
                s.push(c);
            }
            s.push('\'');
            s
        }
    }
}

/// A world directory as an argument for `shell`: in PowerShell with
/// backslashes (except in a `\\?\` path); without trailing separators,
/// which Windows PowerShell 5.1 would pass on as `\"` when it quotes an
/// argument with spaces.
pub fn world_arg(shell: Shell, world: &Path) -> String {
    let mut s = world.to_string_lossy().into_owned();
    if shell == Shell::PowerShell && !s.starts_with(r"\\?\") {
        s = s.replace('/', "\\");
    }
    while s.len() > 1 && s.ends_with(['/', '\\']) && !s[..s.len() - 1].ends_with(':') {
        s.pop();
    }
    s
}

/// The facade tool's first step for a building: export its shell to model
/// on (docs/contributing/facades.md, section 1). Run in the repository
/// folder, where `python -m tools.contrib` works; it writes
/// `contrib/<id>-shell.glb`.
pub fn reference_command(shell: Shell, world: &Path, feature_id: &str) -> String {
    format!(
        "{} -m tools.contrib reference --world {} --building {}",
        shell.python(),
        quote(shell, &world_arg(shell, world)),
        quote(shell, feature_id)
    )
}

/// The file the command writes, as tools/contrib names it.
fn shell_file(feature_id: &str) -> String {
    let last = feature_id.rsplit(':').next().unwrap_or(feature_id);
    format!("contrib/{last}-shell.glb")
}

/// Where the facade tool's command runs: the StreetWide repository folder,
/// the one with `tools/contrib`, as the command's `.venv` and
/// `-m tools.contrib` are relative to it. And the world as the command
/// names it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Site {
    /// Relative to the repository folder when inside it, else absolute.
    pub world: PathBuf,
    /// The repository folder, if found.
    pub root: Option<PathBuf>,
    /// Whether the client was started in it.
    pub started_there: bool,
}

impl Site {
    /// For the world given as `world` to a client started in `cwd`
    /// (absolute). The repository folder is `cwd` if `is_repository` says
    /// so, else the nearest above the world that is, else the nearest above
    /// `cwd`.
    pub fn find(world: &Path, cwd: &Path, is_repository: impl Fn(&Path) -> bool) -> Site {
        let inside = |root: &Path, path: &Path| {
            path.strip_prefix(root)
                .ok()
                .filter(|p| !p.as_os_str().is_empty())
                .map(Path::to_path_buf)
        };
        if is_repository(cwd) {
            // A relative world is named from there already.
            let named = world.is_absolute().then(|| inside(cwd, world)).flatten();
            return Site {
                world: named.unwrap_or_else(|| world.to_path_buf()),
                root: Some(cwd.to_path_buf()),
                started_there: true,
            };
        }
        let absolute = cwd.join(world);
        let root = absolute
            .ancestors()
            .skip(1)
            .chain(cwd.ancestors().skip(1))
            .find(|dir| is_repository(dir))
            .map(Path::to_path_buf);
        Site {
            world: root
                .as_deref()
                .and_then(|root| inside(root, &absolute))
                .unwrap_or(absolute),
            root,
            started_there: false,
        }
    }
}

/// The world as the panel shows it in the command: an absolute path as its
/// last folder only (`…/zurich-altstadt`), so that no user or folder names
/// show on screen; and whether it was shortened. A click prints the command
/// in full.
fn on_screen(world: &Path) -> (PathBuf, bool) {
    match world.file_name() {
        Some(name) if world.is_absolute() => {
            (PathBuf::from(format!("…/{}", name.to_string_lossy())), true)
        }
        _ => (world.to_path_buf(), false),
    }
}

/// `https://www.` left out, for the panel.
fn short(url: &str) -> &str {
    url.trim_start_matches("https://")
        .trim_start_matches("www.")
}

/// Upper case first letter.
fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// Who made a part, starting in lower case.
fn origin_text(p: &Part) -> String {
    match &p.origin {
        Ok(Origin::Baseline { version }) => {
            let from = if p.sources.is_empty() {
                String::new()
            } else {
                format!(" from {}", p.sources.join(", "))
            };
            format!("baseline, made by the ingest ({INGEST_GENERATOR} {version}){from}")
        }
        Ok(Origin::Contribution {
            authors,
            generator,
            version,
        }) => {
            let who = if authors.is_empty() {
                "an unnamed contributor".to_string()
            } else {
                authors.join(", ")
            };
            let fictional = if p.fictional { ", fictional" } else { "" };
            format!("contribution by {who} ({generator} {version}){fictional}")
        }
        Ok(Origin::Other { generator, version }) => {
            format!("made by {generator} {version}, not the ingest, with no contributor listed")
        }
        Err(e) => format!("manifest {} could not be read: {e}", p.manifest_id),
    }
}

/// The licence and the credits.
fn licence_text(p: &Part) -> String {
    std::iter::once(p.license.as_str())
        .chain(p.credits.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// A building's state, after "BUILDING · ".
fn building_state(d: &Description) -> &'static str {
    let part = |layer: Layer| d.parts.iter().find(|p| p.layer == layer);
    let contributed = |layer: Layer| part(layer).is_some_and(Part::is_contribution);
    match (
        part(Layer::Facade).is_some(),
        contributed(Layer::Facade),
        contributed(Layer::Shell),
    ) {
        (false, _, false) => "improvable: no facade yet",
        (false, _, true) => "contributed shell, no facade yet",
        (true, true, true) => "contributed facade and shell",
        (true, true, false) => "contributed facade",
        (true, false, true) => "contributed shell",
        (true, false, false) => "improvable: baseline facade",
    }
}

/// The panel's first line.
fn header(d: &Description) -> String {
    let state = if d.kind.is_building() {
        building_state(d)
    } else {
        match d.parts.first().map(|p| &p.origin) {
            Some(Ok(Origin::Baseline { .. })) => "baseline",
            Some(Ok(Origin::Contribution { .. })) => "contributed",
            Some(Ok(Origin::Other { .. })) => "not from the ingest",
            Some(Err(_)) | None => "unknown origin",
        }
    };
    format!("{} · {state}", d.kind.noun().to_uppercase())
}

/// What it is and how much of the world it covers.
fn scope(d: &Description) -> String {
    let t = d.tile;
    let whole_tile = d.feature_id.starts_with("tile:");
    match d.kind {
        Kind::SwissBuilding => format!("swissBUILDINGS3D building shell, tile {t}"),
        Kind::OsmBuilding => {
            format!("OpenStreetMap building, where swissBUILDINGS3D has none, tile {t}")
        }
        Kind::Building => format!("Building shell, tile {t}"),
        Kind::Terrain => format!("The terrain of tile {t} is one asset"),
        Kind::Street | Kind::Path if whole_tile => {
            format!("All streets and paths of tile {t} are one asset")
        }
        Kind::Water if whole_tile => format!("The water of tile {t} is one asset"),
        Kind::Bridge => format!("Bridge deck, tile {t}"),
        kind => format!("{}, tile {t}", capitalised(kind.noun())),
    }
}

/// How to fix what OpenStreetMap is the source of.
fn upstream_text(d: &Description, url: Option<&str>) -> String {
    let refresh = "the next baseline refresh brings the fix in";
    match (d.kind, url) {
        (Kind::Street | Kind::Path, None) => {
            format!("Fix streets and paths in OpenStreetMap; {refresh}")
        }
        (_, Some(url)) => format!("Fix it in OpenStreetMap: {url}; {refresh}"),
        (_, None) => format!("Fix it in OpenStreetMap; {refresh}"),
    }
}

/// What a facade may take from a shell from OpenStreetMap, whose licence
/// (ODbL) a CC BY-SA facade cannot carry (`tools/contrib` refuses
/// `--derived-from-shell` for it).
const OSM_FACADE: &str = "model its facade from your own observations: the shell is ODbL, so use it only to place your facade (never --derived-from-shell)";

/// What OpenStreetMap is the source of, for a building from there.
fn osm_outline(url: Option<&str>) -> String {
    let at = url.map_or("".into(), |u| format!(" at {u}"));
    format!("its outline and height come from OpenStreetMap: if they are wrong, fix them{at}")
}

/// The panel's header and body, for a world named `world` in the command.
pub fn panel_text(d: &Description, shell: Shell, world: &Path) -> (String, String) {
    let mut lines = vec![scope(d), format!("id {}", d.feature_id)];
    if let [only] = d.parts.as_slice() {
        lines.push(capitalised(&origin_text(only)));
        lines.push(format!("Licence {}", licence_text(only)));
    } else {
        for p in &d.parts {
            lines.push(format!(
                "{}: {}",
                capitalised(p.layer.as_str()),
                origin_text(p)
            ));
            lines.push(format!("  Licence {}", licence_text(p)));
        }
    }
    let osm = d.kind == Kind::OsmBuilding;
    if osm {
        lines.push(capitalised(&osm_outline(d.osm_url.as_deref().map(short))));
    }
    let (shown, shortened) = on_screen(world);
    let full = if shortened {
        " (a click prints the world's full path)"
    } else {
        ""
    };
    let step = format!(
        "Step 1, in {}, in {REPOSITORY}{full}:\n{}",
        shell.name(),
        reference_command(shell, &shown, &d.feature_id)
    );
    match d.improve {
        Improve::Facade if osm => lines.push(format!(
            "{}: {FACADE_GUIDE}. {step}",
            capitalised(OSM_FACADE)
        )),
        Improve::Facade => lines.push(format!("Model its facade: {FACADE_GUIDE}. {step}")),
        Improve::BetterFacade if osm => lines.push(format!(
            "A better facade can replace it; {OSM_FACADE}: {FACADE_GUIDE}"
        )),
        Improve::BetterFacade => {
            lines.push(format!("A better facade can replace it: {FACADE_GUIDE}"))
        }
        Improve::Upstream => lines.push(upstream_text(d, d.osm_url.as_deref().map(short))),
        Improve::NoTool => lines.push(
            "Terrain contributions need measured evidence and have no tool yet (RFC 0002)".into(),
        ),
        Improve::Nothing => lines.push("This layer takes no contributions yet (RFC 0002)".into()),
    }
    (header(d), lines.join("\n"))
}

/// The panel's last line.
pub fn footer(printed: bool, captured: bool) -> &'static str {
    match (printed, captured) {
        (true, _) => "Printed to the terminal.",
        (false, true) => "Click: print it to the terminal · I: leave inspect mode",
        (false, false) => {
            "Click to capture the mouse, then click to print it to the terminal · I: leave inspect mode"
        }
    }
}

/// The panel when nothing is under the crosshair.
pub fn nothing_text() -> (String, String) {
    (
        "INSPECT · nothing under the crosshair".into(),
        "Point it at a building, a street, a bridge, water or the ground · I: leave inspect mode"
            .into(),
    )
}

/// The panel on the map.
pub fn paused_text() -> (String, String) {
    (
        "INSPECT · paused on the map".into(),
        "Inspect works when walking or flying: M back to walking, V to fly · I: leave inspect mode"
            .into(),
    )
}

/// A point on the globe.
#[derive(Clone, Copy, Debug)]
pub struct Where {
    pub lon: f64,
    pub lat: f64,
    /// Height (m), as in the world's frame.
    pub height: f64,
}

/// What a click prints: the description, then, for a building, the
/// facade tool's first command on the last line, alone, to copy. `notes`
/// are warnings about where to run it.
pub fn terminal_text(
    d: &Description,
    shell: Shell,
    world: &Path,
    at: Where,
    notes: &[String],
) -> String {
    let mut out = vec![
        String::new(),
        format!(
            "inspect: {} {}, tile {}",
            d.kind.noun(),
            d.feature_id,
            d.tile
        ),
    ];
    let row = |out: &mut Vec<String>, label: &str, text: &str| {
        out.push(format!("  {label:<8} {text}"));
    };
    let what = if d.kind.is_building() {
        format!("{} · {}", lowercase_first(&scope(d)), building_state(d))
    } else {
        lowercase_first(&scope(d))
    };
    row(&mut out, "what", &what);
    let mut place = format!("{:.6}, {:.6}, {:.1} m", at.lon, at.lat, at.height);
    if d.improve == Improve::Upstream {
        place.push_str(&format!(
            " · https://www.openstreetmap.org/#map=19/{:.5}/{:.5}",
            at.lat, at.lon
        ));
    }
    row(&mut out, "at", &place);
    let single = d.parts.len() == 1;
    for p in &d.parts {
        let (label, indent) = if single {
            ("made by", "  ")
        } else {
            (p.layer.as_str(), "           ")
        };
        row(&mut out, label, &origin_text(p));
        out.push(format!("{indent}{:<8} {}", "licence", p.license));
        if !p.credits.is_empty() {
            out.push(format!(
                "{indent}{:<8} {}",
                "credits",
                p.credits.join(" · ")
            ));
        }
    }
    let osm = d.kind == Kind::OsmBuilding;
    if osm {
        row(&mut out, "osm", &osm_outline(d.osm_url.as_deref()));
    }
    match d.improve {
        Improve::Facade | Improve::BetterFacade => {
            let guide = match (d.improve == Improve::BetterFacade, osm) {
                (true, false) => format!("a better facade can replace this one: {FACADE_GUIDE}"),
                (true, true) => {
                    format!("a better facade can replace this one; {OSM_FACADE}: {FACADE_GUIDE}")
                }
                (false, true) => format!("{OSM_FACADE}: {FACADE_GUIDE}"),
                (false, false) => FACADE_GUIDE.to_string(),
            };
            row(&mut out, "guide", &guide);
            out.push(format!("           {GUIDE_URL}"));
            for n in notes {
                row(&mut out, "note", n);
            }
            let export = if osm {
                "export the shell to place your facade on (don't copy its geometry)"
            } else {
                "export the shell to model on"
            };
            row(
                &mut out,
                "step 1",
                &format!(
                    "{export}, as {}. In {}, in {REPOSITORY}:",
                    shell_file(&d.feature_id),
                    shell.name(),
                ),
            );
            out.push(reference_command(shell, world, &d.feature_id));
        }
        Improve::Upstream => {
            let fix = upstream_text(d, d.osm_url.as_deref());
            row(&mut out, "improve", &lowercase_first(&fix));
        }
        Improve::NoTool => row(
            &mut out,
            "improve",
            "terrain contributions need measured evidence and have no tool yet (RFC 0002)",
        ),
        Improve::Nothing => row(&mut out, "improve", "this layer takes no contributions yet"),
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

/// Lower case first letter, unless the first word is a name such as
/// OpenStreetMap or swissBUILDINGS3D.
fn lowercase_first(s: &str) -> String {
    let word = s.split([' ', ',']).next().unwrap_or("");
    if word.chars().skip(1).any(char::is_uppercase) {
        return s.to_string();
    }
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_lowercase().chain(c).collect())
        .unwrap_or_default()
}

/// Warnings about where to run the command, for the terminal only: it
/// names the repository folder when the client runs elsewhere.
fn notes(shell: Shell, site: &Site) -> Vec<String> {
    let Some(root) = &site.root else {
        return vec![format!(
            "run it in {REPOSITORY}, the one with tools/contrib"
        )];
    };
    let mut notes = Vec::new();
    if !site.started_there {
        notes.push(format!(
            "the client runs elsewhere; {REPOSITORY} is {}",
            root.display()
        ));
    }
    if !root.join(shell.python_file()).is_file() {
        notes.push(format!(
            ".venv not found in {REPOSITORY}: see the README, Getting started, step 3"
        ));
    }
    notes
}

/// Whether a click prints: in inspect mode, off the map, and not the click
/// that captures the mouse (it was captured before).
pub fn wants_print(on: bool, paused: bool, clicked: bool, captured_before: bool) -> bool {
    on && !paused && clicked && captured_before
}

/// A highlighted version of a material: its own look, tinted towards the
/// class colour, plus the class's glow.
pub fn highlighted(base: &StandardMaterial, class: Class, building: bool) -> StandardMaterial {
    let (b, t) = (base.base_color.to_linear(), class.tint());
    StandardMaterial {
        base_color: LinearRgba::new(b.red * t.red, b.green * t.green, b.blue * t.blue, b.alpha)
            .into(),
        emissive: class.glow(building),
        emissive_exposure_weight: 0.0,
        ..base.clone()
    }
}

/// What the crosshair is on.
pub struct Selected {
    pub target: Target,
    pub material: Material,
    pub description: Description,
    /// The panel's header and body.
    pub panel: (String, String),
    /// Where the crosshair meets it (render frame).
    pub point: Vec3,
    /// The scene's keys of its assets.
    pub keys: Vec<AssetKey>,
}

/// Inspect mode's state.
#[derive(Resource)]
pub struct Inspect {
    pub on: bool,
    /// On the map: paused.
    pub paused: bool,
    /// What the crosshair is on: set through [`Inspect::select`] only.
    selected: Option<Selected>,
    /// Seconds the panel still says the selection was printed.
    printed_for: f32,
    /// The shell the command is written for.
    pub shell: Shell,
    /// Where the command runs, and the world as it names it.
    pub site: Site,
    last_ray: Option<(Vec3, Vec3)>,
    last_generation: u64,
}

impl Inspect {
    pub fn new(on: bool) -> Inspect {
        Inspect {
            on,
            paused: false,
            selected: None,
            printed_for: 0.0,
            shell: Shell::host(),
            site: Site::default(),
            last_ray: None,
            last_generation: 0,
        }
    }

    /// What the crosshair is on.
    pub fn selected(&self) -> Option<&Selected> {
        self.selected.as_ref()
    }

    /// Whether the panel says the selection was printed: for
    /// [`PRINTED_S`] after a click printed it, while it stays selected.
    pub fn printed(&self) -> bool {
        self.printed_for > 0.0
    }

    /// Puts `selected` under the crosshair, not printed yet.
    fn select(&mut self, selected: Option<Selected>) {
        self.selected = selected;
        self.printed_for = 0.0;
    }

    /// Whether the crosshair is still on the selection, which then moves
    /// to `point`. Streets and paths share an asset: the surface hit tells
    /// them apart.
    fn still_on(&mut self, target: &Target, material: Material, point: Vec3) -> bool {
        let by_material = matches!(
            target,
            Target::Asset {
                layer: Layer::Transport,
                ..
            }
        );
        match &mut self.selected {
            Some(s) if s.target == *target && (!by_material || s.material == material) => {
                s.point = point;
                true
            }
            _ => false,
        }
    }

    /// A click printed the selection.
    fn mark_printed(&mut self) {
        if self.selected.is_some() {
            self.printed_for = PRINTED_S;
        }
    }

    /// `dt` seconds went by.
    fn tick(&mut self, dt: f32) {
        self.printed_for = (self.printed_for - dt).max(0.0);
    }
}

/// The last pick's cost, for the details panel (F3).
#[derive(Resource, Default)]
pub struct InspectStats {
    pub micros: f32,
    pub pick: PickStats,
    /// How far the crosshair's surface is (m).
    pub distance: Option<f32>,
    /// Assets that can be picked.
    pub shown: usize,
}

/// The world's manifests, read when needed.
#[derive(Resource)]
struct Manifests {
    store: Store,
    cache: BTreeMap<ContentId, Result<Arc<AssetManifest>, String>>,
}

impl Manifests {
    fn get(&mut self, id: &ContentId) -> Result<Arc<AssetManifest>, String> {
        self.cache
            .entry(*id)
            .or_insert_with(|| {
                self.store
                    .read_object(id, ObjectKind::Json)
                    .and_then(|bytes| AssetManifest::from_bytes(&bytes))
                    .map(Arc::new)
                    .map_err(|e| e.to_string())
            })
            .clone()
    }
}

/// What is highlighted, and the highlighted materials made so far.
#[derive(Resource, Default)]
struct Highlight {
    lit: Option<(Class, Vec<AssetKey>)>,
    entities: Vec<Entity>,
    variants: BTreeMap<(AssetId<StandardMaterial>, Class, bool), Handle<StandardMaterial>>,
    /// The highlighted variant of the detailed look's water, while lit: its
    /// ripples drift with the water's.
    water: Option<Handle<StandardMaterial>>,
}

/// The inspect systems; the panel and crosshair follow them.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct InspectSystems;

pub struct InspectPlugin;

impl Plugin for InspectPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InspectStats>()
            .init_resource::<Highlight>()
            .add_systems(Startup, setup)
            .add_systems(
                Update,
                (inspect_key, pick_target, print_on_click, highlight)
                    .chain()
                    .in_set(InspectSystems)
                    .after(SceneSystems),
            );
    }
}

fn setup(mut commands: Commands, session: Res<Session>, mut inspect: ResMut<Inspect>) {
    let cwd = std::env::current_dir().unwrap_or_default();
    inspect.site = Site::find(session.world_dir(), &cwd, |dir| {
        dir.join("tools/contrib").is_dir()
    });
    commands.insert_resource(Manifests {
        store: Store::new(session.world_dir()),
        cache: BTreeMap::new(),
    });
}

fn inspect_key(logical: Res<ButtonInput<Key>>, mut inspect: ResMut<Inspect>) {
    if letter(&logical, 'i') {
        inspect.on = !inspect.on;
        inspect.select(None);
        inspect.last_ray = None;
    }
}

/// Finds what the crosshair is on, when the camera or the scene changed: as
/// far as the camera draws.
#[allow(clippy::too_many_arguments)]
fn pick_target(
    mut inspect: ResMut<Inspect>,
    travel: Res<Travel>,
    session: Res<Session>,
    index: Res<PickIndex>,
    camera: Single<(&Transform, &Projection), With<PlayerCamera>>,
    mut manifests: ResMut<Manifests>,
    mut stats: ResMut<InspectStats>,
) {
    let paused = matches!(travel.view, View::Map { .. });
    if inspect.paused != paused {
        inspect.paused = paused;
    }
    if !inspect.on || paused {
        if inspect.selected.is_some() || inspect.last_ray.is_some() {
            inspect.select(None);
            inspect.last_ray = None;
        }
        return;
    }
    let (transform, projection) = *camera;
    let ray = (transform.translation, transform.forward().as_vec3());
    if inspect.last_ray == Some(ray) && inspect.last_generation == index.generation() {
        return;
    }
    inspect.last_ray = Some(ray);
    inspect.last_generation = index.generation();
    let started = Instant::now();
    let mut pick = PickStats::default();
    let hit = index.cast(ray.0, ray.1, projection.far(), &mut pick);
    *stats = InspectStats {
        micros: started.elapsed().as_secs_f32() * 1e6,
        pick,
        distance: hit.as_ref().map(|h| h.t),
        shown: index.len(),
    };
    let Some(hit) = hit else {
        inspect.select(None);
        return;
    };
    let tile = &session.streamer().index().tiles()[hit.entry.tile];
    let target = Target::of(hit.entry.tile, hit.entry.layer, &hit.key.feature_id);
    if inspect.still_on(&target, hit.material, hit.point) {
        return;
    }
    let description = describe(tile, &target, Some(hit.material), &mut |id| {
        manifests.get(id)
    });
    let panel = panel_text(&description, inspect.shell, &inspect.site.world);
    inspect.select(Some(Selected {
        keys: target.keys(tile.key),
        target,
        material: hit.material,
        description,
        panel,
        point: hit.point,
    }));
}

/// A click prints what the crosshair is on to the terminal.
fn print_on_click(
    time: Res<Time>,
    mouse: Res<ButtonInput<MouseButton>>,
    cursor: Single<&CursorOptions, With<PrimaryWindow>>,
    session: Res<Session>,
    mut inspect: ResMut<Inspect>,
    mut captured_before: Local<bool>,
) {
    let clicked = mouse.just_pressed(MouseButton::Left);
    let text = inspect
        .selected()
        .filter(|_| wants_print(inspect.on, inspect.paused, clicked, *captured_before))
        .map(|selected| {
            let (lon, lat, height) = session
                .frame()
                .unproject(session.render_to_region(selected.point));
            terminal_text(
                &selected.description,
                inspect.shell,
                &inspect.site.world,
                Where { lon, lat, height },
                &notes(inspect.shell, &inspect.site),
            )
        });
    if let Some(text) = text {
        // Not `println!`, which panics when stdout is closed.
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(text.as_bytes()).and_then(|()| out.flush());
        inspect.mark_printed();
    } else if inspect.printed() {
        inspect.tick(time.delta_secs());
    }
    *captured_before = cursor.grab_mode != CursorGrabMode::None;
}

/// Lights up what the crosshair is on: its parts' own materials, with the
/// class's glow added. No extra geometry, so nothing to fight the surface
/// for depth. Works only when what is lit changes, or the scene or the
/// looks do; and, while water is lit, keeps its ripples drifting.
fn highlight(
    inspect: Res<Inspect>,
    shown: Res<Shown>,
    set: Res<MaterialSet>,
    mut state: ResMut<Highlight>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut parts: Query<(&Looks, &mut MeshMaterial3d<StandardMaterial>)>,
) {
    let want = inspect.selected().filter(|_| inspect.on && !inspect.paused);
    let same = match (&state.lit, want) {
        (Some((class, keys)), Some(s)) => *class == s.description.class && *keys == s.keys,
        (None, None) => true,
        _ => false,
    };
    if !same || shown.is_changed() || set.is_changed() {
        let state = &mut *state;
        // F2 has already put every part back to its look this frame, if it
        // switched looks; parts that are gone are skipped.
        for e in std::mem::take(&mut state.entities) {
            if let Ok((looks, mut material)) = parts.get_mut(e) {
                material.0 = set.current(looks);
            }
        }
        state.water = None;
        if let Some(s) = want {
            let (class, building) = (s.description.class, s.description.kind.is_building());
            for key in &s.keys {
                for &e in shown.entities(key) {
                    let Ok((looks, mut material)) = parts.get_mut(e) else {
                        continue;
                    };
                    let base = set.current(looks);
                    let variant = state
                        .variants
                        .entry((base.id(), class, building))
                        .or_insert_with(|| {
                            let lit = materials
                                .get(&base)
                                .map(|m| highlighted(m, class, building))
                                .unwrap_or_default();
                            materials.add(lit)
                        })
                        .clone();
                    if set.water() == Some(&base) {
                        state.water = Some(variant.clone());
                    }
                    material.0 = variant;
                    state.entities.push(e);
                }
            }
        }
        state.lit = want.map(|s| (s.description.class, s.keys.clone()));
    }
    // Highlighted water keeps its ripples drifting.
    if let Some(lit) = &state.water
        && let Some(drift) = set
            .water()
            .and_then(|w| materials.get(w))
            .map(|m| m.uv_transform)
        && let Some(mut m) = materials.get_mut(lit)
    {
        m.uv_transform = drift;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pick::test_util::at_a_wall;
    use crate::pick::{PickEntry, PickIndex};
    use op_stream::WorldIndex;

    fn repo() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// The manifests of the world in `dir`, read as the client reads them.
    fn manifests(dir: &Path) -> Manifests {
        Manifests {
            store: Store::new(dir),
            cache: BTreeMap::new(),
        }
    }

    const FIXTURE_BUILDING: &str =
        "swisstopo:swissbuildings3d:000f609c-4003-495f-9426-b43e3d770167";

    #[test]
    fn origin_follows_rfc_0002() {
        let world = repo().join("fixtures/world");
        let index = WorldIndex::open(&world).unwrap();
        let a = &index.tiles()[0].assets[0];
        let m = manifests(&world).get(&a.manifest_id).unwrap();
        assert_eq!(
            origin(&m),
            Origin::Baseline {
                version: "0.1.0".into()
            }
        );
        let draft = op_world::sample_change::sample_draft(&world).unwrap();
        assert_eq!(
            origin(&draft.manifest),
            Origin::Contribution {
                authors: vec!["streetwide:fixtures".into()],
                generator: "op_world sample change".into(),
                version: "0.1.0".into(),
            }
        );
        // A contribution that names the ingest as its generator is still a
        // contribution.
        let mut spoof = draft.manifest.clone();
        spoof.provenance.generator.name = INGEST_GENERATOR.into();
        assert!(matches!(origin(&spoof), Origin::Contribution { .. }));
        // Neither.
        let mut other = (*m).clone();
        other.provenance.generator.name = "blender-export".into();
        assert_eq!(
            origin(&other),
            Origin::Other {
                generator: "blender-export".into(),
                version: "0.1.0".into()
            }
        );
    }

    /// The material of an asset's first primitive.
    fn first_material(index: &WorldIndex, a: &op_world::TileAsset) -> Material {
        index.load_mesh(&a.content_id).unwrap().nodes[0].primitives[0].material
    }

    #[test]
    fn classifies_every_asset_of_the_fixture_world() {
        let world = repo().join("fixtures/world");
        let index = WorldIndex::open(&world).unwrap();
        let mut cache = manifests(&world);
        let mut manifest = |id: &ContentId| cache.get(id);
        let tile = &index.tiles()[0];
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for a in &tile.assets {
            let target = Target::of(0, a.layer, &a.feature_id);
            let d = describe(
                tile,
                &target,
                Some(first_material(&index, a)),
                &mut manifest,
            );
            // Everything in the fixture was made by the ingest: this pins
            // INGEST_GENERATOR to its real output.
            for p in &d.parts {
                assert_eq!(
                    p.origin,
                    Ok(Origin::Baseline {
                        version: "0.1.0".into()
                    }),
                    "{}",
                    a.feature_id
                );
            }
            *counts.entry(format!("{:?}", d.kind)).or_default() += 1;
            match d.kind {
                Kind::SwissBuilding => {
                    assert_eq!(
                        (d.class, &d.improve, d.osm_url.as_deref()),
                        (Class::Improvable, &Improve::Facade, None)
                    );
                    assert_eq!(d.parts.len(), 1);
                    assert_eq!(d.parts[0].sources, ["swissBUILDINGS3D 3.0 Beta"]);
                    assert_eq!(d.parts[0].credits, ["© swisstopo"]);
                }
                Kind::OsmBuilding => {
                    assert_eq!(a.feature_id, "osm:way:493253453");
                    assert_eq!((d.class, &d.improve), (Class::Improvable, &Improve::Facade));
                    assert_eq!(
                        d.osm_url.as_deref(),
                        Some("https://www.openstreetmap.org/way/493253453")
                    );
                    assert_eq!(
                        d.parts[0].sources,
                        ["OpenStreetMap way 493253453", "swissALTI3D"]
                    );
                }
                Kind::Terrain => {
                    assert_eq!((d.class, &d.improve), (Class::Neutral, &Improve::NoTool))
                }
                Kind::Bridge => {
                    assert_eq!((d.class, &d.improve), (Class::Neutral, &Improve::Upstream));
                    let way = a.feature_id.strip_prefix("osm:way:").unwrap();
                    assert_eq!(
                        d.osm_url,
                        Some(format!("https://www.openstreetmap.org/way/{way}"))
                    );
                }
                Kind::Street | Kind::Path => {
                    assert_eq!(a.feature_id, "tile:17/68646/45903");
                    assert_eq!(
                        (d.class, &d.improve, d.osm_url.as_deref()),
                        (Class::Neutral, &Improve::Upstream, None)
                    );
                    assert_eq!(
                        d.parts[0].sources,
                        ["OpenStreetMap (97 ways)", "swissALTI3D"]
                    );
                    // The ribbons hold roads and paths: the surface hit
                    // tells which.
                    for (m, kind) in [(Material::Road, Kind::Street), (Material::Path, Kind::Path)]
                    {
                        assert_eq!(describe(tile, &target, Some(m), &mut manifest).kind, kind);
                    }
                }
                Kind::Water => {
                    assert_eq!((d.class, &d.improve), (Class::Neutral, &Improve::Upstream));
                    assert_eq!(
                        d.osm_url.as_deref(),
                        Some("https://www.openstreetmap.org/relation/1598599")
                    );
                }
                other => panic!("{other:?} for {}", a.feature_id),
            }
        }
        let want: BTreeMap<String, usize> = [
            ("SwissBuilding", 32),
            ("OsmBuilding", 1),
            ("Terrain", 1),
            ("Bridge", 9),
            ("Street", 1),
            ("Water", 1),
        ]
        .map(|(k, n)| (k.to_string(), n))
        .into();
        assert_eq!(counts, want);
        // Without the surface, a deck is told by its collider.
        let deck = tile
            .assets
            .iter()
            .find(|a| a.feature_id == "osm:way:147692714")
            .unwrap();
        let target = Target::of(0, deck.layer, &deck.feature_id);
        assert_eq!(
            describe(tile, &target, None, &mut manifest).kind,
            Kind::Bridge
        );
    }

    /// A copy of the fixture world in a new temporary directory.
    struct TempWorld(PathBuf);

    impl TempWorld {
        fn copy(name: &str) -> TempWorld {
            fn copy_dir(from: &Path, to: &Path) {
                std::fs::create_dir_all(to).unwrap();
                for e in std::fs::read_dir(from).unwrap() {
                    let e = e.unwrap();
                    if e.file_type().unwrap().is_dir() {
                        copy_dir(&e.path(), &to.join(e.file_name()));
                    } else {
                        std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
                    }
                }
            }
            let dir = std::env::temp_dir()
                .join(format!("streetwide-inspect-{name}-{}", std::process::id()));
            std::fs::remove_dir_all(&dir).ok();
            copy_dir(&repo().join("fixtures/world"), &dir.join("world"));
            TempWorld(dir)
        }
    }

    impl Drop for TempWorld {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    /// The fixture's sample facade, merged into a copy of the fixture world
    /// as RFC 0002 merges it: the building is now contributed, and names
    /// its author, with the facade's licence and credits. Picked as the
    /// scene places it, at its tile's offset in the render frame.
    #[test]
    fn a_merged_facade_makes_its_building_contributed() {
        use op_world::contrib::{Decision, Review, Verdict};
        use op_world::merge;
        let temp = TempWorld::copy("merged");
        let world = temp.0.join("world");
        let first = merge::baseline(&world, "local preview").unwrap();
        merge::merge(
            &world,
            &repo().join("fixtures/changes/facade-sample"),
            &first.to_bytes().unwrap(),
            Decision::Merged,
            vec![Review {
                reviewer: "github:local".into(),
                verdict: Verdict::Better,
                notes: "local preview".into(),
            }],
            "local preview",
            &temp.0.join("scratch"),
        )
        .unwrap();
        let index = WorldIndex::open(&world).unwrap();
        let tile = &index.tiles()[0];
        let offset = Vec3::new(210.0, -3.5, -180.0);
        let mut pick = PickIndex::default();
        for a in tile
            .assets
            .iter()
            .filter(|a| a.feature_id == FIXTURE_BUILDING)
        {
            let mesh = Arc::new(index.load_mesh(&a.content_id).unwrap());
            pick.insert(
                asset_key(tile.key, a.layer, &a.feature_id),
                PickEntry::new(0, a.layer, offset, mesh).unwrap(),
            );
        }
        assert_eq!(pick.len(), 2, "a shell and its facade");
        let shell = tile
            .assets
            .iter()
            .find(|a| a.layer == Layer::Shell && a.feature_id == FIXTURE_BUILDING)
            .unwrap();
        let (o, d, _) = at_a_wall(&index.load_mesh(&shell.content_id).unwrap());
        let hit = pick
            .cast(o + offset, d, 100.0, &mut PickStats::default())
            .unwrap();
        assert_eq!(hit.entry.layer, Layer::Facade);
        // The skin lies 3 cm out from the wall.
        assert!((hit.t - (20.0 - 0.03)).abs() < 0.01, "{}", hit.t);
        assert!(hit.point.distance(o + offset + d * hit.t) < 1e-3);
        let target = Target::of(0, hit.entry.layer, &hit.key.feature_id);
        assert_eq!(
            target,
            Target::Building {
                tile: 0,
                feature_id: FIXTURE_BUILDING.into()
            }
        );
        let mut cache = manifests(&world);
        let desc = describe(tile, &target, Some(hit.material), &mut |id| cache.get(id));
        assert_eq!(
            (desc.class, &desc.improve),
            (Class::Contributed, &Improve::BetterFacade)
        );
        assert_eq!(
            desc.parts.iter().map(|p| p.layer).collect::<Vec<_>>(),
            [Layer::Facade, Layer::Shell]
        );
        let (header, body) = panel_text(&desc, Shell::PowerShell, Path::new("fixtures/world"));
        assert_eq!(header, "BUILDING · contributed facade");
        assert_eq!(
            body,
            "swissBUILDINGS3D building shell, tile 17/68646/45903
id swisstopo:swissbuildings3d:000f609c-4003-495f-9426-b43e3d770167
Facade: contribution by streetwide:fixtures (op_world sample change 0.1.0)
  Licence CC-BY-SA-4.0 AND LicenseRef-swisstopo-OGD · The StreetWide contributors · © swisstopo
Shell: baseline, made by the ingest (streetwide-ingest 0.1.0) from swissBUILDINGS3D 3.0 Beta
  Licence LicenseRef-swisstopo-OGD · © swisstopo
A better facade can replace it: docs/contributing/facades.md"
        );
        // The same building, highlighted blue.
        assert_eq!(desc.class.color(), Color::srgb(0.30, 0.68, 1.0));
        // A shell contributed without a facade is contributed, and still
        // waits for its facade.
        let mut only_shell = desc.clone();
        only_shell.parts.remove(0);
        only_shell.parts[0].origin = desc.parts[0].origin.clone();
        assert_eq!(
            building_state(&only_shell),
            "contributed shell, no facade yet"
        );
    }

    #[test]
    fn commands_quote_for_sh_git_bash_and_powershell() {
        let (sh, git, ps) = (Shell::Sh, Shell::GitBash, Shell::PowerShell);
        assert_eq!(quote(sh, "a b"), "'a b'");
        assert_eq!(quote(sh, "O'Neil"), r"'O'\''Neil'");
        assert_eq!(quote(git, "$HOME `x` {a} (b); c"), "'$HOME `x` {a} (b); c'");
        assert_eq!(quote(ps, "O'Neil"), "'O''Neil'");
        assert_eq!(quote(ps, "Zoë’s ‘world’ ‚x‛"), "'Zoë’’s ‘‘world’’ ‚‚x‛‛'");
        assert_eq!(quote(ps, "$HOME `x` {a} (b); c"), "'$HOME `x` {a} (b); c'");
        // Windows paths: backslashes for PowerShell; trailing separators go,
        // but not a drive's root; `\\?\` paths stay as they are.
        assert_eq!(
            world_arg(ps, Path::new(r"data/world\zurich-altstadt")),
            r"data\world\zurich-altstadt"
        );
        assert_eq!(
            world_arg(ps, Path::new(r"C:\My Worlds\zurich\")),
            r"C:\My Worlds\zurich"
        );
        assert_eq!(world_arg(ps, Path::new(r"C:\")), r"C:\");
        assert_eq!(
            world_arg(ps, Path::new(r"\\?\C:\worlds/a")),
            r"\\?\C:\worlds/a"
        );
        assert_eq!(
            world_arg(sh, Path::new("data/world/zurich-altstadt/")),
            "data/world/zurich-altstadt"
        );
        assert_eq!(world_arg(sh, Path::new("/")), "/");
        assert_eq!(
            world_arg(git, Path::new(r"data/world\zurich-altstadt")),
            r"data/world\zurich-altstadt"
        );
        // Which shell.
        assert_eq!(Shell::detect(false, None), sh);
        assert_eq!(Shell::detect(false, Some("MINGW64")), sh);
        assert_eq!(Shell::detect(true, Some("MINGW64")), git);
        assert_eq!(Shell::detect(true, Some("")), ps);
        assert_eq!(Shell::detect(true, None), ps);
        // The commands for the district.
        let world = Path::new("data/world/zurich-altstadt");
        assert_eq!(
            reference_command(ps, world, FIXTURE_BUILDING),
            r".venv\Scripts\python -m tools.contrib reference --world 'data\world\zurich-altstadt' --building 'swisstopo:swissbuildings3d:000f609c-4003-495f-9426-b43e3d770167'"
        );
        assert_eq!(
            reference_command(sh, world, FIXTURE_BUILDING),
            ".venv/bin/python -m tools.contrib reference --world 'data/world/zurich-altstadt' --building 'swisstopo:swissbuildings3d:000f609c-4003-495f-9426-b43e3d770167'"
        );
        assert_eq!(
            reference_command(git, world, "osm:way:493253453"),
            ".venv/Scripts/python -m tools.contrib reference --world 'data/world/zurich-altstadt' --building 'osm:way:493253453'"
        );
        assert_eq!(
            shell_file(FIXTURE_BUILDING),
            "contrib/000f609c-4003-495f-9426-b43e3d770167-shell.glb"
        );
    }

    /// The command runs in the repository folder: the client's own, or the
    /// one above the world, or above the client's. The world is named from
    /// there when inside it.
    #[test]
    fn the_command_runs_in_the_repository_folder() {
        let at = |p: &str| PathBuf::from(p);
        // An absolute path on this platform: "/a/b" is "C:\a\b" on Windows.
        let abs = |p: &str| {
            let mut path = PathBuf::from(if cfg!(windows) { r"C:\" } else { "/" });
            path.extend(p.split('/').filter(|c| !c.is_empty()));
            path
        };
        let repository = abs("/home/someone/StreetWide");
        let is_repository = |dir: &Path| dir == repository;
        let site = |world: &Path, cwd: &str| Site::find(world, &abs(cwd), is_repository);
        let found = |world: PathBuf, started_there| Site {
            world,
            root: Some(repository.clone()),
            started_there,
        };
        // Started in it: the world as given, or from there if inside it.
        let home = "/home/someone/StreetWide";
        let world = at("data/world/zurich-altstadt");
        assert_eq!(site(&world, home), found(world.clone(), true));
        assert_eq!(
            site(&abs("/home/someone/StreetWide/preview-world"), home),
            found(at("preview-world"), true)
        );
        let outside = abs("/srv/worlds/zurich");
        assert_eq!(site(&outside, home), found(outside.clone(), true));
        let above = at("../worlds/zurich");
        assert_eq!(site(&above, home), found(above.clone(), true));
        // Started elsewhere: the folder above the world, or above the client.
        assert_eq!(
            site(
                &abs("/home/someone/StreetWide/data/world/zurich-altstadt"),
                "/tmp"
            ),
            found(world.clone(), false)
        );
        assert_eq!(
            site(&at("world"), "/home/someone/StreetWide/fixtures"),
            found(at("fixtures/world"), false)
        );
        assert_eq!(
            site(&outside, "/home/someone/StreetWide/crates/op_client"),
            found(outside.clone(), false)
        );
        // Not found: the world in full.
        let lost = site(&at("zurich"), "/srv/worlds");
        assert_eq!(
            lost,
            Site {
                world: outside.clone(),
                root: None,
                started_there: false
            }
        );
        // The terminal says where to run it; the panel shows no absolute
        // path, only the world's folder.
        assert_eq!(
            notes(Shell::Sh, &lost),
            ["run it in the StreetWide repository folder, the one with tools/contrib"]
        );
        let temp = TempWorld::copy("site");
        let elsewhere = Site {
            world: at("data/world/zurich-altstadt"),
            root: Some(temp.0.clone()),
            started_there: false,
        };
        assert_eq!(
            notes(Shell::Sh, &elsewhere),
            [
                format!(
                    "the client runs elsewhere; the StreetWide repository folder is {}",
                    temp.0.display()
                ),
                ".venv not found in the StreetWide repository folder: see the README, Getting started, step 3".into()
            ]
        );
        assert_eq!(on_screen(&world), (world.clone(), false));
        assert_eq!(on_screen(&outside), (at("…/zurich"), true));
    }

    /// Base64, for PowerShell's `-EncodedCommand`.
    fn base64(bytes: &[u8]) -> String {
        const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for c in bytes.chunks(3) {
            let n = c
                .iter()
                .enumerate()
                .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
            for k in 0..4 {
                s.push(if k <= c.len() {
                    char::from(A[((n >> (18 - 6 * k)) & 63) as usize])
                } else {
                    '='
                });
            }
        }
        s
    }

    /// Runs `command` with its interpreter replaced by a function that
    /// prints each argument as UTF-8 hex, and returns the arguments.
    fn arguments_as_parsed(shell: Shell, command: &str) -> Option<Vec<String>> {
        use std::process::{Command, Stdio};
        let rest = command.strip_prefix(shell.python()).unwrap();
        let output = match shell {
            Shell::PowerShell => {
                let script = format!(
                    "function e {{ foreach ($a in $args) {{ [Console]::Out.WriteLine((([Text.Encoding]::UTF8.GetBytes([string]$a) | ForEach-Object {{ $_.ToString('x2') }}) -join '')) }} }}; e{rest}"
                );
                let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
                Command::new("powershell")
                    .args([
                        "-NoProfile",
                        "-NonInteractive",
                        "-EncodedCommand",
                        &base64(&utf16),
                    ])
                    .stdin(Stdio::null())
                    .output()
                    .ok()?
            }
            Shell::Sh | Shell::GitBash => {
                let script = format!(
                    "e() {{ for a in \"$@\"; do printf '%s' \"$a\" | od -An -tx1 -v | tr -d ' \\n'; echo; done; }}; e{rest}\n"
                );
                let mut child = Command::new("sh")
                    .arg("-s")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .spawn()
                    .ok()?;
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(script.as_bytes())
                    .unwrap();
                child.wait_with_output().ok()?
            }
        };
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let unhex = |line: &str| {
            let bytes: Vec<u8> = (0..line.len() / 2)
                .map(|i| u8::from_str_radix(&line[2 * i..2 * i + 2], 16).unwrap())
                .collect();
            String::from_utf8(bytes).unwrap()
        };
        Some(
            String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .map(|l| unhex(l.trim()))
                .collect(),
        )
    }

    /// The printed commands come back, argument for argument, from the
    /// shell they are written for: PowerShell on Windows, sh elsewhere (and
    /// on Windows too, as Git Bash, when it is installed).
    #[test]
    fn commands_parse_back_in_the_platform_shell() {
        let worlds = [
            "data/world/zurich-altstadt",
            r"C:\Users\O'Neil\Zoë’s worlds\zurich ‘old’ town\",
            "/home/o'neil/$HOME/`x` {a} (b); c & d/world",
        ];
        let ids = [
            FIXTURE_BUILDING,
            "osm:way:493253453",
            "test:it's-$x-`y`-{z}",
        ];
        let shells: &[Shell] = if cfg!(windows) {
            &[Shell::PowerShell, Shell::GitBash]
        } else {
            &[Shell::Sh]
        };
        for &shell in shells {
            for (world, id) in worlds.iter().zip(ids) {
                let command = reference_command(shell, Path::new(world), id);
                let Some(args) = arguments_as_parsed(shell, &command) else {
                    assert!(shell == Shell::GitBash, "{shell:?} is missing");
                    continue;
                };
                let want = [
                    "-m",
                    "tools.contrib",
                    "reference",
                    "--world",
                    &world_arg(shell, Path::new(world)),
                    "--building",
                    id,
                ];
                assert_eq!(args, want, "{command}");
            }
        }
    }

    fn fixture_description(
        feature_id: &str,
        layer: Layer,
        material: Option<Material>,
    ) -> Description {
        let world = repo().join("fixtures/world");
        let index = WorldIndex::open(&world).unwrap();
        let tile = &index.tiles()[0];
        let mut cache = manifests(&world);
        describe(
            tile,
            &Target::of(0, layer, feature_id),
            material,
            &mut |id| cache.get(id),
        )
    }

    #[test]
    fn panel_text_for_each_kind() {
        let fixture = Path::new("fixtures/world");
        let d = fixture_description(FIXTURE_BUILDING, Layer::Shell, Some(Material::Wall));
        let (header, body) = panel_text(&d, Shell::PowerShell, fixture);
        assert_eq!(header, "BUILDING · improvable: no facade yet");
        assert_eq!(
            body,
            r"swissBUILDINGS3D building shell, tile 17/68646/45903
id swisstopo:swissbuildings3d:000f609c-4003-495f-9426-b43e3d770167
Baseline, made by the ingest (streetwide-ingest 0.1.0) from swissBUILDINGS3D 3.0 Beta
Licence LicenseRef-swisstopo-OGD · © swisstopo
Model its facade: docs/contributing/facades.md. Step 1, in PowerShell, in the StreetWide repository folder:
.venv\Scripts\python -m tools.contrib reference --world 'fixtures\world' --building 'swisstopo:swissbuildings3d:000f609c-4003-495f-9426-b43e3d770167'"
        );
        assert_eq!(d.class.color(), Color::srgb(1.0, 0.62, 0.10), "amber");
        // A world outside the repository shows as its folder only: no user
        // or folder names on screen. A click prints the full path.
        let away = std::path::absolute("someone’s worlds/zurich old town").unwrap();
        let (_, body) = panel_text(&d, Shell::PowerShell, &away);
        assert!(
            body.ends_with(r"Step 1, in PowerShell, in the StreetWide repository folder (a click prints the world's full path):
.venv\Scripts\python -m tools.contrib reference --world '…\zurich old town' --building 'swisstopo:swissbuildings3d:000f609c-4003-495f-9426-b43e3d770167'"),
            "{body}"
        );
        assert!(!body.contains("someone"), "{body}");

        // From OpenStreetMap: its shell places the facade, and is not
        // copied.
        let d = fixture_description("osm:way:493253453", Layer::Shell, Some(Material::Roof));
        let (header, body) = panel_text(&d, Shell::Sh, fixture);
        assert_eq!(header, "BUILDING · improvable: no facade yet");
        assert_eq!(
            body,
            "OpenStreetMap building, where swissBUILDINGS3D has none, tile 17/68646/45903
id osm:way:493253453
Baseline, made by the ingest (streetwide-ingest 0.1.0) from OpenStreetMap way 493253453, swissALTI3D
Licence ODbL-1.0 AND LicenseRef-swisstopo-OGD · © OpenStreetMap contributors · © swisstopo
Its outline and height come from OpenStreetMap: if they are wrong, fix them at openstreetmap.org/way/493253453
Model its facade from your own observations: the shell is ODbL, so use it only to place your facade (never --derived-from-shell): docs/contributing/facades.md. Step 1, in a terminal, in the StreetWide repository folder:
.venv/bin/python -m tools.contrib reference --world 'fixtures/world' --building 'osm:way:493253453'"
        );
        let mut faced = d.clone();
        faced.improve = Improve::BetterFacade;
        let (_, body) = panel_text(&faced, Shell::Sh, fixture);
        assert!(
            body.ends_with("A better facade can replace it; model its facade from your own observations: the shell is ODbL, so use it only to place your facade (never --derived-from-shell): docs/contributing/facades.md"),
            "{body}"
        );

        let tile = "tile:17/68646/45903";
        let cases = [
            (
                tile,
                Layer::Terrain,
                None,
                "TERRAIN · baseline",
                "The terrain of tile 17/68646/45903 is one asset",
                "Terrain contributions need measured evidence and have no tool yet (RFC 0002)",
            ),
            (
                tile,
                Layer::Transport,
                Some(Material::Road),
                "STREET · baseline",
                "All streets and paths of tile 17/68646/45903 are one asset",
                "Fix streets and paths in OpenStreetMap; the next baseline refresh brings the fix in",
            ),
            (
                tile,
                Layer::Transport,
                Some(Material::Path),
                "PATH · baseline",
                "All streets and paths of tile 17/68646/45903 are one asset",
                "Fix streets and paths in OpenStreetMap; the next baseline refresh brings the fix in",
            ),
            (
                "osm:way:147692714",
                Layer::Transport,
                Some(Material::Bridge),
                "BRIDGE · baseline",
                "Bridge deck, tile 17/68646/45903",
                "Fix it in OpenStreetMap: openstreetmap.org/way/147692714; the next baseline refresh brings the fix in",
            ),
            (
                tile,
                Layer::Water,
                Some(Material::Water),
                "WATER · baseline",
                "The water of tile 17/68646/45903 is one asset",
                "Fix it in OpenStreetMap: openstreetmap.org/relation/1598599; the next baseline refresh brings the fix in",
            ),
        ];
        for (id, layer, material, want_header, first, last) in cases {
            let d = fixture_description(id, layer, material);
            let (header, body) = panel_text(&d, Shell::PowerShell, fixture);
            assert_eq!(header, want_header);
            let lines: Vec<&str> = body.lines().collect();
            assert_eq!(
                (lines[0], lines[1], *lines.last().unwrap()),
                (first, format!("id {id}").as_str(), last),
                "{body}"
            );
            assert!(
                lines[2]
                    .starts_with("Baseline, made by the ingest (streetwide-ingest 0.1.0) from "),
                "{body}"
            );
            assert!(lines[3].starts_with("Licence "), "{body}");
            assert_eq!(d.class, Class::Neutral);
        }

        // A manifest that cannot be read is said so.
        let world = repo().join("fixtures/world");
        let index = WorldIndex::open(&world).unwrap();
        let t = &index.tiles()[0];
        let d = describe(t, &Target::of(0, Layer::Terrain, tile), None, &mut |_| {
            Err("missing".to_string())
        });
        let (header, body) = panel_text(&d, Shell::Sh, fixture);
        assert_eq!(header, "TERRAIN · unknown origin");
        let line = body.lines().nth(2).unwrap();
        assert!(
            line.starts_with("Manifest blake3:") && line.ends_with(" could not be read: missing"),
            "{line}"
        );

        // Nothing under the crosshair, the map, and the last line.
        assert_eq!(nothing_text().0, "INSPECT · nothing under the crosshair");
        assert_eq!(paused_text().0, "INSPECT · paused on the map");
        assert!(paused_text().1.contains("M back to walking, V to fly"));
        assert_eq!(
            footer(false, true),
            "Click: print it to the terminal · I: leave inspect mode"
        );
        assert!(footer(false, false).starts_with("Click to capture the mouse"));
        assert_eq!(footer(true, false), "Printed to the terminal.");
    }

    #[test]
    fn terminal_text_ends_with_the_runnable_command() {
        let d = fixture_description(FIXTURE_BUILDING, Layer::Shell, Some(Material::Wall));
        let at = Where {
            lon: 8.544_051,
            lat: 47.369_474,
            height: 412.34,
        };
        // Started outside the repository folder, which has no .venv yet.
        let site = Site {
            world: PathBuf::from(r"C:\worlds\zurich old town"),
            root: Some(PathBuf::from(r"C:\Users\someone\StreetWide")),
            started_there: false,
        };
        let text = terminal_text(
            &d,
            Shell::PowerShell,
            &site.world,
            at,
            &notes(Shell::PowerShell, &site),
        );
        assert_eq!(
            text,
            r"
inspect: building swisstopo:swissbuildings3d:000f609c-4003-495f-9426-b43e3d770167, tile 17/68646/45903
  what     swissBUILDINGS3D building shell, tile 17/68646/45903 · improvable: no facade yet
  at       8.544051, 47.369474, 412.3 m
  made by  baseline, made by the ingest (streetwide-ingest 0.1.0) from swissBUILDINGS3D 3.0 Beta
  licence  LicenseRef-swisstopo-OGD
  credits  © swisstopo
  guide    docs/contributing/facades.md
           https://github.com/AndromedaSLS/StreetWide/blob/main/docs/contributing/facades.md
  note     the client runs elsewhere; the StreetWide repository folder is C:\Users\someone\StreetWide
  note     .venv not found in the StreetWide repository folder: see the README, Getting started, step 3
  step 1   export the shell to model on, as contrib/000f609c-4003-495f-9426-b43e3d770167-shell.glb. In PowerShell, in the StreetWide repository folder:
.venv\Scripts\python -m tools.contrib reference --world 'C:\worlds\zurich old town' --building 'swisstopo:swissbuildings3d:000f609c-4003-495f-9426-b43e3d770167'
"
        );
        // From OpenStreetMap: the same advice as the panel's.
        let d = fixture_description("osm:way:493253453", Layer::Shell, Some(Material::Roof));
        let fixture = Path::new("fixtures/world");
        let text = terminal_text(&d, Shell::Sh, fixture, at, &[]);
        let lines: Vec<&str> = text.lines().collect();
        for line in [
            "  osm      its outline and height come from OpenStreetMap: if they are wrong, fix them at https://www.openstreetmap.org/way/493253453",
            "  guide    model its facade from your own observations: the shell is ODbL, so use it only to place your facade (never --derived-from-shell): docs/contributing/facades.md",
            "  step 1   export the shell to place your facade on (don't copy its geometry), as contrib/493253453-shell.glb. In a terminal, in the StreetWide repository folder:",
            ".venv/bin/python -m tools.contrib reference --world 'fixtures/world' --building 'osm:way:493253453'",
        ] {
            assert!(lines.contains(&line), "{line}\n{text}");
        }
        // Streets and the like: where to fix them, and no command.
        let d = fixture_description(
            "tile:17/68646/45903",
            Layer::Transport,
            Some(Material::Road),
        );
        let text = terminal_text(&d, Shell::Sh, fixture, at, &[]);
        assert!(
            text.contains("https://www.openstreetmap.org/#map=19/47.36947/8.54405"),
            "{text}"
        );
        assert!(text.trim_end().ends_with("fix streets and paths in OpenStreetMap; the next baseline refresh brings the fix in"), "{text}");
        assert!(!text.contains("tools.contrib"));
    }

    #[test]
    fn only_clicks_after_capture_print() {
        assert!(wants_print(true, false, true, true));
        // The click that captures the mouse, no click, off, on the map.
        assert!(!wants_print(true, false, true, false));
        assert!(!wants_print(true, false, false, true));
        assert!(!wants_print(false, false, true, true));
        assert!(!wants_print(true, true, true, true));
    }

    /// The panel says a selection was printed while it stays selected, for
    /// a few seconds: not of whatever the crosshair moves on to.
    #[test]
    fn the_printed_note_stays_with_what_was_printed() {
        let selection = |id: &str, layer: Layer, material: Material| {
            let description = fixture_description(id, layer, Some(material));
            let target = Target::of(0, layer, id);
            Selected {
                keys: target.keys(description.tile),
                panel: panel_text(&description, Shell::Sh, Path::new("fixtures/world")),
                target,
                material,
                description,
                point: Vec3::ZERO,
            }
        };
        let street = selection("tile:17/68646/45903", Layer::Transport, Material::Road);
        let ribbons = street.target.clone();
        let mut inspect = Inspect::new(true);
        inspect.select(Some(selection(
            FIXTURE_BUILDING,
            Layer::Shell,
            Material::Wall,
        )));
        inspect.mark_printed();
        assert_eq!(footer(inspect.printed(), true), "Printed to the terminal.");
        // A second later, on its roof: the same building, still printed.
        inspect.tick(1.0);
        let building = Target::of(0, Layer::Facade, FIXTURE_BUILDING);
        assert!(inspect.still_on(&building, Material::Roof, Vec3::X));
        assert!(inspect.printed());
        assert_eq!(inspect.selected().map(|s| s.point), Some(Vec3::X));
        // On to the street: not printed.
        assert!(!inspect.still_on(&ribbons, Material::Road, Vec3::Y));
        inspect.select(Some(street));
        assert!(!inspect.printed());
        assert!(footer(inspect.printed(), true).starts_with("Click: print it"));
        // Printed, and three seconds later.
        inspect.mark_printed();
        inspect.tick(2.0);
        assert!(inspect.printed());
        inspect.tick(1.0);
        assert!(!inspect.printed());
        // Streets and paths share the ribbons: a path is another selection.
        inspect.mark_printed();
        assert!(inspect.still_on(&ribbons, Material::Road, Vec3::Z));
        assert!(!inspect.still_on(&ribbons, Material::Path, Vec3::Z));
        // Nothing under the crosshair: nothing printed, nor to print.
        inspect.select(None);
        assert!(!inspect.printed());
        inspect.mark_printed();
        assert!(!inspect.printed());
    }

    /// The crosshair reaches as far as the camera draws: flying 3 km above
    /// the fixture tile, looking down, it finds the ground the camera
    /// shows. Picked as the scene registers meshes, at their tiles'
    /// offsets.
    #[test]
    fn the_crosshair_reaches_as_far_as_the_camera_draws() {
        use bevy::ecs::system::RunSystemOnce;
        let dir = repo().join("fixtures/world");
        let options = crate::Options {
            world: Some(dir.clone()),
            ..default()
        };
        let session = Session::start(&options).unwrap();
        let mut index = PickIndex::default();
        let world = session.streamer().index();
        for (t, tile) in world.tiles().iter().enumerate() {
            let offset = session.offset_m(tile.origin_mm);
            for a in &tile.assets {
                let mesh = Arc::new(world.load_mesh(&a.content_id).unwrap());
                if let Some(entry) = PickEntry::new(t, a.layer, offset, mesh) {
                    index.insert(asset_key(tile.key, a.layer, &a.feature_id), entry);
                }
            }
        }
        let up = session.curr_feet + Vec3::Y * 3_000.0;
        let mut app = World::new();
        app.insert_resource(Travel {
            view: View::Fly { pos: up },
            fly_speed: 30.0,
            places: Vec::new(),
            pending: None,
            cut: false,
        });
        app.insert_resource(index);
        app.insert_resource(manifests(&dir));
        app.insert_resource(InspectStats::default());
        app.insert_resource(Inspect::new(true));
        app.insert_resource(session);
        app.spawn((
            Transform::from_translation(up).looking_to(Vec3::NEG_Y, Vec3::NEG_Z),
            crate::scene::projection(),
            PlayerCamera,
        ));
        app.run_system_once(pick_target).unwrap();
        let distance = app.resource::<InspectStats>().distance;
        assert!(
            distance.is_some_and(|d| (2_990.0..3_010.0).contains(&d)),
            "{distance:?}"
        );
        assert!(app.resource::<Inspect>().selected().is_some());
    }

    /// Highlighted materials keep their look and add the class's glow,
    /// stronger for buildings than for a whole tile's terrain or streets.
    #[test]
    fn highlights_add_the_class_glow() {
        let base = StandardMaterial {
            base_color: Color::srgb(0.5, 0.4, 0.3),
            perceptual_roughness: 0.8,
            ..default()
        };
        let lit = highlighted(&base, Class::Improvable, true);
        assert_eq!(lit.perceptual_roughness, 0.8);
        assert_eq!(lit.emissive_exposure_weight, 0.0);
        // Tinted towards amber: red kept, blue cut; neutral assets keep
        // their colour.
        let (b, l) = (base.base_color.to_linear(), lit.base_color.to_linear());
        assert!(
            (l.red - b.red).abs() < 1e-6 && l.blue < b.blue * 0.5,
            "{l:?}"
        );
        let neutral = highlighted(&base, Class::Neutral, false);
        assert_eq!(neutral.base_color.to_linear(), b);
        let (amber, blue) = (Class::Improvable.glow(true), Class::Contributed.glow(true));
        assert!(
            amber.red > amber.green && amber.green > amber.blue,
            "{amber:?}"
        );
        assert!(blue.blue > blue.green && blue.green > blue.red, "{blue:?}");
        assert!(Class::Neutral.glow(false).red < Class::Neutral.glow(true).red);
    }
}
