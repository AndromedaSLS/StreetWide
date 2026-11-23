//! What you see: the first-person camera, sun and sky, and world meshes
//! streamed in around the camera.
//!
//! The render frame is the simulation's sim frame (X east, Y up, Z south,
//! metres): a mesh is placed at its tile origin minus the sim origin, so
//! what you see is exactly where the simulation collides.

use std::collections::{BTreeMap, BTreeSet};

use bevy::asset::RenderAssetUsages;
use bevy::light::{CascadeShadowConfigBuilder, GlobalAmbientLight};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;
use op_world::ContentId;
use op_world::glb::Material as Palette;

use crate::controls::{Look, turn16_to_yaw};
use crate::session::{EYE_HEIGHT, Session};

/// Meshes within this distance of the camera are loaded and shown (m).
pub const RENDER_RADIUS_M: f32 = 600.0;
/// ... and dropped beyond this one.
const UNLOAD_RADIUS_M: f32 = 700.0;
/// New assets put into the scene per frame (avoids hitches).
const SPAWNS_PER_FRAME: usize = 24;
const SKY: Color = Color::srgb(0.62, 0.74, 0.88);

#[derive(Component)]
pub struct PlayerCamera;

/// (tile x, tile y, layer, feature id): one placed asset.
type AssetId = (u32, u32, String, String);

/// Assets currently in the scene.
#[derive(Resource, Default)]
struct Shown {
    entities: BTreeMap<AssetId, Vec<Entity>>,
}

/// An asset in range that is not in the scene yet.
struct Wanted {
    distance: f32,
    id: AssetId,
    content_id: ContentId,
    offset: Vec3,
}

#[derive(Resource)]
struct Palettes(BTreeMap<&'static str, Handle<StandardMaterial>>);

/// Streaming progress for the HUD.
#[derive(Resource, Default)]
pub struct StreamStats {
    pub shown: usize,
    pub wanted: usize,
}

pub struct ScenePlugin;

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ClearColor(SKY))
            .insert_resource(GlobalAmbientLight {
                color: Color::srgb(0.85, 0.9, 1.0),
                brightness: 600.0,
                ..default()
            })
            .init_resource::<Shown>()
            .init_resource::<StreamStats>()
            .add_systems(Startup, setup)
            .add_systems(Update, (follow_player, stream_meshes));
    }
}

fn setup(mut commands: Commands, mut materials: ResMut<Assets<StandardMaterial>>) {
    let palettes = [
        Palette::Roof,
        Palette::Wall,
        Palette::Ground,
        Palette::Terrain,
        Palette::Road,
        Palette::Path,
        Palette::Bridge,
        Palette::Water,
    ]
    .into_iter()
    .map(|m| {
        let [r, g, b, a] = m.base_color_milli().map(|v| v as f32 / 1000.0);
        let handle = materials.add(StandardMaterial {
            base_color: Color::srgba(r, g, b, a),
            perceptual_roughness: 1.0,
            metallic: 0.0,
            // A few source surfaces face inwards; draw both sides.
            double_sided: true,
            cull_mode: None,
            ..default()
        });
        (m.name(), handle)
    })
    .collect();
    commands.insert_resource(Palettes(palettes));

    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: 70f32.to_radians(),
            near: 0.1,
            far: 2_000.0,
            ..default()
        }),
        DistanceFog {
            color: SKY,
            falloff: FogFalloff::Linear {
                start: 250.0,
                end: RENDER_RADIUS_M,
            },
            ..default()
        },
        PlayerCamera,
    ));

    // Afternoon sun from the south-west, 40 degrees high.
    commands.spawn((
        DirectionalLight {
            illuminance: 12_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(-0.55, 0.64, 0.53).looking_at(Vec3::ZERO, Vec3::Y),
        CascadeShadowConfigBuilder {
            first_cascade_far_bound: 25.0,
            maximum_distance: 350.0,
            ..default()
        }
        .build(),
    ));
}

fn follow_player(
    session: Res<Session>,
    look: Res<Look>,
    time: Res<Time<Fixed>>,
    mut camera: Single<&mut Transform, With<PlayerCamera>>,
) {
    let t = time.overstep_fraction().clamp(0.0, 1.0);
    let feet = session.prev_feet.lerp(session.curr_feet, t);
    let (yaw, pitch) = if session.is_playback() {
        let p = session.sim().player();
        (
            turn16_to_yaw(p.yaw),
            f32::from(p.pitch) / 65_536.0 * std::f32::consts::TAU,
        )
    } else {
        (look.yaw, look.pitch)
    };
    camera.translation = feet + Vec3::Y * EYE_HEIGHT;
    // Yaw is clockwise from north (-Z); Bevy's rotation about +Y is
    // counter-clockwise seen from above.
    camera.rotation = Quat::from_rotation_y(-yaw) * Quat::from_rotation_x(pitch);
}

#[allow(clippy::too_many_arguments)]
fn stream_meshes(
    mut commands: Commands,
    session: Res<Session>,
    palettes: Res<Palettes>,
    camera: Single<&Transform, With<PlayerCamera>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut shown: ResMut<Shown>,
    mut stats: ResMut<StreamStats>,
) {
    let streamer = session.streamer();
    let eye = camera.translation;
    let mut wanted: Vec<Wanted> = Vec::new();
    let mut keep = BTreeSet::new();
    for tile in streamer.index().tiles() {
        let offset = session.offset_m(tile.origin_mm);
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
                wanted.push(Wanted {
                    distance: d,
                    id: key,
                    content_id: a.content_id,
                    offset,
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
    // Load and show the nearest missing assets first.
    wanted.sort_by(|a, b| a.distance.total_cmp(&b.distance));
    stats.wanted = wanted.len();
    let mut spawned = 0;
    for Wanted {
        id: key,
        content_id: id,
        offset,
        ..
    } in wanted
    {
        let Some(glb) = streamer.get(&id) else {
            streamer.request(&id);
            continue;
        };
        if spawned >= SPAWNS_PER_FRAME {
            continue;
        }
        spawned += 1;
        let mut entities = Vec::new();
        for prim in glb.nodes.iter().flat_map(|n| &n.primitives) {
            let mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::RENDER_WORLD,
            )
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, prim.positions.clone())
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, prim.normals.clone())
            .with_inserted_indices(Indices::U32(prim.indices.clone()));
            let material = palettes.0[prim.material.name()].clone();
            entities.push(
                commands
                    .spawn((
                        Mesh3d(meshes.add(mesh)),
                        MeshMaterial3d(material),
                        Transform::from_translation(offset),
                    ))
                    .id(),
            );
        }
        shown.entities.insert(key, entities);
    }
    stats.shown = shown.entities.len();
}
