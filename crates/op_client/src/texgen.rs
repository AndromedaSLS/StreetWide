//! Procedural surface textures: plaster with windows, roof tiles,
//! cobblestones, asphalt, paving, gravel, stone and ground.
//!
//! Everything is generated from code at start-up, so the client ships no
//! image files: no downloads, and no licence beyond the code's own
//! (ADR-0012). Each pattern tiles seamlessly and covers a known physical
//! size; the client maps it in world space, so a cobble is the same size
//! everywhere.
//!
//! Output per pattern: linear albedo, height (m) and roughness per texel,
//! turned into an sRGB albedo map, a tangent-space normal map and a
//! roughness map, each with a full mip chain.

/// A facade style: one bay (window and the wall around it) of one storey.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Facade {
    pub bay_w: f32,
    pub storey_h: f32,
    pub win_w: f32,
    pub win_bottom: f32,
    pub win_h: f32,
    /// Shutter colour (sRGB), if the windows have shutters.
    pub shutters: Option<[f32; 3]>,
    /// A stone surround around each window and a band at floor level.
    pub surround: bool,
}

/// The three facade styles, picked per building.
pub const FACADES: [Facade; 3] = [
    Facade {
        bay_w: 2.6,
        storey_h: 3.1,
        win_w: 1.05,
        win_bottom: 0.85,
        win_h: 1.55,
        shutters: None,
        surround: false,
    },
    Facade {
        bay_w: 2.9,
        storey_h: 3.2,
        win_w: 1.0,
        win_bottom: 0.9,
        win_h: 1.6,
        shutters: Some([0.20, 0.36, 0.24]),
        surround: false,
    },
    Facade {
        bay_w: 2.4,
        storey_h: 3.0,
        win_w: 0.95,
        win_bottom: 0.85,
        win_h: 1.5,
        shutters: None,
        surround: true,
    },
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pattern {
    Facade(Facade),
    /// The ground floor under a facade style (same bay width): shop
    /// windows, doors and windows, or rusticated stone.
    GroundFloor(usize),
    /// Plain plaster: walls too small for windows.
    Plaster,
    /// Clay "beaver tail" tiles, rows across the slope.
    RoofTiles,
    /// Gravel on flat roofs.
    Gravel,
    Asphalt,
    Cobbles,
    /// Paving slabs (building ground surfaces).
    Paving,
    /// Granite setts in rows: the old town's squares and lanes.
    Setts,
    /// Open ground: grass and soil.
    Ground,
    /// Stone slabs (bridges).
    Stone,
}

impl Pattern {
    /// Physical size of one repeat (m): along the texture's u and v.
    pub fn period_m(self) -> [f32; 2] {
        match self {
            Pattern::Facade(f) => [f.bay_w, f.storey_h],
            Pattern::GroundFloor(s) => [FACADES[s].bay_w, FACADES[s].storey_h],
            Pattern::Plaster => [4.0, 4.0],
            Pattern::RoofTiles => [1.08, 0.9],
            Pattern::Gravel => [3.0, 3.0],
            Pattern::Asphalt => [6.0, 6.0],
            Pattern::Cobbles => [2.0, 2.0],
            Pattern::Paving => [2.4, 2.4],
            Pattern::Setts => [2.4, 2.4],
            Pattern::Ground => [16.0, 16.0],
            Pattern::Stone => [2.0, 2.0],
        }
    }
}

/// Generated texels, row 0 at the top (the "up" side of the pattern).
pub struct Texels {
    pub size: usize,
    pub albedo: Vec<[f32; 3]>,
    pub height: Vec<f32>,
    pub roughness: Vec<f32>,
}

/// One mip level, ready to upload.
pub struct Level {
    pub size: usize,
    /// sRGB, RGBA8.
    pub albedo: Vec<u8>,
    /// Tangent space, RGBA8, OpenGL convention (green up), as glTF uses.
    pub normal: Vec<u8>,
    /// glTF metallic-roughness: G = roughness, B = metallic (0).
    pub roughness: Vec<u8>,
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn lin(c: [f32; 3]) -> [f32; 3] {
    c.map(srgb_to_linear)
}

fn scale(c: [f32; 3], k: f32) -> [f32; 3] {
    c.map(|v| v * k)
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn hash(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    h
}

/// Uniform in [0, 1).
fn rand(x: i32, y: i32, seed: u32) -> f32 {
    (hash(x, y, seed) >> 8) as f32 / (1u32 << 24) as f32
}

/// Value noise on a lattice of `cells` × `cells` that wraps, so it tiles
/// over u, v in [0, 1).
fn noise(u: f32, v: f32, cells: i32, seed: u32) -> f32 {
    let x = u * cells as f32;
    let y = v * cells as f32;
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let w = |i: i32| i.rem_euclid(cells);
    let (xi, yi) = (x0 as i32, y0 as i32);
    let r = |dx: i32, dy: i32| rand(w(xi + dx), w(yi + dy), seed);
    let a = r(0, 0) + (r(1, 0) - r(0, 0)) * sx;
    let b = r(0, 1) + (r(1, 1) - r(0, 1)) * sx;
    a + (b - a) * sy
}

/// Fractal noise in about [0, 1].
fn fbm(u: f32, v: f32, cells: i32, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut total, mut c) = (0.0, 0.5, 0.0, cells);
    for o in 0..octaves {
        sum += amp * noise(u, v, c, seed.wrapping_add(o * 7919));
        total += amp;
        amp *= 0.5;
        c *= 2;
    }
    sum / total
}

/// One texel's material.
struct Texel {
    albedo: [f32; 3],
    height: f32,
    roughness: f32,
}

fn plaster(u: f32, v: f32, seed: u32) -> Texel {
    let grain = fbm(u, v, 64, 3, seed);
    let blotch = fbm(u, v, 4, 3, seed + 1);
    let k = 0.86 + 0.08 * grain + 0.06 * blotch;
    Texel {
        albedo: [k, k * 0.995, k * 0.985],
        height: 0.0015 * grain,
        roughness: 0.92,
    }
}

fn facade(f: &Facade, u: f32, v: f32) -> Texel {
    let [w, h] = [f.bay_w, f.storey_h];
    let x = u * w;
    // v runs down the texture; y is the height above the floor.
    let y = (1.0 - v) * h;
    let mut t = plaster(u, v, 11);
    let wx0 = (w - f.win_w) / 2.0;
    let wx1 = wx0 + f.win_w;
    let wy0 = f.win_bottom;
    let wy1 = wy0 + f.win_h;
    let inside = |x0: f32, x1: f32, y0: f32, y1: f32| x >= x0 && x < x1 && y >= y0 && y < y1;
    if f.surround && y < 0.12 {
        // Band at floor level.
        t.albedo = scale(t.albedo, 0.93);
        t.height += 0.025;
    }
    if f.surround && inside(wx0 - 0.14, wx1 + 0.14, wy0 - 0.14, wy1 + 0.14) {
        let stone = lin([0.66, 0.62, 0.54]);
        t.albedo = scale(stone, 0.9 + 0.15 * fbm(u, v, 48, 2, 5));
        t.height = 0.02;
        t.roughness = 0.8;
    }
    if let Some(c) = f.shutters {
        let left = inside(wx0 - f.win_w / 2.0 - 0.03, wx0 - 0.03, wy0, wy1);
        let right = inside(wx1 + 0.03, wx1 + f.win_w / 2.0 + 0.03, wy0, wy1);
        if left || right {
            let slat = ((y - wy0) / 0.05).fract();
            let frame = (x - (wx0 - f.win_w / 2.0 - 0.03)).abs() < 0.04
                || (x - (wx0 - 0.03)).abs() < 0.04
                || (x - (wx1 + 0.03)).abs() < 0.04
                || (x - (wx1 + f.win_w / 2.0 + 0.03)).abs() < 0.04
                || y - wy0 < 0.05
                || wy1 - y < 0.05;
            t.albedo = scale(lin(c), if frame { 1.0 } else { 0.8 + 0.25 * slat });
            t.height = if frame { 0.03 } else { 0.015 + 0.008 * slat };
            t.roughness = 0.55;
        }
    }
    if inside(wx0 - 0.08, wx1 + 0.08, wy0 - 0.07, wy0) {
        // Sill.
        t.albedo = scale(lin([0.74, 0.72, 0.68]), 0.95 + 0.1 * noise(u, v, 32, 3));
        t.height = 0.045;
        t.roughness = 0.7;
    }
    if inside(wx0, wx1, wy0, wy1) {
        let mid = (wx0 + wx1) / 2.0;
        let transom = wy0 + 0.68 * f.win_h;
        let frame = x - wx0 < 0.07
            || wx1 - x < 0.07
            || y - wy0 < 0.07
            || wy1 - y < 0.07
            || (x - mid).abs() < 0.03
            || (y - transom).abs() < 0.03;
        if frame {
            t.albedo = lin([0.86, 0.85, 0.81]);
            t.height = -0.04;
            t.roughness = 0.45;
        } else {
            // Glass: dark, smooth, reflecting the sky; a little lighter
            // towards the bottom where the inside catches light.
            let k = 0.6 + 0.4 * (1.0 - (y - wy0) / f.win_h);
            t.albedo = scale(lin([0.05, 0.06, 0.07]), k);
            t.height = -0.08;
            t.roughness = 0.04;
        }
    }
    t
}

fn ground_floor(style: usize, u: f32, v: f32) -> Texel {
    let f = FACADES[style];
    let [w, h] = [f.bay_w, f.storey_h];
    let x = u * w;
    let y = (1.0 - v) * h;
    let mut t = plaster(u, v, 13);
    let inside = |x0: f32, x1: f32, y0: f32, y1: f32| x >= x0 && x < x1 && y >= y0 && y < y1;
    let glass = |y0: f32, y1: f32| Texel {
        albedo: scale(
            lin([0.05, 0.06, 0.07]),
            0.7 + 0.3 * (1.0 - (y - y0) / (y1 - y0)),
        ),
        height: -0.1,
        roughness: 0.04,
    };
    let frame = |c: [f32; 3]| Texel {
        albedo: lin(c),
        height: -0.03,
        roughness: 0.4,
    };
    match style {
        0 => {
            // Shop windows on a stone base, a sign band above.
            if y < 0.45 {
                t.albedo = scale(lin([0.55, 0.54, 0.5]), 0.9 + 0.15 * fbm(u, v, 32, 2, 7));
                t.height = 0.03;
                t.roughness = 0.8;
            }
            if inside(0.0, w, 2.7, 2.95) {
                t.albedo = scale(t.albedo, 0.85);
                t.height = 0.01;
            }
            let (x0, x1, y0, y1) = ((w - 1.9) / 2.0, (w + 1.9) / 2.0, 0.45, 2.6);
            if inside(x0, x1, y0, y1) {
                let edge = x - x0 < 0.08 || x1 - x < 0.08 || y - y0 < 0.08 || y1 - y < 0.08;
                let bar = (y - 2.05).abs() < 0.03;
                t = if edge || bar {
                    frame([0.16, 0.18, 0.17])
                } else {
                    glass(y0, y1)
                };
            }
        }
        1 => {
            // A door and a window in each bay.
            let (dx0, dx1) = (0.35, 1.3);
            let (wx0, wx1) = (1.65, 2.55);
            if inside(dx0 - 0.08, dx1 + 0.08, 0.0, 2.38) {
                t.albedo = lin([0.62, 0.6, 0.55]);
                t.height = 0.02;
                t.roughness = 0.8;
            }
            if inside(dx0, dx1, 0.0, 2.3) {
                // Wooden door with panels.
                let panel = ((x - dx0) / (dx1 - dx0) * 2.0).fract();
                let rail = ((y / 2.3) * 3.0).fract();
                let groove = panel < 0.08 || panel > 0.92 || rail < 0.06 || rail > 0.94;
                t = Texel {
                    albedo: scale(lin([0.30, 0.19, 0.11]), if groove { 0.7 } else { 1.0 }),
                    height: if groove { -0.06 } else { -0.04 },
                    roughness: 0.6,
                };
            }
            if inside(wx0, wx1, 0.9, 2.3) {
                let edge = x - wx0 < 0.07 || wx1 - x < 0.07 || y - 0.9 < 0.07 || 2.3 - y < 0.07;
                let mid = (x - (wx0 + wx1) / 2.0).abs() < 0.03;
                t = if edge || mid {
                    frame([0.86, 0.85, 0.81])
                } else {
                    glass(0.9, 2.3)
                };
            }
            if inside(wx0 - 0.08, wx1 + 0.08, 0.83, 0.9) {
                t.albedo = lin([0.74, 0.72, 0.68]);
                t.height = 0.045;
                t.roughness = 0.7;
            }
        }
        _ => {
            // Rusticated stone: grooves every 0.35 m, one window per bay.
            let course = (y / 0.35).fract();
            t.albedo = scale(lin([0.66, 0.62, 0.54]), 0.88 + 0.15 * fbm(u, v, 24, 3, 9));
            t.height = if course < 0.08 { -0.015 } else { 0.0 };
            t.roughness = 0.85;
            let (x0, x1) = ((w - f.win_w) / 2.0, (w + f.win_w) / 2.0);
            if inside(x0, x1, 0.9, 2.4) {
                let edge = x - x0 < 0.07 || x1 - x < 0.07 || y - 0.9 < 0.07 || 2.4 - y < 0.07;
                t = if edge {
                    frame([0.86, 0.85, 0.81])
                } else {
                    glass(0.9, 2.4)
                };
            }
        }
    }
    t
}

fn roof_tiles(u: f32, v: f32) -> Texel {
    // 6 tiles of 0.18 m across, 6 rows of 0.15 m down the slope.
    let (cols, rows) = (6.0, 6.0);
    let d = v * rows;
    let r = d.floor();
    let t = d - r;
    let shift = if (r as i32) % 2 == 1 { 0.5 } else { 0.0 };
    let c = u * cols + shift;
    let k = c.floor();
    let s = c - k;
    let tile_id = (k as i32).rem_euclid(cols as i32 * 2);
    let id = (tile_id, r as i32);
    // Rounded lower edge ("beaver tail").
    let edge = 1.0 - 0.3 * (1.0 - (1.0 - (2.0 * s - 1.0).powi(2)).max(0.0).sqrt());
    let gap = (s - 0.5).abs() > 0.47;
    let base = lin([0.58, 0.25, 0.15]);
    let tone = 0.75 + 0.4 * rand(id.0, id.1, 21);
    let hue = 0.08 * (rand(id.0, id.1, 22) - 0.5);
    let weather = fbm(u, v, 3, 3, 23);
    let mut albedo = [
        base[0] * tone * (1.0 + hue),
        base[1] * tone,
        base[2] * tone * (1.0 - hue),
    ];
    // Darker, greyer patches where dirt and moss collect.
    albedo = mix(
        albedo,
        lin([0.22, 0.2, 0.15]),
        smoothstep(0.55, 0.85, weather) * 0.6,
    );
    if t > edge || gap {
        // Shadow under the row above / between tiles.
        return Texel {
            albedo: scale(albedo, 0.35),
            height: 0.0,
            roughness: 0.9,
        };
    }
    Texel {
        albedo: scale(albedo, 0.85 + 0.25 * t),
        height: 0.004 + 0.014 * t,
        roughness: 0.72,
    }
}

/// Jittered-grid cells: distance to the nearest and second-nearest centre
/// (in cell units) and the nearest cell's id.
fn cells(u: f32, v: f32, n: i32, seed: u32) -> (f32, f32, (i32, i32)) {
    let (x, y) = (u * n as f32, v * n as f32);
    let (cx, cy) = (x.floor() as i32, y.floor() as i32);
    let (mut d1, mut d2, mut id) = (f32::MAX, f32::MAX, (0, 0));
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (gx, gy) = (cx + dx, cy + dy);
            let (wx, wy) = (gx.rem_euclid(n), gy.rem_euclid(n));
            let px = gx as f32 + 0.15 + 0.7 * rand(wx, wy, seed);
            let py = gy as f32 + 0.15 + 0.7 * rand(wx, wy, seed + 1);
            let d = ((px - x).powi(2) + (py - y).powi(2)).sqrt();
            if d < d1 {
                d2 = d1;
                d1 = d;
                id = (wx, wy);
            } else if d < d2 {
                d2 = d;
            }
        }
    }
    (d1, d2, id)
}

