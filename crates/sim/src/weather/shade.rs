//! Static scenery that shades the road: grandstands, trees, bridges, hills. Rays from the
//! road towards the sun or the sky pass through it, and each triangle they cross lets
//! through its share of the light (none for solid objects, some for foliage and fences).
//!
//! The rays are cast only while the weather is set up, never during a physics step, so
//! a plain uniform XY grid is enough: a ray walks the columns it crosses, skipping those
//! whose tallest triangle it has already risen above.

use glam::{DVec2, DVec3, Vec3};

/// Edge of a grid column, m.
const CELL: f64 = 8.0;
/// Rays start this far above the road, so that the road itself does not shade them.
pub const RAY_LIFT: f64 = 0.3;

#[derive(Clone, Copy, Debug)]
struct Triangle {
    v0: Vec3,
    e1: Vec3,
    e2: Vec3,
    /// Share of the light let through, 0 for solid objects.
    transmission: f32,
}

impl Triangle {
    /// Distance along the ray to the triangle, if the ray crosses it (Möller–Trumbore).
    fn hit(&self, origin: Vec3, dir: Vec3) -> Option<f32> {
        let p = dir.cross(self.e2);
        let det = self.e1.dot(p);
        if det.abs() < 1e-9 {
            return None;
        }
        let inv = 1.0 / det;
        let s = origin - self.v0;
        let u = s.dot(p) * inv;
        if !(0.0..=1.0).contains(&u) {
            return None;
        }
        let q = s.cross(self.e1);
        let v = dir.dot(q) * inv;
        if v < 0.0 || u + v > 1.0 {
            return None;
        }
        let t = self.e2.dot(q) * inv;
        (t > 1e-3).then_some(t)
    }
}

/// Collects the scenery's triangles, keeping those near the track.
pub struct OccluderBuilder {
    /// Columns within reach of the road.
    near: Vec<bool>,
    origin: DVec2,
    size: [usize; 2],
    triangles: Vec<Triangle>,
}

impl OccluderBuilder {
    /// A builder keeping triangles within `reach` m (horizontally) of any of `road` points.
    pub fn new(road: &[DVec3], reach: f64) -> Self {
        let (mut lo, mut hi) = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
        for p in road {
            lo = lo.min(p.truncate());
            hi = hi.max(p.truncate());
        }
        if road.is_empty() {
            (lo, hi) = (DVec2::ZERO, DVec2::ZERO);
        }
        let origin = lo - DVec2::splat(reach + CELL);
        let extent = hi + DVec2::splat(reach + CELL) - origin;
        let size = [
            (extent.x / CELL).ceil() as usize + 1,
            (extent.y / CELL).ceil() as usize + 1,
        ];
        let mut near = vec![false; size[0] * size[1]];
        let r = (reach / CELL).ceil() as isize;
        for p in road {
            let c = ((p.truncate() - origin) / CELL).floor();
            for dy in -r..=r {
                for dx in -r..=r {
                    let (x, y) = (c.x as isize + dx, c.y as isize + dy);
                    if x >= 0 && y >= 0 && (x as usize) < size[0] && (y as usize) < size[1] {
                        near[y as usize * size[0] + x as usize] = true;
                    }
                }
            }
        }
        Self {
            near,
            origin,
            size,
            triangles: Vec::new(),
        }
    }

    fn column(&self, p: DVec2) -> Option<usize> {
        let c = ((p - self.origin) / CELL).floor();
        (c.x >= 0.0 && c.y >= 0.0 && (c.x as usize) < self.size[0] && (c.y as usize) < self.size[1])
            .then(|| c.y as usize * self.size[0] + c.x as usize)
    }

    /// Adds a mesh (world positions, Z up) that lets `transmission` of the light through.
    pub fn add(&mut self, positions: &[[f32; 3]], indices: &[u32], transmission: f64) {
        for t in indices.as_chunks::<3>().0 {
            let [a, b, c] = t.map(|i| Vec3::from(positions[i as usize]));
            let centre = ((a + b + c) / 3.0).as_dvec3().truncate();
            if !self.column(centre).is_some_and(|i| self.near[i]) {
                continue;
            }
            self.triangles.push(Triangle {
                v0: a,
                e1: b - a,
                e2: c - a,
                transmission: transmission as f32,
            });
        }
    }

    pub fn build(self) -> Occluders {
        let [nx, ny] = self.size;
        let mut counts = vec![0u32; nx * ny + 1];
        let mut top = vec![f32::NEG_INFINITY; nx * ny];
        let columns = |t: &Triangle| {
            let [a, b, c] = [t.v0, t.v0 + t.e1, t.v0 + t.e2];
            let lo = a.min(b).min(c).as_dvec3().truncate();
            let hi = a.max(b).max(c).as_dvec3().truncate();
            let to = |p: DVec2| ((p - self.origin) / CELL).floor();
            let (lo, hi) = (to(lo).max(DVec2::ZERO), to(hi));
            let hi = hi.min(DVec2::new(nx as f64 - 1.0, ny as f64 - 1.0));
            let zmax = a.z.max(b.z).max(c.z);
            (lo.x as usize..=hi.x as usize)
                .flat_map(move |x| (lo.y as usize..=hi.y as usize).map(move |y| y * nx + x))
                .map(move |i| (i, zmax))
        };
        for t in &self.triangles {
            for (i, zmax) in columns(t) {
                counts[i + 1] += 1;
                top[i] = top[i].max(zmax);
            }
        }
        for i in 1..counts.len() {
            counts[i] += counts[i - 1];
        }
        let mut fill = counts.clone();
        let mut index = vec![0u32; counts[nx * ny] as usize];
        for (k, t) in self.triangles.iter().enumerate() {
            for (i, _) in columns(t) {
                index[fill[i] as usize] = k as u32;
                fill[i] += 1;
            }
        }
        let highest = top.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        Occluders {
            triangles: self.triangles,
            origin: self.origin,
            size: self.size,
            start: counts,
            index,
            top,
            highest,
        }
    }
}

