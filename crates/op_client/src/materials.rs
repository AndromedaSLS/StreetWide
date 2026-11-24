//! How streamed meshes look (ADR-0012).
//!
//! Two looks, switched with F2:
//! - **detailed** (default): procedural textures from [`crate::texgen`]
//!   mapped in world space, a colour and window style per building, and
//!   windows on walls of buildings that have no contributed facade;
//! - **plain**: the world format's palette colours, exactly as the data
//!   describes the surfaces.
//!
//! The windows are decoration drawn by the client, not world data: they
//! appear only where no contributor has modelled the real facade.

use std::collections::BTreeMap;

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use op_world::glb::{Material as Palette, Primitive};

use crate::texgen::{self, FACADES, Pattern};

/// Texels per side of each generated texture.
const TEXTURE_SIZE: usize = 512;
/// Buildings lower than this get plain plaster, no windows (m).
const MIN_WINDOWED_HEIGHT_M: f64 = 4.5;
/// Walls narrower than this get no window (m).
const MIN_WINDOWED_WALL_M: f64 = 1.6;
/// Open ground flatter than this is paved; steeper ground is grass and
/// soil (cosine of 12 degrees).
const PAVED_MIN_NORMAL_Y: f64 = 0.978;

/// Facade colours of the old town (sRGB): white, cream, ochre, pink,
/// grey, pale green, sand, blue-grey.
const FACADE_TINTS: [[f32; 3]; 8] = [
    [0.96, 0.95, 0.92],
    [0.95, 0.90, 0.78],
    [0.93, 0.82, 0.62],
    [0.94, 0.84, 0.80],
    [0.84, 0.84, 0.82],
    [0.84, 0.88, 0.78],
    [0.90, 0.84, 0.72],
    [0.82, 0.86, 0.90],
];

/// Roof tile variations (multipliers, sRGB).
const ROOF_TINTS: [[f32; 3]; 4] = [
    [1.0, 1.0, 1.0],
    [0.88, 0.86, 0.86],
    [1.05, 1.0, 0.94],
    [0.82, 0.84, 0.86],
];

/// What a part of a mesh is made of in the detailed look.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Surface {
    /// Walls with windows, in one of the [`FACADES`] styles.
    Facade(usize),
    /// The ground floor of a facade style.
    GroundFloor(usize),
    Plaster,
    Roof,
    FlatRoof,
    Paving,
    Setts,
    Ground,
    Road,
    Path,
    Bridge,
    Water,
}

impl Surface {
    fn pattern(self) -> Option<Pattern> {
        Some(match self {
            Surface::Facade(s) => Pattern::Facade(FACADES[s]),
            Surface::GroundFloor(s) => Pattern::GroundFloor(s),
            Surface::Plaster => Pattern::Plaster,
            Surface::Roof => Pattern::RoofTiles,
            Surface::FlatRoof => Pattern::Gravel,
            Surface::Paving => Pattern::Paving,
            Surface::Setts => Pattern::Setts,
            Surface::Ground => Pattern::Ground,
            Surface::Road => Pattern::Asphalt,
            Surface::Path => Pattern::Cobbles,
            Surface::Bridge => Pattern::Stone,
            Surface::Water => return None,
        })
    }
}

/// How one asset should look, from its manifest entry.
#[derive(Clone, Copy, Debug)]
pub struct AssetLook {
    /// Facade style for its walls; `None` for plain plaster.
    pub style: Option<usize>,
    /// Facade colour and roof variation.
    pub tint: usize,
    pub roof_tint: usize,
}

impl AssetLook {
    /// Picks the look of a shell (or facade) from its feature id, height
    /// and whether a contributed facade covers it.
    pub fn for_building(feature_id: &str, height_m: f64, has_facade: bool) -> Self {
        let h = fnv1a(feature_id.as_bytes());
        let windowed = !has_facade && height_m >= MIN_WINDOWED_HEIGHT_M;
        AssetLook {
            style: windowed.then_some((h % FACADES.len() as u64) as usize),
            tint: ((h >> 8) % FACADE_TINTS.len() as u64) as usize,
            roof_tint: ((h >> 16) % ROOF_TINTS.len() as u64) as usize,
        }
    }

    /// Terrain, transport and anything else without buildings' variety.
    pub fn plain() -> Self {
        AssetLook {
            style: None,
            tint: 0,
            roof_tint: 0,
        }
    }
}

