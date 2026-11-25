//! Lit windows at night, without a shader: the engine note's cheapest way
//! (its Win-A). Facade parts get a second set of texture coordinates, the
//! wall's bay and storey (its first set) plus a whole-cell offset hashed
//! from the building id and the wall; the facade materials read an emissive
//! mask through it. The mask is a grid of window cells, each lit if a hash
//! of the cell is below the share of windows lit at that hour, so which
//! window glows depends only on the building, the wall, the bay, the storey
//! and the hour, never on data about real homes (interiors are fictional),
//! and is the same on every run. As the share rises, windows only switch on.
//!
//! - **Cost:** one more texture fetch per facade pixel and one vertex
//!   attribute on facade parts; nothing per frame; a material write when
//!   the hour's share changes. StandardMaterial stays as it is, so F2 still
//!   swaps handles and turns the windows off with the rest of the look.
//! - **Brightness:** upper windows average about 6 cd/m² (CIE 2025, games
//!   note 5.3), shop windows 25 cd/m² (half CIE's average, see
//!   [`SHOP_CD_M2`]), following the exposure.
//! - **Shares:** a curve per hour set by hand (homes and offices above,
//!   shops and restaurants on the ground floor, Sundays apart): no Swiss
//!   data was found, and no study's numbers are copied.

use std::collections::BTreeMap;
use std::time::Instant;

use bevy::mesh::UvChannel;
use bevy::prelude::*;

use super::textures::{self, Rgba, box_mips};
use super::{LIGHTS_ON_BELOW_DEG, ProtoStats, kelvin, luminance, mix, seed, unit};
use crate::materials::{MaterialSet, Part, Surface};
use crate::sky::{Clock, SunState};
use crate::texgen::FACADES;

/// Window cells in a mask: bays across, storeys down.
const BAYS: u32 = 16;
const STOREYS: u32 = 8;
/// Texels per cell side.
const CELL: usize = 32;
/// Shares are rounded to this step, so a few masks serve the whole day.
const SHARE_STEPS: f32 = 20.0;

/// Average luminance of a lit window (cd/m²): upper storeys, shop fronts
/// (ground-floor style 0) and other ground floors. CIE 2025 measured 50
/// cd/m² on average for shop windows and adverts; at the night exposures
/// ADR-0018 proposes (EV100 3 to 5) that is a white panel, so shops here
/// get half of it (a look choice, for the record to settle).
const UPPER_CD_M2: f32 = 6.0;
const SHOP_CD_M2: f32 = 25.0;
const GROUND_CD_M2: f32 = 10.0;

/// Shares of windows lit by hour (Swiss time), set by hand: homes and
/// offices on upper floors; shops (to about 19:00 on weekdays, 18:00 on
/// Saturdays, shut on Sundays) and restaurants on ground floors.
const UPPER: [f32; 24] = [
    0.10, 0.06, 0.04, 0.03, 0.03, 0.05, 0.12, 0.22, 0.20, 0.15, 0.12, 0.12, 0.12, 0.12, 0.12, 0.14,
    0.18, 0.26, 0.34, 0.40, 0.44, 0.44, 0.38, 0.24,
];
const GROUND_WEEKDAY: [f32; 24] = [
    0.06, 0.05, 0.05, 0.05, 0.05, 0.05, 0.08, 0.20, 0.45, 0.70, 0.75, 0.75, 0.75, 0.75, 0.75, 0.75,
    0.75, 0.75, 0.72, 0.55, 0.38, 0.30, 0.25, 0.15,
];
const GROUND_SATURDAY: [f32; 24] = [
    0.10, 0.08, 0.06, 0.05, 0.05, 0.05, 0.08, 0.20, 0.45, 0.70, 0.75, 0.75, 0.75, 0.75, 0.75, 0.75,
    0.75, 0.70, 0.55, 0.40, 0.35, 0.32, 0.28, 0.20,
];
const GROUND_SUNDAY: [f32; 24] = [
    0.12, 0.08, 0.06, 0.05, 0.05, 0.05, 0.05, 0.06, 0.08, 0.12, 0.15, 0.18, 0.20, 0.20, 0.18, 0.16,
    0.16, 0.18, 0.22, 0.25, 0.25, 0.24, 0.20, 0.12,
];

