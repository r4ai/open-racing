//! Ground around the roads: a height grid that runs just under each road and its strips
//! and continues from their outer edges, smoothed between them.

use glam::{DVec2, DVec3};
use rayon::prelude::*;

use crate::project::Project;
use crate::road::{Layer, MeshData, RoadBuild};

/// How far the ground stays under the roads' surfaces, m, so that the roads always lie
/// on top of it.
const UNDER_ROADS: f64 = 0.25;
/// How far the ground stays under every triangle of the roads' surfaces and strips, m,
/// wherever the grid's cells meet them.
const CLEARANCE: f64 = 0.05;
/// How far the ground drops at the roads' outer edges, m: enough to hide under the
/// edges without showing a step.
const AT_EDGES: f64 = 0.03;
/// Width of the ground held at the edges' height beyond them, m.
const HELD: f64 = 4.0;
/// Rounds of smoothing between the roads.
const SMOOTHING: usize = 60;
/// Cells per side of a rendered chunk.
const CHUNK: usize = 32;
/// Edge of the buckets that frames are looked up in, m.
const BUCKET: f64 = 25.0;

#[derive(Clone)]
pub struct TerrainBuild {
    /// Rendered chunks.
    pub chunks: Vec<MeshData>,
    /// The whole grid for the physics (no UVs).
    pub solid: MeshData,
}

/// Frames of every road in buckets over the plane.
struct Lookup {
    origin: DVec2,
    size: (usize, usize),
    buckets: Vec<Vec<(usize, usize)>>,
}

impl Lookup {
    fn new(roads: &[RoadBuild], lo: DVec2, hi: DVec2) -> Self {
        let size = (
            ((hi.x - lo.x) / BUCKET).ceil() as usize + 1,
            ((hi.y - lo.y) / BUCKET).ceil() as usize + 1,
        );
        let mut buckets = vec![Vec::new(); size.0 * size.1];
        for (r, b) in roads.iter().enumerate() {
            for (k, f) in b.sampled.frames.iter().enumerate() {
                let (x, y) = Self::cell(lo, size, f.pos.truncate());
                buckets[y * size.0 + x].push((r, k));
            }
        }
        Self {
            origin: lo,
            size,
            buckets,
        }
    }

    fn cell(lo: DVec2, size: (usize, usize), p: DVec2) -> (usize, usize) {
        let c = ((p - lo) / BUCKET).floor();
        (
            (c.x.max(0.0) as usize).min(size.0 - 1),
            (c.y.max(0.0) as usize).min(size.1 - 1),
        )
    }

    /// The frame nearest to `p` in the plane: (road, frame).
    fn nearest(&self, roads: &[RoadBuild], p: DVec2) -> Option<(usize, usize)> {
        let (cx, cy) = Self::cell(self.origin, self.size, p);
        let mut best: Option<((usize, usize), f64)> = None;
        for ring in 0..self.size.0.max(self.size.1) {
            // Nothing in a further ring can beat what is already found.
            if let Some((_, d2)) = best
                && ((ring as f64 - 1.0) * BUCKET).max(0.0).powi(2) > d2
            {
                break;
            }
            let r = ring as isize;
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs() != r && dy.abs() != r {
                        continue;
                    }
                    let (x, y) = (cx as isize + dx, cy as isize + dy);
                    if x < 0 || y < 0 || x >= self.size.0 as isize || y >= self.size.1 as isize {
                        continue;
                    }
                    for &(ri, k) in &self.buckets[y as usize * self.size.0 + x as usize] {
                        let d2 = roads[ri].sampled.frames[k]
                            .pos
                            .truncate()
                            .distance_squared(p);
                        if best.is_none_or(|(_, b)| d2 < b) {
                            best = Some(((ri, k), d2));
                        }
                    }
                }
            }
        }
        best.map(|(i, _)| i)
    }
}

