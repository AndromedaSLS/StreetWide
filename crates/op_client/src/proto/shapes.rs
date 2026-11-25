//! A small mesh builder for the prototype's trees and lamps: tubes, boxes
//! and cards, generated in code at start-up.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

#[derive(Default)]
pub struct MeshBuilder {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

impl MeshBuilder {
    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }

    fn vertex(&mut self, p: Vec3, n: Vec3, uv: [f32; 2]) -> u32 {
        self.positions.push(p.to_array());
        self.normals.push(n.to_array());
        self.uvs.push(uv);
        self.positions.len() as u32 - 1
    }

    /// A triangle wound counter-clockwise seen from the side `outward`
    /// points to (Bevy's front face).
    fn triangle(&mut self, [a, b, c]: [u32; 3], outward: Vec3) {
        let p = |i: u32| Vec3::from_array(self.positions[i as usize]);
        let n = (p(b) - p(a)).cross(p(c) - p(a));
        if n.dot(outward) >= 0.0 {
            self.indices.extend([a, b, c]);
        } else {
            self.indices.extend([a, c, b]);
        }
    }

    /// An open tube from `a` (radius `ra`) to `b` (radius `rb`) with smooth
    /// normals; texture u runs round it `u_repeat` times, v along it from
    /// `v0`, one unit per `v_len` metres. Returns v at `b`.
    #[allow(clippy::too_many_arguments)]
    pub fn tube(
        &mut self,
        a: Vec3,
        b: Vec3,
        ra: f32,
        rb: f32,
        sides: u32,
        u_repeat: f32,
        v0: f32,
        v_len: f32,
    ) -> f32 {
        let axis = (b - a).normalize_or(Vec3::Y);
        let (u, w) = axis.any_orthonormal_pair();
        let v1 = v0 + (b - a).length() / v_len;
        let first = self.positions.len() as u32;
        for k in 0..=sides {
            let t = k as f32 / sides as f32;
            let ang = t * std::f32::consts::TAU;
            let dir = u * ang.cos() + w * ang.sin();
            self.vertex(a + dir * ra, dir, [t * u_repeat, v0]);
            self.vertex(b + dir * rb, dir, [t * u_repeat, v1]);
        }
        for k in 0..sides {
            let (a0, b0, a1, b1) = (
                first + 2 * k,
                first + 2 * k + 1,
                first + 2 * k + 2,
                first + 2 * k + 3,
            );
            let mid = (Vec3::from_array(self.normals[a0 as usize])
                + Vec3::from_array(self.normals[a1 as usize]))
            .normalize_or(Vec3::X);
            self.triangle([a0, b0, b1], mid);
            self.triangle([a0, b1, a1], mid);
        }
        v1
    }

    /// A flat disc closing a tube end, facing `outward`.
    pub fn disc(&mut self, centre: Vec3, radius: f32, sides: u32, outward: Vec3) {
        let (u, w) = outward.any_orthonormal_pair();
        let c = self.vertex(centre, outward, [0.5, 0.5]);
        let first = self.positions.len() as u32;
        for k in 0..sides {
            let ang = k as f32 / sides as f32 * std::f32::consts::TAU;
            let d = u * ang.cos() + w * ang.sin();
            self.vertex(
                centre + d * radius,
                outward,
                [0.5 + 0.5 * ang.cos(), 0.5 + 0.5 * ang.sin()],
            );
        }
        for k in 0..sides {
            self.triangle([c, first + k, first + (k + 1) % sides], outward);
        }
    }

    /// A flat triangle facing away from `inside` (a point behind it).
    pub fn facet(&mut self, p: [Vec3; 3], inside: Vec3) {
        let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or(Vec3::Y);
        let n = if n.dot(p[0] - inside) < 0.0 { -n } else { n };
        let ids = p.map(|q| self.vertex(q, n, [0.5, 0.5]));
        self.triangle(ids, n);
    }

    /// A flat quad (corners in order round it), facing `normal`.
    pub fn quad(&mut self, corners: [Vec3; 4], normal: Vec3, uvs: [[f32; 2]; 4]) {
        let ids = [0, 1, 2, 3].map(|k| self.vertex(corners[k], normal, uvs[k]));
        self.triangle([ids[0], ids[1], ids[2]], normal);
        self.triangle([ids[0], ids[2], ids[3]], normal);
    }

