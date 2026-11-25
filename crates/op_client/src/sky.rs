//! Sun, moon and sky: a physically based atmosphere, the real sun and moon
//! positions over the world for a date and time, and camera exposure that
//! follows the daylight down to a night floor (ADR-0012, ADR-0022).
//!
//! **The night** (ADR-0022, option N2): an exposure floor of EV100
//! [`NIGHT_EV100`], and a night fill, because at that exposure the sky and
//! everything between the lights would otherwise be black:
//! - **moonlight:** a dim, bluish directional light from the moon's real
//!   position, following its phase and height, without shadows (as Far Cry
//!   5 treats it). It is brighter than the real moon: a look choice;
//! - **the city's glow:** a dim, warm glow on the sky, brightest at the
//!   horizon, and the light it and the city give: a warm ambient fill. It
//!   stands in for the light of street lamps and windows on the street
//!   until lamps are drawn (RFC 0004), and keeps a new-moon night from
//!   going black.
//!
//! All of it fades in after sunset, so daytime is unchanged. The values
//! are starting values, set by eye against two photographs of the old town
//! at night; the maintainer sets the final ones (ADR-0022, open question 2).
//!
//! **Dusk:** between sunset and the night the sky keeps lighting the
//! street, as real twilight does. Below the horizon the sun only lights
//! the atmosphere (the atmosphere hides its direct light), so the sun keeps
//! the strength that makes the atmosphere's light on the ground follow
//! measured twilight ([`TWILIGHT_LX`]) instead of fading out at 3° down. The
//! exposure then follows the light in the scene (the atmosphere's, the
//! moon's and the fill's) between the night floor and the daylight curve,
//! so the picture darkens smoothly instead of going black.
//!
//! The plain look (F2) has none of this: only the sun, faded out at 3°
//! down, and the exposure curve, as before.
//!
//! The clock is Swiss civil time: the worlds are in Switzerland. It never
//! reaches the simulation; light is rendering only.

use bevy::asset::RenderAssetUsages;
use bevy::camera::Exposure;
use bevy::light::atmosphere::ScatteringMedium;
use bevy::light::light_consts::lux;
use bevy::light::{
    Atmosphere, CascadeShadowConfigBuilder, DirectionalLightShadowMap, NotShadowCaster,
    NotShadowReceiver, SunDisk,
};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use crate::geo::{self, DateTime, MoonPosition};
use crate::materials::MaterialSet;
use crate::scene::{PlayerCamera, SceneSystems};
use crate::session::Session;
use crate::windows::kelvin;

/// The exposure floor at night (EV100): ADR-0018's night-exposure option
/// A and ADR-0022's starting value, between the two 2025 photographs of
/// the old town from the Lindenhof (EV100 3.0 and 4.0). Mid-grey is then
/// about 1.7 cd/m².
pub const NIGHT_EV100: f32 = 3.0;
/// The floor the client used before ADR-0022 (EV100), which the daylight
/// curve still runs down to.
const DUSK_EV100: f32 = 6.5;

/// The highest sun elevation (degrees) the dusk tables cover: from here up
/// the sun has its full strength and the exposure is the daylight curve.
const DUSK_TOP_DEG: f64 = 2.0;
/// The lowest (degrees): below it the sun gives nothing, as before.
const DUSK_END_DEG: f64 = -10.0;
/// Sun and sky light on level ground under a clear sky (lx) for the sun's
/// elevation in whole degrees from [`DUSK_TOP_DEG`] down to
/// [`DUSK_END_DEG`]: the U.S. Naval Observatory's model (Janiczek and
/// DeYoung, "Computer Programs for Sun and Moon Illuminance With Contingent
/// Tables and Diagrams", USNO Circular 171, 1987: its subroutines REFR and
/// ATMOS, 133,775 lx times ATMOS's factor), to three figures. Hundreds of
/// lux at sunset, tens with the sun 3° down, a few at 6° down (the end of
/// civil twilight).
const TWILIGHT_LX: [f32; 13] = [
    1950.0, 1430.0, 984.0, 387.0, 169.0, 66.4, 24.4, 8.65, 2.99, 1.02, 0.345, 0.116, 0.0389,
];
/// What Bevy's atmosphere gives level ground (lx) when the sun keeps its
/// full strength ([`lux::RAW_SUNLIGHT`]) at the same elevations: measured
/// with the release client (Bevy 0.20.1, the default atmosphere settings)
/// at Hirschenplatz on 21 December, from screenshots without tonemapping
/// against a known ambient light, between measured elevations about 0.7°
/// apart; Lindenhof agreed within 14%. It falls far more slowly than real
/// twilight: three times the USNO figure with the sun on the horizon, a
/// hundred times at 10° down. Measure again if Bevy or the atmosphere's
/// settings change.
const ATMOSPHERE_LX: [f32; 13] = [
    5070.0, 3970.0, 2910.0, 1970.0, 1200.0, 743.0, 432.0, 215.0, 97.8, 48.1, 23.1, 9.12, 4.21,
];
/// The light (lx) at which the exposure reaches the night floor: the
/// fill's 5.7 lx and a full moon's 5.0 lx before the air dims it, more than
/// the brightest night the client draws, so every night stays at the floor.
const FLOOR_LX: f32 = std::f32::consts::PI * FILL_CD_M2 + FULL_MOON_LX * MOON_GAIN;
/// How far the exposure follows the light at dusk (EV per doubling): 1
/// would keep every surface as bright on screen as the light falls; a
/// little less lets dusk darken. A look choice.
const DUSK_FOLLOW: f32 = 0.9;