/// The second texture coordinates of a facade or ground-floor part (one
/// window per unit of the first, as `materials::walls` lays them out), or
/// `None` for other surfaces. `origin` is the part's tile origin (region
/// frame, m).
pub fn uv1(
    part: &Part,
    surface: Surface,
    origin: [f64; 3],
    feature_id: &str,
) -> Option<Vec<[f32; 2]>> {
    if !matches!(surface, Surface::Facade(_) | Surface::GroundFloor(_)) {
        return None;
    }
    let building = seed(feature_id);
    let mut out = Vec::with_capacity(part.uvs.len());
    // `walls` emits every triangle with its own three vertices.
    for (tri, uvs) in part.positions.chunks(3).zip(part.uvs.chunks(3)) {
        let w = |p: [f32; 3]| {
            Vec3::new(p[0], p[1], p[2]).as_dvec3() + bevy::math::DVec3::from_array(origin)
        };
        let key = if tri.len() == 3 {
            let (a, b, c) = (w(tri[0]), w(tri[1]), w(tri[2]));
            let n = (b - a).cross(c - a).normalize_or_zero();
            if n.y.abs() <= 0.6 && n.length_squared() > 0.5 {
                // The wall's plane, keyed as `walls` keys it: facing in 2°
                // steps, distance in 0.25 m steps, one orientation per plane.
                let mut h = bevy::math::DVec2::new(n.x, n.z).normalize();
                if h.x < -1e-9 || (h.x.abs() <= 1e-9 && h.y < 0.0) {
                    h = -h;
                }
                let az = (h.y.atan2(h.x).to_degrees() / 2.0).round() as i64;
                let d = ((h.x * a.x + h.y * a.z) / 0.25).round() as i64;
                ((az as u64) << 32) ^ ((d as u64) & 0xffff_ffff)
            } else {
                0
            }
        } else {
            0
        };
        let hsh = mix(building, key);
        let (ou, ov) = (
            (hsh % u64::from(BAYS)) as f32,
            ((hsh >> 32) % u64::from(STOREYS)) as f32,
        );
        for uv in uvs {
            out.push([(uv[0] + ou) / BAYS as f32, (uv[1] + ov) / STOREYS as f32]);
        }
    }
    Some(out)
}

/// Shares of upper and ground-floor windows lit at the clock's time.
pub fn shares(clock: &Clock) -> (f32, f32) {
    let d = clock.local;
    // 1970-01-01 was a Thursday; 0 = Sunday.
    let weekday = (crate::geo::days_from_civil(d.year, d.month, d.day) + 4).rem_euclid(7);
    let ground = match weekday {
        0 => &GROUND_SUNDAY,
        6 => &GROUND_SATURDAY,
        _ => &GROUND_WEEKDAY,
    };
    let h = d.minute / 60;
    let t = (d.minute % 60) as f32 / 60.0;
    let at = |c: &[f32; 24]| c[h as usize] + (c[(h as usize + 1) % 24] - c[h as usize]) * t;
    (at(&UPPER), at(ground))
}

fn level(share: f32) -> u8 {
    (share * SHARE_STEPS).round().clamp(0.0, SHARE_STEPS) as u8
}

/// A window cell's glass, in metres from the bay's left edge and from the
/// storey's floor: (x0, x1, y0, y1).
fn glass(ground: bool, style: usize) -> [f32; 4] {
    let f = FACADES[style];
    let (w, frame) = (f.bay_w, 0.07);
    if !ground {
        let x0 = (w - f.win_w) / 2.0;
        return [
            x0 + frame,
            x0 + f.win_w - frame,
            f.win_bottom + frame,
            f.win_bottom + f.win_h - frame,
        ];
    }
    match style {
        // Shop windows (see `texgen::ground_floor`).
        0 => [(w - 1.9) / 2.0 + 0.08, (w + 1.9) / 2.0 - 0.08, 0.53, 2.52],
        // The window beside the door.
        1 => [1.72, 2.48, 0.97, 2.23],
        _ => [
            (w - f.win_w) / 2.0 + frame,
            (w + f.win_w) / 2.0 - frame,
            0.97,
            2.33,
        ],
    }
}

