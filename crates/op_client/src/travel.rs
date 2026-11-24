//! Getting around fast (ADR-0013): a free-flying camera, a map view, named
//! places and "drop me here".
//!
//! Flying and the map move only the camera; the player waits where it is.
//! Arriving somewhere starts a new session at the destination
//! ([`Session::teleport`]), so the simulation never jumps and every replay
//! still starts at its own spawn point.

use std::path::Path;

use bevy::input::keyboard::Key;
use bevy::input::mouse::{AccumulatedMouseScroll, MouseScrollUnit};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use op_stream::Streamer;
use op_world::world::Spawn;
use op_world::{Collider, Layer};

use crate::Options;
use crate::controls::{Look, yaw_to_turn16};
use crate::scene::PlayerCamera;
use crate::session::Session;
use crate::sky::Clock;

/// Flying speed limits and default (m/s).
const FLY_SPEED: (f32, f32, f32) = (2.0, 400.0, 25.0);
/// Map view height limits and default above the ground (m).
const MAP_HEIGHT: (f32, f32, f32) = (80.0, 2_500.0, 650.0);
/// Arrivals are placed this far above the surface and fall onto it (m).
const DROP_ABOVE_M: f64 = 0.5;
/// Give up on a destination whose ground has not loaded after this (s).
const TRIP_TIMEOUT_S: f32 = 10.0;
/// How far from a destination to look for open ground (m).
const OPEN_GROUND_SEARCH_M: f64 = 40.0;
/// How far to step away from the building beside the open ground found.
const CLEARANCE_M: f64 = 4.0;

/// What the camera does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum View {
    /// First person, following the player.
    Walk,
    /// Free flight from `pos` (render frame).
    Fly { pos: Vec3 },
    /// Straight down from `height` above `centre` (render frame).
    Map { centre: Vec3, height: f32 },
}

impl View {
    pub fn name(&self) -> &'static str {
        match self {
            View::Walk => "walk",
            View::Fly { .. } => "fly",
            View::Map { .. } => "map",
        }
    }
}

/// A named place from the world definition (`worlds/<name>.json`).
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub name: String,
    pub lon: f64,
    pub lat: f64,
    pub yaw_deg: f64,
}

/// A destination waiting for its ground to load.
#[derive(Clone, Debug)]
pub struct Trip {
    pub name: String,
    /// Region frame (m).
    pub x: f64,
    pub z: f64,
    pub landing: Landing,
    /// Heading on arrival (radians, clockwise from north).
    pub yaw: f32,
    pub waited: f32,
    /// Keep flying or the map on arrival (the start trip with `--view`),
    /// instead of walking.
    pub keep_view: bool,
}

/// Where exactly a trip ends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Landing {
    /// On the highest surface below this height (region frame, m), roof
    /// or not: "land below" while flying.
    Below(f64),
    /// On the nearest open ground (terrain, streets, squares, bridges)
    /// rather than on a building: places and the map.
    OpenGround,
}

#[derive(Resource)]
pub struct Travel {
    pub view: View,
    pub fly_speed: f32,
    pub places: Vec<Place>,
    pub pending: Option<Trip>,
    /// Set when the camera jumped: effects that build up over frames
    /// (temporal anti-aliasing) start over.
    pub cut: bool,
}

