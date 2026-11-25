//! Drawing prototype (branch `drawing-prototype`, not for merging as is):
//! the district's real trees, street lamps and lit windows, drawn so that
//! their cost and look can be measured for the decision record that extends
//! ADR-0012 ("how the client draws trees, lamps and lit windows").
//!
//! Everything here is off unless a command-line option turns it on; without
//! them the client behaves exactly as before:
//! - `--proto-points <file>`: trees and lamps converted once from the city's
//!   open data into the region frame (a scratch file, not world data);
//! - `--proto-trees`: procedural trees per genus group, with levels of
//!   detail, leaves or bare twigs by season;
//! - `--proto-lamps <n>`: lamp models with glowing heads, and a pool of the
//!   `n` nearest lamps as real shadowless lights, spot lights aimed down or
//!   with `--proto-lamp-point` point lights (0: heads only);
//! - `--proto-windows`: a seeded share of the procedural windows lit at night;
//! - `--bench <out.json>`: a benchmark of one stop, written as JSON;
//! - `--preset high|fast`, `--proto-night-ev <EV100>` and the tuning options
//!   listed in [`USAGE`].
//!
//! Rendering only: nothing here reaches the simulation. Seeded choices (which
//! window is lit, which tree variant) hash stable ids, so they are the same on
//! every run.

pub mod bench;
mod lamps;
mod points;
mod shapes;
mod textures;
mod trees;
pub mod windows;

use std::path::PathBuf;
use std::time::Instant;

use bevy::prelude::*;

use crate::Options;
use crate::heights::{Height, HeightIndex};
use crate::session::Session;

/// The prototype's options, for the usage text.
pub const USAGE: &str = "prototype: [--proto-points <file>] [--proto-trees] [--proto-lamps <n>] [--proto-lamp-range <m>] [--proto-lamp-lm <lumens>] [--proto-lamp-point] [--proto-windows] [--proto-lod <near>,<mid>,<far>] [--proto-tree-shadows <levels>] [--proto-night-ev <EV100>] [--preset high|fast] [--bench <out.json> [--bench-frames <n>] [--bench-turn]]";

/// The sun is this far below the horizon when lamps and windows switch on
/// (degrees), a third of the way into civil dusk: the prototype's choice,
/// since no switching threshold was found for Zurich (games note, section
/// 9). The client's sunlight fades out between 2° above and 3° below.
pub const LIGHTS_ON_BELOW_DEG: f64 = -2.0;

/// Prototype options; all off by default.
#[derive(Clone, Debug)]
pub struct ProtoOptions {
    pub points: Option<PathBuf>,
    pub trees: bool,
    /// Real spot lights for the nearest lamps; `Some(0)`: heads only.
    pub lamps: Option<usize>,
    pub lamp_range_m: f32,
    pub lamp_lumens: f32,
    /// Point lights instead of spot lights aimed down.
    pub lamp_point: bool,
    pub windows: bool,
    /// Where tree levels 0, 1 and 2 end (m).
    pub lod_m: [f32; 3],
    /// How many tree levels, from the nearest, cast sun shadows.
    pub tree_shadow_levels: u8,
    /// Night exposure floor (EV100) instead of the client's 6.5.
    pub night_ev100: Option<f32>,
    pub preset_fast: bool,
    pub bench: Option<PathBuf>,
    pub bench_frames: usize,
    pub bench_turn: bool,
}

impl Default for ProtoOptions {
    fn default() -> Self {
        ProtoOptions {
            points: None,
            trees: false,
            lamps: None,
            lamp_range_m: 20.0,
            lamp_lumens: 2500.0,
            lamp_point: false,
            windows: false,
            lod_m: [40.0, 120.0, crate::scene::RENDER_RADIUS_M],
            tree_shadow_levels: 2,
            night_ev100: None,
            preset_fast: false,
            bench: None,
            bench_frames: 600,
            bench_turn: false,
        }
    }
}

fn number<T: std::str::FromStr>(text: &str, what: &str) -> Result<T, String> {
    text.parse().map_err(|_| format!("{what} needs a number"))
}

