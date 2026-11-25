//! The prototype's textures, generated in code at start-up like the
//! client's others (ADR-0012: no image files): leaf clusters, needle
//! sprays and bare twigs for tree cards, bark, and the images they become.
//!
//! Card textures are alpha-tested. Box-filtered alpha would thin foliage in
//! the distance, so each mip level's alpha is rescaled to keep the coverage
//! of the full-size level (Castaño, "Computing Alpha Mipmaps", 2010), and
//! colour is averaged by alpha so transparent texels do not darken edges.

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use super::{Rng, mix, unit};

/// Texels per side of the card and bark textures.
pub const SIZE: usize = 256;
/// The alpha test's threshold (`AlphaMode::Mask(0.5)`, and what
/// `AlphaToCoverage` falls back to without multisampling).
pub const CUTOFF: f32 = 0.5;

/// Linear RGBA texels, row 0 at the top.
#[derive(Clone)]
pub struct Rgba {
    pub w: usize,
    pub h: usize,
    pub px: Vec<[f32; 4]>,
}

impl Rgba {
    pub fn new(w: usize, h: usize) -> Self {
        Rgba {
            w,
            h,
            px: vec![[0.0; 4]; w * h],
        }
    }

    /// Paints `c` (linear RGB) over the texel with coverage `a`.
    fn paint(&mut self, x: i32, y: i32, c: [f32; 3], a: f32) {
        if a <= 0.0 || x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return;
        }
        let p = &mut self.px[y as usize * self.w + x as usize];
        let a = a.min(1.0);
        for k in 0..3 {
            p[k] = p[k] * (1.0 - a) + c[k] * a;
        }
        p[3] = p[3] + a * (1.0 - p[3]);
    }

    /// A stroke from `a` to `b` (texture units, 0 to 1) of width `w`
    /// (texels), with a one-texel soft edge.
    fn stroke(&mut self, a: Vec2, b: Vec2, w: f32, c: [f32; 3]) {
        let s = self.w as f32;
        let (pa, pb) = (a * s, b * s);
        let lo = pa.min(pb) - w;
        let hi = pa.max(pb) + w;
        for y in lo.y.floor() as i32..=hi.y.ceil() as i32 {
            for x in lo.x.floor() as i32..=hi.x.ceil() as i32 {
                let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let ab = pb - pa;
                let t = ((p - pa).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                let d = (p - (pa + ab * t)).length();
                self.paint(x, y, c, (w / 2.0 - d + 0.5).clamp(0.0, 1.0));
            }
        }
    }

    /// Fraction of texels the alpha test keeps, with alpha scaled by `k`.
    fn coverage(&self, k: f32) -> f32 {
        self.px.iter().filter(|p| p[3] * k >= CUTOFF).count() as f32 / self.px.len() as f32
    }

    /// Half size, colour averaged by alpha.
    fn half(&self) -> Rgba {
        let (w, h) = ((self.w / 2).max(1), (self.h / 2).max(1));
        let mut out = Rgba::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let mut c = [0.0f32; 3];
                let (mut a, mut plain) = (0.0f32, [0.0f32; 3]);
                let mut n = 0.0;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (sx, sy) = ((2 * x + dx).min(self.w - 1), (2 * y + dy).min(self.h - 1));
                    let p = self.px[sy * self.w + sx];
                    for k in 0..3 {
                        c[k] += p[k] * p[3];
                        plain[k] += p[k];
                    }
                    a += p[3];
                    n += 1.0;
                }
                let rgb = if a > 1e-6 {
                    c.map(|v| v / a)
                } else {
                    plain.map(|v| v / n)
                };
                out.px[y * w + x] = [rgb[0], rgb[1], rgb[2], a / n];
            }
        }
        out
    }
}