fn cobbles(u: f32, v: f32) -> Texel {
    // About 15 stones per 2 m.
    let n = 15;
    let (d1, d2, id) = cells(u, v, n, 31);
    let gap = (d2 - d1) * 2.0 / n as f32; // metres-ish across the joint
    let stone = smoothstep(0.004, 0.03, gap);
    let r = rand(id.0, id.1, 33);
    let base = if r < 0.35 {
        [0.50, 0.50, 0.52]
    } else if r < 0.7 {
        [0.58, 0.55, 0.50]
    } else {
        [0.46, 0.43, 0.40]
    };
    let tone = 0.8 + 0.35 * rand(id.0, id.1, 34) + 0.1 * noise(u, v, 128, 35);
    let mortar = lin([0.30, 0.29, 0.27]);
    Texel {
        albedo: mix(mortar, scale(lin(base), tone), stone),
        height: 0.016 * stone * (1.0 - 0.6 * d1),
        roughness: 0.95 - 0.35 * stone,
    }
}

fn asphalt(u: f32, v: f32) -> Texel {
    let grain = noise(u, v, 512, 41);
    let fine = noise(u, v, 256, 42);
    let patch = fbm(u, v, 3, 3, 43);
    let k = 0.75 + 0.3 * grain * fine + 0.25 * (patch - 0.5);
    Texel {
        albedo: scale(lin([0.27, 0.27, 0.28]), k),
        height: 0.002 * grain,
        roughness: 0.86,
    }
}

