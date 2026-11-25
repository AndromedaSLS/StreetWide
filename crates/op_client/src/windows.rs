//! Lit windows at night (ADR-0022, option W1): a seeded share of the
//! client's procedural windows glows.
//!
//! **Which window is lit** follows only from the building id, the wall, the
//! window's bay and storey, and the clock: a hash of the building, wall,
//! bay, storey and night, compared with the share of windows lit at that
//! hour of that day type. Nothing comes from data about real homes
//! (occupancy, energy, registers); interiors are fictional, and nothing is
//! drawn behind the glass (RFC 0004, ADR-0022). Each window has its own
//! hash, so no pattern repeats along a wall or from one wall to another.
//! As the share rises through an evening, windows only switch on; another
//! night lights other windows.
//!
//! **How:** windowed walls (facade and ground-floor styles) use
//! [`WindowMaterial`], the StandardMaterial with a small shader extension
//! (`windows.wesl`) that adds the glow after lighting. A wall's seed, from
//! the building id and the wall's plane, rides in the second texture
//! coordinates (`materials::walls`); the first count bays and storeys. The
//! glass's shape is a mask per style, made from the same rectangles as the
//! facade textures ([`texgen::window_glow`]). In the plain look (F2) the
//! walls get the palette colour and no lit windows: decoration is not data
//! (ADR-0012).
//!
//! **Brightness** follows the exposure, in cd/m². A lit flat's window
//! averages about 6 cd/m² (CIE 2025, a Czech housing estate); shop windows
//! and adverts about 50 cd/m², which at the night exposure (EV100 3) is a
//! white panel, so shops get less here: a look choice, a starting value
//! for ADR-0022's open question 2. Each lit window gets one of a few
//! colours (mostly warm) and a brightness from half to one and a half times
//! the average.
//!
//! **Shares** by hour and day type are set by hand, not measured: no Swiss
//! data on windows lit by hour was found. They lean on what was found: at
//! most about half of residential windows lit at peak (CIE 2025), household
//! lighting peaking between 7 and 10 pm (UK, 2013), and shops shut on
//! Sundays. Windows switch on as dusk falls ([`lights_on`]), so by day they
//! add nothing.

use bevy::asset::{AssetPath, RenderAssetUsages, embedded_asset, embedded_path};
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat,
};
use bevy::shader::ShaderRef;

use crate::geo::{DateTime, days_from_civil};
use crate::materials::Surface;
use crate::sky::{Clock, SkySystems, SunState};
use crate::texgen::{self, FACADES, Pattern};

/// A windowed wall's material: the StandardMaterial and its lit windows.
pub type WindowMaterial = ExtendedMaterial<StandardMaterial, WindowGlow>;

/// What a lit window averages over its glass (cd/m²): homes and offices,
/// on upper floors and on ground floors without shop windows (CIE 2025).
pub const HOME_CD_M2: f32 = 6.0;
/// The same for shop windows: a fifth of CIE's 50 cd/m², a look choice
/// (see the module's notes).
pub const SHOP_CD_M2: f32 = 10.0;

/// Texels per side of a glass mask (one window cell).
const GLASS_SIZE: usize = 128;

/// The lights' colour temperatures (K) and odds: homes and offices mostly
/// warm, some neutral, a few cool (LED, screens); shops warm and neutral.
const HOME_LIGHTS: [(f32, f32); 3] = [(2800.0, 0.65), (3500.0, 0.27), (5000.0, 0.08)];
const SHOP_LIGHTS: [(f32, f32); 3] = [(3000.0, 0.7), (3600.0, 0.3), (6000.0, 0.0)];