/// The terrain height at a region-frame point (x, z), if known.
pub type GroundFn<'a> = &'a mut dyn FnMut(f64, f64) -> Option<f64>;

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Triangles of one surface, with texture coordinates.
#[derive(Debug, Default)]
pub struct Part {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

impl Part {
    fn push(&mut self, p: [f32; 3], n: [f32; 3], uv: [f32; 2]) {
        self.indices.push(self.positions.len() as u32);
        self.positions.push(p);
        self.normals.push(n);
        self.uvs.push(uv);
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(a: [f64; 3]) -> Option<[f64; 3]> {
    let l = dot(a, a).sqrt();
    (l > 1e-12).then(|| a.map(|v| v / l))
}

/// Top-down projection, keeping the primitive's vertices and indices.
fn top(prim: &Primitive, origin: [f64; 3], period: [f32; 2]) -> Part {
    let uvs = prim
        .positions
        .iter()
        .map(|p| {
            [
                ((f64::from(p[0]) + origin[0]) / f64::from(period[0])) as f32,
                ((f64::from(p[2]) + origin[2]) / f64::from(period[1])) as f32,
            ]
        })
        .collect();
    Part {
        positions: prim.positions.clone(),
        normals: prim.normals.clone(),
        uvs,
        indices: prim.indices.clone(),
    }
}

/// Terrain: flat triangles paved with setts, the rest grass and soil. Both
/// keep the primitive's shared vertices.
fn terrain(prim: &Primitive, origin: [f64; 3]) -> Vec<(Surface, Part)> {
    let mut flat = Vec::new();
    let mut steep = Vec::new();
    for t in prim.indices.as_chunks::<3>().0 {
        let p = [t[0], t[1], t[2]].map(|i| prim.positions[i as usize].map(f64::from));
        let n = normalize(cross(sub(p[1], p[0]), sub(p[2], p[0])));
        if n.is_some_and(|n| n[1].abs() >= PAVED_MIN_NORMAL_Y) {
            flat.extend_from_slice(t);
        } else {
            steep.extend_from_slice(t);
        }
    }
    [(Surface::Setts, flat), (Surface::Ground, steep)]
        .into_iter()
        .filter(|(_, idx)| !idx.is_empty())
        .map(|(surface, idx)| {
            let period = surface.pattern().map_or([1.0, 1.0], Pattern::period_m);
            // Keep only the vertices these triangles use.
            let mut remap = BTreeMap::new();
            let mut sub_prim = Primitive {
                material: prim.material,
                positions: Vec::new(),
                normals: Vec::new(),
                indices: Vec::with_capacity(idx.len()),
            };
            for i in idx {
                let j = *remap.entry(i).or_insert_with(|| {
                    sub_prim.positions.push(prim.positions[i as usize]);
                    sub_prim.normals.push(prim.normals[i as usize]);
                    sub_prim.positions.len() as u32 - 1
                });
                sub_prim.indices.push(j);
            }
            (surface, top(&sub_prim, origin, period))
        })
        .collect()
}

/// A vertex while cutting triangles: local position, normal, and region
/// frame position.
#[derive(Clone, Copy)]
struct Corner {
    local: [f32; 3],
    normal: [f32; 3],
    world: [f64; 3],
}

fn lerp_corner(a: &Corner, b: &Corner, t: f64) -> Corner {
    let tf = t as f32;
    Corner {
        local: [0, 1, 2].map(|k| a.local[k] + (b.local[k] - a.local[k]) * tf),
        normal: [0, 1, 2].map(|k| a.normal[k] + (b.normal[k] - a.normal[k]) * tf),
        world: [0, 1, 2].map(|k| a.world[k] + (b.world[k] - a.world[k]) * t),
    }
}

/// Fan triangulation of a convex polygon.
fn triangulate(poly: &[Corner]) -> Vec<[Corner; 3]> {
    (1..poly.len().saturating_sub(1))
        .map(|k| [poly[0], poly[k], poly[k + 1]])
        .collect()
}

/// Splits a triangle by the horizontal plane at height `y` into the
/// polygon below it and the polygon above it (either may be empty).
fn cut_at(tri: [Corner; 3], y: f64) -> (Vec<Corner>, Vec<Corner>) {
    let (mut below, mut above) = (Vec::new(), Vec::new());
    for i in 0..3 {
        let (a, b) = (&tri[i], &tri[(i + 1) % 3]);
        let (da, db) = (a.world[1] - y, b.world[1] - y);
        if da < 0.0 {
            below.push(*a);
        } else {
            above.push(*a);
        }
        if (da < 0.0) != (db < 0.0) {
            let c = lerp_corner(a, b, da / (da - db));
            below.push(c);
            above.push(c);
        }
    }
    (below, above)
}

/// Walls: u runs along each wall plane, fitted to a whole number of bays
/// so windows are not cut at corners; v counts storeys from the ground at
/// the middle of the wall (else from the wall's lowest point). With
/// windows, the walls are cut at the first floor and the part below gets a
/// ground-floor style; where the ground falls away along the wall, the
/// part below street level is a plain base.
fn walls(
    prim: &Primitive,
    origin: [f64; 3],
    look: &AssetLook,
    ground: GroundFn,
) -> Vec<(Surface, Part)> {
    let tris: Vec<[usize; 3]> = prim
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| [t[0] as usize, t[1] as usize, t[2] as usize])
        .collect();
    let world = |i: usize| {
        let p = prim.positions[i];
        [
            f64::from(p[0]) + origin[0],
            f64::from(p[1]) + origin[1],
            f64::from(p[2]) + origin[2],
        ]
    };
    // Per triangle: the wall plane it lies in, if it is a wall.
    struct Plane {
        key: (i64, i64),
        tangent: [f64; 2],
    }
    let planes: Vec<Option<Plane>> = tris
        .iter()
        .map(|t| {
            let (a, b, c) = (world(t[0]), world(t[1]), world(t[2]));
            let n = normalize(cross(sub(b, a), sub(c, a)))?;
            if n[1].abs() > 0.6 {
                return None;
            }
            let mut h = [n[0], n[2]];
            let l = (h[0] * h[0] + h[1] * h[1]).sqrt();
            h = [h[0] / l, h[1] / l];
            // One orientation per plane, whichever way the face points.
            if h[0] < -1e-9 || (h[0].abs() <= 1e-9 && h[1] < 0.0) {
                h = [-h[0], -h[1]];
            }
            let azimuth = h[1].atan2(h[0]).to_degrees();
            let distance = h[0] * a[0] + h[1] * a[2];
            Some(Plane {
                key: (
                    (azimuth / 2.0).round() as i64,
                    (distance / 0.25).round() as i64,
                ),
                tangent: [-h[1], h[0]],
            })
        })
        .collect();
    let mut extent: BTreeMap<(i64, i64), (f64, f64)> = BTreeMap::new();
    // Per plane: sum of x, sum of z, count, lowest y.
    let mut spread: BTreeMap<(i64, i64), (f64, f64, f64, f64)> = BTreeMap::new();
    for (t, plane) in tris.iter().zip(&planes) {
        let Some(p) = plane else { continue };
        for &i in t {
            let w = world(i);
            let u = p.tangent[0] * w[0] + p.tangent[1] * w[2];
            let e = extent.entry(p.key).or_insert((u, u));
            e.0 = e.0.min(u);
            e.1 = e.1.max(u);
            let s = spread.entry(p.key).or_insert((0.0, 0.0, 0.0, f64::MAX));
            s.0 += w[0];
            s.1 += w[2];
            s.2 += 1.0;
            s.3 = s.3.min(w[1]);
        }
    }
    let base: BTreeMap<(i64, i64), f64> = spread
        .into_iter()
        .map(|(key, (sx, sz, n, lowest))| (key, ground(sx / n, sz / n).unwrap_or(lowest)))
        .collect();
    let [bay_w, storey_h] = match look.style {
        Some(s) => [f64::from(FACADES[s].bay_w), f64::from(FACADES[s].storey_h)],
        None => {
            let p = Pattern::Plaster.period_m();
            [f64::from(p[0]), f64::from(p[1])]
        }
    };
    let uv_of = |c: &Corner, plane: &Option<Plane>| -> [f32; 2] {
        let w = c.world;
        match plane {
            // Not a wall (a horizontal piece): one plaster texel.
            None => [0.05, 0.02],
            Some(p) => {
                let u = p.tangent[0] * w[0] + p.tangent[1] * w[2];
                let (lo, hi) = extent[&p.key];
                let len = hi - lo;
                let v = -((w[1] - base[&p.key]) / storey_h);
                if look.style.is_none() {
                    [(u / bay_w) as f32, v as f32]
                } else if len < MIN_WINDOWED_WALL_M {
                    // Too narrow for a window: plaster beside it.
                    [0.05, v as f32]
                } else {
                    let bays = (len / bay_w).round().max(1.0);
                    [((u - lo) / (len / bays)) as f32, v as f32]
                }
            }
        }
    };
    let (mut upper, mut lower) = (Part::default(), Part::default());
    for (t, plane) in tris.iter().zip(&planes) {
        let tri = t.map(|i| Corner {
            local: prim.positions[i],
            normal: prim.normals[i],
            world: world(i),
        });
        // (polygon, part, fixed texture coordinates).
        let mut pieces: Vec<(Vec<Corner>, bool, Option<[f32; 2]>)> = Vec::new();
        match (look.style, plane) {
            (Some(_), Some(p)) => {
                let street = base[&p.key];
                let (under, rest) = cut_at(tri, street);
                // Below the street line: one texel of the ground floor's
                // base, 0.2 m up at the edge of the bay.
                pieces.push((under, false, Some([0.05, -(0.2 / storey_h) as f32])));
                for poly in triangulate(&rest) {
                    let (ground_floor, above) = cut_at(poly, street + storey_h);
                    pieces.push((ground_floor, false, None));
                    pieces.push((above, true, None));
                }
            }
            _ => pieces.push((tri.to_vec(), true, None)),
        }
        for (poly, is_upper, fixed) in pieces {
            let part = if is_upper { &mut upper } else { &mut lower };
            for t in triangulate(&poly) {
                for c in &t {
                    part.push(c.local, c.normal, fixed.unwrap_or_else(|| uv_of(c, plane)));
                }
            }
        }
    }
    let (top_surface, ground_surface) = match look.style {
        Some(s) => (Surface::Facade(s), Surface::GroundFloor(s)),
        None => (Surface::Plaster, Surface::Plaster),
    };
    [(top_surface, upper), (ground_surface, lower)]
        .into_iter()
        .filter(|(_, p)| !p.indices.is_empty())
        .collect()
}

/// Roofs: tiles in rows across the slope; flat parts get gravel.
fn roofs(prim: &Primitive, origin: [f64; 3]) -> Vec<(Surface, Part)> {
    let (mut tiles, mut flat) = (Part::default(), Part::default());
    let [tu, tv] = Pattern::RoofTiles.period_m().map(f64::from);
    let [gu, gv] = Pattern::Gravel.period_m().map(f64::from);
    for t in prim.indices.as_chunks::<3>().0 {
        let w: Vec<[f64; 3]> = t
            .iter()
            .map(|&i| {
                let p = prim.positions[i as usize];
                [
                    f64::from(p[0]) + origin[0],
                    f64::from(p[1]) + origin[1],
                    f64::from(p[2]) + origin[2],
                ]
            })
            .collect();
        let Some(mut n) = normalize(cross(sub(w[1], w[0]), sub(w[2], w[0]))) else {
            continue;
        };
        if n[1] < 0.0 {
            n = n.map(|v| -v);
        }
        let steep = n[1] < 0.96;
        let axes = steep
            .then(|| {
                // Down the slope, and along it.
                let down = normalize([n[0] * n[1], n[1] * n[1] - 1.0, n[2] * n[1]])?;
                let along = normalize(cross(n, down))?;
                Some((along, down))
            })
            .flatten();
        for (k, &i) in t.iter().enumerate() {
            let p = w[k];
            let (target, uv) = match axes {
                Some((along, down)) => (&mut tiles, [dot(p, along) / tu, dot(p, down) / tv]),
                None => (&mut flat, [p[0] / gu, p[2] / gv]),
            };
            target.push(
                prim.positions[i as usize],
                prim.normals[i as usize],
                uv.map(|v| v as f32),
            );
        }
    }
    [(Surface::Roof, tiles), (Surface::FlatRoof, flat)]
        .into_iter()
        .filter(|(_, p)| !p.indices.is_empty())
        .collect()
}

/// The detailed-look parts of one primitive of an asset whose tile origin
/// is `origin` (region frame, m).
pub fn parts(
    prim: &Primitive,
    origin: [f64; 3],
    look: &AssetLook,
    ground: GroundFn,
) -> Vec<(Surface, Part)> {
    let top_with = |s: Surface| {
        let period = s.pattern().map_or([1.0, 1.0], Pattern::period_m);
        vec![(s, top(prim, origin, period))]
    };
    match prim.material {
        Palette::Wall => walls(prim, origin, look, ground),
        Palette::Roof => roofs(prim, origin),
        Palette::Ground => top_with(Surface::Paving),
        Palette::Terrain => terrain(prim, origin),
        Palette::Road => top_with(Surface::Road),
        Palette::Path => top_with(Surface::Path),
        Palette::Bridge => top_with(Surface::Bridge),
        Palette::Water => top_with(Surface::Water),
    }
}

/// A Bevy mesh with texture coordinates and tangents.
pub fn mesh(part: Part) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, part.positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, part.normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, part.uvs)
    .with_inserted_indices(Indices::U32(part.indices));
    // Degenerate input keeps no tangents and is drawn without normal mapping.
    let _ = mesh.generate_tangents();
    mesh
}