/// Parses one prototype option; `Ok(false)` if `arg` is not one.
pub fn parse_arg(
    o: &mut ProtoOptions,
    arg: &str,
    value: &mut dyn FnMut() -> Result<String, String>,
) -> Result<bool, String> {
    match arg {
        "--proto-points" => o.points = Some(PathBuf::from(value()?)),
        "--proto-trees" => o.trees = true,
        "--proto-lamps" => o.lamps = Some(number(&value()?, arg)?),
        "--proto-lamp-range" => {
            o.lamp_range_m = number::<f32>(&value()?, arg)?;
            if !(1.0..=200.0).contains(&o.lamp_range_m) {
                return Err("--proto-lamp-range needs metres from 1 to 200".into());
            }
        }
        "--proto-lamp-lm" => o.lamp_lumens = number::<f32>(&value()?, arg)?.max(0.0),
        "--proto-lamp-point" => o.lamp_point = true,
        "--proto-windows" => o.windows = true,
        "--proto-lod" => {
            let v: Vec<f32> = value()?
                .split(',')
                .map(|p| p.trim().parse().ok())
                .collect::<Option<_>>()
                .ok_or("--proto-lod needs <near>,<mid>,<far> in metres")?;
            match v[..] {
                [a, b, c] if 0.0 < a && a < b && b < c => o.lod_m = [a, b, c],
                _ => return Err("--proto-lod needs three increasing distances".into()),
            }
        }
        "--proto-tree-shadows" => o.tree_shadow_levels = number::<u8>(&value()?, arg)?.min(3),
        "--proto-night-ev" => o.night_ev100 = Some(number(&value()?, arg)?),
        "--preset" => {
            o.preset_fast = match value()?.as_str() {
                "high" => false,
                "fast" => true,
                _ => return Err("--preset needs high or fast".into()),
            }
        }
        "--bench" => o.bench = Some(PathBuf::from(value()?)),
        "--bench-frames" => o.bench_frames = number::<usize>(&value()?, arg)?.max(10),
        "--bench-turn" => o.bench_turn = true,
        _ => return Ok(false),
    }
    Ok(true)
}

/// Checks options that only make sense together.
pub fn validate(o: &ProtoOptions) -> Result<(), String> {
    if (o.trees || o.lamps.is_some()) && o.points.is_none() {
        return Err("--proto-trees and --proto-lamps need --proto-points <file>".into());
    }
    Ok(())
}

/// What the prototype drew and how long it took, for the benchmark and the
/// log.
#[derive(Resource, Default, Debug)]
pub struct ProtoStats {
    pub points_ms: f64,
    pub tree_assets_ms: f64,
    pub lamp_assets_ms: f64,
    /// Time spent placing points on the ground, spread over frames.
    pub placement_ms: f64,
    pub window_masks_ms: f64,
    pub trees_total: usize,
    pub trees_placed: usize,
    pub tree_entities: usize,
    /// Trees whose crown was narrowed to keep it out of a facade.
    pub crowns_narrowed: usize,
    pub lamps_total: usize,
    pub lamps_placed: usize,
    /// Light records the placed lamps stand for (stacks drawn once).
    pub lamp_records: usize,
    pub lamps_inside_skipped: usize,
    pub lamps_on_bridges: usize,
    /// River lamps with no deck within 5 m: left out.
    pub lamps_in_river_left_out: usize,
    pub lamp_entities: usize,
    pub lights_pool: usize,
    pub lights_lit: usize,
    pub lamps_lit: bool,
    pub leaves_on: Option<bool>,
    pub windows_lit: bool,
    pub windows_upper_share: f32,
    pub windows_ground_share: f32,
    /// Largest difference between the client's terrain and the file's
    /// swissALTI3D ground under a point (m), as a check of the frame.
    pub ground_max_diff_m: f64,
}

impl ProtoStats {
    /// Every point is placed (or there are none to place).
    pub fn placed(&self) -> bool {
        self.trees_placed >= self.trees_total && self.lamps_placed >= self.lamps_total
    }
}

/// The loaded points, waiting to be placed on the ground.
#[derive(Resource)]
struct Pending {
    trees: Vec<points::Tree>,
    lamps: Vec<points::Lamp>,
    next_tree: usize,
    next_lamp: usize,
}

pub struct ProtoPlugin;