/// The scenery's triangles, binned into grid columns.
#[derive(Debug)]
pub struct Occluders {
    triangles: Vec<Triangle>,
    origin: DVec2,
    size: [usize; 2],
    /// Start of each column's triangles in `index`.
    start: Vec<u32>,
    index: Vec<u32>,
    /// Height of each column's tallest triangle, m.
    top: Vec<f32>,
    highest: f32,
}

impl Occluders {
    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }

    /// Share of the light reaching `origin` along `dir` (unit, pointing upwards) that
    /// gets through the scenery.
    pub fn transmission(&self, origin: DVec3, dir: DVec3) -> f64 {
        if self.triangles.is_empty() || dir.z <= 0.0 {
            return if dir.z <= 0.0 { 0.0 } else { 1.0 };
        }
        let [nx, ny] = self.size;
        let p = (origin.truncate() - self.origin) / CELL;
        let (mut x, mut y) = (p.x.floor() as isize, p.y.floor() as isize);
        let d = dir.truncate();
        let step = [d.x.signum() as isize, d.y.signum() as isize];
        // Ray distance to the next column edge in x and y, and between edges.
        let next = |p: f64, c: isize, d: f64| {
            if d.abs() < 1e-12 {
                f64::INFINITY
            } else {
                let edge = if d > 0.0 { c as f64 + 1.0 } else { c as f64 };
                (edge - p) * CELL / d
            }
        };
        let mut t_max = [next(p.x, x, d.x), next(p.y, y, d.y)];
        let t_delta = [CELL / d.x.abs(), CELL / d.y.abs()];
        let (o, v) = (origin.as_vec3(), dir.as_vec3());
        let mut t_enter = 0.0;
        let mut light = 1.0f64;
        loop {
            let z_enter = origin.z + dir.z * t_enter;
            if z_enter > self.highest as f64 {
                return light;
            }
            let t_exit = t_max[0].min(t_max[1]);
            if x >= 0 && y >= 0 && (x as usize) < nx && (y as usize) < ny {
                let i = y as usize * nx + x as usize;
                if z_enter <= self.top[i] as f64 {
                    for &k in &self.index[self.start[i] as usize..self.start[i + 1] as usize] {
                        let tri = &self.triangles[k as usize];
                        // Count a hit in the column it lies in only.
                        if let Some(t) = tri.hit(o, v)
                            && (t_enter..t_exit).contains(&(t as f64))
                        {
                            light *= tri.transmission as f64;
                            if light < 1e-3 {
                                return 0.0;
                            }
                        }
                    }
                }
            } else if (x < 0 && step[0] <= 0)
                || (y < 0 && step[1] <= 0)
                || (x >= nx as isize && step[0] >= 0)
                || (y >= ny as isize && step[1] >= 0)
            {
                return light;
            }
            if !t_exit.is_finite() {
                return light;
            }
            t_enter = t_exit;
            if t_max[0] < t_max[1] {
                x += step[0];
                t_max[0] += t_delta[0];
            } else {
                y += step[1];
                t_max[1] += t_delta[1];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 10 m square roof at `z`, over x, y in [0, 10].
    fn roof(z: f32, transmission: f64) -> Occluders {
        let mut b = OccluderBuilder::new(&[DVec3::ZERO], 50.0);
        b.add(
            &[
                [0.0, 0.0, z],
                [10.0, 0.0, z],
                [10.0, 10.0, z],
                [0.0, 10.0, z],
            ],
            &[0, 1, 2, 0, 2, 3],
            transmission,
        );
        b.build()
    }

    #[test]
    fn a_roof_shades_what_is_under_it_only() {
        let o = roof(5.0, 0.0);
        assert_eq!(o.transmission(DVec3::new(5.0, 5.0, 0.0), DVec3::Z), 0.0);
        assert_eq!(o.transmission(DVec3::new(15.0, 5.0, 0.0), DVec3::Z), 1.0);
        // A low sun from the side reaches under the roof's edge.
        let low = DVec3::new(1.0, 0.0, 0.2).normalize();
        assert_eq!(o.transmission(DVec3::new(9.5, 5.0, 0.0), low), 1.0);
        // A sun behind the roof does not.
        let behind = DVec3::new(-1.0, 0.0, 0.6).normalize();
        assert_eq!(o.transmission(DVec3::new(12.0, 5.0, 0.0), behind), 0.0);
    }

    #[test]
    fn foliage_lets_some_light_through_once() {
        let o = roof(5.0, 0.4);
        let t = o.transmission(
            DVec3::new(3.0, 3.0, 0.0),
            DVec3::new(0.3, 0.2, 1.0).normalize(),
        );
        assert!((t - 0.4).abs() < 1e-6, "{t}");
    }
}