/// Shares of windows lit at each full hour (Swiss time), set by hand;
/// linear in between. Homes and offices on weekdays: few lit late at
/// night, a morning rise, offices by day, a peak at 20:00 to 21:00 under
/// CIE's "at most 50%".
const HOMES_WEEKDAY: [f32; 24] = [
    0.14, 0.08, 0.05, 0.04, 0.04, 0.06, 0.14, 0.22, 0.20, 0.18, 0.16, 0.16, 0.16, 0.16, 0.17, 0.20,
    0.26, 0.34, 0.40, 0.43, 0.45, 0.45, 0.40, 0.28,
];
/// Homes on Saturdays and Sundays: no offices, a later morning, a later
/// night.
const HOMES_WEEKEND: [f32; 24] = [
    0.18, 0.12, 0.07, 0.05, 0.04, 0.04, 0.06, 0.10, 0.15, 0.16, 0.15, 0.14, 0.14, 0.14, 0.15, 0.17,
    0.22, 0.30, 0.36, 0.40, 0.42, 0.42, 0.38, 0.28,
];
/// Shop windows on weekdays: open from about 9:00 to 19:00, displays lit
/// into the evening, most off by 23:00 (a Swiss permit example switches
/// illuminated adverts off at 23:00).
const SHOPS_WEEKDAY: [f32; 24] = [
    0.10, 0.08, 0.08, 0.08, 0.08, 0.08, 0.10, 0.25, 0.60, 0.85, 0.90, 0.90, 0.90, 0.90, 0.90, 0.90,
    0.90, 0.90, 0.88, 0.72, 0.55, 0.45, 0.35, 0.18,
];
/// Shop windows on Saturdays: closing earlier.
const SHOPS_SATURDAY: [f32; 24] = [
    0.12, 0.10, 0.08, 0.08, 0.08, 0.08, 0.10, 0.20, 0.55, 0.85, 0.90, 0.90, 0.90, 0.90, 0.90, 0.90,
    0.88, 0.75, 0.55, 0.48, 0.42, 0.38, 0.32, 0.18,
];
/// Shop windows on Sundays: shut; some displays, cafés and restaurants lit.
const SHOPS_SUNDAY: [f32; 24] = [
    0.12, 0.10, 0.08, 0.08, 0.08, 0.08, 0.08, 0.10, 0.15, 0.25, 0.30, 0.32, 0.32, 0.32, 0.32, 0.32,
    0.32, 0.34, 0.36, 0.36, 0.34, 0.30, 0.25, 0.15,
];

/// What a windowed wall's lights are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub enum Kind {
    /// No lit windows: the plain look.
    #[default]
    Off,
    /// Homes and offices: upper floors, and ground floors with doors and
    /// windows.
    Homes,
    /// Shop windows on the ground floor.
    Shops,
}

impl Kind {
    /// The kind of a windowed surface: ground-floor style 0 has shop
    /// windows (see `texgen::ground_floor`).
    pub fn of(surface: Surface) -> Self {
        match surface {
            Surface::GroundFloor(0) => Kind::Shops,
            Surface::Facade(_) | Surface::GroundFloor(_) => Kind::Homes,
            _ => Kind::Off,
        }
    }
}

/// The shader extension's data: the share lit now, the night, the lights'
/// colours already scaled to luminance, and the glass mask. Bindless where
/// the device allows, like the StandardMaterial, so windowed walls batch
/// with the rest (a non-bindless extension cost several milliseconds a
/// frame in a first version, with a material per style and tint).
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
#[data(50, WindowGlowUniform, binding_array(101))]
#[bindless(index_table(range(50..53), binding(100)))]
pub struct WindowGlow {
    /// The share of windows lit now (0 to 1).
    pub share: f32,
    /// The night (see [`night_of`]): another night lights other windows.
    pub night: u32,
    /// Cumulative odds of a warm and of a neutral light; the rest are cool.
    pub odds: Vec2,
    /// The lights' linear colours, scaled so that a lit window's glass
    /// averages the kind's luminance (cd/m²).
    pub warm: Vec4,
    pub neutral: Vec4,
    pub cool: Vec4,
    /// Their average, for windows too small on screen to tell apart.
    pub mean: Vec4,
    /// The glass's mask for the wall's style.
    #[texture(51)]
    #[sampler(52)]
    pub glass: Option<Handle<Image>>,
    pub kind: Kind,
}

/// [`WindowGlow`]'s data as the shader reads it.
#[derive(Clone, Default, ShaderType)]
pub struct WindowGlowUniform {
    share: f32,
    night: u32,
    odds: Vec2,
    warm: Vec4,
    neutral: Vec4,
    cool: Vec4,
    mean: Vec4,
}

