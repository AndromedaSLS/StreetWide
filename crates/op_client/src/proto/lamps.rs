//! Street lamps (the games note's L2, with L0 beyond the pool): one simple
//! model per mount, generated in code (a post-top lantern on a pole, a
//! bracket on a wall, a pendant on a wire across the lane); an emissive head
//! on every lamp; and a pool of the `n` nearest lamps as real shadowless
//! spot lights aimed down, faded out towards the pool's edge so a lamp that
//! joins or leaves the pool does not pop.
//!
//! - **Colour:** 3000 K, the warm white Swiss guidance gives for historic
//!   areas (2700 to 3000 K, games note 6.1).
//! - **On:** when the sun is [`LIGHTS_ON_BELOW_DEG`] below the horizon.
//! - **Kind:** spot lights aimed down by default; `--proto-lamp-point`
//!   makes them point lights, which also light the trees above a lantern.
//! - **Units:** lumens for the lights (Bevy spreads them over 4π, cone or
//!   not); heads in cd/m² that follow the exposure
//!   (`emissive_exposure_weight` 1, as ADR-0018 proposes).
//! - **Stacks:** one model and one light per position (ADR-0015 option A);
//!   lamps deep inside building footprints are left out.

use std::time::Instant;

use bevy::camera::visibility::VisibilityRange;
use bevy::light::NotShadowCaster;
use bevy::prelude::*;

use super::points::{Lamp, Mount};
use super::shapes::MeshBuilder;
use super::{LIGHTS_ON_BELOW_DEG, ProtoStats, kelvin, luminance, mix, seed, unit};
use crate::Options;
use crate::scene::{PlayerCamera, RENDER_RADIUS_M, SceneSystems};
use crate::sky::SunState;

/// Light colour (K).
const LAMP_KELVIN: f32 = 3000.0;
/// A lantern's glass seen side on (m²): its luminance is the lamp's
/// intensity over this.
const HEAD_AREA_M2: f32 = 0.15;
/// Spot cone: half-angles of full light and of its edge (radians).
const INNER_ANGLE: f32 = 0.55;
const OUTER_ANGLE: f32 = 1.2;
/// Heights of the light sources above the ground (m), per model.
const POST_LIGHT_Y: f32 = 4.62;
const WALL_LIGHT_Y: f32 = 3.68;
const WIRE_LIGHT_Y: f32 = 5.86;
/// Wire anchors on the facades, above the street (m).
const WIRE_ANCHOR_Y: f32 = 6.6;
/// From the wall to the bracket lamp's head (m).
const ARM_M: f32 = 0.84;

#[derive(Resource)]
pub struct LampAssets {
    post: (Handle<Mesh>, Handle<Mesh>),
    bracket: (Handle<Mesh>, Handle<Mesh>),
    pendant: (Handle<Mesh>, Handle<Mesh>),
    wire: Handle<Mesh>,
    metal: Handle<StandardMaterial>,
    glass: Handle<StandardMaterial>,
    head_cd_m2: f32,
}

/// The placed lamps' light sources (render frame).
#[derive(Resource, Default)]
pub struct LampSet {
    pub lights: Vec<Vec3>,
}

#[derive(Resource)]
struct Pool {
    /// Light entities and the lamp each one lights.
    slots: Vec<(Entity, Option<u32>)>,
    lumens: f32,
}

#[derive(Component)]
struct PoolLight;

pub fn build(app: &mut App) {
    app.init_resource::<LampSet>()
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (switch_lamps, update_pool).chain().after(SceneSystems),
        );
}

