//! Triangles of the stand-in shapes, for the editor's view and the baked car's model.

use glam::{DAffine3, DVec3};

use crate::part::{Axis, Shape, ShapeKind};

/// Triangles in a frame: counter-clockwise front faces.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tris {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

impl Tris {
    fn vertex(&mut self, p: DVec3, n: DVec3) -> u32 {
        self.positions.push(p.as_vec3().to_array());
        self.normals.push(n.as_vec3().to_array());
        (self.positions.len() - 1) as u32
    }

    /// Appends `other` moved by `t` (a reflection turns its faces round, so they stay
    /// counter-clockwise).
    pub fn append(&mut self, other: &Tris, t: &DAffine3) {
        let base = self.positions.len() as u32;
        let normal = t.matrix3.inverse().transpose();
        for (p, n) in other.positions.iter().zip(&other.normals) {
            let p = t.transform_point3(DVec3::from(p.map(f64::from)));
            let n = (normal * DVec3::from(n.map(f64::from))).normalize_or_zero();
            self.positions.push(p.as_vec3().to_array());
            self.normals.push(n.as_vec3().to_array());
        }
        let flip = t.matrix3.determinant() < 0.0;
        for tri in other.indices.chunks(3) {
            if flip {
                self.indices
                    .extend([base + tri[0], base + tri[2], base + tri[1]]);
            } else {
                self.indices
                    .extend([base + tri[0], base + tri[1], base + tri[2]]);
            }
        }
    }
}

const SEGMENTS: usize = 24;

/// A shape's triangles in its part's frame.
pub fn shape(s: &Shape) -> Tris {
    let mut t = Tris::default();
    match s.kind {
        ShapeKind::Box { size } => {
            let h = DVec3::from(size) * 0.5;
            for (n, u, v) in [
                (DVec3::X, DVec3::Y, DVec3::Z),
                (DVec3::NEG_X, DVec3::Z, DVec3::Y),
                (DVec3::Y, DVec3::Z, DVec3::X),
                (DVec3::NEG_Y, DVec3::X, DVec3::Z),
                (DVec3::Z, DVec3::X, DVec3::Y),
                (DVec3::NEG_Z, DVec3::Y, DVec3::X),
            ] {
                let c = n * h;
                let (du, dv) = (u * h, v * h);
                let a = t.vertex(c - du - dv, n);
                let b = t.vertex(c + du - dv, n);
                let cc = t.vertex(c + du + dv, n);
                let d = t.vertex(c - du + dv, n);
                t.indices.extend([a, b, cc, a, cc, d]);
            }
        }
        ShapeKind::Cylinder {
            radius,
            length,
            axis,
        } => {
            let (ax, u, v) = match axis {
                Axis::X => (DVec3::X, DVec3::Y, DVec3::Z),
                Axis::Y => (DVec3::Y, DVec3::Z, DVec3::X),
                Axis::Z => (DVec3::Z, DVec3::X, DVec3::Y),
            };
            let half = ax * (0.5 * length);
            let ring = |k: usize| {
                let a = std::f64::consts::TAU * k as f64 / SEGMENTS as f64;
                u * a.cos() + v * a.sin()
            };
            for k in 0..SEGMENTS {
                let (r0, r1) = (ring(k), ring(k + 1));
                let a = t.vertex(r0 * radius - half, r0);
                let b = t.vertex(r1 * radius - half, r1);
                let c = t.vertex(r1 * radius + half, r1);
                let d = t.vertex(r0 * radius + half, r0);
                t.indices.extend([a, b, c, a, c, d]);
            }
            for (sign, end) in [(1.0, half), (-1.0, -half)] {
                let n = ax * sign;
                let centre = t.vertex(end, n);
                for k in 0..SEGMENTS {
                    let a = t.vertex(end + ring(k) * radius, n);
                    let b = t.vertex(end + ring(k + 1) * radius, n);
                    if sign > 0.0 {
                        t.indices.extend([centre, a, b]);
                    } else {
                        t.indices.extend([centre, b, a]);
                    }
                }
            }
        }
        ShapeKind::Sphere { radius } => {
            let rings = SEGMENTS / 2;
            let at = |i: usize, k: usize| {
                let th = std::f64::consts::PI * i as f64 / rings as f64;
                let ph = std::f64::consts::TAU * k as f64 / SEGMENTS as f64;
                DVec3::new(th.sin() * ph.cos(), th.sin() * ph.sin(), th.cos())
            };
            for i in 0..=rings {
                for k in 0..=SEGMENTS {
                    let n = at(i, k);
                    t.vertex(n * radius, n);
                }
            }
            let w = SEGMENTS as u32 + 1;
            for i in 0..rings as u32 {
                for k in 0..SEGMENTS as u32 {
                    let (a, b) = (i * w + k, (i + 1) * w + k);
                    t.indices.extend([a, b, b + 1, a, b + 1, a + 1]);
                }
            }
        }
    }
    let mut out = Tris::default();
    out.append(&t, &crate::assembly::transform(s.at, s.rotation_deg));
    out
}