fn paving(u: f32, v: f32) -> Texel {
    // Slabs of 0.6 × 0.4 m in a running bond: 4 × 6 per repeat.
    let (x, y) = (u * 4.0, v * 6.0);
    let row = y.floor();
    let xs = x + if (row as i32) % 2 == 1 { 0.5 } else { 0.0 };
    let col = xs.floor();
    let (fx, fy) = (xs - col, y - row);
    let joint = fx < 0.01 || fx > 0.99 || fy < 0.015 || fy > 0.985;
    let id = ((col as i32).rem_euclid(8), row as i32);
    let tone = 0.85 + 0.2 * rand(id.0, id.1, 51) + 0.08 * noise(u, v, 96, 52);
    Texel {
        albedo: if joint {
            lin([0.3, 0.3, 0.29])
        } else {
            scale(lin([0.62, 0.61, 0.58]), tone)
        },
        height: if joint { -0.003 } else { 0.0 },
        roughness: 0.8,
    }
}

fn setts(u: f32, v: f32) -> Texel {
    // 20 rows of 0.12 m; along a row, stones of about 0.15 m with uneven
    // joints.
    let rows = 20.0;
    let y = v * rows;
    let row = y.floor();
    let fy = y - row;
    let r = row as i32;
    let per_row = 16;
    let x = u * per_row as f32 + 0.5 * rand(r, 0, 91);
    let k = x.floor() as i32;
    let edge = |i: i32| i as f32 + 0.25 * (rand(i.rem_euclid(per_row), r, 92) - 0.5);
    let (lo, hi) = if x < edge(k) {
        (edge(k - 1), edge(k))
    } else if x >= edge(k + 1) {
        (edge(k + 1), edge(k + 2))
    } else {
        (edge(k), edge(k + 1))
    };
    let id = ((lo.round() as i32).rem_euclid(per_row), r);
    let fx = (x - lo) / (hi - lo);
    // Distance to the joint, in metres.
    let dx = fx.min(1.0 - fx) * (hi - lo) * 2.4 / per_row as f32;
    let dy = fy.min(1.0 - fy) * 2.4 / rows;
    let d = dx.min(dy);
    let stone = smoothstep(0.003, 0.012, d);
    let tone = 0.78 + 0.35 * rand(id.0, id.1, 93) + 0.08 * noise(u, v, 128, 94);
    let base = if rand(id.0, id.1, 95) < 0.3 {
        [0.50, 0.50, 0.53]
    } else {
        [0.56, 0.54, 0.51]
    };
    Texel {
        albedo: mix(lin([0.24, 0.23, 0.22]), scale(lin(base), tone), stone),
        height: 0.01 * stone * smoothstep(0.0, 0.03, d),
        roughness: 0.92 - 0.2 * stone,
    }
}