/// A full moon overhead, outside the atmosphere, lights the ground with
/// about 0.31 lx (Krisciunas and Schaefer, "A model of the brightness of
/// moonlight", PASP 103, 1991).
const FULL_MOON_LX: f32 = 0.31;
/// How much brighter the client's moon is than the real one: at EV100 3
/// real moonlight barely shows. A look choice, as Far Cry 5 raised its
/// moon's brightness.
const MOON_GAIN: f32 = 16.0;
/// Extinction of moonlight per airmass (magnitudes), a typical value for
/// visible light at low altitude.
const MOON_EXTINCTION_MAG: f64 = 0.2;
/// Moonlight's colour (linear RGB): bluish, the usual way to show night
/// vision, rather than the moon's real, slightly warm colour.
const MOON_COLOUR: [f32; 3] = [0.62, 0.74, 1.0];

/// The city's glow on the sky at the horizon and overhead (cd/m²): an
/// overcast sky lit orange-brown by the city, as in the photographs. Far
/// above a real city sky's luminance on a clear night, so a look choice.
const GLOW_HORIZON_CD_M2: f32 = 1.3;
const GLOW_ZENITH_CD_M2: f32 = 0.5;
/// How fast the glow fades from the horizon upwards (degrees).
const GLOW_FALLOFF_DEG: f32 = 14.0;
/// The glow's colour temperature (K), desaturated towards grey (by these
/// shares at the horizon and overhead): the orange-brown of lamplight
/// scattered in haze and cloud.
const GLOW_KELVIN: f32 = 2200.0;
const GLOW_GREY: [f32; 2] = [0.15, 0.4];
/// The ambient fill (cd/m²): the light of the city's glow, lamps and
/// windows on the street, as a uniform environment. A surface of albedo a
/// gets about a times this; 1.8 cd/m² is about 5.7 lx on the ground, the
/// low end of an old-town lane's 5 to 11 lx average (Lucerne's guideline),
/// evenly instead of in pools. Lower it once lamps are drawn.
const FILL_CD_M2: f32 = 1.8;
/// The fill's colour temperature (K), half desaturated: warm, like the
/// lamps.
const FILL_KELVIN: f32 = 3000.0;
const FILL_GREY: f32 = 0.5;
/// The glow dome's radius (m): beyond every drawn mesh (they end 700 m
/// away), inside the far plane (8 km).
const GLOW_DOME_M: f32 = 4000.0;

/// The time of day in the world.
#[derive(Resource, Clone, Copy, Debug)]
pub struct Clock {
    pub local: DateTime,
}

impl Clock {
    /// Today (UTC date) at `minute` past midnight.
    pub fn today_at(minute: u32) -> Self {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let (year, month, day) = geo::civil_from_days((secs / 86_400) as i64);
        Clock {
            local: DateTime {
                year,
                month,
                day,
                minute,
            },
        }
    }

    /// Moves the clock by `minutes`, wrapping around midnight on the same
    /// day.
    pub fn shift(&mut self, minutes: i32) {
        self.local.minute = (self.local.minute as i32 + minutes).rem_euclid(1440) as u32;
    }