impl Plugin for ProtoPlugin {
    fn build(&self, app: &mut App) {
        let o = app
            .world()
            .get_resource::<Options>()
            .map(|o| o.proto.clone())
            .unwrap_or_default();
        app.init_resource::<ProtoStats>();
        if o.trees || o.lamps.is_some() {
            app.add_systems(Startup, load_points).add_systems(
                Update,
                place_points
                    .after(crate::scene::SceneSystems)
                    .run_if(resource_exists::<Pending>),
            );
        }
        if o.trees {
            trees::build(app);
        }
        if o.lamps.is_some() {
            lamps::build(app);
        }
        if o.windows {
            windows::build(app);
        }
        if o.bench.is_some() {
            bench::build(app);
        }
    }
}

fn load_points(mut commands: Commands, options: Res<Options>, mut stats: ResMut<ProtoStats>) {
    let Some(path) = &options.proto.points else {
        return;
    };
    let started = Instant::now();
    match points::load(path) {
        Ok(p) => {
            stats.points_ms = started.elapsed().as_secs_f64() * 1000.0;
            let trees = if options.proto.trees {
                p.trees
            } else {
                Vec::new()
            };
            let lamps = if options.proto.lamps.is_some() {
                p.lamps
            } else {
                Vec::new()
            };
            stats.lamps_inside_skipped = lamps
                .iter()
                .filter(|l| l.mount == points::Mount::Inside)
                .count();
            let lamps: Vec<_> = lamps
                .into_iter()
                .filter(|l| l.mount != points::Mount::Inside)
                .collect();
            stats.trees_total = trees.len();
            stats.lamps_total = lamps.len();
            info!(
                "prototype: {} trees and {} lamp positions from {} in {:.0} ms ({} lamps inside buildings left out)",
                trees.len(),
                lamps.len(),
                path.display(),
                stats.points_ms,
                stats.lamps_inside_skipped
            );
            commands.insert_resource(Pending {
                trees,
                lamps,
                next_tree: 0,
                next_lamp: 0,
            });
        }
        Err(e) => error!("prototype: {}: {e}", path.display()),
    }
}

/// Time per frame for placing points on the ground.
const PLACE_BUDGET_MS: f64 = 4.0;

/// Places trees and lamps on the client's own terrain (as drawn), as soon
/// as it has loaded; lamps over the river stand on the bridge deck.
#[allow(clippy::too_many_arguments)]
fn place_points(
    mut commands: Commands,
    session: Res<Session>,
    mut heights: ResMut<HeightIndex>,
    mut pending: ResMut<Pending>,
    mut stats: ResMut<ProtoStats>,
    tree_assets: Option<Res<trees::TreeAssets>>,
    lamp_assets: Option<Res<lamps::LampAssets>>,
    mut lamp_set: Option<ResMut<lamps::LampSet>>,
    clock: Res<crate::sky::Clock>,
) {
    let started = Instant::now();
    let streamer = session.streamer();
    let budget = |s: &Instant| s.elapsed().as_secs_f64() * 1000.0 > PLACE_BUDGET_MS;
    if let Some(assets) = &tree_assets {
        let leaves = trees::season(&clock);
        while pending.next_tree < pending.trees.len() && !budget(&started) {
            let t = &pending.trees[pending.next_tree];
            let y = match heights.ground(streamer, t.x, t.z) {
                Height::At(y) => y,
                Height::Loading => break,
                Height::Outside => t.ground,
            };
            stats.ground_max_diff_m = stats.ground_max_diff_m.max((y - t.ground).abs());
            let at = session.region_to_render([t.x, y, t.z]);
            let (entities, narrowed) = trees::spawn(&mut commands, assets, t, at, &leaves);
            stats.tree_entities += entities;
            stats.crowns_narrowed += usize::from(narrowed);
            stats.trees_placed += 1;
            pending.next_tree += 1;
        }
    }
    if let (Some(assets), Some(set)) = (&lamp_assets, lamp_set.as_mut()) {
        while pending.next_lamp < pending.lamps.len() && !budget(&started) {
            let l = &pending.lamps[pending.next_lamp];
            let mut y = match heights.ground(streamer, l.x, l.z) {
                Height::At(y) => y,
                Height::Loading => break,
                Height::Outside => l.ground,
            };
            stats.ground_max_diff_m = stats.ground_max_diff_m.max((y - l.ground).abs());
            let (mut x, mut z) = (l.x, l.z);
            if l.water {
                // Lamps over the Limmat: where the ground under them is the
                // water's (the data anchors bridge lamps at river level),
                // stand them on the nearest deck or quay within 5 m; leave
                // out those with none near.
                let river = match crate::travel::water_level(streamer, x, z) {
                    crate::travel::Water::At(level) => level,
                    crate::travel::Water::Loading => break,
                    crate::travel::Water::Dry => y,
                };
                if y < river + 0.5 {
                    match bridge_deck(streamer, x, z, river) {
                        Err(()) => break,
                        Ok(Some(deck)) => {
                            (x, y, z) = deck;
                            stats.lamps_on_bridges += 1;
                        }
                        Ok(None) => {
                            stats.lamps_in_river_left_out += 1;
                            stats.lamps_placed += 1;
                            pending.next_lamp += 1;
                            continue;
                        }
                    }
                }
            }
            let at = session.region_to_render([x, y, z]);
            stats.lamp_entities += lamps::spawn(&mut commands, assets, set, l, at);
            stats.lamp_records += l.records as usize;
            stats.lamps_placed += 1;
            pending.next_lamp += 1;
        }
    }
    stats.placement_ms += started.elapsed().as_secs_f64() * 1000.0;
    if pending.next_tree >= pending.trees.len() && pending.next_lamp >= pending.lamps.len() {
        info!(
            "prototype: placed {} trees ({} entities, {} crowns narrowed at facades) and {} lamps ({} entities, {} moved onto bridge decks, {} in the river left out) in {:.0} ms of frame time; terrain within {:.2} m of the file's ground",
            stats.trees_placed,
            stats.tree_entities,
            stats.crowns_narrowed,
            stats.lamps_placed - stats.lamps_in_river_left_out,
            stats.lamp_entities,
            stats.lamps_on_bridges,
            stats.lamps_in_river_left_out,
            stats.placement_ms,
            stats.ground_max_diff_m
        );
        commands.remove_resource::<Pending>();
    }
}

