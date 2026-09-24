//! Triangle-mesh road surface and walls, for tracks built from 3D models.
//!
//! Tyre queries are vertical rays, so triangles are binned into a uniform XY grid
//! instead of a BVH: a query only visits the triangles overlapping one cell.

use glam::{DVec2, DVec3};
use serde::{Deserialize, Serialize};

use crate::track::Surface;

/// Physical properties of one surface type.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurfaceProps {
    /// Coarse class used by rewards, effects, audio and dirt.
    pub kind: Surface,
    /// Grip multiplier relative to the tyre's nominal μ.
    pub grip: f64,
    /// Extra rolling resistance coefficient on top of the tyre's own.
    pub drag: f64,
    /// How readily a tyre picks up loose material, 0..1; `None` takes the kind's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirt: Option<f64>,
}

impl SurfaceProps {
    pub fn of(kind: Surface) -> Self {
        Self {
            kind,
            grip: kind.grip(),
            drag: kind.drag(),
            dirt: None,
        }
    }

    /// How readily a tyre picks up loose material, 0..1.
    pub fn dirt(&self) -> f64 {
        self.dirt.unwrap_or_else(|| self.kind.dirt())
    }
}

/// A ray hit on the ground.
#[derive(Clone, Copy, Debug)]
pub struct GroundHit {
    pub point: DVec3,
    /// Unit normal, interpolated from the vertex normals for a smooth ride.
    pub normal: DVec3,
    pub surface: SurfaceProps,
}

/// Triangles collected before building the lookup grids.
#[derive(Default)]
pub struct GroundMeshBuilder {
    verts: Vec<DVec3>,
    normals: Vec<DVec3>,
    ground: Vec<([u32; 3], u16)>,
    walls: Vec<[u32; 3]>,
    surfaces: Vec<SurfaceProps>,
}

impl GroundMeshBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a surface type and returns its id for `add_ground`.
    pub fn add_surface(&mut self, props: SurfaceProps) -> u16 {
        self.surfaces.push(props);
        (self.surfaces.len() - 1) as u16
    }

    /// Adds a drivable mesh. `normals` may be empty to use face normals.
    pub fn add_ground(
        &mut self,
        positions: &[DVec3],
        normals: &[DVec3],
        indices: &[u32],
        surface: u16,
    ) {
        let base = self.push_verts(positions, normals);
        for t in indices.as_chunks::<3>().0 {
            self.ground
                .push(([base + t[0], base + t[1], base + t[2]], surface));
        }
    }

    /// Adds a solid mesh the car collides with.
    pub fn add_wall(&mut self, positions: &[DVec3], indices: &[u32]) {
        let base = self.push_verts(positions, &[]);
        for t in indices.as_chunks::<3>().0 {
            self.walls.push([base + t[0], base + t[1], base + t[2]]);
        }
    }

    fn push_verts(&mut self, positions: &[DVec3], normals: &[DVec3]) -> u32 {
        let base = self.verts.len() as u32;
        self.verts.extend_from_slice(positions);
        if normals.len() == positions.len() {
            self.normals
                .extend(normals.iter().map(|n| n.normalize_or_zero()));
        } else {
            self.normals
                .extend(std::iter::repeat_n(DVec3::ZERO, positions.len()));
        }
        base
    }

    pub fn is_empty(&self) -> bool {
        self.ground.is_empty()
    }

    pub fn build(self) -> GroundMesh {
        let tri_bounds = |t: &[u32; 3]| {
            let p = t.map(|i| self.verts[i as usize].truncate());
            (p[0].min(p[1]).min(p[2]), p[0].max(p[1]).max(p[2]))
        };
        let ground = Grid::build(
            self.ground.iter().map(|(t, _)| tri_bounds(t)),
            GROUND_CELL,
            GROUND_MARGIN,
        );
        let walls = Grid::build(self.walls.iter().map(tri_bounds), WALL_CELL, WALL_MARGIN);
        GroundMesh {
            verts: self.verts,
            normals: self.normals,
            ground_tris: self.ground,
            wall_tris: self.walls,
            surfaces: self.surfaces,
            ground_grid: ground,
            wall_grid: walls,
        }
    }
}

const GROUND_CELL: f64 = 4.0;
const WALL_CELL: f64 = 4.0;
/// How far beyond its bounds a triangle is binned, m. Ground queries are points, so
/// only rounding needs covering; wall queries are spheres of up to this radius.
const GROUND_MARGIN: f64 = 1e-3;
const WALL_MARGIN: f64 = 1.0;

/// Road surface and walls of a track, ready for queries.
#[derive(Debug)]
pub struct GroundMesh {
    verts: Vec<DVec3>,
    normals: Vec<DVec3>,
    ground_tris: Vec<([u32; 3], u16)>,
    wall_tris: Vec<[u32; 3]>,
    surfaces: Vec<SurfaceProps>,
    ground_grid: Grid,
    wall_grid: Grid,
}

impl GroundMesh {
    pub fn surfaces(&self) -> &[SurfaceProps] {
        &self.surfaces
    }