/// The materials of both looks.
#[derive(Resource)]
pub struct MaterialSet {
    plain: BTreeMap<&'static str, Handle<StandardMaterial>>,
    detailed: BTreeMap<(Surface, usize), Handle<StandardMaterial>>,
    /// Whether the detailed look is on (F2).
    pub detailed_on: bool,
}

/// Both materials of a placed part, so F2 can switch them.
#[derive(Component, Clone)]
pub struct Looks {
    pub detailed: Handle<StandardMaterial>,
    pub plain: Handle<StandardMaterial>,
}

impl MaterialSet {
    pub fn looks(&self, surface: Surface, palette: Palette, look: &AssetLook) -> Looks {
        let tint = match surface {
            Surface::Facade(_) | Surface::GroundFloor(_) | Surface::Plaster => look.tint,
            Surface::Roof => look.roof_tint,
            _ => 0,
        };
        let detailed = self
            .detailed
            .get(&(surface, tint))
            .or_else(|| self.detailed.get(&(surface, 0)))
            .unwrap_or(&self.plain[palette.name()])
            .clone();
        Looks {
            detailed,
            plain: self.plain[palette.name()].clone(),
        }
    }

    pub fn current(&self, looks: &Looks) -> Handle<StandardMaterial> {
        if self.detailed_on {
            looks.detailed.clone()
        } else {
            looks.plain.clone()
        }
    }
}

