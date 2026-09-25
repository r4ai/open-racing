//! Where roads overlap (a pit lane beside the main straight, a junction), the paved
//! surface wins: strips of one road dip under another road's surface, and barriers are
//! cut away where they would stand across it.

use glam::DVec2;

use crate::road::{Layer, MeshData, RoadBuild};

/// Edge of the buckets frames are looked up in, m.
const BUCKET: f64 = 16.0;
/// How far strips stay under another road's surface, m.
const UNDER: f64 = 0.1;
/// Clearance kept between another road's edge and a barrier, m.
const CLEARANCE: f64 = 1.0;

/// The paved surfaces of all roads, for asking whether a point is on one.
pub struct Footprints<'a> {
    roads: &'a [RoadBuild],
    lo: DVec2,
    size: (usize, usize),
    /// (road, frame) per bucket.
    buckets: Vec<Vec<(usize, usize)>>,
}

impl<'a> Footprints<'a> {
    pub fn new(roads: &'a [RoadBuild]) -> Self {
        let (mut lo, mut hi) = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
        for b in roads {
            for f in &b.sampled.frames {
                lo = lo.min(f.pos.truncate());
                hi = hi.max(f.pos.truncate());
            }
        }
        if !lo.is_finite() {
            lo = DVec2::ZERO;
            hi = DVec2::ZERO;
        }
        let size = (
            ((hi.x - lo.x) / BUCKET) as usize + 1,
            ((hi.y - lo.y) / BUCKET) as usize + 1,
        );
        let mut buckets = vec![Vec::new(); size.0 * size.1];
        for (r, b) in roads.iter().enumerate() {
            for (k, f) in b.sampled.frames.iter().enumerate() {
                let (x, y) = Self::cell(lo, size, f.pos.truncate());
                buckets[y * size.0 + x].push((r, k));
            }
        }
        Self {
            roads,
            lo,
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

    /// Height of the paved surface of a road other than `except` under `p`, if there
    /// is one within `margin` of it, and the highest if several.
    pub fn surface_under(&self, p: DVec2, except: usize, margin: f64) -> Option<f64> {
        // Frames within reach: roads are rarely wider than a bucket each side.
        let reach = 2;
        let (cx, cy) = Self::cell(self.lo, self.size, p);
        let mut best: Vec<(usize, usize, f64)> = Vec::new();
        for y in cy.saturating_sub(reach)..=(cy + reach).min(self.size.1 - 1) {
            for x in cx.saturating_sub(reach)..=(cx + reach).min(self.size.0 - 1) {
                for &(r, k) in &self.buckets[y * self.size.0 + x] {
                    if r == except {
                        continue;
                    }
                    let d2 = self.roads[r].sampled.frames[k]
                        .pos
                        .truncate()
                        .distance_squared(p);
                    match best.iter_mut().find(|b| b.0 == r) {
                        Some(b) if d2 < b.2 => *b = (r, k, d2),
                        Some(_) => {}
                        None => best.push((r, k, d2)),
                    }
                }
            }
        }
        best.into_iter()
            .filter_map(|(r, k, _)| {
                let b = &self.roads[r];
                let smp = &b.sampled;
                let f = &smp.frames[k];
                let t = f.tangent.truncate().normalize_or(DVec2::X);
                let left = DVec2::new(-t.y, t.x);
                let rel = p - f.pos.truncate();
                // Past the ends of an open road there is no road.
                let spacing = smp.length / smp.frames.len().max(1) as f64;
                let along = rel.dot(t);
                let last = smp.frames.len() - 1;
                if !smp.closed && ((k == 0 && along < -margin) || (k == last && along > margin))
                    || along.abs() > spacing + margin
                {
                    return None;
                }
                let d = rel.dot(left);
                if d > f.width_left + margin || d < -f.width_right - margin {
                    return None;
                }
                let d = d.clamp(-f.width_right, f.width_left);
                // The surface's height across, with the crown, at the nearest frame
                // moved along the tangent's slope.
                let point = f.pos + f.lateral * d + f.normal * b.surface_height(k, d);
                Some(point.z + f.tangent.z / t.length().max(1e-9) * along)
            })
            .reduce(f64::max)
    }
}

/// Lowers strips under other roads' surfaces and cuts barriers across them.
pub fn resolve(roads: &mut [RoadBuild]) {
    // Work out the changes against the roads as built, then apply them.
    use rayon::prelude::*;
    let edits: Vec<Vec<Edit>> = {
        let fp = Footprints::new(roads);
        roads
            .iter()
            .enumerate()
            .map(|(r, b)| {
                let parts: Vec<(Layer, &MeshData)> = b
                    .visual
                    .iter()
                    .map(|part| (part.layer, &part.mesh))
                    .chain(b.solid.iter().map(|part| (part.layer, &part.mesh)))
                    .collect();
                parts
                    .into_par_iter()
                    .map(|(layer, mesh)| edit(&fp, r, layer, mesh))
                    .collect()
            })
            .collect()
    };
    for (b, edits) in roads.iter_mut().zip(edits) {
        let meshes = b
            .visual
            .iter_mut()
            .map(|p| &mut p.mesh)
            .chain(b.solid.iter_mut().map(|p| &mut p.mesh));
        for (mesh, e) in meshes.zip(edits) {
            match e {
                Edit::None => {}
                Edit::Heights(z) => {
                    for (p, z) in mesh.positions.iter_mut().zip(z) {
                        p[2] = z;
                    }
                }
                Edit::Indices(i) => mesh.indices = i,
            }
        }
        b.visual.retain(|p| !p.mesh.is_empty());
        b.solid.retain(|p| !p.mesh.is_empty());
    }
}

enum Edit {
    None,
    Heights(Vec<f32>),
    Indices(Vec<u32>),
}

fn edit(fp: &Footprints, road: usize, layer: Layer, mesh: &MeshData) -> Edit {
    let xy = |p: &[f32; 3]| DVec2::new(p[0] as f64, p[1] as f64);
    match layer {
        Layer::Strip => {
            let mut changed = false;
            let z = mesh
                .positions
                .iter()
                .map(|p| match fp.surface_under(xy(p), road, 0.0) {
                    Some(h) if (p[2] as f64) > h - UNDER => {
                        changed = true;
                        (h - UNDER) as f32
                    }
                    _ => p[2],
                })
                .collect();
            if changed {
                Edit::Heights(z)
            } else {
                Edit::None
            }
        }
        Layer::Barrier => {
            let blocked: Vec<bool> = mesh
                .positions
                .iter()
                .map(|p| fp.surface_under(xy(p), road, CLEARANCE).is_some())
                .collect();
            if !blocked.contains(&true) {
                return Edit::None;
            }
            Edit::Indices(
                mesh.indices
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .filter(|t| t.iter().all(|&i| !blocked[i as usize]))
                    .flatten()
                    .copied()
                    .collect(),
            )
        }
        Layer::Surface | Layer::Line => Edit::None,
    }
}