/// A four-sided lantern from `y0` to `y1`, `hb` and `ht` half-wide at the
/// bottom and top: its frame and roof into `metal`, its panes into `glass`.
fn lantern(metal: &mut MeshBuilder, glass: &mut MeshBuilder, y0: f32, y1: f32, hb: f32, ht: f32) {
    let corner = |k: usize, y: f32, h: f32| {
        let a = std::f32::consts::FRAC_PI_4 + k as f32 * std::f32::consts::FRAC_PI_2;
        Vec3::new(
            a.cos() * h * std::f32::consts::SQRT_2,
            y,
            a.sin() * h * std::f32::consts::SQRT_2,
        )
    };
    let centre = Vec3::new(0.0, (y0 + y1) / 2.0, 0.0);
    for k in 0..4 {
        let (b0, b1) = (corner(k, y0, hb * 0.96), corner(k + 1, y0, hb * 0.96));
        let (t0, t1) = (corner(k, y1, ht * 0.96), corner(k + 1, y1, ht * 0.96));
        let n = ((b0 + b1 + t0 + t1) / 4.0 - centre).normalize_or(Vec3::X);
        glass.quad(
            [b0, b1, t1, t0],
            n,
            [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
        );
        metal.tube(
            corner(k, y0, hb),
            corner(k, y1, ht),
            0.014,
            0.014,
            4,
            1.0,
            0.0,
            1.0,
        );
    }
    metal.cuboid(
        Vec3::new(-hb, y0 - 0.05, -hb),
        Vec3::new(hb, y0, hb),
        Quat::IDENTITY,
        Vec3::ZERO,
    );
    let r = ht + 0.05;
    metal.cuboid(
        Vec3::new(-r, y1, -r),
        Vec3::new(r, y1 + 0.04, r),
        Quat::IDENTITY,
        Vec3::ZERO,
    );
    let apex = Vec3::new(0.0, y1 + 0.27, 0.0);
    let rim = |k: usize| corner(k, y1 + 0.04, r);
    for k in 0..4 {
        metal.facet([rim(k), rim(k + 1), apex], Vec3::new(0.0, y1 + 0.05, 0.0));
    }
    metal.tube(
        apex - Vec3::Y * 0.02,
        apex + Vec3::Y * 0.1,
        0.02,
        0.008,
        4,
        1.0,
        0.0,
        1.0,
    );
}

fn setup(
    mut commands: Commands,
    options: Res<Options>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut stats: ResMut<ProtoStats>,
) {
    let started = Instant::now();
    let n = options.proto.lamps.unwrap_or(0);
    // A post-top lantern on a cast-iron pole.
    let (mut metal, mut glass) = (MeshBuilder::default(), MeshBuilder::default());
    metal.tube(
        Vec3::new(0.0, -0.3, 0.0),
        Vec3::new(0.0, 0.45, 0.0),
        0.13,
        0.1,
        8,
        1.0,
        0.0,
        1.0,
    );
    metal.tube(
        Vec3::new(0.0, 0.45, 0.0),
        Vec3::new(0.0, 4.55, 0.0),
        0.065,
        0.045,
        8,
        1.0,
        0.0,
        1.0,
    );
    metal.tube(
        Vec3::new(0.0, 4.5, 0.0),
        Vec3::new(0.0, 4.66, 0.0),
        0.085,
        0.07,
        8,
        1.0,
        0.0,
        1.0,
    );
    lantern(&mut metal, &mut glass, 4.7, 5.25, 0.13, 0.19);
    let post = (
        meshes.add(metal.build(false)),
        meshes.add(glass.build(false)),
    );
    // A bracket lantern: wall plate at -X, the lantern hanging at x = 0.
    let (mut metal, mut glass) = (MeshBuilder::default(), MeshBuilder::default());
    metal.cuboid(
        Vec3::new(-ARM_M - 0.03, 3.95, -0.09),
        Vec3::new(-ARM_M + 0.01, 4.55, 0.09),
        Quat::IDENTITY,
        Vec3::ZERO,
    );
    metal.tube(
        Vec3::new(-ARM_M, 4.42, 0.0),
        Vec3::new(0.0, 4.42, 0.0),
        0.022,
        0.02,
        6,
        1.0,
        0.0,
        1.0,
    );
    metal.tube(
        Vec3::new(-ARM_M, 4.05, 0.0),
        Vec3::new(-0.38, 4.42, 0.0),
        0.016,
        0.016,
        5,
        1.0,
        0.0,
        1.0,
    );
    metal.tube(
        Vec3::new(0.0, 4.44, 0.0),
        Vec3::new(0.0, 4.25, 0.0),
        0.012,
        0.012,
        4,
        1.0,
        0.0,
        1.0,
    );
    lantern(&mut metal, &mut glass, 3.75, 4.2, 0.11, 0.16);
    let bracket = (
        meshes.add(metal.build(false)),
        meshes.add(glass.build(false)),
    );
    // A pendant under a span wire: a shade, a glass bowl below it.
    let (mut metal, mut glass) = (MeshBuilder::default(), MeshBuilder::default());
    metal.tube(
        Vec3::new(0.0, 6.3, 0.0),
        Vec3::new(0.0, 6.04, 0.0),
        0.012,
        0.012,
        4,
        1.0,
        0.0,
        1.0,
    );
    metal.tube(
        Vec3::new(0.0, 6.06, 0.0),
        Vec3::new(0.0, 5.94, 0.0),
        0.05,
        0.31,
        10,
        1.0,
        0.0,
        1.0,
    );
    metal.disc(Vec3::new(0.0, 6.06, 0.0), 0.05, 10, Vec3::Y);
    glass.tube(
        Vec3::new(0.0, 5.95, 0.0),
        Vec3::new(0.0, 5.9, 0.0),
        0.27,
        0.25,
        10,
        1.0,
        0.0,
        1.0,
    );
    glass.disc(Vec3::new(0.0, 5.9, 0.0), 0.25, 10, Vec3::NEG_Y);
    let pendant = (
        meshes.add(metal.build(false)),
        meshes.add(glass.build(false)),
    );
    // A span wire: 1 m along -Z, scaled to its length.
    let mut wire = MeshBuilder::default();
    wire.tube(Vec3::ZERO, Vec3::NEG_Z, 0.008, 0.008, 4, 1.0, 0.0, 1.0);
    let wire = meshes.add(wire.build(false));

    let lumens = options.proto.lamp_lumens;
    let colour = kelvin(LAMP_KELVIN);
    let y = luminance(colour);
    let light_colour = Color::linear_rgb(colour[0] / y, colour[1] / y, colour[2] / y);
    let head_cd_m2 = lumens / (4.0 * std::f32::consts::PI * HEAD_AREA_M2);
    let assets = LampAssets {
        post,
        bracket,
        pendant,
        wire,
        metal: materials.add(StandardMaterial {
            base_color: Color::srgb(0.11, 0.12, 0.11),
            perceptual_roughness: 0.45,
            reflectance: 0.5,
            ..default()
        }),
        glass: materials.add(StandardMaterial {
            base_color: Color::srgb(0.45, 0.43, 0.40),
            perceptual_roughness: 0.25,
            emissive_exposure_weight: 1.0,
            ..default()
        }),
        head_cd_m2,
    };
    let range = options.proto.lamp_range_m;
    let point = options.proto.lamp_point;
    let slots = (0..n)
        .map(|_| {
            let mut e = commands.spawn((
                Transform::default().looking_to(Vec3::NEG_Y, Vec3::X),
                Visibility::Hidden,
                PoolLight,
            ));
            if point {
                // A clear lantern lights all round, the trees above it too.
                e.insert(PointLight {
                    color: light_colour,
                    intensity: 0.0,
                    range,
                    radius: 0.1,
                    shadow_maps_enabled: false,
                    ..default()
                });
            } else {
                e.insert(SpotLight {
                    color: light_colour,
                    intensity: 0.0,
                    range,
                    radius: 0.1,
                    shadow_maps_enabled: false,
                    inner_angle: INNER_ANGLE,
                    outer_angle: OUTER_ANGLE,
                    ..default()
                });
            }
            (e.id(), None)
        })
        .collect();
    commands.insert_resource(Pool { slots, lumens });
    stats.lights_pool = n;
    stats.lamp_assets_ms = started.elapsed().as_secs_f64() * 1000.0;
    commands.insert_resource(assets);
}

fn range() -> VisibilityRange {
    VisibilityRange {
        start_margin: 0.0..0.0,
        end_margin: (RENDER_RADIUS_M - 30.0)..RENDER_RADIUS_M,
        use_aabb: false,
    }
}

/// Spawns one lamp at `at` (render frame, on its ground); returns the
/// number of entities.
pub fn spawn(
    commands: &mut Commands,
    a: &LampAssets,
    set: &mut LampSet,
    l: &Lamp,
    at: Vec3,
) -> usize {
    let h = seed(&l.id);
    let dir = l.facade_dir.map(|d| Vec3::new(d[0], 0.0, d[1]));
    let mut parts = |mesh: &(Handle<Mesh>, Handle<Mesh>), t: Transform| {
        commands.spawn((
            Mesh3d(mesh.0.clone()),
            MeshMaterial3d(a.metal.clone()),
            t,
            range(),
        ));
        commands.spawn((
            Mesh3d(mesh.1.clone()),
            MeshMaterial3d(a.glass.clone()),
            t,
            range(),
            NotShadowCaster,
        ));
    };
    match (l.mount, dir) {
        (Mount::Wall, Some(d)) => {
            // `d` points at the nearest facade; for a point just inside a
            // footprint it points out of the building.
            let (wall, away) = if l.inside {
                (at + d * l.inside_depth, d)
            } else {
                (at + d * l.facade_m.unwrap_or(0.5), -d)
            };
            let origin = wall + away * ARM_M;
            let t = Transform::from_translation(Vec3::new(origin.x, at.y, origin.z))
                .with_rotation(Quat::from_rotation_arc(Vec3::X, away));
            parts(&a.bracket, t);
            set.lights.push(t.translation + Vec3::Y * WALL_LIGHT_Y);
            2
        }
        (Mount::Wire, Some(d)) => {
            let t =
                Transform::from_translation(at).with_rotation(Quat::from_rotation_arc(Vec3::X, d));
            parts(&a.pendant, t);
            let top = at + Vec3::Y * 6.3;
            let mut n = 2;
            for (side, dist) in [(1.0, l.facade_m), (-1.0, l.opposite_m)] {
                let Some(dist) = dist.filter(|v| *v < 30.0) else {
                    continue;
                };
                let anchor = at + d * side * dist + Vec3::Y * WIRE_ANCHOR_Y;
                let span = anchor - top;
                commands.spawn((
                    Mesh3d(a.wire.clone()),
                    MeshMaterial3d(a.metal.clone()),
                    Transform::from_translation(top)
                        .looking_to(span.normalize_or(Vec3::X), Vec3::Y)
                        .with_scale(Vec3::new(1.0, 1.0, span.length())),
                    range(),
                    NotShadowCaster,
                ));
                n += 1;
            }
            set.lights.push(at + Vec3::Y * WIRE_LIGHT_Y);
            n
        }
        (Mount::Inside, _) => 0,
        _ => {
            // Poles, and wall or wire lamps without a facade direction.
            let yaw = l
                .yaw_deg
                .map_or(unit(mix(h, 3)) * std::f32::consts::TAU, f32::to_radians);
            let t = Transform::from_translation(at).with_rotation(Quat::from_rotation_y(-yaw));
            parts(&a.post, t);
            set.lights.push(at + Vec3::Y * POST_LIGHT_Y);
            2
        }
    }
}

/// Heads glow and the pool lights up when the sun is low enough.
fn switch_lamps(
    sun: Res<SunState>,
    assets: Option<Res<LampAssets>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut stats: ResMut<ProtoStats>,
    mut last: Local<Option<bool>>,
) {
    let Some(a) = assets else {
        return;
    };
    let lit = sun.elevation < LIGHTS_ON_BELOW_DEG;
    if *last == Some(lit) {
        return;
    }
    *last = Some(lit);
    stats.lamps_lit = lit;
    if let Some(mut m) = materials.get_mut(&a.glass) {
        let c = kelvin(LAMP_KELVIN);
        let k = a.head_cd_m2 / luminance(c);
        m.emissive = if lit {
            LinearRgba::rgb(c[0] * k, c[1] * k, c[2] * k)
        } else {
            LinearRgba::BLACK
        };
    }
}

/// Gives the pool's lights to the nearest lamps, keeping a lamp in its slot
/// while it stays among them, and fades each one by its distance so that
/// the furthest lit lamp is nearly dark when it is swapped.
#[allow(clippy::type_complexity)]
fn update_pool(
    camera: Single<&Transform, (With<PlayerCamera>, Without<PoolLight>)>,
    set: Res<LampSet>,
    pool: Option<ResMut<Pool>>,
    mut stats: ResMut<ProtoStats>,
    mut lights: Query<
        (
            Option<&mut SpotLight>,
            Option<&mut PointLight>,
            &mut Transform,
            &mut Visibility,
        ),
        With<PoolLight>,
    >,
    mut near: Local<Vec<(f32, u32)>>,
) {
    let Some(mut pool) = pool else {
        return;
    };
    let eye = camera.translation;
    let n = pool.slots.len();
    if !stats.lamps_lit || n == 0 {
        for (_, _, _, mut v) in &mut lights {
            v.set_if_neq(Visibility::Hidden);
        }
        for s in &mut pool.slots {
            s.1 = None;
        }
        stats.lights_lit = 0;
        return;
    }
    near.clear();
    near.extend(
        set.lights
            .iter()
            .enumerate()
            .map(|(i, p)| (p.distance_squared(eye), i as u32)),
    );
    let edge = if near.len() > n {
        near.select_nth_unstable_by(n, |a, b| a.0.total_cmp(&b.0));
        near[n].0.sqrt()
    } else {
        f32::INFINITY
    };
    let chosen = &near[..n.min(near.len())];
    let mut keep = vec![false; set.lights.len()];
    for c in chosen {
        keep[c.1 as usize] = true;
    }
    // Lamps already lit keep their light; the others take free slots.
    let mut placed = vec![false; set.lights.len()];
    for s in &mut pool.slots {
        if let Some(l) = s.1 {
            if keep[l as usize] {
                placed[l as usize] = true;
            } else {
                s.1 = None;
            }
        }
    }
    let mut free = chosen.iter().map(|c| c.1).filter(|l| !placed[*l as usize]);
    for s in &mut pool.slots {
        if s.1.is_none() {
            s.1 = free.next();
        }
    }
    let band = (edge * 0.25).max(6.0);
    let lumens = pool.lumens;
    let mut lit = 0;
    for (entity, lamp) in &pool.slots {
        let Ok((spot, point, mut t, mut v)) = lights.get_mut(*entity) else {
            continue;
        };
        match lamp {
            Some(l) => {
                let p = set.lights[*l as usize];
                let f = ((edge - p.distance(eye)) / band).clamp(0.0, 1.0);
                let fade = f * f * (3.0 - 2.0 * f);
                if t.translation != p {
                    t.translation = p;
                }
                if let Some(mut s) = spot {
                    s.intensity = lumens * fade;
                }
                if let Some(mut p) = point {
                    p.intensity = lumens * fade;
                }
                v.set_if_neq(Visibility::Inherited);
                lit += 1;
            }
            None => {
                v.set_if_neq(Visibility::Hidden);
            }
        }
    }
    stats.lights_lit = lit;
}