fn stone(u: f32, v: f32) -> Texel {
    // Blocks of 0.5 × 0.25 m: 4 × 8 per repeat.
    let (x, y) = (u * 4.0, v * 8.0);
    let row = y.floor();
    let xs = x + if (row as i32) % 2 == 1 { 0.5 } else { 0.0 };
    let col = xs.floor();
    let (fx, fy) = (xs - col, y - row);
    let joint = fx < 0.015 || fx > 0.985 || fy < 0.03 || fy > 0.97;
    let id = ((col as i32).rem_euclid(8), row as i32);
    let tone = 0.85 + 0.2 * rand(id.0, id.1, 61) + 0.1 * fbm(u, v, 32, 2, 62);
    Texel {
        albedo: if joint {
            lin([0.36, 0.35, 0.33])
        } else {
            scale(lin([0.62, 0.6, 0.56]), tone)
        },
        height: if joint { -0.004 } else { 0.0 },
        roughness: 0.75,
    }
}

fn gravel(u: f32, v: f32) -> Texel {
    let n = 100;
    let (d1, _, id) = cells(u, v, n, 71);
    let tone = 0.7 + 0.5 * rand(id.0, id.1, 72);
    Texel {
        albedo: scale(lin([0.5, 0.49, 0.47]), tone * (1.0 - 0.3 * d1)),
        height: 0.006 * (1.0 - d1),
        roughness: 0.9,
    }
}

