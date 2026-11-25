//! Trees (the games note's option T1): a few procedural models generated in
//! code at start-up, per genus group (broad, small, columnar, conifer), three
//! variants each, three levels of detail cross-faded by `VisibilityRange`.
//!
//! - **Bark:** tapered tubes, fewer orders and sides per level.
//! - **Crown:** alpha-tested cards (`AlphaToCoverage`: an alpha test at 0.5
//!   under TAA, where multisampling is off, and coverage with MSAA on the
//!   fast preset), with leaves in leaf and bare twigs between leaf fall and
//!   leaf-out (MeteoSwiss's Zurich medians), needles for evergreens. Fewer,
//!   larger cards per level.
//! - **Shared:** every tree of a group and variant uses the same meshes and
//!   materials, so Bevy draws them as instances; each tree is scaled to its
//!   height and crown and turned by a hash of its id.
//! - **Shadows:** only the nearest levels cast sun shadows.

use std::f32::consts::TAU;
use std::time::Instant;

use bevy::camera::visibility::VisibilityRange;
use bevy::light::NotShadowCaster;
use bevy::prelude::*;

use super::points::{Group, Tree};
use super::shapes::MeshBuilder;
use super::textures::{self, alpha_tested_mips, box_mips};
use super::{ProtoStats, Rng, mix, seed, unit};
use crate::Options;
use crate::sky::Clock;

const VARIANTS: usize = 3;
pub const LEVELS: usize = 3;

/// The size each group's model is built at (height, crown diameter, m);
/// trees are scaled from it.
fn nominal(g: Group) -> (f32, f32) {
    match g {
        Group::Broad => (14.0, 10.0),
        Group::Small => (7.0, 6.0),
        Group::Columnar => (12.0, 3.5),
        Group::Conifer => (14.0, 6.0),
    }
}

/// How a group grows (ellipsoid crowns; conifers grow in whorls).
struct Params {
    /// Trunk top, as a fraction of the height, and its base radius (m).
    trunk_top: f32,
    trunk_r: f32,
    /// Crown ellipsoid: centre height and vertical half-axis (fractions).
    centre_y: f32,
    half_y: f32,
    scaffolds: u32,
    /// Where scaffold branches leave the trunk (fractions of the height).
    attach: (f32, f32),
    /// Scaffold elevation above the horizontal, low and high (degrees).
    elevation: (f32, f32),
    /// Children per branch, by the parent's order (1, 2).
    children: [u32; 2],
    child_angle: [f32; 2],
    child_len: [f32; 2],
    max_order: u8,
    /// Upward bend per segment, by order.
    bend: [f32; 4],
    card_size: f32,
    leader: bool,
}

fn params(g: Group) -> Params {
    match g {
        Group::Broad => Params {
            trunk_top: 0.74,
            trunk_r: 0.24,
            centre_y: 0.62,
            half_y: 0.40,
            scaffolds: 7,
            attach: (0.3, 0.72),
            elevation: (28.0, 62.0),
            children: [5, 3],
            child_angle: [42.0, 45.0],
            child_len: [0.55, 0.55],
            max_order: 3,
            bend: [0.0, 0.10, 0.06, 0.03],
            card_size: 1.7,
            leader: true,
        },
        Group::Small => Params {
            trunk_top: 0.42,
            trunk_r: 0.13,
            centre_y: 0.63,
            half_y: 0.38,
            scaffolds: 4,
            attach: (0.3, 0.42),
            elevation: (38.0, 66.0),
            children: [4, 3],
            child_angle: [40.0, 45.0],
            child_len: [0.6, 0.55],
            max_order: 3,
            bend: [0.0, 0.08, 0.05, 0.03],
            card_size: 1.15,
            leader: true,
        },
        Group::Columnar => Params {
            trunk_top: 0.93,
            trunk_r: 0.19,
            centre_y: 0.55,
            half_y: 0.47,
            scaffolds: 14,
            attach: (0.14, 0.86),
            elevation: (60.0, 78.0),
            children: [3, 2],
            child_angle: [30.0, 35.0],
            child_len: [0.6, 0.5],
            max_order: 2,
            bend: [0.0, 0.12, 0.06, 0.0],
            card_size: 1.0,
            leader: false,
        },
        Group::Conifer => Params {
            trunk_top: 1.0,
            trunk_r: 0.17,
            centre_y: 0.47,
            half_y: 0.52,
            scaffolds: 0,
            attach: (0.08, 0.97),
            elevation: (-14.0, 18.0),
            children: [2, 0],
            child_angle: [55.0, 0.0],
            child_len: [0.35, 0.0],
            max_order: 2,
            bend: [0.0, 0.0, 0.0, 0.0],
            card_size: 1.15,
            leader: false,
        },
    }
}

