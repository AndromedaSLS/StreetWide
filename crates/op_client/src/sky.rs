//! Sun and sky: a physically based atmosphere, the real sun position over
//! the world for a date and time, and camera exposure that follows the
//! daylight (ADR-0012).
//!
//! The clock is Swiss civil time: the worlds are in Switzerland. It never
//! reaches the simulation; light is rendering only.

use bevy::camera::Exposure;
use bevy::light::atmosphere::ScatteringMedium;
use bevy::light::light_consts::lux;
use bevy::light::{Atmosphere, CascadeShadowConfigBuilder, DirectionalLightShadowMap, SunDisk};
use bevy::prelude::*;

use crate::geo::{self, DateTime};
use crate::session::Session;

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

#[derive(Component)]
pub struct Sun;

pub struct SkyPlugin;

impl Plugin for SkyPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ClearColor(Color::BLACK))
            .insert_resource(GlobalAmbientLight::NONE)
            .insert_resource(DirectionalLightShadowMap { size: 4096 })
            .init_resource::<SunState>()
            .add_systems(Startup, spawn_sky)
            .add_systems(Update, update_sun);
    }
}

fn spawn_sky(mut commands: Commands, mut mediums: ResMut<Assets<ScatteringMedium>>) {
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

/// Camera exposure (EV100) for a sun elevation: daylight, dimmer at dusk,
/// night.
pub fn exposure_ev100(elevation_deg: f64) -> f32 {
    let t = ((elevation_deg + 10.0) / 25.0).clamp(0.0, 1.0) as f32;
    let s = t * t * (3.0 - 2.0 * t);
    6.5 + 7.3 * s
}

fn update_sun(
    clock: Res<Clock>,
    session: Res<Session>,
    mut state: ResMut<SunState>,
    mut sun: Single<(&mut Transform, &mut DirectionalLight), With<Sun>>,
    mut exposure: Query<&mut Exposure>,
    mut first: Local<bool>,
) {
    if !clock.is_changed() && *first {
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
    for mut e in &mut exposure {
        e.ev100 = exposure_ev100(elevation);
    }
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
        assert!(exposure_ev100(-20.0) < 7.0);
        assert!(exposure_ev100(0.0) < exposure_ev100(10.0));
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