    /// Highest ground point straight below `p + up·Z`. Starting the ray a little above
    /// the query point keeps the wheel on the deck it is driving on under an overpass.
    pub fn raycast_down(&self, p: DVec3, up: f64) -> Option<GroundHit> {
        let top = p.z + up;
        let xy = p.truncate();
        let mut best: Option<(f64, usize, DVec3)> = None;
        for &ti in self.ground_grid.cell(xy) {
            let (t, _) = &self.ground_tris[ti as usize];
            let [a, b, c] = t.map(|i| self.verts[i as usize]);
            let Some(w) = barycentric(xy, a.truncate(), b.truncate(), c.truncate()) else {
                continue;
            };
            let z = w.x * a.z + w.y * b.z + w.z * c.z;
            if z <= top && best.is_none_or(|(bz, _, _)| z > bz) {
                best = Some((z, ti as usize, w));
            }
        }
        let (z, ti, w) = best?;
        let (t, surface) = &self.ground_tris[ti];
        let [a, b, c] = t.map(|i| self.verts[i as usize]);
        let mut face = (b - a).cross(c - a).normalize_or_zero();
        if face.z < 0.0 {
            face = -face;
        }
        let [na, nb, nc] = t.map(|i| self.normals[i as usize]);
        let mut normal = (na * w.x + nb * w.y + nc * w.z).normalize_or_zero();
        if normal.z < 0.0 {
            normal = -normal;
        }
        // Visual normals can be bent for shading; fall back to the true face.
        if normal.dot(face) < 0.9 {
            normal = face;
        }
        Some(GroundHit {
            point: xy.extend(z),
            normal,
            surface: self.surfaces[*surface as usize],
        })
    }

    /// Deepest penetration of a sphere into the walls: (unit direction pushing the
    /// sphere out, depth).
    pub fn wall_contact(&self, center: DVec3, radius: f64) -> Option<(DVec3, f64)> {
        let mut best: Option<(DVec3, f64)> = None;
        for &ti in self.wall_grid.cell(center.truncate()) {
            let [a, b, c] = self.wall_tris[ti as usize].map(|i| self.verts[i as usize]);
            let q = closest_point_on_triangle(center, a, b, c);
            let delta = center - q;
            let dist = delta.length();
            let depth = radius - dist;
            if depth > 0.0 && best.is_none_or(|(_, d)| depth > d) {
                let dir = if dist > 1e-9 {
                    delta / dist
                } else {
                    (b - a).cross(c - a).normalize_or_zero()
                };
                best = Some((dir, depth));
            }
        }
        best
    }
}

/// Barycentric weights of `p` in the 2D triangle, or `None` when outside or degenerate.
fn barycentric(p: DVec2, a: DVec2, b: DVec2, c: DVec2) -> Option<DVec3> {
    let det = (b - a).perp_dot(c - a);
    if det.abs() < 1e-12 {
        return None;
    }
    let wb = (p - a).perp_dot(c - a) / det;
    let wc = (b - a).perp_dot(p - a) / det;
    let wa = 1.0 - wb - wc;
    const EPS: f64 = -1e-9;
    (wa >= EPS && wb >= EPS && wc >= EPS).then_some(DVec3::new(wa, wb, wc))
}

/// Closest point on a triangle (Ericson, Real-Time Collision Detection 5.1.5).
fn closest_point_on_triangle(p: DVec3, a: DVec3, b: DVec3, c: DVec3) -> DVec3 {
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    a + ab * (vb * denom) + ac * (vc * denom)
}

/// Uniform XY grid of item indices, stored as compressed rows.
#[derive(Debug)]
struct Grid {
    origin: DVec2,
    cell: f64,
    nx: usize,
    ny: usize,
    starts: Vec<u32>,
    items: Vec<u32>,
}

impl Grid {
    /// Bins items by their XY bounds grown by `margin`, so a sphere query of radius up
    /// to `margin` only needs the cell containing its centre.
    fn build(bounds: impl Iterator<Item = (DVec2, DVec2)> + Clone, cell: f64, margin: f64) -> Self {
        let pad = DVec2::splat(cell);
        let (lo, hi) = bounds.clone().fold(
            (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY)),
            |(lo, hi), (a, b)| (lo.min(a), hi.max(b)),
        );
        if !lo.is_finite() {
            return Self {
                origin: DVec2::ZERO,
                cell,
                nx: 0,
                ny: 0,
                starts: vec![0],
                items: Vec::new(),
            };
        }
        let (lo, hi) = (lo - pad, hi + pad);
        let nx = ((hi.x - lo.x) / cell).ceil() as usize + 1;
        let ny = ((hi.y - lo.y) / cell).ceil() as usize + 1;
        let mut grid = Self {
            origin: lo,
            cell,
            nx,
            ny,
            starts: Vec::new(),
            items: Vec::new(),
        };

