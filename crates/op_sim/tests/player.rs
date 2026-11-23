//! Player physics on a small synthetic world: flat ground, a wall, a 20
//! degree ramp, a 0.2 m kerb, and a box far away; and on small worlds with
//! concave corners. Checks the behaviour the client relies on, and that runs
//! are deterministic.

use std::collections::BTreeMap;
use std::sync::Arc;

use op_kernel::input::buttons;
use op_kernel::{
    AssetKey, BoundsMm, ColliderAsset, ColliderMesh, Fetch, GeometryError, GeometrySource,
    PlayerInput, StateHash, TickOutcome,
};
use op_sim::Sim;
use op_sim::player::{JUMP_SPEED, WALK_SPEED};

const EAST: u16 = 16_384;
const WEST: u16 = 49_152;
const NORTH: u16 = 0;

/// An axis-aligned box from `min` to `max` (mm).
fn cuboid(min: [i64; 3], max: [i64; 3]) -> ColliderMesh {
    let v = (0..8)
        .map(|i| {
            [
                if i & 1 == 0 { min[0] } else { max[0] },
                if i & 2 == 0 { min[1] } else { max[1] },
                if i & 4 == 0 { min[2] } else { max[2] },
            ]
        })
        .collect();
    let quads = [
        [0, 2, 3, 1],
        [4, 5, 7, 6],
        [0, 1, 5, 4],
        [2, 6, 7, 3],
        [0, 4, 6, 2],
        [1, 3, 7, 5],
    ];
    let triangles = quads
        .iter()
        .flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]])
        .collect();
    ColliderMesh {
        vertices_mm: v,
        triangles,
    }
}

fn quad(a: [i64; 3], b: [i64; 3], c: [i64; 3], d: [i64; 3]) -> ColliderMesh {
    ColliderMesh {
        vertices_mm: vec![a, b, c, d],
        triangles: vec![[0, 1, 2], [0, 2, 3]],
    }
}

struct TestWorld {
    assets: Vec<(ColliderAsset, Arc<ColliderMesh>)>,
    /// Answer `Pending` to this many mesh requests first.
    pending_meshes: u32,
}

fn ground() -> ColliderMesh {
    quad(
        [-100_000, 0, -100_000],
        [-100_000, 0, 100_000],
        [100_000, 0, 100_000],
        [100_000, 0, -100_000],
    )
}

impl TestWorld {
    fn new() -> Self {
        let mut parts = BTreeMap::new();
        parts.insert(("terrain", "ground"), ground());
        parts.insert(
            ("shell", "wall"),
            cuboid([5_000, 0, -10_000], [6_000, 4_000, 10_000]),
        );
        // Rises from x = -5 m to x = -15 m by 3.64 m (20 degrees).
        parts.insert(
            ("shell", "ramp"),
            quad(
                [-5_000, 0, -3_000],
                [-5_000, 0, 3_000],
                [-15_000, 3_640, 3_000],
                [-15_000, 3_640, -3_000],
            ),
        );
        parts.insert(
            ("shell", "kerb"),
            cuboid([-3_000, 0, -8_000], [3_000, 200, -6_000]),
        );
        parts.insert(
            ("shell", "far"),
            cuboid([900_000, 0, 0], [901_000, 1_000, 1_000]),
        );
        Self::from_parts(parts)
    }

    fn from_parts(parts: BTreeMap<(&str, &str), ColliderMesh>) -> Self {
        let assets = parts
            .into_iter()
            .enumerate()
            .map(|(i, ((layer, name), mesh))| {
                let mut min = [i64::MAX; 3];
                let mut max = [i64::MIN; 3];
                for v in &mesh.vertices_mm {
                    for k in 0..3 {
                        min[k] = min[k].min(v[k]);
                        max[k] = max[k].max(v[k]);
                    }
                }
                let asset = ColliderAsset {
                    key: AssetKey {
                        tile: [0, 0],
                        layer: layer.into(),
                        feature_id: format!("test:{name}"),
                    },
                    content_id: [i as u8; 32],
                    bounds_mm: BoundsMm { min, max },
                };
                (asset, Arc::new(mesh))
            })
            .collect();
        TestWorld {
            assets,
            pending_meshes: 0,
        }
    }
}