    pub fn hhmm(&self) -> String {
        format!(
            "{:02}:{:02}",
            self.local.minute / 60,
            self.local.minute % 60
        )
    }
}

/// Where the sun is (degrees): azimuth clockwise from north, elevation.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct SunState {
    pub azimuth: f64,
    pub elevation: f64,
}

/// Where the moon is, and its phase.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct MoonState(pub MoonPosition);

#[derive(Component)]
pub struct Sun;

#[derive(Component)]
pub struct Moon;

/// The city's glow on the sky: a dome around the camera, added to the sky.
#[derive(Component)]
struct SkyGlow;

/// The sky systems: sun, moon, exposure and the night fill have been set
/// for the clock when they have run.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct SkySystems;

pub struct SkyPlugin;

impl Plugin for SkyPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ClearColor(Color::BLACK))
            .insert_resource(GlobalAmbientLight::NONE)
            .insert_resource(DirectionalLightShadowMap { size: 4096 })
            .init_resource::<SunState>()
            .init_resource::<MoonState>()
            .add_systems(Startup, spawn_sky)
            .add_systems(Update, update_sky.in_set(SkySystems))
            .add_systems(Update, follow_camera.after(SceneSystems));
    }
}

fn spawn_sky(
    mut commands: Commands,
    mut mediums: ResMut<Assets<ScatteringMedium>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // The planet's centre goes 6360 km below the frame origin, so the
    // ground is at height 0, close to the ellipsoid heights the world uses.
    commands.spawn(Atmosphere::earth(
        mediums.add(ScatteringMedium::earth(256, 256)),
    ));
    commands.spawn((
        Sun,
        DirectionalLight {
            illuminance: lux::RAW_SUNLIGHT,
            shadow_maps_enabled: true,
            ..default()
        },
        SunDisk::EARTH,
        Transform::default(),
        CascadeShadowConfigBuilder {
            num_cascades: 4,
            first_cascade_far_bound: 15.0,
            maximum_distance: 450.0,
            ..default()
        }
        .build(),
    ));
    // No disc: the light only, hidden while it gives none. Shadows would
    // bring back the sun's cascades at night (ADR-0022).
    commands.spawn((
        Moon,
        Visibility::Hidden,
        DirectionalLight {
            illuminance: 0.0,
            color: Color::linear_rgb(MOON_COLOUR[0], MOON_COLOUR[1], MOON_COLOUR[2]),
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::default(),
    ));
    commands.spawn((
        SkyGlow,
        Mesh3d(meshes.add(glow_dome())),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::BLACK,
            unlit: true,
            alpha_mode: AlphaMode::Add,
            fog_enabled: false,
            double_sided: true,
            cull_mode: None,
            ..default()
        })),
        Transform::default(),
        Visibility::Hidden,
        NotShadowCaster,
        NotShadowReceiver,
    ));
}

/// The direction towards the sun in the frame (X east, Y up, Z south).
pub fn to_sun(azimuth_deg: f64, elevation_deg: f64) -> Vec3 {
    let (az, el) = (azimuth_deg.to_radians(), elevation_deg.to_radians());
    Vec3::new(
        (az.sin() * el.cos()) as f32,
        el.sin() as f32,
        -(az.cos() * el.cos()) as f32,
    )
}

fn smoothstep(t: f64) -> f32 {
    let t = t.clamp(0.0, 1.0) as f32;
    t * t * (3.0 - 2.0 * t)
}

/// How far into the night it is (0 to 1) for a sun elevation (degrees):
/// 0 until the sun is 1° down, 1 from 8° down, a little after civil dusk.
/// The 2025 photographs were taken with the sun 7.5° down.
pub fn night(elevation_deg: f64) -> f32 {
    smoothstep((-1.0 - elevation_deg) / 7.0)
}

/// The daylight exposure curve (EV100) the client always had: 13.8 with
/// the sun high, 6.5 with it 10° down.
fn daylight_ev100(elevation_deg: f64) -> f32 {
    DUSK_EV100 + 7.3 * smoothstep((elevation_deg + 10.0) / 25.0)
}