pub fn build(project: &Project, roads: &[RoadBuild]) -> Option<TerrainBuild> {
    let t = &project.terrain;
    if !t.enabled || roads.is_empty() || t.cell <= 0.0 {
        return None;
    }
    let (mut lo, mut hi) = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
    for b in roads {
        for f in &b.sampled.frames {
            lo = lo.min(f.pos.truncate());
            hi = hi.max(f.pos.truncate());
        }
    }
    lo -= DVec2::splat(t.margin);
    hi += DVec2::splat(t.margin);
    let nx = ((hi.x - lo.x) / t.cell).ceil() as usize + 1;
    let ny = ((hi.y - lo.y) / t.cell).ceil() as usize + 1;
    let lookup = Lookup::new(roads, lo, hi);

    // Heights, and whether smoothing may move them, a row at a time in parallel.
    let rows: Vec<(Vec<f64>, Vec<bool>)> = (0..ny)
        .into_par_iter()
        .map(|j| {
            let mut z = vec![0.0; nx];
            let mut free = vec![false; nx];
            for i in 0..nx {
                let p = lo + DVec2::new(i as f64, j as f64) * t.cell;
                let Some((r, k)) = lookup.nearest(roads, p) else {
                    continue;
                };
                let b = &roads[r];
                let f = &b.sampled.frames[k];
                let flat_left = DVec3::Z.cross(f.tangent).truncate().normalize_or(DVec2::Y);
                let d = (p - f.pos.truncate()).dot(flat_left);
                let [(left, left_edge), (right, right_edge)] = b.edges(k);
                z[i] = if d <= left && d >= right {
                    let road = &project.roads[r];
                    let point = f.pos + f.lateral * d + f.normal * b.height_at(road, k, d);
                    point.z - UNDER_ROADS
                } else {
                    let (beyond, edge) = if d > left {
                        (d - left, left_edge)
                    } else {
                        (right - d, right_edge)
                    };
                    free[i] = beyond > HELD;
                    edge.z - AT_EDGES
                };
            }
            (z, free)
        })
        .collect();
    let mut z: Vec<f64> = rows.iter().flat_map(|r| r.0.iter().copied()).collect();
    let free: Vec<bool> = rows.iter().flat_map(|r| r.1.iter().copied()).collect();
    let mut next = z.clone();
    for _ in 0..SMOOTHING {
        next.par_chunks_mut(nx)
            .enumerate()
            .filter(|(j, _)| *j > 0 && *j < ny - 1)
            .for_each(|(j, row)| {
                for (i, h) in row.iter_mut().enumerate().take(nx - 1).skip(1) {
                    let v = j * nx + i;
                    if free[v] {
                        *h = 0.25 * (z[v - 1] + z[v + 1] + z[v - nx] + z[v + nx]);
                    }
                }
            });
        std::mem::swap(&mut z, &mut next);
    }

    clamp_under(&mut z, roads, lo, t.cell, (nx, ny));

    let position = |i: usize, j: usize| {
        let p = lo + DVec2::new(i as f64, j as f64) * t.cell;
        [p.x as f32, p.y as f32, z[j * nx + i] as f32]
    };
    let normal = |i: usize, j: usize| {
        let h = |i: usize, j: usize| z[j.min(ny - 1) * nx + i.min(nx - 1)];
        let dx = h(i + 1, j) - h(i.saturating_sub(1), j);
        let dy = h(i, j + 1) - h(i, j.saturating_sub(1));
        DVec3::new(-dx, -dy, 2.0 * t.cell)
            .normalize()
            .as_vec3()
            .to_array()
    };
    let tile = project
        .material_index(&t.material)
        .map_or([1.0; 2], |m| project.materials[m].tile);
    let grid = |i0: usize, j0: usize, i1: usize, j1: usize, uvs: bool| {
        let mut m = MeshData::default();
        let w = i1 - i0 + 1;
        for j in j0..=j1 {
            for i in i0..=i1 {
                let p = position(i, j);
                m.positions.push(p);
                m.normals.push(normal(i, j));
                if uvs {
                    m.uvs.push([p[0] / tile[0], p[1] / tile[1]]);
                }
            }
        }
        for j in 0..j1 - j0 {
            for i in 0..i1 - i0 {
                let a = (j * w + i) as u32;
                let (b, c, d) = (a + 1, a + w as u32, a + w as u32 + 1);
                m.indices.extend([a, b, d, a, d, c]);
            }
        }
        m
    };
    let corners: Vec<(usize, usize)> = (0..ny - 1)
        .step_by(CHUNK)
        .flat_map(|j0| (0..nx - 1).step_by(CHUNK).map(move |i0| (i0, j0)))
        .collect();
    let chunks = corners
        .into_par_iter()
        .map(|(i0, j0)| {
            grid(
                i0,
                j0,
                (i0 + CHUNK).min(nx - 1),
                (j0 + CHUNK).min(ny - 1),
                true,
            )
        })
        .collect();
    Some(TerrainBuild {
        chunks,
        solid: grid(0, 0, nx - 1, ny - 1, false),
    })
}

/// Lowers the corners of every cell that a road's surface or strip triangle reaches
/// below that triangle's lowest vertex. The ground is linear within a cell, so it then
/// stays under the roads wherever they are, on grades, in sags and over banking alike.
fn clamp_under(z: &mut [f64], roads: &[RoadBuild], lo: DVec2, cell: f64, (nx, ny): (usize, usize)) {
    let index = |v: f64, n: usize| ((v / cell).floor().max(0.0) as usize).min(n - 2);
    let parts = roads
        .iter()
        .flat_map(|b| &b.solid)
        .filter(|p| matches!(p.layer, Layer::Surface | Layer::Strip));
    for part in parts {
        let m = &part.mesh;
        for &t in m.indices.as_chunks::<3>().0 {
            let p = t.map(|i| m.positions[i as usize].map(f64::from));
            let x = p.map(|p| p[0] - lo.x);
            let y = p.map(|p| p[1] - lo.y);
            let low = p.iter().map(|p| p[2]).fold(f64::INFINITY, f64::min) - CLEARANCE;
            let min = |a: [f64; 3]| a.into_iter().fold(f64::INFINITY, f64::min);
            let max = |a: [f64; 3]| a.into_iter().fold(f64::NEG_INFINITY, f64::max);
            let (i0, i1) = (index(min(x), nx), index(max(x), nx) + 1);
            let (j0, j1) = (index(min(y), ny), index(max(y), ny) + 1);
            for j in j0..=j1 {
                for v in &mut z[j * nx + i0..=j * nx + i1] {
                    *v = v.min(low);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ground_lies_under_the_road() {
        let project = Project::new("t");
        let road = crate::road::build(&project, 0);
        let terrain = build(&project, std::slice::from_ref(&road)).unwrap();
        let f = &road.sampled.frames[10];
        // The grid vertex nearest the road's centre is below it.
        let v = terrain
            .solid
            .positions
            .iter()
            .min_by(|a, b| {
                let d =
                    |p: &[f32; 3]| DVec2::new(p[0] as f64, p[1] as f64).distance(f.pos.truncate());
                d(a).total_cmp(&d(b))
            })
            .unwrap();
        assert!((v[2] as f64) < f.pos.z - 0.1, "{v:?} vs {}", f.pos);
        // Every triangle faces up.
        let m = &terrain.solid;
        for &t in m.indices.as_chunks::<3>().0 {
            let p = t.map(|i| DVec3::from(m.positions[i as usize].map(f64::from)));
            assert!((p[1] - p[0]).cross(p[2] - p[0]).z > 0.0);
        }
    }
}
