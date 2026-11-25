//! The crowd prototype's character catalogue, generated in code (character
//! models wait for ADR-0017): simple figures of boxes (legs with knees,
//! hips, a torso or coat, arms, hands, a neck, a head with hair, sometimes
//! a hat or a bag), with colours from a seeded palette per season.
//!
//! Each look is drawn by a flipbook of meshes: a step cycle of
//! [`WALK_NEAR`] poses near the camera and [`WALK_FAR`] farther away (with
//! fewer boxes), standing and sitting. The pose follows the distance
//! walked, one cycle per stride, so that a planted foot stays put, and
//! starts at a seeded phase so that people do not step in time. The
//! function picks a look by index; nothing here decides who is where.

use bevy::asset::RenderAssetUsages;
use bevy::math::Affine3A;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

/// Looks per season in the catalogue.
pub const LOOKS: u32 = 48;
/// Poses of the step cycle near the camera and far from it.
pub const WALK_NEAR: usize = 32;
pub const WALK_FAR: usize = 8;
/// Mesh slots per look: near walk, far walk, then stand and sit, near and
/// far.
pub const STAND: usize = WALK_NEAR + WALK_FAR;
pub const SIT: usize = STAND + 1;
pub const STAND_FAR: usize = STAND + 2;
pub const SIT_FAR: usize = STAND + 3;
pub const SLOTS: usize = STAND + 4;

/// Hip height and leg swing of the figures (m, degrees): a step of
/// 2 * 0.92 * sin(22 degrees) = 0.69 m for a 1.75 m person, which the
/// population function's stride assumes.
const HIP: f32 = 0.92;
const SWING_DEG: f32 = 22.0;
/// The figures' height (m): people are scaled from it.
pub const HEIGHT: f32 = 1.75;

#[derive(Clone, Copy, Debug)]
struct Look {
    top: [f32; 3],
    bottom: [f32; 3],
    skin: [f32; 3],
    hair: [f32; 3],
    shoes: [f32; 3],
    hat: Option<[f32; 3]>,
    bag: Option<[f32; 3]>,
    long_hair: bool,
    long_coat: bool,
    shorts: bool,
    short_sleeves: bool,
    shoulders: f32,
}

fn srgb(hex: u32) -> [f32; 3] {
    let c = Color::srgb_u8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8).to_linear();
    [c.red, c.green, c.blue]
}

/// A small integer hash for the catalogue (seeded by look and season, so
/// the catalogue is the same on every run).
fn h(a: u32, b: u32) -> u32 {
    let mut x = a.wrapping_mul(0x9E37_79B9) ^ b.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^ (x >> 16)
}

fn pick<T: Copy>(list: &[T], v: u32) -> T {
    list[(v as usize) % list.len()]
}