/// The mask for a style at a share level, and the mean luminance of its lit
/// glass texels (for 1 cd/m² of emissive).
fn mask(ground: bool, style: usize, level: u8) -> (Rgba, f32) {
    let f = FACADES[style];
    let [x0, x1, y0, y1] = glass(ground, style);
    let share = f32::from(level) / SHARE_STEPS;
    let salt = if ground { 0x6f66 } else { 0x7570 } + style as u64;
    let (w, h) = (CELL * BAYS as usize, CELL * STOREYS as usize);
    let mut img = Rgba::new(w, h);
    let (mut sum, mut n) = (0.0f32, 0.0f32);
    for j in 0..STOREYS {
        for i in 0..BAYS {
            let cell = mix(salt, u64::from(j * BAYS + i));
            if unit(cell) >= share {
                continue;
            }
            // Warm white mostly, some neutral, a few cool; a dimmer glow
            // behind curtains. No detail inside.
            let pick = unit(mix(cell, 1));
            let k = if pick < 0.7 {
                2600.0 + 500.0 * unit(mix(cell, 2))
            } else if pick < 0.92 {
                3500.0 + 600.0 * unit(mix(cell, 2))
            } else {
                5000.0
            };
            let colour = kelvin(k);
            let curtain = unit(mix(cell, 3)) < 0.35;
            let bright = if curtain { 0.45 } else { 0.7 } + 0.3 * unit(mix(cell, 4));
            // The window's bars stay dark (`texgen::facade` draws a mullion
            // and a transom; shop windows have a bar at 2.05 m).
            let bar = |x: f32, y: f32| {
                if ground {
                    style == 0 && (y - 2.05).abs() < 0.05
                } else {
                    (x - f.bay_w / 2.0).abs() < 0.05
                        || (y - (f.win_bottom + 0.68 * f.win_h)).abs() < 0.05
                }
            };
            for py in 0..CELL {
                let y = (1.0 - (py as f32 + 0.5) / CELL as f32) * f.storey_h;
                if y < y0 || y > y1 {
                    continue;
                }
                // Ceiling lights: a little brighter towards the top.
                let fall = if curtain {
                    1.0
                } else {
                    0.8 + 0.2 * (y - y0) / (y1 - y0)
                };
                for px in 0..CELL {
                    let x = (px as f32 + 0.5) / CELL as f32 * f.bay_w;
                    if x < x0 || x > x1 || bar(x, y) {
                        continue;
                    }
                    let c = colour.map(|v| v * bright * fall);
                    let at = (j as usize * CELL + py) * w + i as usize * CELL + px;
                    img.px[at] = [c[0], c[1], c[2], 1.0];
                    sum += luminance(c);
                    n += 1.0;
                }
            }
        }
    }
    let mean = if n > 0.0 { sum / n } else { 0.5 };
    (img, mean)
}

#[derive(Resource, Default)]
struct Masks {
    images: BTreeMap<(bool, usize, u8), (Handle<Image>, f32)>,
}

impl Masks {
    fn get(
        &mut self,
        images: &mut Assets<Image>,
        stats: &mut ProtoStats,
        ground: bool,
        style: usize,
        level: u8,
    ) -> (Handle<Image>, f32) {
        self.images
            .entry((ground, style, level))
            .or_insert_with(|| {
                let started = Instant::now();
                let (img, mean) = mask(ground, style, level);
                let (w, h) = (img.w, img.h);
                let handle = images.add(textures::image(w, h, box_mips(img, true), true));
                stats.window_masks_ms += started.elapsed().as_secs_f64() * 1000.0;
                (handle, mean)
            })
            .clone()
    }
}

pub fn build(app: &mut App) {
    app.init_resource::<Masks>()
        .add_systems(Update, update_windows);
}