/// The `places` of the world definition whose `name` matches, from the
/// `worlds/` directory of the repository (the current directory).
pub fn load_places(dir: &Path, world_name: &str) -> Vec<Place> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<_> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    files.sort();
    for path in files
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
    {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(spec) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if spec["name"] != world_name {
            continue;
        }
        return spec["places"]
            .as_array()
            .map(|places| {
                places
                    .iter()
                    .filter_map(|p| {
                        Some(Place {
                            name: p["name"].as_str()?.to_string(),
                            lon: p["lon"].as_f64()?,
                            lat: p["lat"].as_f64()?,
                            yaw_deg: p["yaw_deg"].as_f64().unwrap_or(0.0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
    }
    Vec::new()
}

/// A place by 1-based number or by (the start of) its name.
pub fn find_place<'a>(places: &'a [Place], query: &str) -> Option<&'a Place> {
    if let Ok(n) = query.parse::<usize>() {
        return n.checked_sub(1).and_then(|i| places.get(i));
    }
    let q = query.to_lowercase();
    places
        .iter()
        .find(|p| p.name.to_lowercase() == q)
        .or_else(|| {
            places
                .iter()
                .find(|p| p.name.to_lowercase().starts_with(&q))
        })
}

/// What is under a point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Probe {
    /// The height of the highest surface (region frame, m), and whether it
    /// belongs to a building.
    Found { y: f64, building: bool },
    /// Meshes there are still loading.
    Loading,
    /// Nothing to stand on.
    Nothing,
}

/// Where a trip can end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Arrival {
    /// A point on a surface (region frame, m).
    At {
        x: f64,
        y: f64,
        z: f64,
    },
    Loading,
    Nothing,
}

/// The landing point of a trip to (x, z). For [`Landing::OpenGround`],
/// the destination itself if it is open ground, else the first open
/// ground on rings of 2 m, 4 m, ... around it.
pub fn find_landing(streamer: &Streamer, x: f64, z: f64, landing: Landing) -> Arrival {
    match landing {
        Landing::Below(limit) => match probe_ground(streamer, x, z, Some(limit)) {
            Probe::Found { y, .. } => Arrival::At { x, y, z },
            Probe::Loading => Arrival::Loading,
            Probe::Nothing => Arrival::Nothing,
        },
        Landing::OpenGround => {
            let mut candidates = vec![(x, z)];
            let mut r = 2.0;
            while r <= OPEN_GROUND_SEARCH_M {
                let n = 16;
                candidates.extend((0..n).map(|k| {
                    let a = std::f64::consts::TAU * f64::from(k) / f64::from(n);
                    (x + r * a.cos(), z + r * a.sin())
                }));
                r += 2.0;
            }
            for (i, (cx, cz)) in candidates.into_iter().enumerate() {
                match probe_ground(streamer, cx, cz, None) {
                    Probe::Found { y, building: false } => {
                        if i == 0 {
                            return Arrival::At { x: cx, y, z: cz };
                        }
                        return step_clear(streamer, (x, z), (cx, cz), y);
                    }
                    Probe::Loading => return Arrival::Loading,
                    Probe::Found { building: true, .. } | Probe::Nothing => {}
                }
            }
            Arrival::Nothing
        }
    }
}

/// Moves on from the first open ground found beside a building, away from
/// it, while the ground stays open: up to [`CLEARANCE_M`] further, so the
/// arrival does not face a wall at arm's length.
fn step_clear(streamer: &Streamer, from: (f64, f64), found: (f64, f64), y: f64) -> Arrival {
    let (dx, dz) = (found.0 - from.0, found.1 - from.1);
    let l = dx.hypot(dz);
    let (ux, uz) = (dx / l, dz / l);
    let mut best = (found.0, y, found.1);
    let mut s = 1.0;
    while s <= CLEARANCE_M {
        let (cx, cz) = (found.0 + ux * s, found.1 + uz * s);
        match probe_ground(streamer, cx, cz, None) {
            Probe::Found { y, building: false } => best = (cx, y, cz),
            Probe::Loading => return Arrival::Loading,
            _ => break,
        }
        s += 1.0;
    }
    Arrival::At {
        x: best.0,
        y: best.1,
        z: best.2,
    }
}

/// The highest solid surface (terrain, paths, buildings) at a region-frame
/// point, below `below` if given. Loads the meshes it needs.
pub fn probe_ground(streamer: &Streamer, x: f64, z: f64, below: Option<f64>) -> Probe {
    let mut best: Option<(f64, bool)> = None;
    let mut loading = false;
    for tile in streamer.index().tiles() {
        let o = tile.origin_mm.map(|v| v as f64 / 1000.0);
        let (lx, lz) = (x - o[0], z - o[2]);
        for a in &tile.assets {
            if a.collider != Collider::Trimesh {
                continue;
            }
            let building = !matches!(a.layer, Layer::Terrain | Layer::Transport);
            let b = a.bounds_mm;
            let inside = |k: usize, v: f64| {
                v >= b.min[k] as f64 / 1000.0 - 1e-3 && v <= b.max[k] as f64 / 1000.0 + 1e-3
            };
            if !inside(0, lx) || !inside(2, lz) {
                continue;
            }
            let Some(mesh) = streamer.get(&a.content_id) else {
                streamer.request(&a.content_id);
                loading = true;
                continue;
            };
            for prim in mesh.nodes.iter().flat_map(|n| &n.primitives) {
                for t in prim.indices.as_chunks::<3>().0 {
                    let p = [0, 1, 2].map(|k| prim.positions[t[k] as usize].map(f64::from));
                    let Some(y) = height_in_triangle(p, lx, lz) else {
                        continue;
                    };
                    let y = y + o[1];
                    if below.is_some_and(|limit| y > limit) {
                        continue;
                    }
                    if best.is_none_or(|(b, _)| y > b) {
                        best = Some((y, building));
                    }
                }
            }
        }
    }
    match (loading, best) {
        (true, _) => Probe::Loading,
        (false, Some((y, building))) => Probe::Found { y, building },
        (false, None) => Probe::Nothing,
    }
}

/// The height of a triangle above a horizontal point, if the point lies
/// inside it seen from above.
pub fn height_in_triangle(p: [[f64; 3]; 3], x: f64, z: f64) -> Option<f64> {
    let [a, b, c] = p;
    let det = (b[2] - c[2]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[2] - c[2]);
    if det.abs() < 1e-12 {
        return None;
    }
    let w0 = ((b[2] - c[2]) * (x - c[0]) + (c[0] - b[0]) * (z - c[2])) / det;
    let w1 = ((c[2] - a[2]) * (x - c[0]) + (a[0] - c[0]) * (z - c[2])) / det;
    let w2 = 1.0 - w0 - w1;
    let eps = -1e-9;
    (w0 >= eps && w1 >= eps && w2 >= eps).then(|| w0 * a[1] + w1 * b[1] + w2 * c[1])
}

/// The travel systems; the camera follows after them.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct TravelSystems;

pub struct TravelPlugin;

impl Plugin for TravelPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init_travel).add_systems(
            Update,
            (travel_keys, move_free_camera, resolve_trip)
                .chain()
                .in_set(TravelSystems),
        );
    }
}