impl GeometrySource for TestWorld {
    fn colliders_in(
        &mut self,
        area: &BoundsMm,
    ) -> Result<Fetch<Vec<ColliderAsset>>, GeometryError> {
        // Deliberately reversed: the simulation must not depend on the order.
        Ok(Fetch::Ready(
            self.assets
                .iter()
                .rev()
                .filter(|(a, _)| a.bounds_mm.intersects(area))
                .map(|(a, _)| a.clone())
                .collect(),
        ))
    }

    fn mesh(&mut self, asset: &ColliderAsset) -> Result<Fetch<Arc<ColliderMesh>>, GeometryError> {
        if self.pending_meshes > 0 {
            self.pending_meshes -= 1;
            return Ok(Fetch::Pending);
        }
        self.assets
            .iter()
            .find(|(a, _)| a.key == asset.key)
            .map(|(_, m)| Fetch::Ready(Arc::clone(m)))
            .ok_or_else(|| GeometryError("unknown asset".into()))
    }
}

fn input(yaw: u16, forward: i8, buttons: u8) -> PlayerInput {
    PlayerInput {
        forward,
        strafe: 0,
        yaw,
        pitch: 0,
        buttons,
    }
}

fn run(sim: &mut Sim, world: &mut TestWorld, i: PlayerInput, ticks: u32) -> Vec<StateHash> {
    (0..ticks)
        .map(|_| match sim.tick(&i, world).expect("tick") {
            TickOutcome::Advanced { hash, .. } => hash,
            TickOutcome::Stalled => panic!("unexpected stall"),
        })
        .collect()
}

/// A player standing on the ground at the origin, facing `yaw`.
fn landed(yaw: u16) -> (Sim, TestWorld) {
    let mut world = TestWorld::new();
    let mut sim = Sim::new(7, [0, 1_000, 0], yaw);
    run(&mut sim, &mut world, input(yaw, 0, 0), 60);
    assert!(sim.player().grounded, "landed after falling 1 m");
    (sim, world)
}

fn feet(sim: &Sim) -> [f32; 3] {
    sim.player().feet()
}

#[test]
fn falls_and_lands_on_the_ground() {
    let (sim, _) = landed(NORTH);
    let y = feet(&sim)[1];
    assert!((0.0..=0.05).contains(&y), "feet at {y}");
    assert_eq!(sim.player().vertical_speed, 0.0);
    assert_eq!(sim.active_colliders(), 4, "the far box is not loaded");
}

#[test]
fn walks_and_sprints_at_the_set_speeds() {
    let (mut sim, mut world) = landed(EAST);
    let x0 = feet(&sim)[0];
    run(&mut sim, &mut world, input(EAST, 127, 0), 60);
    let walked = feet(&sim)[0] - x0;
    assert!(
        (walked - WALK_SPEED).abs() < 0.02,
        "walked {walked} m in 1 s"
    );
    let (mut sim, mut world) = landed(NORTH);
    run(&mut sim, &mut world, input(NORTH, 127, buttons::SPRINT), 60);
    let [x, _, z] = feet(&sim);
    // This path runs along the ground quad's internal diagonal, where the
    // controller's tiny normal nudge adds about 0.5 mm of sideways drift.
    assert!(
        x.abs() < 0.01 && (z + 4.0).abs() < 0.05,
        "sprinted north to ({x}, {z})"
    );
}

#[test]
fn walls_stop_the_player() {
    let (mut sim, mut world) = landed(EAST);
    run(&mut sim, &mut world, input(EAST, 127, 0), 300);
    let x = feet(&sim)[0];
    assert!((4.6..5.0 - 0.3).contains(&x), "stopped at x = {x}");
    let m = sim.last_move();
    assert!(m.desired[0] > 0.02 && m.actual[0].abs() < 1e-3, "{m:?}");
}