fn look(index: u32, season: u32) -> Look {
    let r = |k: u32| h(index * 31 + k, season + 7);
    let (tops, bottoms): (&[u32], &[u32]) = match season {
        // Winter: coats and dark colours.
        0 => (
            &[
                0x1f2a3a, 0x222222, 0x3a3a3a, 0x4b3a2a, 0x8a6a45, 0x5a1f2a, 0x2f3f2a, 0x5f6368,
                0xb9a88f, 0x3c4a5c, 0x7a2e2e, 0x101418, 0x6b5844, 0x2b3550,
            ],
            &[
                0x1e2633, 0x202020, 0x34383e, 0x2f3a52, 0x4a4036, 0x283044, 0x3b3b3b,
            ],
        ),
        // Summer: light colours, short sleeves.
        2 => (
            &[
                0xf2f2ee, 0x9cc3e6, 0xe8b4b8, 0xf2d16b, 0xc9c9c9, 0xb8322e, 0x24365c, 0x6aa06a,
                0xe0cfa8, 0x7fb7b0, 0xffffff, 0xd98c5f, 0x5b7fb5, 0xa7a0d0,
            ],
            &[
                0x4a6a9a, 0x2f4a73, 0xd8c9a8, 0x7c8a9a, 0x2a2a2a, 0xe6e1d6, 0x5c6b52,
            ],
        ),
        // Spring and autumn.
        _ => (
            &[
                0x34495e, 0x7f8c8d, 0xa0522d, 0x556b2f, 0x8b0000, 0xd2b48c, 0x2c3e50, 0x9e9e9e,
                0x6d4c41, 0x3e5f8a, 0xc0a060, 0x4a4a4a, 0x7b6a9a, 0xe0e0d8,
            ],
            &[0x2f4a73, 0x1e2633, 0x4a4036, 0x6b6b6b, 0x2a2a2a, 0x3f5a7a],
        ),
    };
    let skins = [
        0xf1d1bb, 0xe0b597, 0xc68e6a, 0x9c6b4a, 0x6e4a33, 0xf5dccb, 0xd8a47f,
    ];
    let hairs = [
        0x1b1410, 0x3b2a1e, 0x5a4030, 0x8a6a3a, 0xc9a96a, 0x9a9a9a, 0x6a2e1e, 0x2a2a2a,
    ];
    let shoes = [0x151515, 0x2a1d14, 0xe8e8e8, 0x3a3a3a, 0x5a4030];
    let hats = [0x2b2b2b, 0x8a1f1f, 0x2f4a73, 0xd8d0c0, 0x5a5a5a];
    let bags = [0x3a2a1a, 0x151515, 0xd8c8a8, 0x8a1f1f, 0x2f4a73, 0xf0f0f0];
    Look {
        top: srgb(pick(tops, r(1))),
        bottom: srgb(pick(bottoms, r(2))),
        skin: srgb(pick(&skins, r(3))),
        hair: srgb(pick(&hairs, r(4))),
        shoes: srgb(pick(&shoes, r(5))),
        hat: (season == 0 && r(6) % 100 < 35).then(|| srgb(pick(&hats, r(7)))),
        bag: (r(8) % 100 < 30).then(|| srgb(pick(&bags, r(9)))),
        long_hair: r(10) % 100 < 40,
        long_coat: season == 0 && r(11) % 100 < 55,
        shorts: season == 2 && r(12) % 100 < 35,
        short_sleeves: season == 2 && r(13) % 100 < 70,
        shoulders: 0.34 + (r(14) % 100) as f32 * 0.0008,
    }
}

/// Positions, normals, colours and indices of one mesh.
#[derive(Default)]
struct Builder {
    pos: Vec<[f32; 3]>,
    nrm: Vec<[f32; 3]>,
    col: Vec<[f32; 4]>,
    idx: Vec<u32>,
}

impl Builder {
    /// A box of half-extents `half` centred at `centre` in `frame`.
    fn cuboid(&mut self, frame: Affine3A, centre: Vec3, half: Vec3, colour: [f32; 3]) {
        let faces: [(Vec3, Vec3, Vec3); 6] = [
            (Vec3::X, Vec3::Y, Vec3::Z),
            (Vec3::NEG_X, Vec3::Y, Vec3::NEG_Z),
            (Vec3::Y, Vec3::Z, Vec3::X),
            (Vec3::NEG_Y, Vec3::Z, Vec3::NEG_X),
            (Vec3::Z, Vec3::X, Vec3::Y),
            (Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y),
        ];
        for (n, u, v) in faces {
            let base = self.pos.len() as u32;
            let c = centre + n * half;
            let (u, v) = (u * half, v * half);
            for (a, b) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let p = frame.transform_point3(c + u * a + v * b);
                self.pos.push(p.to_array());
                self.nrm
                    .push(frame.transform_vector3(n).normalize().to_array());
                self.col.push([colour[0], colour[1], colour[2], 1.0]);
            }
            // Counter-clockwise seen from outside: u x v = n.
            let flip = u.cross(v).dot(n) < 0.0;
            if flip {
                self.idx
                    .extend([base, base + 2, base + 1, base, base + 3, base + 2]);
            } else {
                self.idx
                    .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
            }
        }
    }

    fn mesh(self) -> Mesh {
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.pos)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.nrm)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.col)
        .with_inserted_indices(Indices::U32(self.idx))
    }
}