struct Seg {
    a: Vec3,
    b: Vec3,
    ra: f32,
    rb: f32,
    order: u8,
}

struct Card {
    p: Vec3,
    n: Vec3,
    spin: f32,
    size: f32,
    /// At the end of a branch: the cards a bare crown keeps, for its twigs.
    tip: bool,
}

struct Skeleton {
    segs: Vec<Seg>,
    cards: Vec<Card>,
    centre: Vec3,
    radii: Vec3,
}

/// How far from `p` along `d` the crown ellipsoid ends (0 if outside).
fn room(sk: &Skeleton, p: Vec3, d: Vec3) -> f32 {
    let q = (p - sk.centre) / sk.radii;
    let e = d / sk.radii;
    let (a, b, k) = (e.dot(e), q.dot(e), q.dot(q) - 1.0);
    let disc = b * b - a * k;
    if disc <= 0.0 {
        return 0.0;
    }
    ((-b + disc.sqrt()) / a).max(0.0)
}

fn random_dir(rng: &mut Rng) -> Vec3 {
    let z = rng.range(-1.0, 1.0);
    let phi = rng.range(0.0, TAU);
    let r = (1.0 - z * z).max(0.0).sqrt();
    Vec3::new(r * phi.cos(), z, r * phi.sin())
}

/// A point, direction and radius a fraction `f` along a polyline.
fn along(pts: &[Vec3], f: f32, r0: f32, r1: f32) -> (Vec3, Vec3, f32) {
    let n = pts.len() - 1;
    let x = (f * n as f32).clamp(0.0, n as f32 - 1e-3);
    let k = x.floor() as usize;
    let t = x - k as f32;
    let p = pts[k].lerp(pts[k + 1], t);
    let d = (pts[k + 1] - pts[k]).normalize_or(Vec3::Y);
    (p, d, r0 + (r1 - r0) * f)
}

#[allow(clippy::too_many_arguments)]
fn grow(
    sk: &mut Skeleton,
    rng: &mut Rng,
    p: &Params,
    start: Vec3,
    dir: Vec3,
    len: f32,
    r0: f32,
    order: u8,
) {
    let steps = if order <= 1 { 3 } else { 2 };
    let r1 = (r0 * 0.4).max(0.006);
    let mut pts = vec![start];
    let mut d = dir;
    for k in 0..steps {
        let wiggle = random_dir(rng) * 0.12;
        d = (d + Vec3::Y * p.bend[order as usize] + wiggle).normalize_or(d);
        pts.push(pts[k] + d * (len / steps as f32));
    }
    for k in 0..steps {
        let (t0, t1) = (k as f32 / steps as f32, (k + 1) as f32 / steps as f32);
        sk.segs.push(Seg {
            a: pts[k],
            b: pts[k + 1],
            ra: r0 + (r1 - r0) * t0,
            rb: r0 + (r1 - r0) * t1,
            order,
        });
    }
    if order >= p.max_order {
        // Leaves (or twigs) round the end and the middle of the finest
        // branches.
        for (k, scale) in [(steps, 1.0), (steps / 2, 0.8)] {
            sk.cards.push(Card {
                p: pts[k],
                n: random_dir(rng),
                spin: rng.range(0.0, TAU),
                size: p.card_size * scale * rng.range(0.85, 1.15),
                tip: k == steps,
            });
        }
        return;
    }
    let n = p.children[order as usize - 1];
    for i in 0..n {
        let f = 0.25 + 0.7 * (i as f32 + rng.range(0.0, 0.8)) / n as f32;
        let (at, at_dir, at_r) = along(&pts, f, r0, r1);
        let (u, w) = at_dir.any_orthonormal_pair();
        let phi = i as f32 * 2.4 + rng.range(-0.5, 0.5);
        let ang = (p.child_angle[order as usize - 1] + rng.range(-10.0, 10.0)).to_radians();
        let side = u * phi.cos() + w * phi.sin();
        let cd = (at_dir * ang.cos() + side * ang.sin() + Vec3::Y * 0.15).normalize_or(at_dir);
        let space = room(sk, at, cd);
        let cl = (p.child_len[order as usize - 1] * len * (1.0 - 0.45 * f)).min(space * 0.95);
        if cl < 0.15 {
            continue;
        }
        grow(sk, rng, p, at, cd, cl, (at_r * 0.62).max(0.006), order + 1);
    }
}