#[test]
fn climbs_a_twenty_degree_ramp() {
    let (mut sim, mut world) = landed(WEST);
    run(&mut sim, &mut world, input(WEST, 127, 0), 360);
    let [x, y, _] = feet(&sim);
    let expected = (-x - 5.0) * 0.364;
    assert!(x < -7.0, "reached x = {x}");
    assert!(
        (y - expected).abs() < 0.15,
        "feet at {y}, ramp at {expected}"
    );
    assert!(sim.player().grounded);
}

#[test]
fn steps_onto_a_kerb() {
    let (mut sim, mut world) = landed(NORTH);
    run(&mut sim, &mut world, input(NORTH, 127, 0), 300);
    let [_, y, z] = feet(&sim);
    assert!(z < -6.5, "walked onto the kerb, z = {z}");
    assert!((y - 0.2).abs() < 0.05, "feet at {y}");
}

#[test]
fn jumps_and_comes_back_down() {
    let (mut sim, mut world) = landed(NORTH);
    run(&mut sim, &mut world, input(NORTH, 0, buttons::JUMP), 1);
    assert!(!sim.player().grounded);
    let mut peak = 0.0f32;
    let mut airborne = 1;
    while !sim.player().grounded && airborne < 200 {
        run(&mut sim, &mut world, input(NORTH, 0, 0), 1);
        peak = peak.max(feet(&sim)[1]);
        airborne += 1;
    }
    let expected_peak = JUMP_SPEED * JUMP_SPEED / (2.0 * 9.81);
    assert!((peak - expected_peak).abs() < 0.1, "peak {peak}");
    assert!(
        (35..=50).contains(&airborne),
        "airborne for {airborne} ticks"
    );
}

/// Walk, turn, run into the wall, jump, climb the ramp.
fn script() -> Vec<PlayerInput> {
    let mut s = Vec::new();
    s.extend([input(NORTH, 0, 0); 60]);
    s.extend([input(EAST, 127, 0); 240]);
    s.extend([input(EAST, 127, buttons::JUMP); 30]);
    s.extend((0..120).map(|t| input(EAST.wrapping_add(t * 273), 90, 0)));
    s.extend([input(WEST, 127, buttons::SPRINT); 300]);
    s
}

fn trail(seed: u64, pending: u32) -> (Vec<StateHash>, Sim) {
    let mut world = TestWorld::new();
    world.pending_meshes = pending;
    let mut sim = Sim::new(seed, [0, 1_000, 0], NORTH);
    let mut hashes = Vec::new();
    let mut stalls = 0;
    for i in script() {
        loop {
            match sim.tick(&i, &mut world).expect("tick") {
                TickOutcome::Advanced { hash, .. } => {
                    hashes.push(hash);
                    break;
                }
                TickOutcome::Stalled => stalls += 1,
            }
        }
    }
    assert!(stalls >= pending.min(1), "expected stalls");
    (hashes, sim)
}

#[test]
fn identical_runs_give_identical_hash_trails() {
    let (a, sim_a) = trail(7, 0);
    let (b, sim_b) = trail(7, 0);
    assert_eq!(a, b);
    assert_eq!(sim_a.player(), sim_b.player());
    let (c, _) = trail(8, 0);
    assert_ne!(a.last(), c.last(), "the seed is part of the state");
}

#[test]
fn stalls_while_loading_do_not_change_the_result() {
    let (a, _) = trail(7, 0);
    let (b, _) = trail(7, 3);
    assert_eq!(a, b);
}

/// Ticks, among `ticks` more, in which the player moved horizontally
/// (more than 1 mm) against its previous horizontal move.
fn reversals(sim: &mut Sim, world: &mut TestWorld, i: PlayerInput, ticks: u32) -> u32 {
    let mut previous: Option<[f32; 2]> = None;
    let mut count = 0;
    for _ in 0..ticks {
        run(sim, world, i, 1);
        let m = sim.last_move().actual;
        let h = [m[0], m[2]];
        if let Some(p) = previous
            && (h[0] * h[0] + h[1] * h[1]).sqrt() > 0.001
            && p[0] * h[0] + p[1] * h[1] < 0.0
        {
            count += 1;
        }
        previous = Some(h);
    }
    count
}