impl From<&WindowGlow> for WindowGlowUniform {
    fn from(g: &WindowGlow) -> Self {
        WindowGlowUniform {
            share: g.share,
            night: g.night,
            odds: g.odds,
            warm: g.warm,
            neutral: g.neutral,
            cool: g.cool,
            mean: g.mean,
        }
    }
}

impl MaterialExtension for WindowGlow {
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Path(
            AssetPath::from_path_buf(embedded_path!("windows.wesl")).with_source("embedded"),
        )
    }
}

impl WindowGlow {
    /// Windows of a kind, with the style's glass mask and the mean of its
    /// glass texels (see [`glass_image`]); unlit until [`light_windows`]
    /// sets the share.
    pub fn new(kind: Kind, glass: Handle<Image>, glass_mean: f32) -> Self {
        let (lights, target) = match kind {
            Kind::Off => return WindowGlow::default(),
            Kind::Homes => (HOME_LIGHTS, HOME_CD_M2),
            Kind::Shops => (SHOP_LIGHTS, SHOP_CD_M2),
        };
        let scale = target / glass_mean.max(1e-3);
        let colour = |(k, _): (f32, f32)| {
            let c = kelvin(k);
            let l = luminance(c);
            Vec4::new(c[0] / l, c[1] / l, c[2] / l, 1.0) * scale
        };
        let [warm, neutral, cool] = lights.map(colour);
        let mean = lights
            .iter()
            .zip([warm, neutral, cool])
            .map(|((_, p), c)| c * *p)
            .sum::<Vec4>();
        WindowGlow {
            share: 0.0,
            night: 0,
            odds: Vec2::new(lights[0].1, lights[0].1 + lights[1].1),
            warm,
            neutral,
            cool,
            mean,
            glass: Some(glass),
            kind,
        }
    }
}

/// The plugin: the material, its shader, and the system that lights the
/// windows by the clock.
pub struct WindowsPlugin;

impl Plugin for WindowsPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "windows.wesl");
        app.add_plugins(MaterialPlugin::<WindowMaterial>::default())
            .add_systems(Update, light_windows.after(SkySystems));
    }
}

/// Shares of windows lit at a Swiss civil time: homes and offices, and
/// shop windows.
pub fn shares(local: DateTime) -> (f32, f32) {
    // 1970-01-01 was a Thursday; 0 = Sunday.
    let weekday = (days_from_civil(local.year, local.month, local.day) + 4).rem_euclid(7);
    let (homes, shops) = match weekday {
        0 => (&HOMES_WEEKEND, &SHOPS_SUNDAY),
        6 => (&HOMES_WEEKEND, &SHOPS_SATURDAY),
        _ => (&HOMES_WEEKDAY, &SHOPS_WEEKDAY),
    };
    let h = (local.minute / 60) as usize % 24;
    let t = (local.minute % 60) as f32 / 60.0;
    let at = |c: &[f32; 24]| c[h] + (c[(h + 1) % 24] - c[h]) * t;
    (at(homes), at(shops))
}

/// How far the lights are on (0 to 1) for a sun elevation (degrees): off
/// with the sun 3° up or more, all on with the sun 3° down. Rooms get dark
/// before the street does; by day windows add nothing.
pub fn lights_on(sun_elevation_deg: f64) -> f32 {
    let t = ((3.0 - sun_elevation_deg) / 6.0).clamp(0.0, 1.0) as f32;
    t * t * (3.0 - 2.0 * t)
}

/// The night a Swiss civil time belongs to: its date, until noon the next
/// day, so that the windows lit stay the same through one night.
pub fn night_of(local: DateTime) -> u32 {
    let day = days_from_civil(local.year, local.month, local.day);
    (day - i64::from(local.minute < 12 * 60)) as u32
}