fn ground(u: f32, v: f32) -> Texel {
    let patch = fbm(u, v, 4, 4, 81);
    let fine = fbm(u, v, 128, 2, 82);
    let grass = lin([0.33, 0.40, 0.22]);
    let soil = lin([0.42, 0.37, 0.30]);
    let g = smoothstep(0.42, 0.62, patch);
    Texel {
        albedo: scale(mix(soil, grass, g), 0.8 + 0.35 * fine),
        height: 0.01 * fine,
        roughness: 0.95,
    }
}

/// Generates a pattern at `size` × `size` texels.
pub fn generate(pattern: Pattern, size: usize) -> Texels {
    let n = size * size;
    let (mut albedo, mut height, mut roughness) = (
        Vec::with_capacity(n),
        Vec::with_capacity(n),
        Vec::with_capacity(n),
    );
    for row in 0..size {
        for col in 0..size {
            let u = (col as f32 + 0.5) / size as f32;
            let v = (row as f32 + 0.5) / size as f32;
            let t = match pattern {
                Pattern::Facade(f) => facade(&f, u, v),
                Pattern::GroundFloor(s) => ground_floor(s, u, v),
                Pattern::Plaster => plaster(u, v, 11),
                Pattern::RoofTiles => roof_tiles(u, v),
                Pattern::Gravel => gravel(u, v),
                Pattern::Asphalt => asphalt(u, v),
                Pattern::Cobbles => cobbles(u, v),
                Pattern::Paving => paving(u, v),
                Pattern::Setts => setts(u, v),
                Pattern::Ground => ground(u, v),
                Pattern::Stone => stone(u, v),
            };
            albedo.push(t.albedo);
            height.push(t.height);
            roughness.push(t.roughness);
        }
    }
    Texels {
        size,
        albedo,
        height,
        roughness,
    }
}

