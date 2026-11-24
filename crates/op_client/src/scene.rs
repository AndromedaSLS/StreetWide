//! What you see: the camera and its rendering effects, and world meshes
//! streamed in around the camera.
//!
//! The render frame is fixed for the run (see `session`): a mesh is placed
//! at its tile origin minus the render origin, and the player at its sim
//! position plus the sim origin's offset, so what you see is exactly where
//! the simulation collides. The one exception is flat terrain under water,
//! drawn just below the water so the water shows (see `water`); the
//! simulation still collides with the terrain as it is.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use bevy::anti_alias::taa::TemporalAntiAliasing;
use bevy::camera::Exposure;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::light::{AtmosphereEnvironmentMapLight, NotShadowCaster, ShadowFilteringMethod};
use bevy::pbr::{AtmosphereSettings, ScreenSpaceAmbientOcclusion};
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use op_world::{ContentId, Layer};

use crate::controls::{Look, turn16_to_yaw};
use crate::heights::{Height, HeightIndex};
use crate::materials::{self, AssetLook, MaterialSet, Surface};
use crate::session::{EYE_HEIGHT, Session};
use crate::travel::{Travel, View};
use crate::water::{self, WaterSurface};

/// Meshes within this distance of the camera are loaded and shown (m).
pub const RENDER_RADIUS_M: f32 = 600.0;
/// ... and dropped beyond this one.
const UNLOAD_RADIUS_M: f32 = 700.0;
/// Time per frame for turning loaded meshes into scene objects.
const SPAWN_BUDGET: Duration = Duration::from_millis(6);

#[derive(Component)]
pub struct PlayerCamera;

/// (tile x, tile y, layer, feature id): one placed asset.
type AssetId = (u32, u32, String, String);

/// Assets currently in the scene.
#[derive(Resource, Default)]
struct Shown {
    entities: BTreeMap<AssetId, Vec<Entity>>,
}

/// Buildings that have a contributed facade: their walls get no
/// client-drawn windows.
#[derive(Resource, Default)]
struct Facaded(BTreeSet<String>);

/// An asset in range that is not in the scene yet.
struct Wanted {
    distance: f32,
    id: AssetId,
    content_id: ContentId,
    offset: Vec3,
    origin_m: [f64; 3],
    look: AssetLook,
    /// Region-frame centre (x, z) of the asset's bounds.
    centre: (f64, f64),
    /// Terrain only: the water of its tile, under which it is drawn lower.
    water: Option<ContentId>,
}

/// Streaming progress for the HUD.
#[derive(Resource, Default)]
pub struct StreamStats {
    pub shown: usize,
    pub wanted: usize,
}

/// Rendering effects: `high` (default) or a lighter set for slower GPUs.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quality {
    pub high: bool,
}

pub struct ScenePlugin;

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Shown>()
            .init_resource::<StreamStats>()
            .init_resource::<HeightIndex>()
            .insert_resource(Quality { high: true })
            .add_systems(Startup, (materials::setup, setup_camera, find_facades))
            .add_systems(
                Update,
                (
                    scene_keys,
                    apply_quality,
                    materials::apply_look,
                    materials::drift_ripples,
                    follow_player,
                    stream_meshes,
                )
                    .chain()
                    .after(crate::travel::TravelSystems),
            );
    }
}

fn setup_camera(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: 70f32.to_radians(),
            near: 0.1,
            far: 8_000.0,
            ..default()
        }),
        AtmosphereSettings::default(),
        AtmosphereEnvironmentMapLight::default(),
        Exposure { ev100: 13.5 },
        // Compared with TonyMcMapface and AgX on the same street: the most
        // contrast without clipping the white plaster.
        Tonemapping::AcesFitted,
        Bloom::NATURAL,
        PlayerCamera,
    ));
}

fn find_facades(mut commands: Commands, session: Res<Session>) {
    let ids = session
        .streamer()
        .index()
        .tiles()
        .iter()
        .flat_map(|t| &t.assets)
        .filter(|a| a.layer == Layer::Facade)
        .map(|a| a.feature_id.clone())
        .collect();
    commands.insert_resource(Facaded(ids));
}