/// A trunk as a polyline from below ground to `top`, leaning a little.
fn trunk(sk: &mut Skeleton, rng: &mut Rng, height: f32, p: &Params) -> Vec<Vec3> {
    let top = Vec3::new(
        rng.range(-0.03, 0.03) * height,
        p.trunk_top * height,
        rng.range(-0.03, 0.03) * height,
    );
    let steps = 5;
    let pts: Vec<Vec3> = (0..=steps)
        .map(|k| {
            let t = k as f32 / steps as f32;
            let wobble = Vec3::new(rng.range(-1.0, 1.0), 0.0, rng.range(-1.0, 1.0))
                * 0.02
                * height
                * t
                * (1.0 - t);
            Vec3::new(0.0, -0.5, 0.0).lerp(top, t) + wobble
        })
        .collect();
    let r_top = p.trunk_r * 0.35;
    for k in 0..steps {
        let (t0, t1) = (k as f32 / steps as f32, (k + 1) as f32 / steps as f32);
        // A little flare at the root.
        let flare = |t: f32| if t < 0.1 { 1.25 - 2.5 * t } else { 1.0 };
        sk.segs.push(Seg {
            a: pts[k],
            b: pts[k + 1],
            ra: (p.trunk_r + (r_top - p.trunk_r) * t0) * flare(t0),
            rb: (p.trunk_r + (r_top - p.trunk_r) * t1) * flare(t1),
            order: 0,
        });
    }
    pts
}