        let cells = |g: &Self, a: DVec2, b: DVec2| {
            let (x0, y0) = g.coords(a - DVec2::splat(margin));
            let (x1, y1) = g.coords(b + DVec2::splat(margin));
            (x0..=x1).flat_map(move |x| (y0..=y1).map(move |y| y * nx + x))
        };
        // Two passes: count, then fill.
        let mut starts = vec![0u32; nx * ny + 1];
        for (a, b) in bounds.clone() {
            for c in cells(&grid, a, b) {
                starts[c + 1] += 1;
            }
        }
        for c in 0..nx * ny {
            starts[c + 1] += starts[c];
        }
        let mut items = vec![0; starts[nx * ny] as usize];
        let mut fill = starts.clone();
        for (i, (a, b)) in bounds.enumerate() {
            for c in cells(&grid, a, b) {
                items[fill[c] as usize] = i as u32;
                fill[c] += 1;
            }
        }
        grid.starts = starts;
        grid.items = items;
        grid
    }

    fn coords(&self, p: DVec2) -> (usize, usize) {
        let q = ((p - self.origin) / self.cell).floor();
        (
            q.x.clamp(0.0, (self.nx - 1) as f64) as usize,
            q.y.clamp(0.0, (self.ny - 1) as f64) as usize,
        )
    }

    fn cell(&self, p: DVec2) -> &[u32] {
        let q = ((p - self.origin) / self.cell).floor();
        if q.x < 0.0 || q.y < 0.0 || q.x >= self.nx as f64 || q.y >= self.ny as f64 {
            return &[];
        }
        let c = q.y as usize * self.nx + q.x as usize;
        &self.items[self.starts[c] as usize..self.starts[c + 1] as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(b: &mut GroundMeshBuilder, lo: DVec2, hi: DVec2, z: f64, surface: u16) {
        let p = [
            DVec3::new(lo.x, lo.y, z),
            DVec3::new(hi.x, lo.y, z),
            DVec3::new(hi.x, hi.y, z),
            DVec3::new(lo.x, hi.y, z),
        ];
        b.add_ground(&p, &[DVec3::Z; 4], &[0, 1, 2, 0, 2, 3], surface);
    }

    fn overpass() -> GroundMesh {
        let mut b = GroundMeshBuilder::new();
        let road = b.add_surface(SurfaceProps::of(Surface::Asphalt));
        let grass = b.add_surface(SurfaceProps::of(Surface::Grass));
        quad(
            &mut b,
            DVec2::new(-50.0, -50.0),
            DVec2::new(50.0, 50.0),
            0.0,
            grass,
        );
        quad(
            &mut b,
            DVec2::new(-5.0, -50.0),
            DVec2::new(5.0, 50.0),
            6.0,
            road,
        );
        b.build()
    }

    #[test]
    fn ray_picks_the_deck_below_the_start() {
        let g = overpass();
        let lower = g.raycast_down(DVec3::new(0.0, 0.0, 0.3), 1.0).unwrap();
        assert!(lower.point.z.abs() < 1e-9);
        assert_eq!(lower.surface.kind, Surface::Grass);
        let upper = g.raycast_down(DVec3::new(0.0, 0.0, 6.3), 1.0).unwrap();
        assert!((upper.point.z - 6.0).abs() < 1e-9);
        assert_eq!(upper.surface.kind, Surface::Asphalt);
        assert!((upper.normal - DVec3::Z).length() < 1e-9);
        assert!(g.raycast_down(DVec3::new(80.0, 0.0, 0.0), 1.0).is_none());
    }

    #[test]
    fn sloped_surface_height_and_normal() {
        let mut b = GroundMeshBuilder::new();
        let s = b.add_surface(SurfaceProps::of(Surface::Asphalt));
        // z = 0.1 x
        let p = [
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(10.0, 0.0, 1.0),
            DVec3::new(10.0, 10.0, 1.0),
            DVec3::new(0.0, 10.0, 0.0),
        ];
        b.add_ground(&p, &[], &[0, 1, 2, 0, 2, 3], s);
        let g = b.build();
        let hit = g.raycast_down(DVec3::new(5.0, 5.0, 2.0), 1.0).unwrap();
        assert!((hit.point.z - 0.5).abs() < 1e-9);
        let expect = DVec3::new(-0.1, 0.0, 1.0).normalize();
        assert!((hit.normal - expect).length() < 1e-9, "{}", hit.normal);
    }

    #[test]
    fn sphere_touches_wall() {
        let mut b = GroundMeshBuilder::new();
        // Wall in the plane x = 10.
        let p = [
            DVec3::new(10.0, -5.0, 0.0),
            DVec3::new(10.0, 5.0, 0.0),
            DVec3::new(10.0, 5.0, 3.0),
            DVec3::new(10.0, -5.0, 3.0),
        ];
        b.add_wall(&p, &[0, 1, 2, 0, 2, 3]);
        let g = b.build();
        let (dir, depth) = g.wall_contact(DVec3::new(9.8, 0.0, 0.3), 0.33).unwrap();
        assert!((depth - 0.13).abs() < 1e-9);
        assert!((dir - DVec3::NEG_X).length() < 1e-9);
        assert!(g.wall_contact(DVec3::new(9.0, 0.0, 0.3), 0.33).is_none());
    }
}