fn init_travel(mut commands: Commands, options: Res<Options>, mut session: ResMut<Session>) {
    let places = load_places(Path::new("worlds"), session.world_name());
    let mut travel = Travel {
        view: View::Walk,
        fly_speed: FLY_SPEED.2,
        places,
        pending: None,
        cut: false,
    };
    if !session.is_playback() {
        let start = if let Some(q) = &options.place {
            match find_place(&travel.places, q) {
                Some(p) => Some((p.name.clone(), p.lon, p.lat, p.yaw_deg)),
                None => {
                    session.message = Some((format!("No place \"{q}\" in this world"), 6.0));
                    None
                }
            }
        } else {
            options
                .at
                .map(|(lon, lat, yaw)| (format!("{lon:.5}, {lat:.5}"), lon, lat, yaw))
        };
        if let Some(v) = &options.view {
            let feet = session.curr_feet;
            travel.view = match v.as_str() {
                "fly" => View::Fly {
                    pos: feet + Vec3::Y * 30.0,
                },
                "map" => View::Map {
                    centre: feet,
                    height: MAP_HEIGHT.2,
                },
                _ => View::Walk,
            };
        }
        if let Some((name, lon, lat, yaw)) = start {
            let p = session.frame().project(lon, lat, 0.0);
            travel.pending = Some(Trip {
                name,
                x: p[0],
                z: p[2],
                landing: Landing::OpenGround,
                yaw: yaw.to_radians() as f32,
                waited: 0.0,
                keep_view: options.view.is_some(),
            });
        }
    }
    commands.insert_resource(travel);
}