fn skeleton(g: Group, variant: usize) -> Skeleton {
    let (height, crown) = nominal(g);
    let p = params(g);
    let mut rng = Rng::new(mix(0x7ee5, (g.index() * 16 + variant) as u64));
    let mut sk = Skeleton {
        segs: Vec::new(),
        cards: Vec::new(),
        centre: Vec3::new(0.0, p.centre_y * height, 0.0),
        radii: Vec3::new(crown / 2.0, p.half_y * height, crown / 2.0),
    };
    let stem = trunk(&mut sk, &mut rng, height, &p);
    let r_at = |y: f32| p.trunk_r + (p.trunk_r * 0.35 - p.trunk_r) * (y / (p.trunk_top * height));
    if g == Group::Conifer {
        // Whorls of near-horizontal branches, shorter towards the top.
        let (y0, y1) = (p.attach.0 * height, p.attach.1 * height);
        let mut y = y0;
        let mut whorl = 0;
        while y < y1 {
            let t = (y - y0) / (y1 - y0);
            let reach = crown / 2.0 * (1.0 - t).powf(0.85);
            for k in 0..5 {
                let az = whorl as f32 * 0.7 + k as f32 / 5.0 * TAU + rng.range(-0.3, 0.3);
                let el =
                    (p.elevation.0 + (p.elevation.1 - p.elevation.0) * t + rng.range(-6.0, 6.0))
                        .to_radians();
                let d = Vec3::new(az.cos() * el.cos(), el.sin(), az.sin() * el.cos());
                let len = (reach * rng.range(0.85, 1.05)).max(0.3);
                let (at, _, _) = along(&stem, (y + 0.5) / (height + 0.5), 0.0, 0.0);
                let r0 = (0.03 * len.powf(0.7)).clamp(0.01, 0.07);
                let end = at + d * len;
                sk.segs.push(Seg {
                    a: at,
                    b: at.lerp(end, 0.5),
                    ra: r0,
                    rb: r0 * 0.7,
                    order: 1,
                });
                let droop = end - Vec3::Y * 0.12 * len;
                sk.segs.push(Seg {
                    a: at.lerp(end, 0.5),
                    b: droop,
                    ra: r0 * 0.7,
                    rb: r0 * 0.3,
                    order: 1,
                });
                for (f, s) in [(0.4, 1.0), (0.72, -1.0)] {
                    let base = at.lerp(droop, f);
                    let side = d.cross(Vec3::Y).normalize_or(Vec3::X) * s;
                    let sd = (d * 0.6 + side * 0.8).normalize();
                    sk.segs.push(Seg {
                        a: base,
                        b: base + sd * len * 0.35,
                        ra: r0 * 0.4,
                        rb: 0.006,
                        order: 2,
                    });
                }
                for f in [0.45, 0.95] {
                    let n = (Vec3::Y + random_dir(&mut rng) * 0.55).normalize();
                    sk.cards.push(Card {
                        p: at.lerp(droop, f),
                        n,
                        spin: rng.range(0.0, TAU),
                        size: p.card_size * (0.55 + 0.6 * len / (crown / 2.0)),
                        tip: f > 0.9,
                    });
                }
            }
            whorl += 1;
            y += 0.5 + rng.range(-0.08, 0.08);
        }
        // The tip.
        sk.cards.push(Card {
            p: Vec3::new(stem[5].x, height * 0.98, stem[5].z),
            n: Vec3::X,
            spin: 0.0,
            size: p.card_size * 0.7,
            tip: true,
        });
        return sk;
    }
    let n = p.scaffolds + variant as u32;
    for i in 0..n {
        let t = (i as f32 + rng.range(0.0, 0.7)) / n as f32;
        let y = (p.attach.0 + (p.attach.1 - p.attach.0) * t) * height;
        let (at, _, _) = along(&stem, (y + 0.5) / (p.trunk_top * height + 0.5), 0.0, 0.0);
        let az = i as f32 * 2.399 + rng.range(-0.35, 0.35);
        let el = (p.elevation.0 + (p.elevation.1 - p.elevation.0) * t + rng.range(-7.0, 7.0))
            .to_radians();
        let d = Vec3::new(az.cos() * el.cos(), el.sin(), az.sin() * el.cos());
        let len = room(&sk, at, d) * rng.range(0.82, 0.95);
        if len < 0.3 {
            continue;
        }
        grow(
            &mut sk,
            &mut rng,
            &p,
            at,
            d,
            len,
            (r_at(y) * 0.55).max(0.02),
            1,
        );
    }
    if p.leader {
        let top = *stem.last().unwrap_or(&Vec3::Y);
        let d = (Vec3::Y + random_dir(&mut rng) * 0.1).normalize();
        let len = room(&sk, top, d) * 0.9;
        if len > 0.3 {
            grow(&mut sk, &mut rng, &p, top, d, len, p.trunk_r * 0.35, 1);
        }
    }
    sk
}

/// Bark of one level: finer orders and fewer sides further away.
fn bark_mesh(sk: &Skeleton, g: Group, level: usize) -> (Mesh, usize) {
    let max_order = match (g, level) {
        (Group::Conifer, 0) => 2,
        (Group::Conifer, 1) => 1,
        (Group::Conifer, _) => 0,
        (_, 0) => 3,
        (_, 1) => 2,
        _ => 1,
    };
    let sides: [u32; 4] = match level {
        0 => [8, 6, 4, 3],
        1 => [6, 4, 3, 3],
        _ => [5, 3, 3, 3],
    };
    let mut m = MeshBuilder::default();
    for s in sk.segs.iter().filter(|s| s.order <= max_order) {
        let round = (TAU * s.ra / 0.6).round().max(1.0);
        m.tube(
            s.a,
            s.b,
            s.ra,
            s.rb,
            sides[s.order as usize],
            round,
            0.0,
            1.2,
        );
    }
    let tris = m.triangles();
    (m.build(true), tris)
}

