//! Ground around the roads: a height grid that runs just under each road and its strips
//! and continues from their outer edges, smoothed between them, then shaped by
//! landforms and sculpting strokes and painted with ground layers.
//!
//! The build has two steps. `base` finds the grid's heights from the roads, the
//! elevation data and the landforms, the slow part, and changes only when they do.
//! `finish` sculpts and paints it and makes the meshes, quick enough to run on every
//! brush stroke.

use std::sync::Arc;

use glam::{DVec2, DVec3};
use rayon::prelude::*;

use crate::project::{Brush, GroundLayer, LayerStroke, Project, Stroke, Terrain};
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
/// Beyond the held ground, how far the ground eases from the roads' edges to the
/// elevation data's heights, m.
const BLEND: f64 = 30.0;
/// Over how far beyond the held ground landforms and brushes come in, m, so that the
/// ground still meets the roads' edges.
const LANDFORM_EASE: f64 = 10.0;
/// Cells per side of a rendered chunk.
pub const CHUNK: usize = 32;
/// Edge of the buckets that frames are looked up in, m.
const BUCKET: f64 = 25.0;
/// Most texels along either side of the painted layers' mask.
const MAX_MASK: usize = 4096;

/// The ground's heights on a regular grid over the plane, and how far each point lies
/// beyond the roads' outer edges.
#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    /// Where point (0, 0) is, m.
    pub lo: DVec2,
    /// Distance between neighbouring points, m.
    pub cell: f64,
    /// Points along x and y.
    pub nx: usize,
    pub ny: usize,
    /// Heights, row by row from `lo` up y.
    pub z: Vec<f64>,
    /// How far beyond the roads' outer edges each point is, m; 0 under the roads.
    pub away: Vec<f64>,
}

impl Grid {
    pub fn point(&self, i: usize, j: usize) -> DVec2 {
        self.lo + DVec2::new(i as f64, j as f64) * self.cell
    }

    /// The far corner of the grid.
    pub fn hi(&self) -> DVec2 {
        self.point(self.nx - 1, self.ny - 1)
    }

    /// The height at `p`, between the grid's points; none off the grid.
    pub fn height_at(&self, p: DVec2) -> Option<f64> {
        let q = (p - self.lo) / self.cell;
        if q.x < 0.0 || q.y < 0.0 || q.x > (self.nx - 1) as f64 || q.y > (self.ny - 1) as f64 {
            return None;
        }
        let (i, j) = (
            (q.x.floor() as usize).min(self.nx - 2),
            (q.y.floor() as usize).min(self.ny - 2),
        );
        let (fx, fy) = (q.x - i as f64, q.y - j as f64);
        let h = |i: usize, j: usize| self.z[j * self.nx + i];
        let a = h(i, j) + (h(i + 1, j) - h(i, j)) * fx;
        let b = h(i, j + 1) + (h(i + 1, j + 1) - h(i, j + 1)) * fx;
        Some(a + (b - a) * fy)
    }

    /// The points within the box from `lo` to `hi`: first and last columns and rows.
    pub fn span(&self, lo: DVec2, hi: DVec2) -> Option<[usize; 4]> {
        let a = ((lo - self.lo) / self.cell).ceil().max(DVec2::ZERO);
        let b = ((hi - self.lo) / self.cell).floor();
        let max = DVec2::new((self.nx - 1) as f64, (self.ny - 1) as f64);
        if !(a.is_finite() && b.is_finite()) || b.x < 0.0 || b.y < 0.0 || a.x > max.x || a.y > max.y
        {
            return None;
        }
        let b = b.min(max);
        (a.x <= b.x && a.y <= b.y).then_some([
            a.x as usize,
            a.y as usize,
            b.x as usize,
            b.y as usize,
        ])
    }

    /// How much brushes and landforms act at point `v`: nothing where the ground meets
    /// the roads, fully a little beyond them.
    pub fn ease(&self, v: usize) -> f64 {
        ((self.away[v] - HELD) / LANDFORM_EASE).clamp(0.0, 1.0)
    }

