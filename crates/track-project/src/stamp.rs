//! Brush stamps: a shape of texture laid over the ground under a stroke, so that it
//! acts in patches rather than evenly: clumps of trees, spots of dirt, streaks of
//! sand, lumpy sculpting. The texture lies still in the world (it is not dragged with
//! the brush), so strokes painted again over the same place build up the same shapes,
//! and it is the same every build.

use glam::DVec2;
use serde::{Deserialize, Serialize};

/// A stamp: its shape, the size of its features and the way it turns.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stamp {
    pub shape: StampShape,
    /// The size of its patches, m.
    pub size: f64,
    /// Which way it runs, radians anticlockwise from the east (streaks run along it).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub angle: f64,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// The shape of a stamp's texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StampShape {
    /// Cloud-like patches with soft, ragged edges, about as wide as they are apart.
    Clouds,
    /// Round spots `size` apart, about half as wide.
    Spots,
    /// Long streaks along `angle`, six times as long as they are wide.
    Streaks,
}

impl StampShape {
    pub const ALL: [StampShape; 3] = [StampShape::Clouds, StampShape::Spots, StampShape::Streaks];

    pub fn label(self) -> &'static str {
        match self {
            StampShape::Clouds => "Clouds",
            StampShape::Spots => "Spots",
            StampShape::Streaks => "Streaks",
        }
    }
}

/// A number in [0, 1) for a point of the lattice, the same every time.
fn hash(i: i64, j: i64, k: u64) -> f64 {
    let mut h = (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ (j as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f)
        ^ k.wrapping_mul(0x1656_67b1_9e37_79f9);
    h ^= h >> 31;
    h = h.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h ^= h >> 29;
    (h >> 11) as f64 / (1u64 << 53) as f64
}

fn smooth(a: f64, b: f64, x: f64) -> f64 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Value noise in [0, 1], its features a unit apart.
fn noise(p: DVec2, octave: u64) -> f64 {
    let (i, j) = (p.x.floor(), p.y.floor());
    let (fx, fy) = (p.x - i, p.y - j);
    let (u, v) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let (i, j) = (i as i64, j as i64);
    let h = |a, b| hash(i + a, j + b, octave);
    let top = h(0, 0) + (h(1, 0) - h(0, 0)) * u;
    let bottom = h(0, 1) + (h(1, 1) - h(0, 1)) * u;
    top + (bottom - top) * v
}

/// Three octaves of value noise, in [0, 1].
fn clouds(p: DVec2) -> f64 {
    (noise(p, 1) * 0.57 + noise(p * 2.03, 2) * 0.29 + noise(p * 4.1, 3) * 0.14).clamp(0.0, 1.0)
}

impl Stamp {
    /// How much of a stroke acts at `p`, 0 to 1.
    pub fn at(&self, p: DVec2) -> f64 {
        let size = self.size.max(0.01);
        let q = DVec2::from_angle(-self.angle).rotate(p) / size;
        match self.shape {
            StampShape::Clouds => smooth(0.4, 0.6, clouds(q)),
            StampShape::Streaks => smooth(0.42, 0.58, clouds(DVec2::new(q.x / 6.0, q.y))),
            StampShape::Spots => {
                // The nearest of the points jittered one to a cell.
                let (ci, cj) = (q.x.floor() as i64, q.y.floor() as i64);
                let mut nearest = f64::INFINITY;
                for dj in -1..=1 {
                    for di in -1..=1 {
                        let (i, j) = (ci + di, cj + dj);
                        let c = DVec2::new(i as f64, j as f64)
                            + DVec2::new(0.2 + 0.6 * hash(i, j, 7), 0.2 + 0.6 * hash(i, j, 8));
                        nearest = nearest.min(c.distance(q));
                    }
                }
                1.0 - smooth(0.18, 0.26, nearest)
            }
        }
    }

    pub fn is_valid(&self) -> bool {
        self.size.is_finite() && self.size > 0.0 && self.angle.is_finite()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The share of points on a 200 × 200 m square a stamp acts fully at, and nowhere.
    fn shares(s: &Stamp) -> (f64, f64) {
        let n = 200;
        let values: Vec<f64> = (0..n * n)
            .map(|k| s.at(DVec2::new((k % n) as f64, (k / n) as f64)))
            .collect();
        let full = values.iter().filter(|&&v| v > 0.99).count() as f64 / values.len() as f64;
        let none = values.iter().filter(|&&v| v < 0.01).count() as f64 / values.len() as f64;
        (full, none)
    }

    #[test]
    fn stamps_act_in_patches_and_the_same_every_time() {
        for shape in StampShape::ALL {
            let s = Stamp {
                shape,
                size: 12.0,
                angle: 0.3,
            };
            let (full, none) = shares(&s);
            assert!(full > 0.05 && none > 0.2, "{shape:?}: {full} full, {none} none");
            assert_eq!(s.at(DVec2::new(3.3, -7.1)), s.at(DVec2::new(3.3, -7.1)));
        }
        // Streaks run along their angle: the stamp changes slower along it than across.
        let s = Stamp {
            shape: StampShape::Streaks,
            size: 10.0,
            angle: 0.0,
        };
        let change = |d: DVec2| {
            (0..400)
                .map(|k| {
                    let p = DVec2::new((k % 20) as f64 * 13.0, (k / 20) as f64 * 17.0);
                    (s.at(p) - s.at(p + d)).abs()
                })
                .sum::<f64>()
        };
        assert!(change(DVec2::X * 3.0) < 0.5 * change(DVec2::Y * 3.0));
    }
}