fn letter(keys: &ButtonInput<Key>, c: char) -> bool {
    [c.to_ascii_lowercase(), c.to_ascii_uppercase()]
        .iter()
        .any(|l| keys.just_pressed(Key::Character(l.to_string().into())))
}

const DIGITS: [(KeyCode, KeyCode); 9] = [
    (KeyCode::Digit1, KeyCode::Numpad1),
    (KeyCode::Digit2, KeyCode::Numpad2),
    (KeyCode::Digit3, KeyCode::Numpad3),
    (KeyCode::Digit4, KeyCode::Numpad4),
    (KeyCode::Digit5, KeyCode::Numpad5),
    (KeyCode::Digit6, KeyCode::Numpad6),
    (KeyCode::Digit7, KeyCode::Numpad7),
    (KeyCode::Digit8, KeyCode::Numpad8),
    (KeyCode::Digit9, KeyCode::Numpad9),
];

#[allow(clippy::too_many_arguments)]
fn travel_keys(
    keys: Res<ButtonInput<KeyCode>>,
    logical: Res<ButtonInput<Key>>,
    mouse: Res<ButtonInput<MouseButton>>,
    scroll: Res<AccumulatedMouseScroll>,
    mut window: Single<(&Window, &mut CursorOptions), With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform, &Transform), With<PlayerCamera>>,
    mut travel: ResMut<Travel>,
    mut session: ResMut<Session>,
    mut clock: ResMut<Clock>,
    look: Res<Look>,
) {
    let (cam, cam_global, cam_transform) = *camera;
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);

    if letter(&logical, 't') {
        clock.shift(if shift { -30 } else { 30 });
    }
    if letter(&logical, 'v') {
        travel.view = match travel.view {
            View::Fly { .. } => View::Walk,
            _ => View::Fly {
                pos: cam_transform.translation,
            },
        };
        travel.cut = true;
    }
    if letter(&logical, 'm') {
        travel.view = match travel.view {
            View::Map { .. } => View::Walk,
            _ => {
                // The map needs the mouse pointer.
                window.1.grab_mode = CursorGrabMode::None;
                window.1.visible = true;
                View::Map {
                    centre: cam_transform.translation.with_y(session.curr_feet.y),
                    height: MAP_HEIGHT.2,
                }
            }
        };
        travel.cut = true;
    }

    // Scrolling: flying speed, or map height.
    let notches = match scroll.unit {
        MouseScrollUnit::Line => scroll.delta.y,
        MouseScrollUnit::Pixel => scroll.delta.y / 40.0,
    };
    if notches != 0.0 {
        match &mut travel.view {
            View::Fly { .. } => {
                travel.fly_speed =
                    (travel.fly_speed * 1.25f32.powf(notches)).clamp(FLY_SPEED.0, FLY_SPEED.1);
            }
            View::Map { height, .. } => {
                *height = (*height * 0.85f32.powf(notches)).clamp(MAP_HEIGHT.0, MAP_HEIGHT.1);
            }
            View::Walk => {}
        }
    }

    let mut trip: Option<Trip> = None;
    if let View::Fly { pos } = travel.view
        && (keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter))
    {
        let r = session.render_to_region(pos);
        trip = Some(Trip {
            name: "the ground below".into(),
            x: r[0],
            z: r[2],
            landing: Landing::Below(r[1]),
            yaw: look.yaw,
            waited: 0.0,
            keep_view: false,
        });
    }
    if let View::Map { centre, .. } = travel.view
        && mouse.just_pressed(MouseButton::Left)
        && let Some(cursor) = window.0.cursor_position()
        && let Ok(ray) = cam.viewport_to_world(cam_global, cursor)
        && let Some(d) = ray.intersect_plane(centre, InfinitePlane3d::new(Vec3::Y))
    {
        let hit = ray.get_point(d);
        let r = session.render_to_region(hit);
        trip = Some(Trip {
            name: "the point on the map".into(),
            x: r[0],
            z: r[2],
            landing: Landing::OpenGround,
            yaw: look.yaw,
            waited: 0.0,
            keep_view: false,
        });
    }
    for (i, (a, b)) in DIGITS.iter().enumerate() {
        if (keys.just_pressed(*a) || keys.just_pressed(*b))
            && let Some(p) = travel.places.get(i).cloned()
        {
            let r = session.frame().project(p.lon, p.lat, 0.0);
            trip = Some(Trip {
                name: p.name,
                x: r[0],
                z: r[2],
                landing: Landing::OpenGround,
                yaw: (p.yaw_deg as f32).to_radians(),
                waited: 0.0,
                keep_view: false,
            });
        }
    }
    if let Some(t) = trip {
        if session.is_playback() {
            session.message = Some(("Travel is off while a replay plays".into(), 3.0));
        } else {
            session.message = Some((format!("Going to {}…", t.name), 10.0));
            travel.pending = Some(t);
        }
    }
}