/// A wall's seed for its windows, from the building's hash and the wall's
/// plane (as `materials::walls` keys it): two 16-bit halves, stored as
/// whole numbers in the wall's second texture coordinates, which carry them
/// exactly.
pub fn wall_seed(building: u64, plane: (i64, i64)) -> [f32; 2] {
    let mut z = building ^ (plane.0 as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    z ^= (plane.1 as u64).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    [(z & 0xffff) as f32, ((z >> 16) & 0xffff) as f32]
}

/// The glass mask of a windowed surface's style, with its mip chain, and
/// the mean of its glass texels; `None` for other surfaces.
pub fn glass_image(surface: Surface) -> Option<(Image, f32)> {
    let pattern = match surface {
        Surface::Facade(s) => Pattern::Facade(FACADES[s]),
        Surface::GroundFloor(s) => Pattern::GroundFloor(s),
        _ => return None,
    };
    let n = GLASS_SIZE;
    let mut level: Vec<f32> = (0..n * n)
        .map(|i| {
            let (row, col) = (i / n, i % n);
            texgen::window_glow(
                pattern,
                (col as f32 + 0.5) / n as f32,
                (row as f32 + 0.5) / n as f32,
            )
        })
        .collect();
    let lit: Vec<f32> = level.iter().copied().filter(|g| *g > 0.0).collect();
    let mean = lit.iter().sum::<f32>() / lit.len().max(1) as f32;
    // Box-filtered mips, so a far window glows with its cell's average.
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let mut data = Vec::new();
    let mut size = n;
    let mut levels = 0;
    loop {
        data.extend(level.iter().flat_map(|g| {
            let b = byte(*g);
            [b, b, b, 255]
        }));
        levels += 1;
        if size == 1 {
            break;
        }
        let half = size / 2;
        level = (0..half * half)
            .map(|i| {
                let (r, c) = (2 * (i / half), 2 * (i % half));
                (level[r * size + c]
                    + level[r * size + c + 1]
                    + level[(r + 1) * size + c]
                    + level[(r + 1) * size + c + 1])
                    / 4.0
            })
            .collect();
        size = half;
    }
    let mut image = Image::new_uninit(
        Extent3d {
            width: n as u32,
            height: n as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.mip_level_count = levels;
    image.data = Some(data);
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        address_mode_w: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 16,
        ..default()
    });
    Some((image, mean))
}

/// Lights the windows by the clock: sets every window material's share
/// (inspect mode's highlighted copies too) when the hour, the night or the
/// dusk changes.
fn light_windows(
    clock: Res<Clock>,
    sun: Res<SunState>,
    mut materials: ResMut<Assets<WindowMaterial>>,
    mut last: Local<Option<(u32, u32, u32)>>,
) {
    let on = lights_on(sun.elevation);
    let (homes, shops) = shares(clock.local);
    let (homes, shops) = (homes * on, shops * on);
    let night = night_of(clock.local);
    let state = (homes.to_bits(), shops.to_bits(), night);
    if *last == Some(state) {
        return;
    }
    *last = Some(state);
    for (_, m) in materials.iter_mut() {
        let g = &mut m.extension;
        g.share = match g.kind {
            Kind::Off => 0.0,
            Kind::Homes => homes,
            Kind::Shops => shops,
        };
        g.night = night;
    }
}

/// The shader's choice of lit windows, in Rust, for the tests: the same
/// hash, salts and odds as `windows.wesl`, bit for bit.
#[cfg(test)]
mod mirror {
    use super::*;

    /// Salts for a window's colour and brightness, apart from whether it is
    /// lit. The shader uses the same.
    pub const COLOUR_SALT: u32 = 0x68e3_1da4;
    pub const BRIGHT_SALT: u32 = 0xb529_7a4d;

    /// A PCG hash (Jarzynski and Olano, "Hash Functions for GPU Rendering",
    /// JCGT 9(3), 2020): what the shader computes, bit for bit.
    pub fn pcg(v: u32) -> u32 {
        let state = v.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
        let word = ((state >> ((state >> 28) + 4)) ^ state).wrapping_mul(277_803_737);
        (word >> 22) ^ word
    }

    /// A window's hash, from its wall's seed, its bay and storey (whole texture
    /// coordinates, as the shader floors them) and the night.
    pub fn window_hash(seed: [u32; 2], bay: i32, storey: i32, night: u32) -> u32 {
        let mut h = pcg(night);
        h = pcg(h ^ storey as u32);
        h = pcg(h ^ bay as u32);
        h = pcg(h ^ seed[1]);
        pcg(h ^ seed[0])
    }

    /// Uniform in [0, 1) from a hash.
    pub fn unit(h: u32) -> f32 {
        (h >> 8) as f32 / 16_777_216.0
    }

    /// Whether a window is lit when a share of windows is.
    pub fn is_lit(seed: [u32; 2], bay: i32, storey: i32, night: u32, share: f32) -> bool {
        unit(window_hash(seed, bay, storey, night)) < share
    }

    /// A window's light (linear RGB, cd/m² on average over its glass), as the
    /// shader computes it for a window large enough on screen.
    pub fn window_light(glow: &WindowGlow, seed: [u32; 2], bay: i32, storey: i32) -> Vec3 {
        let h = window_hash(seed, bay, storey, glow.night);
        if unit(h) >= glow.share {
            return Vec3::ZERO;
        }
        let pick = unit(pcg(h ^ COLOUR_SALT));
        let colour = if pick < glow.odds.x {
            glow.warm
        } else if pick < glow.odds.y {
            glow.neutral
        } else {
            glow.cool
        };
        colour.truncate() * (0.5 + unit(pcg(h ^ BRIGHT_SALT)))
    }
}

/// Linear RGB of a black body at `kelvin`, scaled so its largest component
/// is 1 (Tanner Helland's fit, good to a few percent from 1,000 to
/// 10,000 K, enough for lamps and windows).
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
    let lin = [r, g, b].map(|c| {
        let c = (c / 255.0).clamp(0.0, 1.0);
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
    use super::mirror::*;
    use super::*;

    fn at(year: i32, month: u32, day: u32, minute: u32) -> DateTime {
        DateTime {
            year,
            month,
            day,
            minute,
        }
    }

    fn seed_bits(s: [f32; 2]) -> [u32; 2] {
        s.map(|v| v as u32)
    }

    /// A homes glow with a plain mask (mean 1), lit at a share.
    fn homes(share: f32, night: u32) -> WindowGlow {
        WindowGlow {
            share,
            night,
            ..WindowGlow::new(Kind::Homes, Handle::default(), 1.0)
        }
    }

    /// 21 December 2026 is a Monday: shops are open at 18:00. 21 June 2026
    /// is a Sunday: at 22:30 shops are shut and homes past their peak.
    #[test]
    fn shares_follow_the_hour_and_the_day() {
        let (h, s) = shares(at(2026, 12, 21, 18 * 60));
        assert!(
            (h - 0.40).abs() < 1e-6 && (s - 0.88).abs() < 1e-6,
            "{h} {s}"
        );
        let (h, s) = shares(at(2026, 6, 21, 22 * 60 + 30));
        assert!(
            (h - 0.33).abs() < 1e-6 && (s - 0.20).abs() < 1e-6,
            "{h} {s}"
        );
        // Saturday shops close earlier than weekday ones.
        assert!(shares(at(2026, 12, 19, 18 * 60)).1 < shares(at(2026, 12, 18, 18 * 60)).1);
        // At most about half of homes lit at any hour (CIE 2025).
        for c in [HOMES_WEEKDAY, HOMES_WEEKEND] {
            assert!(c.iter().all(|v| (0.0..=0.5).contains(v)));
        }
        // Just before midnight, nearly the first hour's value.
        let (h, _) = shares(at(2026, 12, 21, 23 * 60 + 59));
        assert!((h - 0.14).abs() < 0.01, "{h}");
    }

    #[test]
    fn windows_switch_on_at_dusk_only() {
        assert_eq!(lights_on(31.6), 0.0);
        assert_eq!(lights_on(3.0), 0.0);
        assert_eq!(lights_on(-3.0), 1.0);
        assert_eq!(lights_on(-13.2), 1.0);
        assert!(lights_on(0.0) > 0.4 && lights_on(0.0) < 0.6);
    }

    #[test]
    fn a_night_runs_from_noon_to_noon() {
        let evening = night_of(at(2026, 12, 21, 18 * 60));
        assert_eq!(night_of(at(2026, 12, 22, 2 * 60)), evening);
        assert_eq!(night_of(at(2026, 12, 22, 11 * 60 + 59)), evening);
        assert_eq!(night_of(at(2026, 12, 22, 12 * 60)), evening + 1);
    }

    /// The same window, wall and night give the same answer on every run.
    #[test]
    fn the_choice_is_stable() {
        let seed = seed_bits(wall_seed(42, (10, -7)));
        assert_eq!(wall_seed(42, (10, -7)), wall_seed(42, (10, -7)));
        let a: Vec<bool> = (0..200)
            .map(|i| is_lit(seed, i % 10, -(i / 10), 20_000, 0.4))
            .collect();
        let b: Vec<bool> = (0..200)
            .map(|i| is_lit(seed, i % 10, -(i / 10), 20_000, 0.4))
            .collect();
        assert_eq!(a, b);
        // Known values guard the hash the shader mirrors.
        assert_eq!(pcg(0), 129_708_002);
        assert_eq!(pcg(1), 2_831_084_092);
    }

    /// Over many windows, the share lit matches the curve, and windows only
    /// switch on as the share rises through a night.
    #[test]
    fn the_share_lit_matches_the_curve() {
        let windows: Vec<([u32; 2], i32, i32)> = (0..400u64)
            .flat_map(|b| {
                let seed = seed_bits(wall_seed(b * 7919, (b as i64 % 90, b as i64)));
                (0..8).flat_map(move |bay| (1..6).map(move |s| (seed, bay, -s)))
            })
            .collect();
        let night = night_of(at(2026, 12, 21, 18 * 60));
        for minute in [17 * 60, 18 * 60, 20 * 60 + 30, 23 * 60] {
            let (share, _) = shares(at(2026, 12, 21, minute));
            let lit = windows
                .iter()
                .filter(|(s, b, st)| is_lit(*s, *b, *st, night, share))
                .count();
            let got = lit as f32 / windows.len() as f32;
            assert!(
                (got - share).abs() < 0.02,
                "{minute}: {got} against {share}"
            );
        }
        for (s, b, st) in &windows {
            assert!(!is_lit(*s, *b, *st, night, 0.2) || is_lit(*s, *b, *st, night, 0.4));
        }
        // On average a lit window has the kind's luminance.
        let glow = homes(0.4, night);
        let total: f32 = windows
            .iter()
            .map(|(s, b, st)| luminance(window_light(&glow, *s, *b, *st).to_array()))
            .sum();
        let mean = total / (windows.len() as f32 * 0.4);
        assert!((mean / HOME_CD_M2 - 1.0).abs() < 0.05, "{mean}");
    }

    /// No pattern repeats: two walls of one style, or two buildings, light
    /// different windows, and a long wall does not repeat itself every 16
    /// bays and 8 storeys as the prototype's masks did.
    #[test]
    fn no_pattern_repeats() {
        let pattern = |seed: [u32; 2]| -> Vec<bool> {
            (0..64)
                .flat_map(|bay| (1..17).map(move |s| is_lit(seed, bay, -s, 7, 0.5)))
                .collect()
        };
        let agree = |a: &[bool], b: &[bool]| {
            a.iter().zip(b).filter(|(x, y)| x == y).count() as f32 / a.len() as f32
        };
        let wall_a = pattern(seed_bits(wall_seed(1, (45, 100))));
        let wall_b = pattern(seed_bits(wall_seed(1, (0, 100))));
        let other = pattern(seed_bits(wall_seed(2, (45, 100))));
        // Independent windows agree about half the time at a share of 0.5.
        for (x, y) in [(&wall_a, &wall_b), (&wall_a, &other)] {
            let a = agree(x, y);
            assert!((0.4..0.6).contains(&a), "{a}");
        }
        // Along one wall: bay b and bay b + 16, storey s and s + 8.
        let seed = seed_bits(wall_seed(1, (45, 100)));
        let (mut same, mut n) = (0, 0);
        for bay in 0..48 {
            for s in 1..9 {
                same += usize::from(
                    is_lit(seed, bay, -s, 7, 0.5) == is_lit(seed, bay + 16, -s - 8, 7, 0.5),
                );
                n += 1;
            }
        }
        let a = same as f32 / n as f32;
        assert!((0.4..0.6).contains(&a), "{a}");
        // Another night lights other windows.
        let tomorrow: Vec<bool> = (0..64)
            .flat_map(|bay| (1..17).map(move |s| is_lit(seed, bay, -s, 8, 0.5)))
            .collect();
        let a = agree(&wall_a, &tomorrow);
        assert!((0.4..0.6).contains(&a), "{a}");
    }

    #[test]
    fn wall_seeds_are_whole_16_bit_numbers() {
        for b in 0..100u64 {
            for s in wall_seed(
                b.wrapping_mul(0x1234_5678_9abc),
                (b as i64 - 50, 3 * b as i64),
            ) {
                assert!(s.fract() == 0.0 && (0.0..65_536.0).contains(&s), "{s}");
            }
        }
        assert_ne!(wall_seed(1, (0, 0)), wall_seed(2, (0, 0)));
        assert_ne!(wall_seed(1, (0, 0)), wall_seed(1, (0, 1)));
    }

    /// The masks glow on the glass, and the colours are scaled so that a
    /// lit window's glass averages the kind's luminance.
    #[test]
    fn lit_glass_averages_the_target_luminance() {
        for surface in [
            Surface::Facade(0),
            Surface::Facade(2),
            Surface::GroundFloor(0),
            Surface::GroundFloor(1),
        ] {
            let (image, mean) = glass_image(surface).unwrap();
            assert!(mean > 0.7 && mean <= 1.0, "{surface:?} {mean}");
            let data = image.data.as_ref().unwrap();
            // Mip 0 then smaller levels down to 1 x 1.
            let n = GLASS_SIZE * GLASS_SIZE * 4;
            assert!(data.len() > n && data.len() < n * 2);
            let glass: Vec<f32> = data[..n]
                .chunks(4)
                .map(|p| p[0] as f32 / 255.0)
                .filter(|g| *g > 0.0)
                .collect();
            let kind = Kind::of(surface);
            let glow = WindowGlow::new(kind, Handle::default(), mean);
            let target = if kind == Kind::Shops {
                SHOP_CD_M2
            } else {
                HOME_CD_M2
            };
            let mean_colour = luminance(glow.mean.truncate().to_array());
            let average = glass.iter().sum::<f32>() / glass.len() as f32 * mean_colour;
            assert!(
                (average / target - 1.0).abs() < 0.01,
                "{surface:?}: {average}"
            );
        }
        assert!(glass_image(Surface::Roof).is_none());
        assert_eq!(Kind::of(Surface::Facade(1)), Kind::Homes);
        assert_eq!(Kind::of(Surface::GroundFloor(0)), Kind::Shops);
        assert_eq!(Kind::of(Surface::Plaster), Kind::Off);
    }

    /// The shader computes the same hash, salts and odds.
    #[test]
    fn the_shader_mirrors_this_module() {
        let shader = include_str!("windows.wesl");
        for needle in [
            "747796405u",
            "2891336453u",
            "277803737u",
            "0x68e31da4u",
            "0xb5297a4du",
            "16777216.0",
            "0.5 + unit(",
        ] {
            assert!(shader.contains(needle), "{needle}");
        }
        assert_eq!(COLOUR_SALT, 0x68e3_1da4);
        assert_eq!(BRIGHT_SALT, 0xb529_7a4d);
    }

    #[test]
    fn warm_white_is_orange_and_daylight_is_neutral() {
        let w = kelvin(3000.0);
        assert!(w[0] == 1.0 && w[1] < 0.6 && w[2] < w[1], "{w:?}");
        let d = kelvin(6500.0);
        assert!(d.iter().all(|c| *c > 0.9), "{d:?}");
    }
}