/// Crown cards of one level: every card near, a third of them half again as
/// large in the middle distance, an eighth at two and a half times far away,
/// moved in so the crown keeps its outline. A bare crown keeps only the
/// cards at branch ends, smaller, for its twigs.
fn card_mesh(sk: &Skeleton, level: usize, bare: bool) -> (Mesh, usize) {
    let (every, grow) = if bare {
        [(1usize, 0.8f32), (2, 1.1), (4, 1.6)][level]
    } else {
        [(1, 1.0), (3, 1.65), (8, 2.6)][level]
    };
    let mut m = MeshBuilder::default();
    let (c, r) = (sk.centre, sk.radii);
    let cards = sk.cards.iter().filter(|card| card.tip || !bare);
    for (i, card) in cards.enumerate() {
        if i % every != 0 {
            continue;
        }
        let out = ((card.p - c) / r).normalize_or(Vec3::Y);
        let p = card.p - out * card.size * (grow - 1.0) * 0.35;
        m.card(p, card.n, card.spin, card.size * grow, |q| {
            ((q - c) / r).normalize_or(Vec3::Y)
        });
    }
    let tris = m.triangles();
    (m.build(false), tris)
}

/// When deciduous trees are in leaf (day of the year): leaf unfolding and
/// leaf fall, the medians of MeteoSwiss's three Zurich phenology stations,
/// 2006 to 2026 (games note M3; Source: MeteoSwiss, whose terms ask for
/// that credit). Lime has no recorded leaf fall: a week after its
/// colouring.
fn phenology(genus: &str) -> (u16, u16) {
    match genus {
        // 12 April, 24 October.
        "Aesculus" => (102, 297),
        // 23 April (small- and large-leaved), about 31 October.
        "Tilia" => (113, 304),
        // Beech: 21 April, 27 October; hornbeam and oak taken alike.
        "Fagus" | "Carpinus" | "Quercus" => (111, 300),
        // Sycamore maple: 26 April; colouring 15 October.
        "Acer" => (116, 300),
        _ => (110, 300),
    }
}

/// The day of the year of the clock's date.
pub struct Season {
    doy: u16,
}

pub fn season(clock: &Clock) -> Season {
    let d = clock.local;
    let doy = crate::geo::days_from_civil(d.year, d.month, d.day)
        - crate::geo::days_from_civil(d.year, 1, 1)
        + 1;
    Season { doy: doy as u16 }
}

fn in_leaf(genus: &str, s: &Season) -> bool {
    let (out, drop) = phenology(genus);
    (out..drop).contains(&s.doy)
}

/// The prototype's tree meshes and materials.
#[derive(Resource)]
pub struct TreeAssets {
    /// By group, variant and level: bark, crown cards in leaf, bare crown.
    meshes: Vec<Vec<[[Handle<Mesh>; 3]; LEVELS]>>,
    bark: Handle<StandardMaterial>,
    /// Leaf cards by group (needles for conifers).
    foliage: [Handle<StandardMaterial>; 4],
    twigs: Handle<StandardMaterial>,
    ranges: [VisibilityRange; LEVELS],
    shadow_levels: u8,
}

/// A tree's crown cards: which mesh and material they take by season.
#[derive(Component, Clone, Copy)]
pub struct TreeCards {
    group: Group,
    variant: usize,
    level: usize,
    evergreen: bool,
    phenology: (u16, u16),
}

/// One part of a tree, for counting.
#[derive(Component)]
pub struct TreePart {
    pub level: u8,
}

pub fn build(app: &mut App) {
    app.add_systems(Startup, setup)
        .add_systems(Update, update_season);
}

fn ranges(lod: [f32; 3]) -> [VisibilityRange; LEVELS] {
    let m = |d: f32| (d * 0.1).max(2.0);
    let [d0, d1, d2] = lod;
    [
        VisibilityRange {
            start_margin: 0.0..0.0,
            end_margin: (d0 - m(d0))..(d0 + m(d0)),
            use_aabb: false,
        },
        VisibilityRange {
            start_margin: (d0 - m(d0))..(d0 + m(d0)),
            end_margin: (d1 - m(d1))..(d1 + m(d1)),
            use_aabb: false,
        },
        VisibilityRange {
            start_margin: (d1 - m(d1))..(d1 + m(d1)),
            end_margin: (d2 - m(d2))..d2,
            use_aabb: false,
        },
    ]
}