fn move_free_camera(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    look: Res<Look>,
    mut travel: ResMut<Travel>,
) {
    let dt = time.delta_secs();
    let axis = |plus: KeyCode, minus: KeyCode| -> f32 {
        f32::from(i8::from(keys.pressed(plus)) - i8::from(keys.pressed(minus)))
    };
    let (forward, right) = (
        axis(KeyCode::KeyW, KeyCode::KeyS),
        axis(KeyCode::KeyD, KeyCode::KeyA),
    );
    let fast = if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
        4.0
    } else {
        1.0
    };
    let speed = travel.fly_speed;
    match &mut travel.view {
        View::Fly { pos } => {
            let rot = Quat::from_rotation_y(-look.yaw) * Quat::from_rotation_x(look.pitch);
            let up = axis(KeyCode::Space, KeyCode::ControlLeft);
            let dir = rot * Vec3::NEG_Z * forward + rot * Vec3::X * right + Vec3::Y * up;
            if dir != Vec3::ZERO {
                *pos += dir.normalize() * speed * fast * dt;
            }
        }
        View::Map { centre, height } => {
            // North is up on the map.
            let dir = Vec3::new(right, 0.0, -forward);
            if dir != Vec3::ZERO {
                *centre += dir.normalize() * *height * 0.8 * fast * dt;
            }
        }
        View::Walk => {}
    }
}