/// Tangent-space normals from the height field (wrapping at the edges).
fn normals(t: &Texels, period_m: [f32; 2]) -> Vec<[f32; 3]> {
    let s = t.size;
    let (du, dv) = (period_m[0] / s as f32, period_m[1] / s as f32);
    let h = |c: usize, r: usize| t.height[r * s + c];
    let mut out = Vec::with_capacity(s * s);
    for r in 0..s {
        for c in 0..s {
            let (cl, cr) = ((c + s - 1) % s, (c + 1) % s);
            let (ru, rd) = ((r + s - 1) % s, (r + 1) % s);
            let dx = (h(cr, r) - h(cl, r)) / (2.0 * du);
            // Row 0 is the top: "up" is towards smaller rows.
            let dy = (h(c, ru) - h(c, rd)) / (2.0 * dv);
            let n = [-dx, -dy, 1.0];
            let l = (n[0] * n[0] + n[1] * n[1] + 1.0).sqrt();
            out.push([n[0] / l, n[1] / l, n[2] / l]);
        }
    }
    out
}

fn encode(albedo: &[[f32; 3]], normal: &[[f32; 3]], rough: &[f32], size: usize) -> Level {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let mut a = Vec::with_capacity(size * size * 4);
    let mut n = Vec::with_capacity(size * size * 4);
    let mut r = Vec::with_capacity(size * size * 4);
    for i in 0..size * size {
        let c = albedo[i];
        a.extend([
            byte(linear_to_srgb(c[0])),
            byte(linear_to_srgb(c[1])),
            byte(linear_to_srgb(c[2])),
            255,
        ]);
        let m = normal[i];
        n.extend([
            byte(m[0] * 0.5 + 0.5),
            byte(m[1] * 0.5 + 0.5),
            byte(m[2] * 0.5 + 0.5),
            255,
        ]);
        r.extend([0, byte(rough[i]), 0, 255]);
    }
    Level {
        size,
        albedo: a,
        normal: n,
        roughness: r,
    }
}