/// Joint angles of a pose (radians; positive swings forward).
#[derive(Clone, Copy, Debug, Default)]
struct Pose {
    /// Thigh and knee (bending back) of the left and right leg.
    thigh: [f32; 2],
    knee: [f32; 2],
    /// Shoulder and elbow (bending forward).
    shoulder: [f32; 2],
    elbow: [f32; 2],
    hip_y: f32,
    lean: f32,
}

fn smooth(u: f32) -> f32 {
    u * u * (3.0 - 2.0 * u)
}

/// The step cycle at phase `p` (0 to 1): the left foot strikes at 0, the
/// right at 0.5. The standing leg swings back at a steady angle rate, so
/// its foot moves back at nearly the walking speed; the hip rises and
/// falls so that it stays on the ground.
fn walk(p: f32) -> Pose {
    let a = SWING_DEG.to_radians();
    let leg = |q: f32| {
        let q = q.rem_euclid(1.0);
        if q < 0.5 {
            (a * (1.0 - 4.0 * q), 5f32.to_radians())
        } else {
            let u = (q - 0.5) / 0.5;
            (
                -a + 2.0 * a * smooth(u),
                55f32.to_radians() * (std::f32::consts::PI * u).sin(),
            )
        }
    };
    let (tl, kl) = leg(p);
    let (tr, kr) = leg(p + 0.5);
    let stance = if p.rem_euclid(1.0) < 0.5 { tl } else { tr };
    Pose {
        thigh: [tl, tr],
        knee: [kl, kr],
        shoulder: [-0.75 * tl, -0.75 * tr],
        elbow: [0.25 + 0.2 * (-tl).max(0.0), 0.25 + 0.2 * (-tr).max(0.0)],
        hip_y: HIP * stance.cos(),
        lean: 3f32.to_radians(),
    }
}

fn stand() -> Pose {
    Pose {
        thigh: [0.0, 0.04],
        knee: [0.0, 0.1],
        shoulder: [0.05, -0.03],
        elbow: [0.15, 0.2],
        hip_y: HIP - 0.005,
        lean: 0.0,
    }
}

fn sit() -> Pose {
    Pose {
        thigh: [1.5, 1.45],
        knee: [1.5, 1.4],
        shoulder: [0.5, 0.45],
        elbow: [0.6, 0.6],
        hip_y: 0.47,
        lean: -0.05,
    }
}