fn setup(
    mut commands: Commands,
    options: Res<Options>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut stats: ResMut<ProtoStats>,
) {
    let started = Instant::now();
    let mut all = Vec::new();
    let mut tris = Vec::new();
    for g in Group::ALL {
        let mut variants = Vec::new();
        for v in 0..VARIANTS {
            let sk = skeleton(g, v);
            let levels: [[Handle<Mesh>; 3]; LEVELS] = std::array::from_fn(|l| {
                let (bark, bt) = bark_mesh(&sk, g, l);
                let (cards, ct) = card_mesh(&sk, l, false);
                let (bare, wt) = card_mesh(&sk, l, true);
                tris.push(format!("{g:?}/{v}/{l}: {bt}+{ct}/{wt}"));
                [meshes.add(bark), meshes.add(cards), meshes.add(bare)]
            });
            variants.push(levels);
        }
        all.push(variants);
    }
    let card = |images: &mut Assets<Image>, base: textures::Rgba| {
        let (w, h) = (base.w, base.h);
        images.add(textures::image(w, h, alpha_tested_mips(base), true))
    };
    let leaf_tex = card(&mut images, textures::leaves(11));
    let needle_tex = card(&mut images, textures::needles(12));
    let twig_tex = card(&mut images, textures::twigs(13));
    let (bark_albedo, bark_normal) = textures::bark(14, [0.6, 1.2]);
    let (bw, bh) = (bark_albedo.w, bark_albedo.h);
    let bark_tex = images.add(textures::image(bw, bh, box_mips(bark_albedo, true), true));
    let bark_nrm = images.add(textures::image(bw, bh, box_mips(bark_normal, false), false));
    let foliage = |tex: &Handle<Image>, tint: [f32; 3], roughness: f32| StandardMaterial {
        base_color: Color::srgb(tint[0], tint[1], tint[2]),
        base_color_texture: Some(tex.clone()),
        alpha_mode: AlphaMode::AlphaToCoverage,
        // Both sides, without flipping the bent normals on the back.
        cull_mode: None,
        double_sided: false,
        perceptual_roughness: roughness,
        reflectance: 0.35,
        ..default()
    };
    let assets = TreeAssets {
        meshes: all,
        bark: materials.add(StandardMaterial {
            base_color_texture: Some(bark_tex),
            normal_map_texture: Some(bark_nrm),
            perceptual_roughness: 0.92,
            ..default()
        }),
        foliage: [
            materials.add(foliage(&leaf_tex, [1.0, 1.0, 1.0], 0.7)),
            materials.add(foliage(&leaf_tex, [1.08, 1.06, 0.86], 0.7)),
            materials.add(foliage(&leaf_tex, [0.92, 0.97, 0.92], 0.7)),
            materials.add(foliage(&needle_tex, [1.0, 1.0, 1.0], 0.8)),
        ],
        twigs: materials.add(foliage(&twig_tex, [1.0, 1.0, 1.0], 0.9)),
        ranges: ranges(options.proto.lod_m),
        shadow_levels: options.proto.tree_shadow_levels,
    };
    stats.tree_assets_ms = started.elapsed().as_secs_f64() * 1000.0;
    info!(
        "prototype: tree models in {:.0} ms (group/variant/level: bark+card triangles) {}",
        stats.tree_assets_ms,
        tris.join(", ")
    );
    commands.insert_resource(assets);
}

/// Spawns one tree's parts at `at` (render frame, on the ground); returns
/// the number of entities and whether its crown was narrowed to stay out of
/// a facade.
pub fn spawn(
    commands: &mut Commands,
    a: &TreeAssets,
    t: &Tree,
    at: Vec3,
    season: &Season,
) -> (usize, bool) {
    let h = seed(&t.id);
    let variant = (mix(h, 1) % VARIANTS as u64) as usize;
    let yaw = unit(mix(h, 2)) * TAU;
    let (nh, nc) = nominal(t.group);
    // Real crowns grow away from walls: keep the drawn crown out of the
    // nearest facade (its radius at most the trunk's distance to it plus a
    // little overhang at the eaves).
    let limit = 2.0 * (t.facade_m + 0.6).max(0.9);
    let crown = t.crown.min(limit);
    let sy = t.height / nh;
    let sxz = (crown / nc).clamp(sy * 0.45, sy * 1.8).min(limit / nc);
    let transform = Transform {
        translation: at,
        rotation: Quat::from_rotation_y(yaw),
        scale: Vec3::new(sxz, sy, sxz),
    };
    let g = t.group.index();
    for level in 0..LEVELS {
        let cards = TreeCards {
            group: t.group,
            variant,
            level,
            evergreen: t.evergreen,
            phenology: phenology(&t.genus),
        };
        let (card, material) = crown_parts(a, &cards, season);
        let bark = &a.meshes[g][variant][level][0];
        let mut e = commands.spawn((
            Mesh3d(bark.clone()),
            MeshMaterial3d(a.bark.clone()),
            transform,
            a.ranges[level].clone(),
            TreePart { level: level as u8 },
        ));
        if level >= a.shadow_levels as usize {
            e.insert(NotShadowCaster);
        }
        let mut e = commands.spawn((
            Mesh3d(card),
            MeshMaterial3d(material),
            transform,
            a.ranges[level].clone(),
            TreePart { level: level as u8 },
            cards,
        ));
        if level >= a.shadow_levels as usize {
            e.insert(NotShadowCaster);
        }
    }
    (2 * LEVELS, crown < t.crown - 0.01)
}