/// All mip levels, from `size` down to 1 × 1. Albedo and roughness are
/// box-filtered in linear space; normals are averaged and renormalised,
/// which flattens bumps that are too small to see.
pub fn mip_chain(t: &Texels, period_m: [f32; 2]) -> Vec<Level> {
    assert!(t.size.is_power_of_two());
    let mut albedo = t.albedo.clone();
    let mut normal = normals(t, period_m);
    let mut rough = t.roughness.clone();
    let mut size = t.size;
    let mut levels = vec![encode(&albedo, &normal, &rough, size)];
    while size > 1 {
        let half = size / 2;
        let at = |r: usize, c: usize| r * size + c;
        let mut a2 = Vec::with_capacity(half * half);
        let mut n2 = Vec::with_capacity(half * half);
        let mut r2 = Vec::with_capacity(half * half);
        for r in 0..half {
            for c in 0..half {
                let ids = [
                    at(2 * r, 2 * c),
                    at(2 * r, 2 * c + 1),
                    at(2 * r + 1, 2 * c),
                    at(2 * r + 1, 2 * c + 1),
                ];
                let mut ca = [0.0; 3];
                let mut cn = [0.0; 3];
                let mut cr = 0.0;
                for i in ids {
                    for k in 0..3 {
                        ca[k] += albedo[i][k] / 4.0;
                        cn[k] += normal[i][k] / 4.0;
                    }
                    cr += rough[i] / 4.0;
                }
                let l = (cn[0] * cn[0] + cn[1] * cn[1] + cn[2] * cn[2])
                    .sqrt()
                    .max(1e-6);
                a2.push(ca);
                n2.push([cn[0] / l, cn[1] / l, cn[2] / l]);
                r2.push(cr);
            }
        }
        albedo = a2;
        normal = n2;
        rough = r2;
        size = half;
        levels.push(encode(&albedo, &normal, &rough, size));
    }
    levels
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Pattern; 15] = [
        Pattern::Facade(FACADES[0]),
        Pattern::Facade(FACADES[1]),
        Pattern::Facade(FACADES[2]),
        Pattern::GroundFloor(0),
        Pattern::GroundFloor(1),
        Pattern::GroundFloor(2),
        Pattern::Plaster,
        Pattern::RoofTiles,
        Pattern::Gravel,
        Pattern::Asphalt,
        Pattern::Cobbles,
        Pattern::Paving,
        Pattern::Setts,
        Pattern::Ground,
        Pattern::Stone,
    ];

    #[test]
    fn patterns_are_deterministic_and_in_range() {
        for p in ALL {
            let a = generate(p, 64);
            let b = generate(p, 64);
            assert_eq!(a.albedo, b.albedo, "{p:?}");
            assert!(
                a.albedo.iter().flatten().all(|v| (0.0..=1.0).contains(v)),
                "{p:?}"
            );
            assert!(a.roughness.iter().all(|v| (0.0..=1.0).contains(v)), "{p:?}");
        }
    }

    #[test]
    fn noise_tiles_seamlessly() {
        for u in [0.0f32, 0.3, 0.77] {
            let a = noise(u, 0.0, 8, 1);
            let b = noise(u, 1.0, 8, 1);
            assert!((a - b).abs() < 1e-5, "{a} {b}");
            assert!((noise(0.0, u, 8, 1) - noise(1.0, u, 8, 1)).abs() < 1e-5);
        }
    }

    #[test]
    fn windows_are_glass_in_plaster() {
        let f = FACADES[0];
        let t = generate(Pattern::Facade(f), 256);
        let at = |x: f32, y: f32| {
            let c = ((x / f.bay_w) * 256.0) as usize;
            let r = ((1.0 - y / f.storey_h) * 256.0) as usize;
            r * 256 + c
        };
        // Middle of the lower pane: glass, smooth and dark.
        let glass = at(f.bay_w / 2.0 - 0.25, f.win_bottom + 0.4);
        assert!(t.roughness[glass] < 0.1 && t.albedo[glass][0] < 0.1);
        // Beside the window: rough, light plaster.
        let wall = at(0.2, 1.5);
        assert!(t.roughness[wall] > 0.85 && t.albedo[wall][0] > 0.7);
    }

    #[test]
    fn mip_chain_reaches_one_texel_with_unit_normals() {
        let t = generate(Pattern::Cobbles, 64);
        let levels = mip_chain(&t, Pattern::Cobbles.period_m());
        assert_eq!(
            levels.iter().map(|l| l.size).collect::<Vec<_>>(),
            [64, 32, 16, 8, 4, 2, 1]
        );
        for l in &levels {
            assert_eq!(l.albedo.len(), l.size * l.size * 4);
            for px in l.normal.chunks(4) {
                let n = [0, 1, 2].map(|i| px[i] as f32 / 255.0 * 2.0 - 1.0);
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                assert!((len - 1.0).abs() < 0.03, "{len}");
                assert!(n[2] > 0.0);
            }
        }
    }
}