    /// Shapes the ground by a sculpting stroke; other strokes do nothing.
    pub fn sculpt(&mut self, stroke: &Stroke) {
        let (lo, hi) = stroke.bounds();
        let Some([i0, j0, i1, j1]) = self.span(lo, hi) else {
            return;
        };
        let w = i1 - i0 + 1;
        // How much it acts at each point of its box.
        let weights: Vec<f64> = (j0..=j1)
            .flat_map(|j| (i0..=i1).map(move |i| (i, j)))
            .map(|(i, j)| stroke.weight(self.point(i, j)) * self.ease(j * self.nx + i))
            .collect();
        let at = |i: usize, j: usize| (j - j0) * w + (i - i0);
        match stroke.brush {
            Brush::Raise | Brush::Noise | Brush::Flatten(_) => {
                for j in j0..=j1 {
                    for i in i0..=i1 {
                        let k = weights[at(i, j)];
                        if k <= 0.0 {
                            continue;
                        }
                        let v = j * self.nx + i;
                        let p = self.point(i, j);
                        let h = &mut self.z[v];
                        match stroke.brush {
                            Brush::Raise => *h += stroke.strength * k,
                            Brush::Noise => {
                                let scale = (0.3 * stroke.radius).max(2.0 * self.cell);
                                *h += stroke.strength * k * noise(p / scale);
                            }
                            Brush::Flatten(level) => *h += (level - *h) * stroke.strength * k,
                            _ => unreachable!(),
                        }
                    }
                }
            }
            Brush::Smooth => {
                // Rounds of evening out with the neighbours, as many as reach across
                // the brush.
                let rounds = (stroke.radius / self.cell).round().clamp(1.0, 12.0) as usize;
                let nx = self.nx;
                // The box and a point round it, as it was before each round.
                let (bi0, bj0) = (i0.saturating_sub(1), j0.saturating_sub(1));
                let (bi1, bj1) = ((i1 + 1).min(nx - 1), (j1 + 1).min(self.ny - 1));
                let bw = bi1 - bi0 + 1;
                let mut before = vec![0.0; bw * (bj1 - bj0 + 1)];
                for _ in 0..rounds {
                    for j in bj0..=bj1 {
                        before[(j - bj0) * bw..][..bw]
                            .copy_from_slice(&self.z[j * nx + bi0..=j * nx + bi1]);
                    }
                    let old = |i: usize, j: usize| before[(j - bj0) * bw + (i - bi0)];
                    for j in j0.max(1)..=j1.min(self.ny - 2) {
                        for i in i0.max(1)..=i1.min(nx - 2) {
                            let k = weights[at(i, j)] * stroke.strength;
                            if k <= 0.0 {
                                continue;
                            }
                            let mean = 0.25
                                * (old(i - 1, j) + old(i + 1, j) + old(i, j - 1) + old(i, j + 1));
                            self.z[j * nx + i] += (mean - old(i, j)) * k;
                        }
                    }
                }
            }
            Brush::Paint | Brush::Erase => {}
        }
    }

    /// The ground's normal at point (i, j).
    pub fn normal(&self, i: usize, j: usize) -> DVec3 {
        let h = |i: usize, j: usize| self.z[j.min(self.ny - 1) * self.nx + i.min(self.nx - 1)];
        let dx = h(i + 1, j) - h(i.saturating_sub(1), j);
        let dy = h(i, j + 1) - h(i, j.saturating_sub(1));
        DVec3::new(-dx, -dy, 2.0 * self.cell).normalize()
    }

    /// The rendered chunks: each one's first and last columns and rows.
    pub fn chunks(&self) -> Vec<[usize; 4]> {
        (0..self.ny - 1)
            .step_by(CHUNK)
            .flat_map(|j0| {
                (0..self.nx - 1).step_by(CHUNK).map(move |i0| {
                    [
                        i0,
                        j0,
                        (i0 + CHUNK).min(self.nx - 1),
                        (j0 + CHUNK).min(self.ny - 1),
                    ]
                })
            })
            .collect()
    }