/// A crown's cards and material for the season: leaves (or needles), or
/// bare twigs.
fn crown_parts(
    a: &TreeAssets,
    c: &TreeCards,
    s: &Season,
) -> (Handle<Mesh>, Handle<StandardMaterial>) {
    let leaves = c.evergreen || (c.phenology.0..c.phenology.1).contains(&s.doy);
    let meshes = &a.meshes[c.group.index()][c.variant][c.level];
    if leaves {
        (meshes[1].clone(), a.foliage[c.group.index()].clone())
    } else {
        (meshes[2].clone(), a.twigs.clone())
    }
}

/// Leaves come and go with the date (the clock's day; time keys keep it).
fn update_season(
    clock: Res<Clock>,
    assets: Option<Res<TreeAssets>>,
    mut stats: ResMut<ProtoStats>,
    mut cards: Query<(
        &TreeCards,
        &mut Mesh3d,
        &mut MeshMaterial3d<StandardMaterial>,
    )>,
    mut last: Local<Option<u16>>,
) {
    let Some(assets) = assets else {
        return;
    };
    let s = season(&clock);
    stats.leaves_on = Some(in_leaf("", &s));
    if *last == Some(s.doy) && !cards.is_empty() {
        return;
    }
    *last = Some(s.doy);
    for (c, mut mesh, mut material) in &mut cards {
        let (want_mesh, want_material) = crown_parts(&assets, c, &s);
        if mesh.0 != want_mesh {
            mesh.0 = want_mesh;
        }
        if material.0 != want_material {
            material.0 = want_material;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_model_has_bark_and_cards_and_levels_get_lighter() {
        for g in Group::ALL {
            for v in 0..VARIANTS {
                let sk = skeleton(g, v);
                assert!(sk.cards.len() > 40, "{g:?} {v}: {} cards", sk.cards.len());
                let tris: Vec<(usize, usize)> = (0..LEVELS)
                    .map(|l| (bark_mesh(&sk, g, l).1, card_mesh(&sk, l, false).1))
                    .collect();
                assert!(
                    tris[0].0 > tris[1].0 && tris[1].0 > tris[2].0,
                    "{g:?} {tris:?}"
                );
                assert!(
                    tris[0].1 > tris[1].1 && tris[1].1 > tris[2].1,
                    "{g:?} {tris:?}"
                );
                // Branches stay inside the crown, with a little slack
                // (conifers grow to a cone, not the ellipsoid).
                for s in sk.segs.iter().filter(|_| g != Group::Conifer) {
                    let q = (s.b - sk.centre) / sk.radii;
                    assert!(q.length() < 1.25 || s.order == 0, "{g:?}: {q}");
                }
            }
        }
    }

    #[test]
    fn deciduous_trees_are_bare_in_december_and_in_leaf_in_june() {
        let at = |month, day| Season {
            doy: (crate::geo::days_from_civil(2026, month, day)
                - crate::geo::days_from_civil(2026, 1, 1)
                + 1) as u16,
        };
        for genus in ["Tilia", "Platanus", "Aesculus", "Acer", "Prunus"] {
            assert!(!in_leaf(genus, &at(12, 21)), "{genus}");
            assert!(in_leaf(genus, &at(6, 21)), "{genus}");
        }
    }
}