/// One figure in one pose, facing -Z, feet at y = 0, 1.75 m tall.
fn figure(l: &Look, pose: &Pose, near: bool) -> Mesh {
    let mut b = Builder::default();
    let hip = Vec3::new(0.0, pose.hip_y, 0.0);
    let body = Affine3A::from_translation(hip) * Affine3A::from_rotation_x(-pose.lean);
    let legs = if l.shorts { l.skin } else { l.bottom };
    for (side, x) in [(0usize, -0.095f32), (1, 0.095)] {
        let thigh = body
            * Affine3A::from_translation(Vec3::new(x, 0.0, 0.0))
            * Affine3A::from_rotation_x(pose.thigh[side] + pose.lean);
        if near {
            b.cuboid(
                thigh,
                Vec3::new(0.0, -0.225, 0.0),
                Vec3::new(0.075, 0.235, 0.085),
                l.bottom,
            );
            let knee = thigh
                * Affine3A::from_translation(Vec3::new(0.0, -0.45, 0.0))
                * Affine3A::from_rotation_x(-pose.knee[side]);
            b.cuboid(
                knee,
                Vec3::new(0.0, -0.2, 0.0),
                Vec3::new(0.06, 0.21, 0.07),
                legs,
            );
            let ankle = knee
                * Affine3A::from_translation(Vec3::new(0.0, -0.4, 0.0))
                * Affine3A::from_rotation_x(pose.knee[side] - pose.thigh[side]);
            b.cuboid(
                ankle,
                Vec3::new(0.0, -0.035, -0.04),
                Vec3::new(0.055, 0.035, 0.13),
                l.shoes,
            );
        } else {
            b.cuboid(
                thigh,
                Vec3::new(0.0, -0.45, 0.0),
                Vec3::new(0.07, 0.46, 0.08),
                l.bottom,
            );
        }
    }
    // Hips, torso (a coat reaches down over the thighs).
    b.cuboid(
        body,
        Vec3::new(0.0, 0.05, 0.0),
        Vec3::new(0.17, 0.08, 0.1),
        l.bottom,
    );
    let shoulder_w = l.shoulders;
    b.cuboid(
        body,
        Vec3::new(0.0, 0.33, 0.0),
        Vec3::new(shoulder_w / 2.0, 0.27, 0.11),
        l.top,
    );
    if l.long_coat {
        b.cuboid(
            body,
            Vec3::new(0.0, -0.12, 0.0),
            Vec3::new(shoulder_w / 2.0 + 0.01, 0.2, 0.115),
            l.top,
        );
    }
    let neck_y = 0.6;
    if near {
        b.cuboid(
            body,
            Vec3::new(0.0, neck_y + 0.03, 0.0),
            Vec3::new(0.05, 0.04, 0.05),
            l.skin,
        );
    }
    // Head, hair, hat.
    let head = body * Affine3A::from_translation(Vec3::new(0.0, neck_y + 0.17, 0.0));
    b.cuboid(head, Vec3::ZERO, Vec3::new(0.085, 0.11, 0.1), l.skin);
    b.cuboid(
        head,
        Vec3::new(0.0, 0.085, 0.015),
        Vec3::new(0.092, 0.035, 0.105),
        l.hair,
    );
    if l.long_hair {
        b.cuboid(
            head,
            Vec3::new(0.0, -0.04, 0.085),
            Vec3::new(0.09, 0.12, 0.03),
            l.hair,
        );
    }
    if let Some(hat) = l.hat {
        b.cuboid(
            head,
            Vec3::new(0.0, 0.12, 0.01),
            Vec3::new(0.095, 0.045, 0.11),
            hat,
        );
    }
    // Arms.
    let sleeves = if l.short_sleeves { l.skin } else { l.top };
    for (side, x) in [(0usize, -1.0f32), (1, 1.0)] {
        let sx = x * (shoulder_w / 2.0 + 0.05);
        let arm = body
            * Affine3A::from_translation(Vec3::new(sx, 0.56, 0.0))
            * Affine3A::from_rotation_x(pose.shoulder[side] + pose.lean);
        if near {
            b.cuboid(
                arm,
                Vec3::new(0.0, -0.15, 0.0),
                Vec3::new(0.045, 0.16, 0.05),
                l.top,
            );
            let elbow = arm
                * Affine3A::from_translation(Vec3::new(0.0, -0.3, 0.0))
                * Affine3A::from_rotation_x(pose.elbow[side]);
            b.cuboid(
                elbow,
                Vec3::new(0.0, -0.13, 0.0),
                Vec3::new(0.04, 0.14, 0.045),
                sleeves,
            );
            b.cuboid(
                elbow,
                Vec3::new(0.0, -0.3, 0.0),
                Vec3::new(0.035, 0.045, 0.045),
                l.skin,
            );
            if side == 1
                && let Some(bag) = l.bag
            {
                let hand = elbow
                    * Affine3A::from_translation(Vec3::new(0.0, -0.33, 0.0))
                    * Affine3A::from_rotation_x(-pose.elbow[side] - pose.shoulder[side]);
                b.cuboid(
                    hand,
                    Vec3::new(0.02, -0.15, 0.0),
                    Vec3::new(0.05, 0.14, 0.16),
                    bag,
                );
            }
        } else {
            b.cuboid(
                arm,
                Vec3::new(0.0, -0.29, 0.0),
                Vec3::new(0.045, 0.3, 0.05),
                l.top,
            );
        }
    }
    b.mesh()
}