/// The nearest bridge deck (or quay) above the river within 5 m of a lamp
/// anchored at river level `river` (region frame): its highest point on the
/// first ring that has one. `Err` while meshes load.
fn bridge_deck(
    streamer: &op_stream::Streamer,
    x: f64,
    z: f64,
    river: f64,
) -> Result<Option<(f64, f64, f64)>, ()> {
    for r in [0.0, 0.75, 1.5, 2.5, 3.5, 5.0] {
        let n = if r == 0.0 { 1 } else { 8 };
        let mut best: Option<(f64, f64, f64)> = None;
        for k in 0..n {
            let a = f64::from(k) / f64::from(n) * std::f64::consts::TAU;
            let (px, pz) = (x + r * a.cos(), z + r * a.sin());
            // A deck or quay above the water, or a street or bridge drawn
            // at the water's level (some bridges are, in this world).
            if let Some((y, street)) = ground_or_deck(streamer, px, pz, river + 8.0)?
                && (y > river + 0.5 || (street && y > river - 0.3))
                && best.is_none_or(|b| y > b.1)
            {
                best = Some((px, y, pz));
            }
        }
        if best.is_some() {
            return Ok(best);
        }
    }
    Ok(None)
}

/// The highest terrain or street surface below `limit` at a region-frame
/// point, and whether it is a street or bridge, from the meshes as drawn (bridge decks can be drawn by a
/// transport mesh the simulation does not collide with). `Err` while
/// meshes load.
fn ground_or_deck(
    streamer: &op_stream::Streamer,
    x: f64,
    z: f64,
    limit: f64,
) -> Result<Option<(f64, bool)>, ()> {
    let mut best: Option<(f64, bool)> = None;
    for tile in streamer.index().tiles() {
        let o = tile.origin_mm.map(|v| v as f64 / 1000.0);
        let (lx, lz) = (x - o[0], z - o[2]);
        for a in &tile.assets {
            if !matches!(
                a.layer,
                op_world::Layer::Terrain | op_world::Layer::Transport
            ) {
                continue;
            }
            let b = a.bounds_mm;
            let inside = |k: usize, v: f64| {
                v >= b.min[k] as f64 / 1000.0 - 1e-3 && v <= b.max[k] as f64 / 1000.0 + 1e-3
            };
            if !inside(0, lx) || !inside(2, lz) {
                continue;
            }
            let Some(mesh) = streamer.get(&a.content_id) else {
                if streamer.error(&a.content_id).is_none() {
                    streamer.request(&a.content_id);
                    return Err(());
                }
                continue;
            };
            for prim in mesh.nodes.iter().flat_map(|n| &n.primitives) {
                for t in prim.indices.as_chunks::<3>().0 {
                    let p = [0, 1, 2].map(|k| prim.positions[t[k] as usize].map(f64::from));
                    if let Some(y) = crate::travel::height_in_triangle(p, lx, lz) {
                        let y = y + o[1];
                        if y <= limit && best.is_none_or(|b| y > b.0) {
                            best = Some((y, a.layer == op_world::Layer::Transport));
                        }
                    }
                }
            }
        }
    }
    Ok(best)
}

