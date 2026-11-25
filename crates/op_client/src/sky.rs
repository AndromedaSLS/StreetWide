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
//! The plain look (F2) has none of the fill: only the sun and the
//! exposure, as before.
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

/// Camera exposure (EV100) for a sun elevation: daylight, dimmer at dusk,
/// down to [`NIGHT_EV100`] at night. Above 1° below the horizon it is the
/// curve the client always had.
pub fn exposure_ev100(elevation_deg: f64) -> f32 {
    let day = DUSK_EV100 + 7.3 * smoothstep((elevation_deg + 10.0) / 25.0);
    day - (DUSK_EV100 - NIGHT_EV100) * night(elevation_deg)
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
/// (and the look: the plain look has no fill).
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
    // Fade out as the sun sets, instead of lighting from below.
    let day = ((elevation + 3.0) / 5.0).clamp(0.0, 1.0) as f32;
    light.illuminance = lux::RAW_SUNLIGHT * day;
    light.shadow_maps_enabled = day > 0.0;
    let ev100 = exposure_ev100(elevation);
    for mut e in &mut exposure {
        e.ev100 = ev100;
    }

    // The night fill, in the detailed look only.
    let detailed = set.as_ref().is_none_or(|s| s.detailed_on);
    let dark = if detailed { night(elevation) } else { 0.0 };
    let m = geo::moon_position(days, minute, lon, lat);
    moon_state.0 = m;
    let dir = to_sun(m.azimuth, m.elevation);
    let up = if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    let (transform, light, visibility) = &mut *moon;
    **transform = Transform::IDENTITY.looking_to(-dir, up);
    light.illuminance = moonlight_lux(&m) * dark;
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

    #[test]
    fn exposure_drops_at_night() {
        assert!(exposure_ev100(40.0) > 13.0);
        assert!(exposure_ev100(0.0) < exposure_ev100(10.0));
        // Daylight and sunset as before ADR-0022.
        let before = |e: f64| 6.5 + 7.3 * smoothstep((e + 10.0) / 25.0);
        for e in [60.0, 31.6, 19.6, 10.0, 0.0, -1.0] {
            assert_eq!(exposure_ev100(e), before(e), "{e}");
        }
        // The floor: EV100 3 once the sun is 8° down, so at both night
        // stops (18:00 on 21 December, sun 13.2° down; 22:30 on 21 June,
        // 8.7° down).
        assert_eq!(NIGHT_EV100, 3.0);
        assert!((exposure_ev100(-13.2) - 3.0).abs() < 1e-6);
        assert!((exposure_ev100(-8.7) - 3.0).abs() < 0.1);
        assert!((exposure_ev100(-90.0) - 3.0).abs() < 1e-6);
        // Never brighter than the floor, and darker as the sun goes down.
        let mut last = f32::MAX;
        for e in (-200..=600).rev() {
            let ev = exposure_ev100(f64::from(e) / 10.0);
            assert!(ev >= 3.0 - 1e-6 && ev <= last + 1e-6, "{e}");
            last = ev;
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