/// F2: detailed or plain look. F4: rendering quality.
fn scene_keys(
    keys: Res<ButtonInput<KeyCode>>,
    mut set: ResMut<MaterialSet>,
    mut quality: ResMut<Quality>,
    mut session: ResMut<Session>,
) {
    if keys.just_pressed(KeyCode::F2) {
        set.detailed_on = !set.detailed_on;
        session.message = Some((
            if set.detailed_on {
                "Detailed look (client-drawn textures and windows)".into()
            } else {
                "Plain look (the world data's palette colours)".into()
            },
            4.0,
        ));
    }
    if keys.just_pressed(KeyCode::F4) {
        quality.high = !quality.high;
        session.message = Some((
            format!("Graphics: {}", if quality.high { "high" } else { "fast" }),
            3.0,
        ));
    }
}

/// Puts the quality's effects on the camera.
fn apply_quality(
    mut commands: Commands,
    quality: Res<Quality>,
    camera: Single<Entity, With<PlayerCamera>>,
    mut shadow_map: ResMut<bevy::light::DirectionalLightShadowMap>,
) {
    if !quality.is_changed() {
        return;
    }
    let mut cam = commands.entity(*camera);
    if quality.high {
        // Temporal anti-aliasing (and its soft shadow filtering) needs
        // multisampling off; ambient occlusion darkens corners and the
        // gaps between buildings.
        cam.insert((
            Msaa::Off,
            TemporalAntiAliasing::default(),
            ScreenSpaceAmbientOcclusion::default(),
            ShadowFilteringMethod::Temporal,
        ));
        shadow_map.size = 4096;
    } else {
        cam.remove::<(TemporalAntiAliasing, ScreenSpaceAmbientOcclusion)>()
            .insert((Msaa::Sample4, ShadowFilteringMethod::Gaussian));
        shadow_map.size = 2048;
    }
}