    /// A box from `lo` to `hi`, rotated by `rot` about `pivot`.
    pub fn cuboid(&mut self, lo: Vec3, hi: Vec3, rot: Quat, pivot: Vec3) {
        let t = |p: Vec3| pivot + rot * (p - pivot);
        let c = |x: f32, y: f32, z: f32| t(Vec3::new(x, y, z));
        let (a, b) = (lo, hi);
        let faces = [
            (
                Vec3::X,
                [
                    c(b.x, a.y, a.z),
                    c(b.x, b.y, a.z),
                    c(b.x, b.y, b.z),
                    c(b.x, a.y, b.z),
                ],
            ),
            (
                Vec3::NEG_X,
                [
                    c(a.x, a.y, a.z),
                    c(a.x, a.y, b.z),
                    c(a.x, b.y, b.z),
                    c(a.x, b.y, a.z),
                ],
            ),
            (
                Vec3::Y,
                [
                    c(a.x, b.y, a.z),
                    c(a.x, b.y, b.z),
                    c(b.x, b.y, b.z),
                    c(b.x, b.y, a.z),
                ],
            ),
            (
                Vec3::NEG_Y,
                [
                    c(a.x, a.y, a.z),
                    c(b.x, a.y, a.z),
                    c(b.x, a.y, b.z),
                    c(a.x, a.y, b.z),
                ],
            ),
            (
                Vec3::Z,
                [
                    c(a.x, a.y, b.z),
                    c(b.x, a.y, b.z),
                    c(b.x, b.y, b.z),
                    c(a.x, b.y, b.z),
                ],
            ),
            (
                Vec3::NEG_Z,
                [
                    c(a.x, a.y, a.z),
                    c(a.x, b.y, a.z),
                    c(b.x, b.y, a.z),
                    c(b.x, a.y, a.z),
                ],
            ),
        ];
        for (n, q) in faces {
            self.quad(q, rot * n, [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]);
        }
    }

    /// A card: a square of side `size` centred at `centre`, facing `normal`
    /// and spun by `spin` about it; vertex normals are bent towards
    /// `bend_to` (the crown's outward direction), so a crown of cards is lit
    /// like a volume. Cards are drawn from both sides.
    pub fn card(
        &mut self,
        centre: Vec3,
        normal: Vec3,
        spin: f32,
        size: f32,
        bend: impl Fn(Vec3) -> Vec3,
    ) {
        let (u0, w0) = normal.any_orthonormal_pair();
        let (s, c) = spin.sin_cos();
        let u = (u0 * c + w0 * s) * (size / 2.0);
        let w = (w0 * c - u0 * s) * (size / 2.0);
        let corners = [
            centre - u - w,
            centre + u - w,
            centre + u + w,
            centre - u + w,
        ];
        let uvs = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        let ids = [0, 1, 2, 3].map(|k| {
            let p = corners[k];
            let n = (bend(p) * 0.7 + normal * 0.3).normalize_or(normal);
            self.vertex(p, n, uvs[k])
        });
        self.indices
            .extend([ids[0], ids[1], ids[2], ids[0], ids[2], ids[3]]);
    }

    pub fn build(self, tangents: bool) -> Mesh {
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs)
        .with_inserted_indices(Indices::U32(self.indices));
        if tangents {
            let _ = mesh.generate_tangents();
        }
        mesh
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every triangle of a tube faces away from its axis.
    #[test]
    fn tubes_face_outwards() {
        let mut m = MeshBuilder::default();
        m.tube(
            Vec3::ZERO,
            Vec3::new(0.3, 2.0, -0.2),
            0.3,
            0.1,
            7,
            1.0,
            0.0,
            1.0,
        );
        let p = |i: u32| Vec3::from_array(m.positions[i as usize]);
        let axis = Vec3::new(0.3, 2.0, -0.2).normalize();
        for t in m.indices.chunks(3) {
            let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
            let n = (b - a).cross(c - a);
            let centre = (a + b + c) / 3.0;
            let radial = centre - axis * centre.dot(axis);
            assert!(n.dot(radial) > 0.0);
        }
    }

    #[test]
    fn boxes_are_closed_and_face_outwards() {
        let mut m = MeshBuilder::default();
        m.cuboid(
            Vec3::splat(-1.0),
            Vec3::splat(1.0),
            Quat::from_rotation_y(0.4),
            Vec3::ZERO,
        );
        assert_eq!(m.triangles(), 12);
        let p = |i: u32| Vec3::from_array(m.positions[i as usize]);
        for t in m.indices.chunks(3) {
            let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
            assert!((b - a).cross(c - a).dot((a + b + c) / 3.0) > 0.0);
        }
    }
}