/// The catalogue of one season: [`LOOKS`] looks, each [`SLOTS`] meshes.
#[derive(Resource)]
pub struct Figures {
    pub meshes: Vec<[Handle<Mesh>; SLOTS]>,
    /// The opaque material, and fading ones for the edge of the radius.
    pub material: Handle<StandardMaterial>,
    pub fades: Vec<Handle<StandardMaterial>>,
    pub vertices: usize,
}

/// Alpha of the fading materials, from nearly gone to nearly whole.
pub const FADE_LEVELS: [f32; 4] = [0.2, 0.4, 0.6, 0.8];

impl Figures {
    pub fn build(
        season: u32,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
    ) -> Figures {
        let mut out = Vec::new();
        let mut vertices = 0;
        for i in 0..LOOKS {
            let l = look(i, season);
            let mut add = |m: Mesh| {
                vertices += m.count_vertices();
                meshes.add(m)
            };
            let mut slots: Vec<Handle<Mesh>> = Vec::with_capacity(SLOTS);
            for k in 0..WALK_NEAR {
                slots.push(add(figure(&l, &walk(k as f32 / WALK_NEAR as f32), true)));
            }
            for k in 0..WALK_FAR {
                slots.push(add(figure(&l, &walk(k as f32 / WALK_FAR as f32), false)));
            }
            slots.push(add(figure(&l, &stand(), true)));
            slots.push(add(figure(&l, &sit(), true)));
            slots.push(add(figure(&l, &stand(), false)));
            slots.push(add(figure(&l, &sit(), false)));
            out.push(slots.try_into().unwrap_or_else(|_| unreachable!()));
        }
        let base = StandardMaterial {
            base_color: Color::WHITE,
            perceptual_roughness: 0.85,
            reflectance: 0.3,
            ..default()
        };
        let material = materials.add(base.clone());
        let fades = FADE_LEVELS
            .iter()
            .map(|&a| {
                materials.add(StandardMaterial {
                    base_color: Color::srgba(1.0, 1.0, 1.0, a),
                    alpha_mode: AlphaMode::Blend,
                    ..base.clone()
                })
            })
            .collect();
        Figures {
            meshes: out,
            material,
            fades,
            vertices,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The planted foot moves back at the walking speed: over the standing
    /// half of the cycle the hip passes over it by half a stride.
    #[test]
    fn a_stride_matches_the_population_function() {
        let a = SWING_DEG.to_radians();
        let step = 2.0 * HIP * a.sin();
        // population.rs: stride = 1380 mm for a 1.75 m person.
        assert!((2.0 * step - 1.38).abs() < 0.01, "{}", 2.0 * step);
        // Foot under the hip: forward at the strike, back at toe-off.
        let p0 = walk(0.0);
        let p1 = walk(0.499);
        assert!(p0.thigh[0] > 0.3 && p1.thigh[0] < -0.3);
        // The hip is lowest at double support, highest mid-stance.
        assert!(walk(0.25).hip_y > walk(0.0).hip_y);
    }

    #[test]
    fn looks_vary_and_follow_the_season() {
        let winter: Vec<_> = (0..LOOKS).map(|i| look(i, 0)).collect();
        let summer: Vec<_> = (0..LOOKS).map(|i| look(i, 2)).collect();
        assert!(winter.iter().filter(|l| l.long_coat).count() > 10);
        assert!(summer.iter().all(|l| !l.long_coat && l.hat.is_none()));
        assert!(summer.iter().filter(|l| l.shorts).count() > 5);
        let tops: std::collections::BTreeSet<_> =
            winter.iter().map(|l| l.top.map(f32::to_bits)).collect();
        assert!(tops.len() >= 10, "{}", tops.len());
    }
}