fn to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn byte(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

fn encode(img: &Rgba, srgb: bool) -> Vec<u8> {
    img.px
        .iter()
        .flat_map(|p| {
            let f = |v: f32| if srgb { to_srgb(v) } else { v };
            [byte(f(p[0])), byte(f(p[1])), byte(f(p[2])), byte(p[3])]
        })
        .collect()
}

/// Mip levels of an alpha-tested texture, each level's alpha scaled so the
/// alpha test keeps the same share of texels as at full size.
pub fn alpha_tested_mips(mut base: Rgba) -> Vec<Vec<u8>> {
    // Transparent texels take the mean opaque colour, so bilinear filtering
    // at the edges does not pull in black.
    let (mut sum, mut n) = ([0.0f32; 3], 0.0f32);
    for p in &base.px {
        if p[3] >= CUTOFF {
            for k in 0..3 {
                sum[k] += p[k];
            }
            n += 1.0;
        }
    }
    let mean = sum.map(|v| v / n.max(1.0));
    for p in &mut base.px {
        let a = p[3];
        for k in 0..3 {
            p[k] = p[k] * a + mean[k] * (1.0 - a);
        }
    }
    let target = base.coverage(1.0);
    let mut levels = vec![encode(&base, true)];
    let mut level = base;
    while level.w > 1 || level.h > 1 {
        level = level.half();
        // Bisection on the scale that restores the coverage.
        let (mut lo, mut hi) = (0.0f32, 8.0f32);
        for _ in 0..20 {
            let mid = (lo + hi) / 2.0;
            if level.coverage(mid) < target {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let mut scaled = level.clone();
        for p in &mut scaled.px {
            p[3] = (p[3] * hi).min(1.0);
        }
        levels.push(encode(&scaled, true));
    }
    levels
}

/// Mip levels by plain box filtering (opaque textures, normal maps when
/// `srgb` is false, and emissive masks).
pub fn box_mips(base: Rgba, srgb: bool) -> Vec<Vec<u8>> {
    let mut levels = vec![encode(&base, srgb)];
    let mut level = base;
    while level.w > 1 || level.h > 1 {
        let (w, h) = ((level.w / 2).max(1), (level.h / 2).max(1));
        let mut out = Rgba::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let mut s = [0.0f32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (sx, sy) = ((2 * x + dx).min(level.w - 1), (2 * y + dy).min(level.h - 1));
                    let p = level.px[sy * level.w + sx];
                    for k in 0..4 {
                        s[k] += p[k] / 4.0;
                    }
                }
                out.px[y * w + x] = s;
            }
        }
        levels.push(encode(&out, srgb));
        level = out;
    }
    levels
}

/// An image from mip levels (RGBA8), repeating, trilinear and anisotropic.
pub fn image(w: usize, h: usize, levels: Vec<Vec<u8>>, srgb: bool) -> Image {
    let mut image = Image::new_uninit(
        Extent3d {
            width: w as u32,
            height: h as u32,
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
    image.data = Some(levels.into_iter().flatten().collect());
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

fn lin(c: [f32; 3]) -> [f32; 3] {
    c.map(|v| {
        if v <= 0.040_45 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    })
}

fn scale(c: [f32; 3], k: f32) -> [f32; 3] {
    c.map(|v| v * k)
}

/// A cluster of broad leaves round the card's centre, on a few short
/// twigs: about half the card is leaf.
pub fn leaves(seed: u64) -> Rgba {
    let mut img = Rgba::new(SIZE, SIZE);
    let mut rng = Rng::new(seed);
    let centre = Vec2::splat(0.5);
    let twig = lin([0.30, 0.25, 0.20]);
    let twigs: Vec<Vec2> = (0..8)
        .map(|k| {
            let a = k as f32 / 8.0 * std::f32::consts::TAU + rng.range(-0.3, 0.3);
            centre + Vec2::new(a.cos(), a.sin()) * rng.range(0.28, 0.4)
        })
        .collect();
    for t in &twigs {
        img.stroke(centre, *t, 1.6, twig);
    }
    let base = lin([0.26, 0.40, 0.12]);
    for k in 0..170 {
        // Leaves sit along the twigs, pointing out from the centre: lime
        // leaves of 8 to 12 cm on the broad trees' 1.7 m cards.
        let t = twigs[k % twigs.len()];
        let along = rng.range(0.15, 1.1);
        let at = centre
            + (t - centre) * along
            + Vec2::new(rng.range(-0.07, 0.07), rng.range(-0.07, 0.07));
        let out = (at - centre).normalize_or(Vec2::Y);
        let ang = out.y.atan2(out.x) + rng.range(-1.0, 1.0);
        let len = rng.range(0.055, 0.085);
        let wid = len * rng.range(0.55, 0.7);
        let tone = rng.range(0.7, 1.25);
        let hue = rng.range(-0.08, 0.08);
        let colour = [
            base[0] * tone * (1.0 + 2.0 * hue),
            base[1] * tone,
            base[2] * tone * (1.0 - hue),
        ];
        leaf(&mut img, at, ang, len, wid, colour);
    }
    img
}

/// One broad leaf: base at `at`, pointing along `ang`.
fn leaf(img: &mut Rgba, at: Vec2, ang: f32, len: f32, wid: f32, colour: [f32; 3]) {
    let s = img.w as f32;
    let dir = Vec2::new(ang.cos(), ang.sin());
    let side = Vec2::new(-dir.y, dir.x);
    let tip = at + dir * len;
    let lo = (at.min(tip) - Vec2::splat(wid)) * s;
    let hi = (at.max(tip) + Vec2::splat(wid)) * s;
    for y in lo.y.floor() as i32..=hi.y.ceil() as i32 {
        for x in lo.x.floor() as i32..=hi.x.ceil() as i32 {
            let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) / s - at;
            let (a, b) = (p.dot(dir) / len, p.dot(side));
            if !(0.0..=1.0).contains(&a) {
                continue;
            }
            // Widest a third of the way up, pointed at the tip.
            let half = wid / 2.0
                * (std::f32::consts::PI * a.powf(0.8))
                    .sin()
                    .max(0.0)
                    .powf(0.7);
            let edge = (half - b.abs()) * s;
            if edge <= -1.0 {
                continue;
            }
            let rib = if b.abs() * s < 0.8 { 1.0 } else { 0.0 };
            let vein = 0.9 + 0.1 * (a * 40.0).sin().abs();
            let c = scale(colour, (0.85 + 0.25 * a) * vein * (1.0 + 0.25 * rib));
            img.paint(x, y, c, (edge + 0.5).clamp(0.0, 1.0));
        }
    }
}

/// Needle sprays: dark green needles on both sides of a few twigs.
pub fn needles(seed: u64) -> Rgba {
    let mut img = Rgba::new(SIZE, SIZE);
    let mut rng = Rng::new(seed);
    let centre = Vec2::splat(0.5);
    let twig = lin([0.28, 0.22, 0.16]);
    for k in 0..9 {
        let a = k as f32 / 9.0 * std::f32::consts::TAU + rng.range(-0.25, 0.25);
        let dir = Vec2::new(a.cos(), a.sin());
        let side = Vec2::new(-dir.y, dir.x);
        let reach = rng.range(0.32, 0.46);
        let end = centre + dir * reach;
        img.stroke(centre, end, 1.6, twig);
        let mut t = 0.04;
        while t < reach {
            let p = centre + dir * t;
            let n = rng.range(0.05, 0.08) * (1.0 - 0.4 * t / reach);
            let tone = rng.range(0.75, 1.2);
            let c = scale(lin([0.12, 0.22, 0.10]), tone);
            for s in [-1.0, 1.0] {
                let d = (dir * 0.55 + side * s).normalize();
                img.stroke(p, p + d * n, 1.7, c);
            }
            t += 0.008;
        }
    }
    img
}

/// Bare twigs fanning out from the card's centre, with buds: a winter
/// crown's fine branches, which cylinders cannot draw.
pub fn twigs(seed: u64) -> Rgba {
    let mut img = Rgba::new(SIZE, SIZE);
    let mut rng = Rng::new(seed);
    let colour = lin([0.42, 0.37, 0.32]);
    #[allow(clippy::too_many_arguments)]
    fn grow(
        img: &mut Rgba,
        rng: &mut Rng,
        from: Vec2,
        dir: Vec2,
        len: f32,
        w: f32,
        depth: u32,
        c: [f32; 3],
    ) {
        let mut p = from;
        let mut d = dir;
        let steps = 4;
        for _ in 0..steps {
            let a = rng.range(-0.25, 0.25);
            d = Vec2::new(d.x * a.cos() - d.y * a.sin(), d.x * a.sin() + d.y * a.cos());
            let q = p + d * (len / steps as f32);
            img.stroke(p, q, w, c);
            p = q;
        }
        if depth == 0 {
            img.stroke(p, p + d * 0.012, w + 1.0, scale(c, 0.8));
            return;
        }
        for s in [-1.0, 1.0] {
            let a = s * rng.range(0.35, 0.7);
            let nd = Vec2::new(d.x * a.cos() - d.y * a.sin(), d.x * a.sin() + d.y * a.cos());
            let at = from + (p - from) * rng.range(0.45, 0.8);
            grow(
                img,
                rng,
                at,
                nd,
                len * 0.55,
                (w - 0.45).max(1.0),
                depth - 1,
                c,
            );
        }
        grow(img, rng, p, d, len * 0.4, (w - 0.45).max(1.0), depth - 1, c);
    }
    for k in 0..5 {
        let a = k as f32 / 5.0 * std::f32::consts::TAU + rng.range(-0.3, 0.3);
        let tone = rng.range(0.8, 1.15);
        let len = rng.range(0.24, 0.32);
        grow(
            &mut img,
            &mut rng,
            Vec2::splat(0.5),
            Vec2::new(a.cos(), a.sin()),
            len,
            1.9,
            2,
            scale(colour, tone),
        );
    }
    img
}

/// Wrapping value noise on an `nx` × `ny` lattice over [0, 1)².
fn noise(u: f32, v: f32, nx: i32, ny: i32, seed: u64) -> f32 {
    let (x, y) = (u * nx as f32, v * ny as f32);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let r = |i: i32, j: i32| {
        let (i, j) = (i.rem_euclid(nx), j.rem_euclid(ny));
        unit(mix(seed, ((i as u64) << 32) | j as u64))
    };
    let (xi, yi) = (x0 as i32, y0 as i32);
    let a = r(xi, yi) + (r(xi + 1, yi) - r(xi, yi)) * sx;
    let b = r(xi, yi + 1) + (r(xi + 1, yi + 1) - r(xi, yi + 1)) * sx;
    a + (b - a) * sy
}

fn fbm(u: f32, v: f32, nx: i32, ny: i32, octaves: u32, seed: u64) -> f32 {
    let (mut sum, mut amp, mut total) = (0.0, 0.5, 0.0);
    for o in 0..octaves {
        let k = 1 << o;
        sum += amp * noise(u, v, nx * k, ny * k, mix(seed, u64::from(o)));
        total += amp;
        amp *= 0.5;
    }
    sum / total
}

/// Bark: furrows along the stem (v), ridges between them. Returns the
/// albedo and the normal map's base levels. One repeat covers
/// `period_m` (round, along).
pub fn bark(seed: u64, period_m: [f32; 2]) -> (Rgba, Rgba) {
    let n = SIZE;
    let mut height = vec![0.0f32; n * n];
    let mut albedo = Rgba::new(n, n);
    for y in 0..n {
        for x in 0..n {
            let (u, v) = ((x as f32 + 0.5) / n as f32, (y as f32 + 0.5) / n as f32);
            let f = fbm(u, v, 10, 2, 4, seed);
            let ridge = 1.0 - (2.0 * f - 1.0).abs();
            let fine = fbm(u, v, 48, 24, 2, mix(seed, 9));
            let lichen = fbm(u, v, 3, 2, 3, mix(seed, 5));
            height[y * n + x] = 0.012 * ridge.powf(1.5) + 0.002 * fine;
            let dark = lin([0.16, 0.14, 0.12]);
            let light = lin([0.42, 0.39, 0.35]);
            let mut c = [0, 1, 2].map(|k| dark[k] + (light[k] - dark[k]) * ridge.powf(1.3));
            let moss = ((lichen - 0.6) * 4.0).clamp(0.0, 1.0) * 0.5;
            let green = lin([0.36, 0.40, 0.30]);
            c = [0, 1, 2].map(|k| (c[k] * (1.0 - moss) + green[k] * moss) * (0.85 + 0.3 * fine));
            albedo.px[y * n + x] = [c[0], c[1], c[2], 1.0];
        }
    }
    let mut normal = Rgba::new(n, n);
    let (du, dv) = (period_m[0] / n as f32, period_m[1] / n as f32);
    for y in 0..n {
        for x in 0..n {
            let h = |c: usize, r: usize| height[(r % n) * n + (c % n)];
            let dx = (h(x + 1, y) - h(x + n - 1, y)) / (2.0 * du);
            let dy = (h(x, y + n - 1) - h(x, y + 1)) / (2.0 * dv);
            let m = Vec3::new(-dx, -dy, 1.0).normalize();
            normal.px[y * n + x] = [m.x * 0.5 + 0.5, m.y * 0.5 + 0.5, m.z * 0.5 + 0.5, 1.0];
        }
    }
    (albedo, normal)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coverage_of(level: &[u8]) -> f32 {
        let n = level.len() / 4;
        level
            .chunks(4)
            .filter(|p| p[3] as f32 / 255.0 >= CUTOFF)
            .count() as f32
            / n as f32
    }

    #[test]
    fn card_textures_cover_what_they_should() {
        let l = leaves(1);
        let t = twigs(2);
        let n = needles(3);
        let (cl, ct, cn) = (l.coverage(1.0), t.coverage(1.0), n.coverage(1.0));
        assert!((0.25..0.75).contains(&cl), "leaves {cl}");
        assert!((0.04..0.35).contains(&ct), "twigs {ct}");
        assert!((0.15..0.7).contains(&cn), "needles {cn}");
        assert_eq!(leaves(1).px, l.px, "seeded: the same texture every run");
    }

    /// The far mip levels keep the full-size level's coverage, so cards do
    /// not thin out in the distance.
    #[test]
    fn mips_keep_their_coverage() {
        let l = leaves(1);
        let target = l.coverage(1.0);
        let levels = alpha_tested_mips(l);
        assert_eq!(levels.len(), 9);
        for level in &levels[..6] {
            let c = coverage_of(level);
            assert!((c - target).abs() < 0.08, "{c} vs {target}");
        }
    }
}