/// FNV-1a of a stable id: the seed of every per-object choice.
pub fn seed(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// SplitMix64's finaliser: mixes a seed with a salt into a well-spread hash.
pub fn mix(seed: u64, salt: u64) -> u64 {
    let mut z = seed ^ salt.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Uniform in [0, 1) from a hash.
pub fn unit(h: u64) -> f32 {
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// A small seeded generator for shapes and textures.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        mix(self.0, 0)
    }

    /// Uniform in [0, 1).
    pub fn f(&mut self) -> f32 {
        unit(self.next_u64())
    }

    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f()
    }
}

/// Linear RGB of a black body at `kelvin`, scaled so its largest component
/// is 1 (Tanner Helland's fit of the CIE colour matching functions; good to
/// a few percent between 1,000 and 10,000 K, enough for lamps).
pub fn kelvin(kelvin: f32) -> [f32; 3] {
    let t = kelvin / 100.0;
    let r = if t <= 66.0 {
        255.0
    } else {
        329.7 * (t - 60.0).powf(-0.1332)
    };
    let g = if t <= 66.0 {
        99.47 * t.ln() - 161.12
    } else {
        288.12 * (t - 60.0).powf(-0.0755)
    };
    let b = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.52 * (t - 10.0).ln() - 305.04
    };
    let srgb = [r, g, b].map(|c| (c / 255.0).clamp(0.0, 1.0));
    let lin = srgb.map(|c| {
        if c <= 0.040_45 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    });
    let m = lin[0].max(lin[1]).max(lin[2]);
    lin.map(|c| c / m)
}

/// Relative luminance of a linear RGB colour (Rec. 709).
pub fn luminance(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_parse_and_default_off() {
        let o = ProtoOptions::default();
        assert!(!o.trees && o.lamps.is_none() && !o.windows && o.bench.is_none());
        let mut o = ProtoOptions::default();
        let mut args = ["40,90,500".to_string()].into_iter();
        let mut value = || args.next().ok_or("needs a value".to_string());
        assert!(parse_arg(&mut o, "--proto-lod", &mut value).unwrap());
        assert_eq!(o.lod_m, [40.0, 90.0, 500.0]);
        assert!(!parse_arg(&mut o, "--not-ours", &mut value).unwrap());
        assert!(
            validate(&ProtoOptions {
                trees: true,
                ..ProtoOptions::default()
            })
            .is_err()
        );
    }

    #[test]
    fn seeds_are_stable_and_spread() {
        assert_eq!(seed("AA-1656"), seed("AA-1656"));
        assert_ne!(seed("AA-1656"), seed("AA-1657"));
        let mut r = Rng::new(7);
        let v: Vec<f32> = (0..1000).map(|_| r.f()).collect();
        assert!(v.iter().all(|x| (0.0..1.0).contains(x)));
        let mean = v.iter().sum::<f32>() / 1000.0;
        assert!((mean - 0.5).abs() < 0.05, "{mean}");
    }

    #[test]
    fn warm_white_is_orange_and_daylight_is_neutral() {
        let w = kelvin(3000.0);
        assert!(w[0] == 1.0 && w[1] < 0.6 && w[2] < w[1], "{w:?}");
        let d = kelvin(6500.0);
        assert!(d.iter().all(|c| *c > 0.9), "{d:?}");
    }
}