fn follow_player(
    session: Res<Session>,
    look: Res<Look>,
    time: Res<Time<Fixed>>,
    mut travel: ResMut<Travel>,
    camera: Single<(&mut Transform, Option<&mut TemporalAntiAliasing>), With<PlayerCamera>>,
) {
    let (mut transform, taa) = camera.into_inner();
    if travel.cut {
        if let Some(mut taa) = taa {
            taa.reset = true;
        }
        travel.cut = false;
    }
    let (yaw, pitch) = if session.is_playback() && travel.view == View::Walk {
        let p = session.sim().player();
        (
            turn16_to_yaw(p.yaw),
            f32::from(p.pitch) / 65_536.0 * std::f32::consts::TAU,
        )
    } else {
        (look.yaw, look.pitch)
    };
    // Yaw is clockwise from north (-Z); Bevy's rotation about +Y is
    // counter-clockwise seen from above.
    let rotation = Quat::from_rotation_y(-yaw) * Quat::from_rotation_x(pitch);
    match travel.view {
        View::Walk => {
            let t = time.overstep_fraction().clamp(0.0, 1.0);
            let feet = session.prev_feet.lerp(session.curr_feet, t);
            transform.translation = feet + Vec3::Y * EYE_HEIGHT;
            transform.rotation = rotation;
        }
        View::Fly { pos } => {
            transform.translation = pos;
            transform.rotation = rotation;
        }
        View::Map { centre, height } => {
            *transform = Transform::from_translation(centre + Vec3::Y * height)
                .looking_to(Vec3::NEG_Y, Vec3::NEG_Z);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn stream_meshes(
    mut commands: Commands,
    session: Res<Session>,
    set: Res<MaterialSet>,
    facaded: Res<Facaded>,
    camera: Single<&Transform, With<PlayerCamera>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut shown: ResMut<Shown>,
    mut stats: ResMut<StreamStats>,
    mut heights: ResMut<HeightIndex>,
) {
    let streamer = session.streamer();
    let eye = camera.translation;
    let mut wanted: Vec<Wanted> = Vec::new();
    let mut keep = BTreeSet::new();
    for tile in streamer.index().tiles() {
        let offset = session.offset_m(tile.origin_mm);
        let origin_m = tile.origin_mm.map(|v| v as f64 / 1000.0);
        // A tile has at most one water asset (RFC 0003).
        let water = tile
            .assets
            .iter()
            .find(|a| a.layer == Layer::Water)
            .map(|a| a.content_id);
        for a in &tile.assets {
            let b = a.bounds_mm;
            let lo = offset + Vec3::new(b.min[0] as f32, b.min[1] as f32, b.min[2] as f32) / 1000.0;
            let hi = offset + Vec3::new(b.max[0] as f32, b.max[1] as f32, b.max[2] as f32) / 1000.0;
            // Horizontal distance from the eye to the asset's bounds.
            let dx = (lo.x - eye.x).max(eye.x - hi.x).max(0.0);
            let dz = (lo.z - eye.z).max(eye.z - hi.z).max(0.0);
            let d = (dx * dx + dz * dz).sqrt();
            let key = (
                tile.key.x,
                tile.key.y,
                a.layer.as_str().to_string(),
                a.feature_id.clone(),
            );
            if d < UNLOAD_RADIUS_M {
                keep.insert(key.clone());
            }
            if d < RENDER_RADIUS_M && !shown.entities.contains_key(&key) {
                let height = (b.max[1] - b.min[1]) as f64 / 1000.0;
                let look = match a.layer {
                    Layer::Shell => AssetLook::for_building(
                        &a.feature_id,
                        height,
                        facaded.0.contains(&a.feature_id),
                    ),
                    Layer::Facade => AssetLook::for_building(&a.feature_id, height, true),
                    _ => AssetLook::plain(),
                };
                let centre = (
                    origin_m[0] + (b.min[0] + b.max[0]) as f64 / 2000.0,
                    origin_m[2] + (b.min[2] + b.max[2]) as f64 / 2000.0,
                );
                wanted.push(Wanted {
                    distance: d,
                    id: key,
                    content_id: a.content_id,
                    offset,
                    origin_m,
                    look,
                    centre,
                    water: water.filter(|_| a.layer == Layer::Terrain),
                });
            }
        }
    }
    // Drop what is now far away.
    let far: Vec<_> = shown
        .entities
        .keys()
        .filter(|k| !keep.contains(*k))
        .cloned()
        .collect();
    for k in far {
        for e in shown.entities.remove(&k).unwrap_or_default() {
            commands.entity(e).despawn();
        }
    }
    // Load and show the nearest missing assets first, within the budget.
    wanted.sort_by(|a, b| a.distance.total_cmp(&b.distance));
    stats.wanted = wanted.len();
    let start = Instant::now();
    for w in wanted {
        let Some(glb) = streamer.get(&w.content_id) else {
            streamer.request(&w.content_id);
            continue;
        };
        if start.elapsed() > SPAWN_BUDGET {
            continue;
        }
        // Windowed walls count storeys from the terrain: wait for it.
        if w.look.style.is_some()
            && heights.ground(streamer, w.centre.0, w.centre.1) == Height::Loading
        {
            continue;
        }
        // Terrain is drawn lower under water: wait for the tile's water. If
        // that cannot be loaded, the terrain is drawn as it is.
        let tile_water = match w.water {
            Some(id) => match streamer.get(&id) {
                Some(mesh) => Some(WaterSurface::new(&mesh)),
                None if streamer.error(&id).is_some() => None,
                None => {
                    streamer.request(&id);
                    continue;
                }
            },
            None => None,
        };
        let mut ground = |x: f64, z: f64| match heights.ground(streamer, x, z) {
            Height::At(y) => Some(y),
            _ => None,
        };
        let sunk = tile_water.map(|surface| water::sink_terrain(&glb, &surface));
        let mesh = sunk.as_ref().unwrap_or(&*glb);
        let mut entities = Vec::new();
        for prim in mesh.nodes.iter().flat_map(|n| &n.primitives) {
            for (surface, part) in materials::parts(prim, w.origin_m, &w.look, &mut ground) {
                let looks = set.looks(surface, prim.material, &w.look);
                let mut entity = commands.spawn((
                    Mesh3d(meshes.add(materials::mesh(part))),
                    MeshMaterial3d(set.current(&looks)),
                    looks,
                    Transform::from_translation(w.offset),
                ));
                // Water lies on the ground: its shadow would fall on
                // nothing you see, so it stays out of the shadow maps.
                if surface == Surface::Water {
                    entity.insert(NotShadowCaster);
                }
                entities.push(entity.id());
            }
        }
        shown.entities.insert(w.id, entities);
    }
    stats.shown = shown.entities.len();
}