/// A player standing at the origin of a world made of `parts` and ground.
fn landed_in(mut parts: BTreeMap<(&'static str, &'static str), ColliderMesh>) -> (Sim, TestWorld) {
    parts.insert(("terrain", "ground"), ground());
    let mut world = TestWorld::from_parts(parts);
    let mut sim = Sim::new(7, [0, 1_000, 0], NORTH);
    run(&mut sim, &mut world, input(NORTH, 0, 0), 60);
    assert!(sim.player().grounded);
    (sim, world)
}

/// A vertical wall, 3 m high, from `a` to `b` on the ground (x, z in mm).
fn wall(a: [i64; 2], b: [i64; 2]) -> ColliderMesh {
    quad(
        [a[0], 0, a[1]],
        [b[0], 0, b[1]],
        [b[0], 3_000, b[1]],
        [a[0], 3_000, a[1]],
    )
}

#[test]
fn stops_steadily_in_a_wedge_of_two_walls() {
    // Two walls meet 6 m north of the start at 50 degrees, opening south;
    // the player sprints slightly east of north into the apex. Rapier
    // resolves two vertical walls without oscillating; this guards that
    // the corner rule keeps it that way.
    let mut parts = BTreeMap::new();
    parts.insert(("shell", "left"), wall([0, -6_000], [-1_800, -2_000]));
    parts.insert(("shell", "right"), wall([0, -6_000], [3_000, -1_500]));
    let (mut sim, mut world) = landed_in(parts);
    let push = input(NORTH.wrapping_add(1_200), 127, buttons::SPRINT);
    run(&mut sim, &mut world, push, 240);
    let [x, _, z] = feet(&sim);
    assert!(z < -4.5 && x.abs() < 0.5, "reached the apex: ({x}, {z})");
    assert_eq!(reversals(&mut sim, &mut world, push, 200), 0);
}

#[test]
fn pushing_into_a_wall_beside_a_steep_slope_does_not_oscillate() {
    // A 60 degree slope rises north from z = -4 m; a wall along x = 0.6 m
    // closes it to the east. The player pushes north-east into the corner
    // between them, like the lane in `op_replay`'s corner test. Without the
    // corner rule, the player reverses 6 times in these 200 ticks (on the
    // real, triangulated terrain of that test, 174 times).
    let mut parts = BTreeMap::new();
    parts.insert(
        ("terrain", "slope"),
        quad(
            [-10_000, 0, -4_000],
            [10_000, 0, -4_000],
            [10_000, 5_196, -7_000],
            [-10_000, 5_196, -7_000],
        ),
    );
    parts.insert(("shell", "wall"), wall([600, 0], [600, -10_000]));
    let (mut sim, mut world) = landed_in(parts);
    let push = input(NORTH.wrapping_add(6_000), 127, buttons::SPRINT);
    run(&mut sim, &mut world, push, 240);
    let [x, _, z] = feet(&sim);
    assert!(z < -3.0 && x > 0.2, "reached the corner: ({x}, {z})");
    assert_eq!(reversals(&mut sim, &mut world, push, 200), 0);
}

#[test]
fn sliding_along_a_single_wall_is_unchanged() {
    // Walking north-east into the wall at x = 5 m slides north along it at
    // the walking speed times cos 45 degrees.
    let (mut sim, mut world) = landed(NORTH);
    let ne = input(8_192, 127, 0);
    run(&mut sim, &mut world, ne, 330);
    let [x0, _, z0] = feet(&sim);
    assert!((4.6..4.7).contains(&x0), "at the wall: x = {x0}");
    assert_eq!(reversals(&mut sim, &mut world, ne, 60), 0);
    let [x1, _, z1] = feet(&sim);
    let slid = z0 - z1;
    assert!(
        (slid - WALK_SPEED * std::f32::consts::FRAC_1_SQRT_2).abs() < 0.03
            && (x1 - x0).abs() < 1e-3,
        "slid {slid} m north in 1 s, {} m east",
        x1 - x0
    );
}