fn resolve_trip(
    time: Res<Time>,
    mut travel: ResMut<Travel>,
    mut session: ResMut<Session>,
    mut look: ResMut<Look>,
) {
    let Some(trip) = &mut travel.pending else {
        return;
    };
    let arrival = find_landing(session.streamer(), trip.x, trip.z, trip.landing);
    let trip = trip.clone();
    match arrival {
        Arrival::At { x, y, z } => {
            let position_mm = [x, y + DROP_ABOVE_M, z].map(|v| (v * 1000.0).round() as i64);
            session.teleport(Spawn {
                position_mm,
                yaw: yaw_to_turn16(trip.yaw),
            });
            look.yaw = trip.yaw;
            look.pitch = 0.0;
            let feet = session.curr_feet;
            travel.view = match travel.view {
                View::Fly { .. } if trip.keep_view => View::Fly {
                    pos: feet + Vec3::Y * 30.0,
                },
                View::Map { height, .. } if trip.keep_view => View::Map {
                    centre: feet,
                    height,
                },
                _ => View::Walk,
            };
            travel.cut = true;
            travel.pending = None;
            session.message = Some((format!("Arrived at {}", trip.name), 4.0));
        }
        Arrival::Loading => {
            let waited = trip.waited + time.delta_secs();
            if waited > TRIP_TIMEOUT_S {
                travel.pending = None;
                session.message =
                    Some((format!("Could not load the ground at {}", trip.name), 5.0));
            } else if let Some(t) = &mut travel.pending {
                t.waited = waited;
            }
        }
        Arrival::Nothing => {
            travel.pending = None;
            session.message = Some((format!("Nothing to stand on at {}", trip.name), 5.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn repo() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn heights_inside_triangles() {
        let tri = [[0.0, 1.0, 0.0], [10.0, 2.0, 0.0], [0.0, 3.0, 10.0]];
        assert_eq!(height_in_triangle(tri, 0.0, 0.0), Some(1.0));
        let y = height_in_triangle(tri, 2.5, 2.5).unwrap();
        assert!((y - 1.75).abs() < 1e-12, "{y}");
        assert_eq!(height_in_triangle(tri, 9.0, 9.0), None);
        // Vertical triangles have no height.
        let wall = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        assert_eq!(height_in_triangle(wall, 0.2, 0.0), None);
    }

    #[test]
    fn worlds_list_their_places() {
        let places = load_places(&repo().join("worlds"), "zurich-altstadt");
        assert!(places.len() >= 9, "{places:?}");
        assert_eq!(find_place(&places, "1"), places.first());
        assert_eq!(
            find_place(&places, "grossm").map(|p| p.name.as_str()),
            Some("Grossmünsterplatz")
        );
        assert!(find_place(&places, "nowhere").is_none());
        assert!(load_places(&repo().join("worlds"), "no-such-world").is_empty());
    }

    fn fixture() -> Arc<Streamer> {
        Arc::new(Streamer::open(&repo().join("fixtures/world"), 2, 64 << 20).unwrap())
    }

    fn landing(streamer: &Streamer, x: f64, z: f64, how: Landing) -> Arrival {
        for _ in 0..4000 {
            match find_landing(streamer, x, z, how) {
                Arrival::Loading => std::thread::sleep(std::time::Duration::from_millis(5)),
                done => return done,
            }
        }
        panic!("the fixture did not load")
    }

    /// The world spawn is defined 1 m above the ground: the probe finds
    /// that ground once the meshes have loaded.
    #[test]
    fn finds_the_ground_under_the_fixture_spawn() {
        let streamer = fixture();
        let spawn = streamer
            .index()
            .world()
            .spawn
            .position_mm
            .map(|v| v as f64 / 1000.0);
        let Arrival::At { x, y, z } = landing(&streamer, spawn[0], spawn[2], Landing::OpenGround)
        else {
            panic!()
        };
        assert_eq!((x, z), (spawn[0], spawn[2]));
        assert!(
            (spawn[1] - y - 1.0).abs() < 0.05,
            "ground {y}, spawn {}",
            spawn[1]
        );
        // Far outside the world there is nothing.
        assert_eq!(
            landing(&streamer, spawn[0] + 5_000.0, spawn[2], Landing::OpenGround),
            Arrival::Nothing
        );
    }

    /// A place inside a building lands on the ground next to it; flying
    /// over the building and landing below lands on its roof.
    #[test]
    fn places_land_beside_buildings_not_on_them() {
        let streamer = fixture();
        let frame = crate::geo::RegionFrame::new(streamer.index().world().region.anchor_e9);
        // The middle of the Grossmünster (OpenStreetMap).
        let p = frame.project(8.54409, 47.37006, 0.0);
        let Arrival::At { y: roof, .. } = landing(&streamer, p[0], p[2], Landing::Below(1e9))
        else {
            panic!()
        };
        assert!(matches!(
            probe_ground(&streamer, p[0], p[2], None),
            Probe::Found { building: true, .. }
        ));
        let Arrival::At { x, y, z } = landing(&streamer, p[0], p[2], Landing::OpenGround) else {
            panic!()
        };
        let moved = (x - p[0]).hypot(z - p[2]);
        assert!(
            moved > 1.0 && moved <= OPEN_GROUND_SEARCH_M + CLEARANCE_M,
            "{moved}"
        );
        assert!(y < roof - 5.0, "ground {y}, roof {roof}");
        assert!(matches!(
            probe_ground(&streamer, x, z, None),
            Probe::Found {
                building: false,
                ..
            }
        ));
    }
}