/// Lights the windows at night with the hour's shares: a material write
/// per facade style and tint when the share or the night changes.
#[allow(clippy::too_many_arguments)]
fn update_windows(
    clock: Res<Clock>,
    sun: Res<SunState>,
    set: Res<MaterialSet>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut masks: ResMut<Masks>,
    mut stats: ResMut<ProtoStats>,
    mut last: Local<Option<(bool, u8, u8)>>,
) {
    let night = sun.elevation < LIGHTS_ON_BELOW_DEG;
    let (up, ground) = shares(&clock);
    let state = (night, level(up), level(ground));
    if *last == Some(state) {
        return;
    }
    *last = Some(state);
    stats.windows_lit = night;
    stats.windows_upper_share = f32::from(state.1) / SHARE_STEPS;
    stats.windows_ground_share = f32::from(state.2) / SHARE_STEPS;
    for (surface, handle) in set.window_materials() {
        let (is_ground, style) = match surface {
            Surface::Facade(s) => (false, s),
            Surface::GroundFloor(s) => (true, s),
            _ => continue,
        };
        let lit = night.then(|| {
            let lvl = if is_ground { state.2 } else { state.1 };
            masks.get(&mut images, &mut stats, is_ground, style, lvl)
        });
        let Some(mut m) = materials.get_mut(&handle) else {
            continue;
        };
        m.emissive_channel = UvChannel::Uv1;
        m.emissive_exposure_weight = 1.0;
        match lit {
            Some((mask, mean)) => {
                let target = match (is_ground, style) {
                    (false, _) => UPPER_CD_M2,
                    (true, 0) => SHOP_CD_M2,
                    _ => GROUND_CD_M2,
                };
                let k = target / mean.max(1e-3);
                m.emissive = LinearRgba::rgb(k, k, k);
                m.emissive_texture = Some(mask);
            }
            None => {
                m.emissive = LinearRgba::BLACK;
                m.emissive_texture = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::DateTime;

    fn clock(year: i32, month: u32, day: u32, minute: u32) -> Clock {
        Clock {
            local: DateTime {
                year,
                month,
                day,
                minute,
            },
        }
    }

    /// 21 December 2026 is a Monday: shops are open at 18:00; on Sunday 21
    /// June they are not.
    #[test]
    fn shares_follow_the_hour_and_the_day() {
        let (up, ground) = shares(&clock(2026, 12, 21, 18 * 60));
        assert!((0.3..0.4).contains(&up) && ground > 0.7, "{up} {ground}");
        let (_, sunday) = shares(&clock(2026, 6, 21, 18 * 60));
        assert!(sunday < 0.3, "{sunday}");
        let (late, _) = shares(&clock(2026, 6, 21, 3 * 60));
        assert!(late < 0.05);
    }

    /// A lit window stays lit as the share rises; the masks are stable.
    #[test]
    fn windows_only_switch_on_as_the_share_rises() {
        let (a, _) = mask(false, 0, 6);
        let (b, _) = mask(false, 0, 12);
        let lit = |m: &Rgba| m.px.iter().map(|p| p[3] > 0.5).collect::<Vec<_>>();
        let (la, lb) = (lit(&a), lit(&b));
        assert!(la.iter().zip(&lb).all(|(x, y)| !*x || *y));
        assert!(lb.iter().filter(|v| **v).count() > la.iter().filter(|v| **v).count());
        assert_eq!(mask(false, 0, 6).0.px, a.px);
    }

    /// Windows of one wall share an offset; another building gets another.
    #[test]
    fn offsets_come_from_the_building_and_the_wall() {
        let part = Part {
            positions: vec![[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [4.0, 6.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            uvs: vec![[0.0, 0.0], [2.0, 0.0], [2.0, -2.0]],
            indices: vec![0, 1, 2],
        };
        let a = uv1(&part, Surface::Facade(0), [10.0, 400.0, 20.0], "b:1").unwrap();
        let b = uv1(&part, Surface::Facade(0), [10.0, 400.0, 20.0], "b:2").unwrap();
        let again = uv1(&part, Surface::Facade(0), [10.0, 400.0, 20.0], "b:1").unwrap();
        assert_eq!(a, again);
        assert_ne!(a, b);
        // One window per unit of the first coordinates, one cell per unit
        // of the mask's grid.
        assert!(((a[1][0] - a[0][0]) * BAYS as f32 - 2.0).abs() < 1e-5);
        assert!(uv1(&part, Surface::Roof, [0.0; 3], "b:1").is_none());
    }
}