/// The plain look's exposure (EV100): the daylight curve, pulled down to
/// [`NIGHT_EV100`] as the night comes, as before the dusk fix.
pub fn plain_ev100(elevation_deg: f64) -> f32 {
    daylight_ev100(elevation_deg) - (DUSK_EV100 - NIGHT_EV100) * night(elevation_deg)
}

/// A table by whole degrees from [`DUSK_TOP_DEG`] down to
/// [`DUSK_END_DEG`], read at an elevation between its ends, in logarithms.
fn by_elevation(table: &[f32; 13], elevation_deg: f64) -> f32 {
    let x = (DUSK_TOP_DEG - elevation_deg).clamp(0.0, 12.0);
    let i = (x as usize).min(11);
    let t = (x - i as f64) as f32;
    (table[i].ln() * (1.0 - t) + table[i + 1].ln() * t).exp()
}

/// Real twilight's light on level ground (lx), clear sky: [`TWILIGHT_LX`]
/// between its ends.
pub fn twilight_lux(elevation_deg: f64) -> f32 {
    by_elevation(&TWILIGHT_LX, elevation_deg)
}

/// The sun's fade (0 to 1) as it sets, the one the client always had:
/// full from 2° up, nothing from 3° down.
fn sun_fade(elevation_deg: f64) -> f32 {
    ((elevation_deg + 3.0) / 5.0).clamp(0.0, 1.0) as f32
}

/// The sun light's strength, as a share of [`lux::RAW_SUNLIGHT`]. In the
/// plain look, the fade. In the detailed look, below the horizon, at least
/// the strength that makes the atmosphere's light on the ground the real
/// twilight's ([`TWILIGHT_LX`] over [`ATMOSPHERE_LX`]), down to 10° below.
/// The atmosphere hides the sun's direct light once it has set, so the
/// strength left there only lights the sky and the light the sky gives.
pub fn sun_strength(elevation_deg: f64, detailed: bool) -> f32 {
    let fade = sun_fade(elevation_deg);
    if !detailed || elevation_deg >= DUSK_TOP_DEG {
        return fade;
    }
    if elevation_deg < DUSK_END_DEG {
        return 0.0;
    }
    fade.max(twilight_lux(elevation_deg) / by_elevation(&ATMOSPHERE_LX, elevation_deg))
}

/// The light in the detailed look's scene (lx) below [`DUSK_TOP_DEG`], as
/// the exposure meters it: the atmosphere's on level ground, the moon's
/// (`moon_lx`, from [`moonlight_lux`]) as it fades in, and the fill's.
pub fn scene_lux(elevation_deg: f64, moon_lx: f32) -> f32 {
    let sky = if elevation_deg < DUSK_END_DEG {
        0.0
    } else {
        sun_strength(elevation_deg, true) * by_elevation(&ATMOSPHERE_LX, elevation_deg)
    };
    let dark = night(elevation_deg);
    sky + (moon_lx + std::f32::consts::PI * FILL_CD_M2) * dark
}

/// The detailed look's exposure (EV100) for a sun elevation and the
/// moon's light (`moon_lx`, from [`moonlight_lux`]): the daylight curve
/// from 2° up; below, it follows the light in the scene ([`scene_lux`],
/// by [`DUSK_FOLLOW`]), never above the daylight curve and never below
/// [`NIGHT_EV100`], which it reaches at [`FLOOR_LX`].
pub fn exposure_ev100(elevation_deg: f64, moon_lx: f32) -> f32 {
    let day = daylight_ev100(elevation_deg);
    if elevation_deg >= DUSK_TOP_DEG {
        return day;
    }
    let light = scene_lux(elevation_deg, moon_lx);
    (NIGHT_EV100 + DUSK_FOLLOW * (light / FLOOR_LX).log2()).clamp(NIGHT_EV100, day)
}

/// What the exposure multiplies scene luminance (cd/m²) by, as Bevy does.
pub fn exposure_scale(ev100: f32) -> f32 {
    (-ev100).exp2() / 1.2
}