    /// The mesh of the points from (i0, j0) to (i1, j1), with texture coordinates as
    /// `uv` gives them for a point.
    pub fn mesh(
        &self,
        [i0, j0, i1, j1]: [usize; 4],
        uv: Option<&dyn Fn(DVec2) -> [f32; 2]>,
    ) -> MeshData {
        let mut m = MeshData::default();
        let w = i1 - i0 + 1;
        for j in j0..=j1 {
            for i in i0..=i1 {
                let p = self.point(i, j);
                m.positions
                    .push([p.x as f32, p.y as f32, self.z[j * self.nx + i] as f32]);
                m.normals.push(self.normal(i, j).as_vec3().to_array());
                if let Some(uv) = uv {
                    m.uvs.push(uv(p));
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
    }
}

/// Smooth noise in [-1, 1] with features about a unit apart, the same everywhere
/// every time.
fn noise(p: DVec2) -> f64 {
    let hash = |x: i64, y: i64| {
        let mut h = (x as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ (y as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
        h ^= h >> 29;
        h = h.wrapping_mul(0xbf58_476d_1ce4_e5b9);
        h ^= h >> 32;
        (h & 0xffff) as f64 / 32767.5 - 1.0
    };
    let (x0, y0) = (p.x.floor(), p.y.floor());
    let (fx, fy) = (p.x - x0, p.y - y0);
    let s = |t: f64| t * t * (3.0 - 2.0 * t);
    let (sx, sy) = (s(fx), s(fy));
    let (x0, y0) = (x0 as i64, y0 as i64);
    let a = hash(x0, y0) + (hash(x0 + 1, y0) - hash(x0, y0)) * sx;
    let b = hash(x0, y0 + 1) + (hash(x0 + 1, y0 + 1) - hash(x0, y0 + 1)) * sx;
    a + (b - a) * sy
}

/// Weights of the ground's own material and its painted layers over the terrain: a
/// texture the renderer blends the layers' textures by, and what the physics takes
/// each triangle's surface from.
#[derive(Clone, Debug, PartialEq)]
pub struct PaintMask {
    /// Where the mask's corner is, and how big it is, m.
    pub lo: DVec2,
    pub size: DVec2,
    pub width: usize,
    pub height: usize,
    /// R the ground's own material, G, B and A the layers, 0 to 255 adding up to 255,
    /// row by row from `lo` up y.
    pub rgba: Vec<u8>,
}

impl PaintMask {
    /// A mask over the box from `lo` to `hi` of texels about `texel` m, all the ground's
    /// own material.
    pub fn new(lo: DVec2, hi: DVec2, texel: f64) -> Self {
        let size = (hi - lo).max(DVec2::splat(1.0));
        let texel = texel.max(size.x.max(size.y) / MAX_MASK as f64);
        let width = ((size.x / texel).ceil() as usize).clamp(1, MAX_MASK);
        let height = ((size.y / texel).ceil() as usize).clamp(1, MAX_MASK);
        Self {
            lo,
            size,
            width,
            height,
            rgba: [255, 0, 0, 0].repeat(width * height),
        }
    }

    /// Where `p` is on the mask, 0 to 1 across each way.
    pub fn uv(&self, p: DVec2) -> [f32; 2] {
        let q = (p - self.lo) / self.size;
        [q.x as f32, q.y as f32]
    }

    /// The texel's middle.
    fn centre(&self, x: usize, y: usize) -> DVec2 {
        self.lo
            + DVec2::new(
                (x as f64 + 0.5) * self.size.x / self.width as f64,
                (y as f64 + 0.5) * self.size.y / self.height as f64,
            )
    }

    /// The texels within the box from `lo` to `hi`.
    pub fn span(&self, lo: DVec2, hi: DVec2) -> Option<[usize; 4]> {
        let texel = self.size / DVec2::new(self.width as f64, self.height as f64);
        let a = ((lo - self.lo) / texel).floor().max(DVec2::ZERO);
        let b = ((hi - self.lo) / texel).floor();
        let max = DVec2::new((self.width - 1) as f64, (self.height - 1) as f64);
        if b.x < 0.0 || b.y < 0.0 || a.x > max.x || a.y > max.y || !(a.is_finite() && b.is_finite())
        {
            return None;
        }
        let b = b.min(max);
        Some([a.x as usize, a.y as usize, b.x as usize, b.y as usize])
    }

    /// Paints one stroke: its layer's weight grows towards all of it, the others' shrink,
    /// by the stroke's strength and weight. `layers` are the terrain's layers.
    pub fn paint(&mut self, layers: &[GroundLayer], s: &LayerStroke) {
        let channel = match &s.layer {
            None => 0,
            Some(name) => match layers.iter().position(|l| &l.name == name) {
                Some(i) if i < 3 => i + 1,
                _ => return,
            },
        };
        let (lo, hi) = s.stroke.bounds();
        let Some([x0, y0, x1, y1]) = self.span(lo, hi) else {
            return;
        };
        for y in y0..=y1 {
            for x in x0..=x1 {
                let k = s.stroke.strength * s.stroke.weight(self.centre(x, y));
                if k <= 0.0 {
                    continue;
                }
                let texel = &mut self.rgba[(y * self.width + x) * 4..][..4];
                let mut w = [0.0; 4];
                for (v, &c) in w.iter_mut().zip(texel.iter()) {
                    *v = c as f64 / 255.0;
                }
                for (c, v) in w.iter_mut().enumerate() {
                    let target = if c == channel { 1.0 } else { 0.0 };
                    *v += (target - *v) * k;
                }
                // Whole numbers that add up to 255, the rounding left with the layer.
                let mut q = w.map(|v| (v * 255.0).round().clamp(0.0, 255.0) as i32);
                let rest = 255 - q.iter().sum::<i32>();
                q[channel] = (q[channel] + rest).clamp(0, 255);
                for (t, v) in texel.iter_mut().zip(q) {
                    *t = v as u8;
                }
            }
        }
    }

    /// The weights at `p` of the texel it is in.
    pub fn weights(&self, p: DVec2) -> [u8; 4] {
        let q = (p - self.lo) / self.size;
        let x = ((q.x * self.width as f64).floor().max(0.0) as usize).min(self.width - 1);
        let y = ((q.y * self.height as f64).floor().max(0.0) as usize).min(self.height - 1);
        let i = (y * self.width + x) * 4;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }
}

/// The terrain built.
#[derive(Clone, Debug)]
pub struct TerrainBuild {
    /// Rendered chunks, as `grid.chunks()` lists them.
    pub chunks: Vec<MeshData>,
    /// The ground for the physics (no UVs), in parts of one surface each: the index of
    /// the surface in `Project::surfaces`, and the mesh.
    pub solid: Vec<(usize, MeshData)>,
    /// The heights, sculpted and kept under the roads.
    pub grid: Arc<Grid>,
    /// The painted layers' weights, if the terrain has layers. The chunks' texture
    /// coordinates are then places on it, and the layers' textures are laid by the
    /// world's x and y.
    pub mask: Option<Arc<PaintMask>>,
}

/// Frames of every road in buckets over the plane.
pub(crate) struct Lookup {
    origin: DVec2,
    size: (usize, usize),
    buckets: Vec<Vec<(usize, usize)>>,
}

impl Lookup {
    pub(crate) fn new(roads: &[RoadBuild], lo: DVec2, hi: DVec2) -> Self {
        let size = (
            ((hi.x - lo.x) / BUCKET).ceil().max(0.0) as usize + 1,
            ((hi.y - lo.y) / BUCKET).ceil().max(0.0) as usize + 1,
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
    pub(crate) fn nearest(&self, roads: &[RoadBuild], p: DVec2) -> Option<(usize, usize)> {
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

    /// How far `p` is beyond the outer edge of the road nearest to it, m: negative on
    /// the road or its strips.
    pub(crate) fn beyond(&self, roads: &[RoadBuild], p: DVec2) -> Option<f64> {
        let (r, k) = self.nearest(roads, p)?;
        let b = &roads[r];
        let f = &b.sampled.frames[k];
        let flat_left = DVec3::Z.cross(f.tangent).truncate().normalize_or(DVec2::Y);
        let d = (p - f.pos.truncate()).dot(flat_left);
        let [(left, _), (right, _)] = b.edges(k);
        Some(if d > left {
            d - left
        } else if d < right {
            right - d
        } else {
            -(left - d).min(d - right)
        })
    }
}

/// Whether two terrains give the same `base` grid: everything but the brushes.
pub fn same_base(a: &Terrain, b: &Terrain) -> bool {
    a.enabled == b.enabled
        && a.margin == b.margin
        && a.cell == b.cell
        && a.heights == b.heights
        && a.heights_offset == b.heights_offset
        && a.landforms == b.landforms
}

/// The ground's heights round the built roads, before sculpting: level with their edges
/// and smoothed between them, following `heights` (the project's elevation data read
/// in) away from them, and shaped by the landforms.
pub fn base(
    project: &Project,
    roads: &[RoadBuild],
    heights: Option<&crate::dem::Heights>,
) -> Option<Grid> {
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
    if !lo.is_finite() {
        return None;
    }
    lo -= DVec2::splat(t.margin);
    hi += DVec2::splat(t.margin);
    let nx = ((hi.x - lo.x) / t.cell).ceil() as usize + 1;
    let ny = ((hi.y - lo.y) / t.cell).ceil() as usize + 1;
    let lookup = Lookup::new(roads, lo, hi);

    let offset = t.heights_offset;
    // Heights, whether smoothing may move them, and how far beyond the roads' edges
    // each is, a row at a time in parallel.
    let rows: Vec<(Vec<f64>, Vec<bool>, Vec<f64>)> = (0..ny)
        .into_par_iter()
        .map(|j| {
            let mut z = vec![0.0; nx];
            let mut free = vec![false; nx];
            let mut away = vec![0.0; nx];
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
                    away[i] = beyond;
                    // Far enough out, the ground is the real place's: held there.
                    let real = heights
                        .filter(|_| beyond > HELD + BLEND)
                        .and_then(|h| h.at(p))
                        .map(|h| h + offset);
                    free[i] = beyond > HELD && real.is_none();
                    real.unwrap_or(edge.z - AT_EDGES)
                };
            }
            (z, free, away)
        })
        .collect();
    let mut z: Vec<f64> = rows.iter().flat_map(|r| r.0.iter().copied()).collect();
    let free: Vec<bool> = rows.iter().flat_map(|r| r.1.iter().copied()).collect();
    let away: Vec<f64> = rows.iter().flat_map(|r| r.2.iter().copied()).collect();
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
    let mut grid = Grid {
        lo,
        cell: t.cell,
        nx,
        ny,
        z,
        away,
    };

    // Landforms shape the ground away from the roads, coming in gently beyond the
    // held ground so that it still meets their edges.
    if !t.landforms.is_empty() {
        let g = &grid;
        let shaped: Vec<f64> = (0..nx * ny)
            .into_par_iter()
            .map(|v| {
                let mut h = g.z[v];
                let ease = g.ease(v);
                if ease <= 0.0 {
                    return h;
                }
                let p = g.point(v % nx, v / nx);
                for l in &t.landforms {
                    let w = l.weight(p) * ease;
                    if w <= 0.0 {
                        continue;
                    }
                    h = match l.kind {
                        crate::project::LandformKind::Raise(d) => h + d * w,
                        crate::project::LandformKind::Level(v) => h + (v - h) * w,
                    };
                }
                h
            })
            .collect();
        grid.z = shaped;
    }
    Some(grid)
}

/// The terrain from its `base` grid: sculpted, kept under the roads, painted, and made
/// into meshes.
pub fn finish(project: &Project, roads: &[RoadBuild], base: &Grid) -> TerrainBuild {
    let t = &project.terrain;
    let mut grid = base.clone();
    for s in &t.sculpt {
        grid.sculpt(s);
    }
    clamp_under(&mut grid, roads);

    let mask = (!t.layers.is_empty()).then(|| {
        let mut m = PaintMask::new(grid.lo, grid.hi(), t.paint_texel);
        for s in &t.paint {
            m.paint(&t.layers, s);
        }
        m
    });
    let tile = project
        .material_index(&t.material)
        .map_or([1.0; 2], |m| project.materials[m].tile);
    let tiled = |p: DVec2| [p.x as f32 / tile[0], p.y as f32 / tile[1]];
    let on_mask = |p: DVec2| mask.as_ref().expect("a mask").uv(p);
    let uv: &(dyn Fn(DVec2) -> [f32; 2] + Sync) = if mask.is_some() { &on_mask } else { &tiled };
    let chunks = grid
        .chunks()
        .into_par_iter()
        .map(|c| grid.mesh(c, Some(&|p| uv(p))))
        .collect();
    let solid = solid(project, &grid, mask.as_ref());
    TerrainBuild {
        chunks,
        solid,
        grid: Arc::new(grid),
        mask: mask.map(Arc::new),
    }
}

/// The terrain round the built roads: `base`, then `finish`.
pub fn build(
    project: &Project,
    roads: &[RoadBuild],
    heights: Option<&crate::dem::Heights>,
) -> Option<TerrainBuild> {
    let base = base(project, roads, heights)?;
    Some(finish(project, roads, &base))
}

/// The ground for the physics, in parts of one surface each: the ground's own surface,
/// and where a layer is painted over most of a cell, that layer's.
fn solid(project: &Project, grid: &Grid, mask: Option<&PaintMask>) -> Vec<(usize, MeshData)> {
    let t = &project.terrain;
    let own = project.surface_index(&t.surface).unwrap_or(0);
    let whole = grid.mesh([0, 0, grid.nx - 1, grid.ny - 1], None);
    let Some(mask) = mask else {
        return vec![(own, whole)];
    };
    let surfaces: Vec<usize> = std::iter::once(own)
        .chain(
            t.layers
                .iter()
                .map(|l| project.surface_index(&l.surface).unwrap_or(own)),
        )
        .collect();
    // Each cell's surface: what most of it is painted with at its middle.
    let (nx, ny) = (grid.nx, grid.ny);
    let cell_surface = |i: usize, j: usize| {
        let w = mask.weights(grid.point(i, j) + DVec2::splat(0.5 * grid.cell));
        let c = (0..surfaces.len()).max_by_key(|&c| w[c]).unwrap_or(0);
        surfaces[c]
    };
    let mut parts: Vec<(usize, Vec<u32>)> = Vec::new();
    for j in 0..ny - 1 {
        for i in 0..nx - 1 {
            let s = cell_surface(i, j);
            let list = match parts.iter().position(|(k, _)| *k == s) {
                Some(k) => &mut parts[k].1,
                None => {
                    parts.push((s, Vec::new()));
                    &mut parts.last_mut().expect("just pushed").1
                }
            };
            let a = (j * nx + i) as u32;
            let (b, c, d) = (a + 1, a + nx as u32, a + nx as u32 + 1);
            list.extend([a, b, d, a, d, c]);
        }
    }
    parts
        .into_iter()
        .map(|(s, indices)| (s, compact(&whole, &indices)))
        .collect()
}

/// The part of `m` its triangles `indices` use, its vertices numbered afresh.
fn compact(m: &MeshData, indices: &[u32]) -> MeshData {
    let mut new = vec![u32::MAX; m.positions.len()];
    let mut out = MeshData::default();
    for &i in indices {
        let slot = &mut new[i as usize];
        if *slot == u32::MAX {
            *slot = out.positions.len() as u32;
            out.positions.push(m.positions[i as usize]);
            out.normals.push(m.normals[i as usize]);
        }
        out.indices.push(*slot);
    }
    out
}

/// Lowers the corners of every cell that a road's surface or strip triangle reaches
/// below that triangle's lowest vertex. The ground is linear within a cell, so it then
/// stays under the roads wherever they are, on grades, in sags and over banking alike.
fn clamp_under(grid: &mut Grid, roads: &[RoadBuild]) {
    let (lo, cell, nx, ny) = (grid.lo, grid.cell, grid.nx, grid.ny);
    let z = &mut grid.z;
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
    use crate::project::{Landform, LandformKind};

    /// The terrain's height nearest `p`.
    fn height(t: &TerrainBuild, p: DVec2) -> f64 {
        let v = t
            .solid
            .iter()
            .flat_map(|(_, m)| &m.positions)
            .min_by(|a, b| {
                let d = |q: &[f32; 3]| DVec2::new(q[0] as f64, q[1] as f64).distance(p);
                d(a).total_cmp(&d(b))
            })
            .unwrap();
        v[2] as f64
    }

    #[test]
    fn away_from_the_roads_the_ground_follows_elevation_data_and_landforms() {
        let mut project = Project::new("t");
        // Flat elevation data 20 m up over the whole place.
        let asc = "ncols 3\nnrows 3\nxllcorner -1000\nyllcorner -1000\ncellsize 1000\n20 20 20\n20 20 20\n20 20 20\n";
        let heights = crate::dem::read("ground.asc", asc, None).unwrap();
        project.terrain.heights_offset = -5.0;
        project.terrain.landforms = vec![
            Landform {
                name: "pad".into(),
                center: DVec2::new(200.0, -150.0),
                to: None,
                radius: 20.0,
                falloff: 10.0,
                kind: LandformKind::Level(2.0),
            },
            Landform {
                name: "bank".into(),
                center: DVec2::new(0.0, 400.0),
                to: Some(DVec2::new(200.0, 400.0)),
                radius: 10.0,
                falloff: 20.0,
                kind: LandformKind::Raise(6.0),
            },
        ];
        let road = crate::road::build(&project, 0);
        let t = build(&project, std::slice::from_ref(&road), Some(&heights)).unwrap();
        // Far out: the data's 20 m, less the offset.
        assert!((height(&t, DVec2::new(-300.0, -150.0)) - 15.0).abs() < 0.01);
        // The pad, level at 2 m; along the bank, 6 m up.
        assert!((height(&t, DVec2::new(200.0, -150.0)) - 2.0).abs() < 0.01);
        assert!((height(&t, DVec2::new(100.0, 400.0)) - 21.0).abs() < 0.01);
        // The road still lies on the ground under it.
        let f = &road.sampled.frames[10];
        assert!(height(&t, f.pos.truncate()) < f.pos.z);
    }

    #[test]
    fn ground_lies_under_the_road() {
        let project = Project::new("t");
        let road = crate::road::build(&project, 0);
        let terrain = build(&project, std::slice::from_ref(&road), None).unwrap();
        let f = &road.sampled.frames[10];
        // The grid vertex nearest the road's centre is below it.
        assert!(height(&terrain, f.pos.truncate()) < f.pos.z - 0.1);
        // Every triangle faces up.
        for (_, m) in &terrain.solid {
            for &t in m.indices.as_chunks::<3>().0 {
                let p = t.map(|i| DVec3::from(m.positions[i as usize].map(f64::from)));
                assert!((p[1] - p[0]).cross(p[2] - p[0]).z > 0.0);
            }
        }
    }

    fn stroke(brush: Brush, strength: f64, points: &[(f64, f64)]) -> Stroke {
        Stroke {
            brush,
            radius: 20.0,
            strength,
            points: points.iter().map(|&(x, y)| DVec2::new(x, y)).collect(),
            fill: false,
        }
    }

    #[test]
    fn strokes_sculpt_the_ground_away_from_the_road_only() {
        let mut project = Project::new("t");
        project.terrain.cell = 4.0;
        let road = crate::road::build(&project, 0);
        let roads = std::slice::from_ref(&road);
        let flat = build(&project, roads, None).unwrap();
        // Inside the oval, well away from it: raised 3 m along a line, then a pad
        // levelled at 1 m.
        let hill = DVec2::new(150.0, 130.0);
        project.terrain.sculpt = vec![
            stroke(Brush::Raise, 3.0, &[(120.0, 130.0), (180.0, 130.0)]),
            stroke(Brush::Flatten(1.0), 1.0, &[(250.0, 100.0)]),
        ];
        let t = build(&project, roads, None).unwrap();
        let rise = height(&t, hill) - height(&flat, hill);
        assert!((rise - 3.0).abs() < 0.01, "{rise}");
        assert!((height(&t, DVec2::new(250.0, 100.0)) - 1.0).abs() < 0.01);
        // A stroke across the road leaves the road's edges where they were.
        project.terrain.sculpt = vec![stroke(Brush::Raise, 5.0, &[(100.0, -30.0), (100.0, 30.0)])];
        let t = build(&project, roads, None).unwrap();
        let f = &road.sampled.frames[road.sampled.nearest(DVec3::new(100.0, 0.0, 0.0))];
        assert!(height(&t, f.pos.truncate()) < f.pos.z);
        // Smoothing evens out what was raised; noise roughens, the same every time.
        let g = |p: &Project| build(p, roads, None).unwrap().grid;
        project.terrain.sculpt = vec![stroke(Brush::Raise, 6.0, &[(150.0, 130.0)])];
        let peak = g(&project).height_at(hill).unwrap();
        project
            .terrain
            .sculpt
            .push(stroke(Brush::Smooth, 1.0, &[(150.0, 130.0)]));
        assert!(g(&project).height_at(hill).unwrap() < peak - 0.1);
        project.terrain.sculpt = vec![stroke(Brush::Noise, 2.0, &[(150.0, 130.0)])];
        assert_eq!(g(&project), g(&project));
    }

    #[test]
    fn a_lasso_stroke_acts_fully_inside_its_outline() {
        let mut project = Project::new("t");
        project.terrain.cell = 4.0;
        let road = crate::road::build(&project, 0);
        let roads = std::slice::from_ref(&road);
        let flat = build(&project, roads, None).unwrap().grid;
        // A pad of 100 × 80 m inside the oval, raised 3 m, softened over 10 m outside.
        project.terrain.sculpt = vec![Stroke {
            fill: true,
            radius: 10.0,
            ..stroke(
                Brush::Raise,
                3.0,
                &[(100.0, 90.0), (200.0, 90.0), (200.0, 170.0), (100.0, 170.0)],
            )
        }];
        let g = build(&project, roads, None).unwrap().grid;
        let rise = |p: DVec2| g.height_at(p).unwrap() - flat.height_at(p).unwrap();
        for p in [(110.0, 100.0), (150.0, 130.0), (195.0, 165.0)] {
            assert!((rise(DVec2::new(p.0, p.1)) - 3.0).abs() < 1e-6, "{p:?}");
        }
        assert!(rise(DVec2::new(205.0, 130.0)) > 0.1);
        assert!(rise(DVec2::new(215.0, 130.0)).abs() < 1e-6);
    }

    #[test]
    fn painted_layers_blend_and_give_their_grip() {
        use crate::project::{GroundLayer, LayerStroke};
        let mut project = Project::new("t");
        project.terrain.layers = vec![GroundLayer {
            name: "sand".into(),
            surface: "gravel".into(),
            material: "gravel".into(),
        }];
        let paint = |layer: Option<&str>, at: (f64, f64)| LayerStroke {
            layer: layer.map(str::to_string),
            stroke: stroke(Brush::Paint, 1.0, &[at]),
        };
        project.terrain.paint = vec![
            paint(Some("sand"), (150.0, 130.0)),
            paint(None, (180.0, 130.0)),
        ];
        let road = crate::road::build(&project, 0);
        let t = build(&project, std::slice::from_ref(&road), None).unwrap();
        let mask = t.mask.as_ref().unwrap();
        // Sand where only the first stroke went, the grass back where the second did.
        assert_eq!(mask.weights(DVec2::new(145.0, 130.0)), [0, 255, 0, 0]);
        assert_eq!(mask.weights(DVec2::new(180.0, 130.0)), [255, 0, 0, 0]);
        let w = mask.weights(DVec2::new(165.0, 130.0));
        assert_eq!(w.iter().map(|&c| c as u32).sum::<u32>(), 255);
        // The physics: gravel under the sand.
        let gravel = project.surface_index("gravel").unwrap();
        let part = t.solid.iter().find(|(s, _)| *s == gravel).unwrap();
        assert!(!part.1.is_empty());
        // Chunks lie on the mask.
        let uv = t.chunks[0].uvs[0];
        assert!((0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1]));
    }
}