fn texture(levels: &[texgen::Level], pick: fn(&texgen::Level) -> &Vec<u8>, srgb: bool) -> Image {
    let size = levels[0].size as u32;
    let mut image = Image::new_uninit(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        if srgb {
            TextureFormat::Rgba8UnormSrgb
        } else {
            TextureFormat::Rgba8Unorm
        },
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.mip_level_count = levels.len() as u32;
    image.data = Some(
        levels
            .iter()
            .flat_map(|l| pick(l).iter().copied())
            .collect(),
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        address_mode_w: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 16,
        ..default()
    });
    image
}

fn srgb(c: [f32; 3]) -> Color {
    Color::srgb(c[0], c[1], c[2])
}

/// Generates every texture and material (about a second of CPU time).
pub fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let plain = [
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

    let surfaces = [
        Surface::Facade(0),
        Surface::Facade(1),
        Surface::Facade(2),
        Surface::GroundFloor(0),
        Surface::GroundFloor(1),
        Surface::GroundFloor(2),
        Surface::Plaster,
        Surface::Roof,
        Surface::FlatRoof,
        Surface::Paving,
        Surface::Setts,
        Surface::Ground,
        Surface::Road,
        Surface::Path,
        Surface::Bridge,
    ];
    let started = std::time::Instant::now();
    let mut detailed = BTreeMap::new();
    for surface in surfaces {
        let pattern = surface.pattern().expect("textured surface");
        let levels =
            texgen::mip_chain(&texgen::generate(pattern, TEXTURE_SIZE), pattern.period_m());
        let albedo = images.add(texture(&levels, |l| &l.albedo, true));
        let normal = images.add(texture(&levels, |l| &l.normal, false));
        let rough = images.add(texture(&levels, |l| &l.roughness, false));
        let tints: Vec<Color> = match surface {
            Surface::Facade(_) | Surface::GroundFloor(_) | Surface::Plaster => {
                FACADE_TINTS.iter().map(|c| srgb(*c)).collect()
            }
            Surface::Roof => ROOF_TINTS.iter().map(|c| srgb(*c)).collect(),
            _ => vec![Color::WHITE],
        };
        for (i, tint) in tints.into_iter().enumerate() {
            let handle = materials.add(StandardMaterial {
                base_color: tint,
                base_color_texture: Some(albedo.clone()),
                normal_map_texture: Some(normal.clone()),
                metallic_roughness_texture: Some(rough.clone()),
                perceptual_roughness: 1.0,
                metallic: 0.0,
                double_sided: true,
                cull_mode: None,
                ..default()
            });
            detailed.insert((surface, i), handle);
        }
    }
    info!(
        "generated {} surface textures in {:.0} ms",
        surfaces.len(),
        started.elapsed().as_secs_f64() * 1000.0
    );
    commands.insert_resource(MaterialSet {
        plain,
        detailed,
        detailed_on: true,
    });
}