/// Moonlight on a surface facing the moon (lx), as the client draws it:
/// the real moon's, from its phase angle (Krisciunas and Schaefer 1991's
/// magnitude law) and the air it shines through (Kasten and Young's
/// airmass), times [`MOON_GAIN`]. Nothing below the horizon, and nothing
/// at new moon: the city's glow keeps those nights from going black.
pub fn moonlight_lux(moon: &MoonPosition) -> f32 {
    let el = moon.elevation;
    if el <= -0.5 {
        return 0.0;
    }
    let a = moon.phase_angle.abs();
    let phase = 10f64.powf(-0.4 * (0.026 * a + 4e-9 * a.powi(4)));
    let h = el.max(0.0);
    let airmass = 1.0 / (h.to_radians().sin() + 0.505_72 * (h + 6.079_95).powf(-1.6364));
    let air = 10f64.powf(-0.4 * MOON_EXTINCTION_MAG * airmass);
    let rise = smoothstep((el + 0.5) / 1.5);
    FULL_MOON_LX * MOON_GAIN * (phase * air) as f32 * rise
}

/// A colour temperature's linear RGB, mixed towards grey by `grey` and
/// scaled to a luminance of 1.
fn tint(kelvin_k: f32, grey: f32) -> Vec3 {
    let c = Vec3::from_array(kelvin(kelvin_k)).lerp(Vec3::ONE, grey);
    c / (0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z)
}

/// The glow dome: rings from 20° below the horizon to the zenith, inside
/// out, with the glow's relative brightness and colour in the vertex
/// colours (1 at the horizon).
fn glow_dome() -> Mesh {
    // Rings every 5°, one of them on the horizon.
    let (rings, segments) = (22, 48);
    let horizon = tint(GLOW_KELVIN, GLOW_GREY[0]);
    let zenith = tint(GLOW_KELVIN, GLOW_GREY[1]);
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut colours = Vec::new();
    for r in 0..=rings {
        let el = -20.0 + 110.0 * r as f32 / rings as f32;
        let up = el.max(0.0);
        let k = (-up / GLOW_FALLOFF_DEG).exp();
        let rel = GLOW_ZENITH_CD_M2 / GLOW_HORIZON_CD_M2;
        let c = zenith.lerp(horizon, k) * (rel + (1.0 - rel) * k);
        for s in 0..=segments {
            let az = std::f32::consts::TAU * s as f32 / segments as f32;
            let d = Vec3::new(
                az.sin() * el.to_radians().cos(),
                el.to_radians().sin(),
                -az.cos() * el.to_radians().cos(),
            );
            positions.push((d * GLOW_DOME_M).to_array());
            normals.push((-d).to_array());
            colours.push([c.x, c.y, c.z, 1.0]);
        }
    }
    let mut indices = Vec::new();
    let row = segments + 1;
    for r in 0..rings {
        for s in 0..segments {
            let (a, b) = (r * row + s, (r + 1) * row + s);
            indices.extend([a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colours)
    .with_inserted_indices(Indices::U32(indices))
}

/// Sets the sun, the moon, the exposure and the night fill for the clock
/// (and the look: the plain look has no fill and no twilight).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_sky(
    clock: Res<Clock>,
    session: Res<Session>,
    set: Option<Res<MaterialSet>>,
    mut state: ResMut<SunState>,
    mut moon_state: ResMut<MoonState>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut sun: Single<(&mut Transform, &mut DirectionalLight), (With<Sun>, Without<Moon>)>,
    mut moon: Single<
        (&mut Transform, &mut DirectionalLight, &mut Visibility),
        (With<Moon>, Without<Sun>, Without<SkyGlow>),
    >,
    glow: Single<
        (&MeshMaterial3d<StandardMaterial>, &mut Visibility),
        (With<SkyGlow>, Without<Moon>),
    >,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut exposure: Query<&mut Exposure>,
    mut first: Local<bool>,
) {
    let look_changed = set.as_ref().is_some_and(|s| s.is_changed());
    if !clock.is_changed() && !look_changed && *first {
        return;
    }
    *first = true;
    let (lon, lat) = session.world_lonlat();
    let (days, minute) = geo::swiss_to_utc(clock.local);
    let (azimuth, elevation) = geo::sun_position(days, minute, lon, lat);
    *state = SunState { azimuth, elevation };
    let dir = to_sun(azimuth, elevation);
    let up = if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    let (transform, light) = &mut *sun;
    **transform = Transform::IDENTITY.looking_to(-dir, up);
    // The detailed look keeps the twilight in the sky; shadows end with the
    // fade, as before, since nothing on the ground sees the sun by then.
    let detailed = set.as_ref().is_none_or(|s| s.detailed_on);
    light.illuminance = lux::RAW_SUNLIGHT * sun_strength(elevation, detailed);
    light.shadow_maps_enabled = sun_fade(elevation) > 0.0;

    // The night fill, in the detailed look only.
    let dark = if detailed { night(elevation) } else { 0.0 };
    let m = geo::moon_position(days, minute, lon, lat);
    moon_state.0 = m;
    let moon_lx = moonlight_lux(&m);
    let ev100 = if detailed {
        exposure_ev100(elevation, moon_lx)
    } else {
        plain_ev100(elevation)
    };
    for mut e in &mut exposure {
        e.ev100 = ev100;
    }
    let dir = to_sun(m.azimuth, m.elevation);
    let up = if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    let (transform, light, visibility) = &mut *moon;
    **transform = Transform::IDENTITY.looking_to(-dir, up);
    light.illuminance = moon_lx * dark;
    // Without light, hidden: the renderer and the atmosphere then skip it.
    **visibility = if light.illuminance > 0.0 {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    let fill = tint(FILL_KELVIN, FILL_GREY);
    *ambient = GlobalAmbientLight {
        color: Color::linear_rgb(fill.x, fill.y, fill.z),
        brightness: FILL_CD_M2 * dark,
        affects_lightmapped_meshes: true,
    };
    let (material, mut visibility) = glow.into_inner();
    *visibility = if dark > 0.0 {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    if let Some(mut mat) = materials.get_mut(&material.0) {
        // Unlit colours skip the exposure: apply it here.
        let k = GLOW_HORIZON_CD_M2 * dark * exposure_scale(ev100);
        mat.base_color = Color::linear_rgb(k, k, k);
    }
}

/// Keeps the glow dome centred on the camera, like the sky.
fn follow_camera(
    camera: Single<&Transform, (With<PlayerCamera>, Without<SkyGlow>)>,
    mut dome: Single<&mut Transform, With<SkyGlow>>,
) {
    dome.translation = camera.translation;
}

#[cfg(test)]
mod tests {
    use std::f32::consts::PI;

    use super::*;

    #[test]
    fn sun_directions_follow_the_frame_axes() {
        // Due south at 30 degrees: towards +Z (south) and up.
        let d = to_sun(180.0, 30.0);
        assert!(
            d.x.abs() < 1e-6 && d.z > 0.8 && (d.y - 0.5).abs() < 1e-6,
            "{d}"
        );
        // Due east on the horizon: +X.
        let d = to_sun(90.0, 0.0);
        assert!((d.x - 1.0).abs() < 1e-6 && d.y.abs() < 1e-6, "{d}");
    }

    /// Sun elevations from 90° up to 90° down, every tenth of a degree.
    fn elevations() -> impl Iterator<Item = f64> {
        (-900..=900).rev().map(|e| f64::from(e) / 10.0)
    }

    /// The daylight curve as the client always had it.
    fn daylight(e: f64) -> f32 {
        6.5 + 7.3 * smoothstep((e + 10.0) / 25.0)
    }

    /// No moon, the moon at 18:00 on 21 December (about 1.6 lx), and the
    /// brightest the client draws (full, overhead).
    fn moons() -> [f32; 3] {
        let full = MoonPosition {
            azimuth: 180.0,
            elevation: 90.0,
            illuminated: 1.0,
            phase_angle: 0.0,
        };
        [0.0, 1.6, moonlight_lux(&full)]
    }

    #[test]
    fn exposure_drops_at_night() {
        assert!(exposure_ev100(40.0, 0.0) > 13.0);
        assert!(exposure_ev100(0.0, 0.0) < exposure_ev100(10.0, 0.0));
        // The plain look: daylight and sunset as before ADR-0022, then down
        // to the floor by 8° down, as before the dusk fix.
        for e in [60.0, 31.6, 19.6, 10.0, 0.0, -1.0] {
            assert_eq!(plain_ev100(e), daylight(e), "{e}");
        }
        for e in elevations() {
            assert_eq!(plain_ev100(e), daylight(e) - 3.5 * night(e), "{e}");
        }
        for m in moons() {
            // Unchanged with the sun 2° up or higher.
            for e in elevations().filter(|&e| e >= 2.0) {
                assert_eq!(exposure_ev100(e, m), daylight(e), "{e}");
            }
            // The floor, EV100 3, from 7° down whatever the moon, so at
            // both night stops (18:00 on 21 December, sun 13.2° down;
            // 22:30 on 21 June, 8.7° down).
            assert_eq!(NIGHT_EV100, 3.0);
            for e in elevations().filter(|&e| e <= -7.0) {
                assert_eq!(exposure_ev100(e, m), NIGHT_EV100, "{e} {m}");
            }
            // Never above the daylight curve, never below the floor, and
            // darker as the sun goes down.
            let mut last = f32::MAX;
            for e in elevations() {
                let ev = exposure_ev100(e, m);
                assert!(ev >= NIGHT_EV100 && ev <= daylight(e), "{e} {m}");
                assert!(ev <= last + 1e-6, "{e} {m}");
                last = ev;
            }
        }
    }

    #[test]
    fn the_twilight_table_is_the_usno_model() {
        // USNO Circular 171's REFR and ATMOS (degrees), and its sun's
        // illuminance under a clear sky, 133,775 lx times ATMOS's factor.
        fn usno(h: f64) -> f64 {
            let dr = std::f64::consts::PI / 180.0;
            let ha = if h < -5.0 / 6.0 {
                h
            } else {
                h + 1.0 / ((h + 8.6 / (h + 4.42)) * dr).tan() / 60.0
            };
            let u = (ha * dr).sin();
            let x = 753.661_56;
            let s = (x * (ha * dr).cos() / (x + 1.0)).asin();
            let m = x * (s.cos() - u) + s.cos();
            let m = (-0.21 * m).exp() * u
                + 0.0289 * (-0.042 * m).exp() * (1.0 + (ha + 90.0) * u / 57.295_779_51);
            133_775.0 * m
        }
        for (i, &lx) in TWILIGHT_LX.iter().enumerate() {
            let e = DUSK_TOP_DEG - i as f64;
            let model = usno(e);
            assert!(
                ((f64::from(lx) - model) / model).abs() < 0.005,
                "{e}: {lx} against {model}"
            );
            assert!((twilight_lux(e) / lx - 1.0).abs() < 1e-5, "{e}");
        }
        // Hundreds of lux at sunset (the sun's upper edge on the horizon),
        // tens at 3° down, a few at 6° down, and less and less.
        assert!((300.0..800.0).contains(&twilight_lux(-0.83)));
        assert!((10.0..100.0).contains(&twilight_lux(-3.0)));
        assert!((1.0..10.0).contains(&twilight_lux(-6.0)));
        assert!(TWILIGHT_LX.windows(2).all(|w| w[1] < w[0]));
        let mut last = f32::MAX;
        for e in elevations() {
            assert!(twilight_lux(e) <= last, "{e}");
            last = twilight_lux(e);
        }
    }

    #[test]
    fn the_sky_keeps_lighting_the_street_at_dusk() {
        // The measured atmosphere falls far more slowly than real twilight.
        assert!(ATMOSPHERE_LX.windows(2).all(|w| w[1] < w[0]));
        for (a, t) in ATMOSPHERE_LX.iter().zip(TWILIGHT_LX) {
            assert!(*a > 2.5 * t, "{a} {t}");
        }
        for e in elevations() {
            // The plain look: the fade, as before.
            let fade = ((e + 3.0) / 5.0).clamp(0.0, 1.0) as f32;
            assert_eq!(sun_strength(e, false), fade, "{e}");
            // The detailed look: full strength from 2° up, nothing below
            // 10° down, and never less than the fade.
            let s = sun_strength(e, true);
            if e >= 2.0 {
                assert_eq!(s, 1.0, "{e}");
            }
            if e < -10.0 {
                assert_eq!(s, 0.0, "{e}");
            }
            assert!(s >= fade && s <= 1.0, "{e}");
            // Down to 10° the atmosphere gives the ground at least the real
            // twilight, and exactly it once the fade has gone.
            if (-10.0..2.0).contains(&e) {
                let sky = scene_lux(e, 0.0) - PI * FILL_CD_M2 * night(e);
                let real = twilight_lux(e);
                assert!(sky >= real * 0.9999, "{e}: {sky} < {real}");
                if e <= -3.0 {
                    assert!((sky / real - 1.0).abs() < 1e-3, "{e}: {sky} {real}");
                }
            }
        }
        // The brightest night stays below the light that leaves the floor.
        for m in moons() {
            assert!(scene_lux(-10.0, m) < FLOOR_LX, "{m}");
        }
    }

    #[test]
    fn exposure_and_light_agree_at_dusk() {
        // What a mid-grey surface (albedo 0.18) lit by the whole scene
        // shows on screen: its luminance times the exposure, as Bevy
        // applies it, before tonemapping.
        let grey =
            |e: f64, m: f32| 0.18 / PI * scene_lux(e, m) * exposure_scale(exposure_ev100(e, m));
        for m in moons() {
            let at_night = grey(-12.0, m);
            // 0.03 with no moon, 0.06 under the brightest: dim but
            // readable, ADR-0022's night.
            assert!((0.03..0.07).contains(&at_night), "{at_night}");
            // From sunset to the night, never darker than the night and at
            // most four times brighter, darker as the sun goes down. Before
            // the fix, the same grey went below a tenth of the night's at
            // 4° down: the exposure was still near daylight while the sky
            // had gone out.
            let mut last = f32::MAX;
            for e in elevations().filter(|&e| (-12.0..=-0.8).contains(&e)) {
                let g = grey(e, m);
                assert!(g >= at_night * 0.9999, "{e} {m}: {g} < {at_night}");
                assert!(g <= at_night * 4.0, "{e} {m}: {g} > 4 x {at_night}");
                assert!(g <= last * 1.0001, "{e} {m}: {g} > {last}");
                last = g;
            }
        }
    }

    #[test]
    fn the_night_fades_in_after_sunset() {
        assert_eq!(night(31.6), 0.0);
        assert_eq!(night(-1.0), 0.0);
        assert_eq!(night(-8.0), 1.0);
        assert_eq!(night(-13.2), 1.0);
        assert!((0.0..1.0).contains(&night(-4.0)));
    }

    #[test]
    fn moonlight_follows_phase_and_height() {
        let full = |elevation| MoonPosition {
            azimuth: 180.0,
            elevation,
            illuminated: 1.0,
            phase_angle: 0.0,
        };
        let overhead = moonlight_lux(&full(90.0));
        // 0.31 lx outside the air, 0.83 of it through one airmass.
        assert!((overhead / MOON_GAIN - 0.257).abs() < 0.01, "{overhead}");
        assert!(moonlight_lux(&full(20.0)) < overhead);
        assert_eq!(moonlight_lux(&full(-5.0)), 0.0);
        // A quarter moon gives about a tenth of a full moon's light, not
        // half (the opposition surge).
        let quarter = MoonPosition {
            phase_angle: 90.0,
            ..full(90.0)
        };
        let ratio = moonlight_lux(&quarter) / overhead;
        assert!((0.07..0.12).contains(&ratio), "{ratio}");
        let new = MoonPosition {
            phase_angle: 178.0,
            ..full(90.0)
        };
        assert!(moonlight_lux(&new) / overhead < 0.001);
    }

    #[test]
    fn the_glow_is_warm_and_brightest_at_the_horizon() {
        let mesh = glow_dome();
        let Some(bevy::mesh::VertexAttributeValues::Float32x4(colours)) =
            mesh.attribute(Mesh::ATTRIBUTE_COLOR)
        else {
            panic!("no colours");
        };
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("no positions");
        };
        let lum = |c: &[f32; 4]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let at = |y: f32| {
            positions
                .iter()
                .zip(colours)
                .filter(|(p, _)| (p[1] - y).abs() < 1.0)
                .map(|(_, c)| lum(c))
                .next()
                .unwrap()
        };
        assert!((at(0.0) - 1.0).abs() < 1e-3);
        assert!((at(GLOW_DOME_M) - GLOW_ZENITH_CD_M2 / GLOW_HORIZON_CD_M2).abs() < 0.01);
        assert!(colours.iter().all(|c| c[0] > c[2]), "warm");
        assert!((exposure_scale(3.0) * 1.2 * 8.0 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_clock_wraps_at_midnight() {
        let mut c = Clock {
            local: DateTime {
                year: 2026,
                month: 11,
                day: 24,
                minute: 23 * 60 + 45,
            },
        };
        c.shift(30);
        assert_eq!(c.hhmm(), "00:15");
        c.shift(-30);
        assert_eq!(c.hhmm(), "23:45");
    }
}