/// F2: switches every placed part between the two looks.
pub fn apply_look(
    set: Res<MaterialSet>,
    mut parts: Query<(&Looks, &mut MeshMaterial3d<StandardMaterial>)>,
) {
    if !set.is_changed() {
        return;
    }
    for (looks, mut material) in &mut parts {
        material.0 = set.current(looks);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box-shaped building: 10 m × 6 m, 12 m high, with a pitched roof.
    fn wall_prim(len: f32) -> Primitive {
        // One wall facing south (+Z), from x = 0 to `len`, 0 to 12 m high.
        Primitive {
            material: Palette::Wall,
            positions: vec![
                [0.0, 0.0, 0.0],
                [len, 0.0, 0.0],
                [len, 12.0, 0.0],
                [0.0, 12.0, 0.0],
            ],
            normals: vec![[0.0, 0.0, 1.0]; 4],
            indices: vec![0, 1, 2, 0, 2, 3],
        }
    }

    #[test]
    fn walls_fit_a_whole_number_of_bays() {
        let look = AssetLook {
            style: Some(0),
            tint: 0,
            roof_tint: 0,
        };
        // Tile origin far from zero, like real tiles, at 400 m; no terrain
        // known, so storeys count from the bottom of the wall.
        let origin = [1234.5, 400.0, -987.25];
        let out = walls(&wall_prim(10.0), origin, &look, &mut |_, _| None);
        let surfaces: Vec<Surface> = out.iter().map(|(s, _)| *s).collect();
        assert_eq!(surfaces, [Surface::Facade(0), Surface::GroundFloor(0)]);
        // Cut at the first floor, 3.1 m up.
        let (_, ground) = &out[1];
        assert!(ground.positions.iter().all(|p| p[1] <= 3.1 + 1e-4));
        assert!(out[0].1.positions.iter().all(|p| p[1] >= 3.1 - 1e-4));
        let part: Part = Part {
            positions: [&out[0].1.positions[..], &ground.positions[..]].concat(),
            normals: Vec::new(),
            uvs: [&out[0].1.uvs[..], &ground.uvs[..]].concat(),
            indices: Vec::new(),
        };
        let us: Vec<f32> = part.uvs.iter().map(|uv| uv[0]).collect();
        let (lo, hi) = us
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), u| (a.min(*u), b.max(*u)));
        // 10 m / 2.6 m per bay rounds to 4 bays.
        assert!(lo.abs() < 1e-4 && (hi - 4.0).abs() < 1e-4, "{lo} {hi}");
        // v counts storeys down from the base: 12 m = -12/3.1.
        let vs: Vec<f32> = part.uvs.iter().map(|uv| uv[1]).collect();
        assert!(vs.iter().any(|v| v.abs() < 1e-5));
        assert!(vs.iter().any(|v| (v + 12.0 / 3.1).abs() < 1e-4));
    }

    /// On a slope the wall reaches below the street: storeys count from the
    /// ground at the wall, so the ground floor is at street level.
    #[test]
    fn storeys_count_from_the_ground_at_the_wall() {
        let look = AssetLook {
            style: Some(2),
            ..AssetLook::plain()
        };
        // The wall runs from 0 to 12 m; the street in front is at 5 m.
        let out = walls(&wall_prim(10.0), [0.0; 3], &look, &mut |_, _| Some(5.0));
        let (s, ground_floor) = &out[1];
        assert_eq!(*s, Surface::GroundFloor(2));
        let ys: Vec<f32> = ground_floor.positions.iter().map(|p| p[1]).collect();
        assert!(
            ys.iter().all(|y| *y <= 8.0 + 1e-4),
            "cut at 5 + 3.0 m: {ys:?}"
        );
        // Below the street (0 to 5 m): one texel of the base.
        let below: Vec<usize> = (0..ground_floor.positions.len())
            .filter(|&i| ground_floor.positions[i][1] < 5.0 - 1e-4)
            .collect();
        assert!(!below.is_empty());
        for i in below {
            assert_eq!(ground_floor.uvs[i], [0.05, -0.2 / 3.0]);
        }
        // v is 0 at the street, so the ground floor's texture starts there.
        let at_street: Vec<f32> = (0..ground_floor.positions.len())
            .filter(|&i| (ground_floor.positions[i][1] - 5.0).abs() < 1e-4)
            .map(|i| ground_floor.uvs[i][1])
            .collect();
        assert!(at_street.iter().any(|v| v.abs() < 1e-4), "{at_street:?}");
    }

    #[test]
    fn narrow_walls_and_low_buildings_get_no_windows() {
        let look = AssetLook::for_building("test:a", 3.0, false);
        assert_eq!(look.style, None);
        let look = AssetLook::for_building("test:a", 12.0, true);
        assert_eq!(
            look.style, None,
            "a contributed facade replaces the windows"
        );
        let windowed = AssetLook {
            style: Some(1),
            ..AssetLook::plain()
        };
        for (_, part) in walls(&wall_prim(1.2), [0.0; 3], &windowed, &mut |_, _| None) {
            assert!(part.uvs.iter().all(|uv| uv[0] == 0.05));
        }
    }

    #[test]
    fn roof_tiles_run_down_the_slope() {
        // A 45 degree roof face rising to the north (-Z).
        let prim = Primitive {
            material: Palette::Roof,
            positions: vec![
                [0.0, 10.0, 0.0],
                [4.0, 10.0, 0.0],
                [4.0, 14.0, -4.0],
                [0.0, 14.0, -4.0],
            ],
            normals: vec![
                [
                    0.0,
                    std::f32::consts::FRAC_1_SQRT_2,
                    std::f32::consts::FRAC_1_SQRT_2
                ];
                4
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
        };
        let out = roofs(&prim, [0.0; 3]);
        assert_eq!(out.len(), 1);
        let (surface, part) = &out[0];
        assert_eq!(*surface, Surface::Roof);
        // The eave (y = 10) is further down the slope than the ridge.
        let v_eave = part.uvs[0][1];
        let v_ridge = part.uvs[2][1];
        assert!(v_eave > v_ridge, "{v_eave} {v_ridge}");
        // Along the eave only u changes.
        assert!((part.uvs[0][1] - part.uvs[1][1]).abs() < 1e-5);
        assert!(((part.uvs[1][0] - part.uvs[0][0]).abs() - 4.0 / 1.08).abs() < 1e-4);
    }

    #[test]
    fn flat_terrain_is_paved_and_slopes_are_green() {
        // Two triangles: one flat, one at 45 degrees, sharing an edge.
        let prim = Primitive {
            material: Palette::Terrain,
            positions: vec![
                [0.0, 0.0, 0.0],
                [10.0, 0.0, 0.0],
                [10.0, 0.0, 10.0],
                [10.0, 10.0, 20.0],
            ],
            normals: vec![[0.0, 1.0, 0.0]; 4],
            indices: vec![0, 2, 1, 1, 2, 3],
        };
        let out = terrain(&prim, [100.0, 0.0, 200.0]);
        let surfaces: Vec<Surface> = out.iter().map(|(s, _)| *s).collect();
        assert_eq!(surfaces, [Surface::Setts, Surface::Ground]);
        for (_, part) in &out {
            assert_eq!(part.indices.len(), 3);
            assert_eq!(part.positions.len(), 3, "only the vertices it uses");
        }
        // World-space mapping: the vertex at x = 10, z = 0 of the tile at
        // (100, 200) is at 110 m / 2.4 m.
        let (_, flat) = &out[0];
        let i = flat
            .positions
            .iter()
            .position(|p| *p == [10.0, 0.0, 0.0])
            .unwrap();
        assert!((flat.uvs[i][0] - 110.0 / 2.4).abs() < 1e-4);
    }

    #[test]
    fn building_looks_are_stable() {
        let a = AssetLook::for_building("swisstopo:swissbuildings3d:x", 15.0, false);
        let b = AssetLook::for_building("swisstopo:swissbuildings3d:x", 15.0, false);
        assert_eq!(
            (a.style, a.tint, a.roof_tint),
            (b.style, b.tint, b.roof_tint)
        );
        assert!(a.style.is_some());
    }
}
