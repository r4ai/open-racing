//! Models scattered over the ground where brush strokes painted them: woods, bushes,
//! rocks. Each scatter has a grid of places `spacing` apart, each jittered within its
//! cell; its strokes paint how much of the scatter each place has, in order, and a
//! place has a copy where that is above a threshold of its own. So a stroke painted
//! lightly gives fewer copies, a stroke painted again more, and the same strokes the
//! same copies every build. Copies keep off the roads and their strips, off drivable
//! splines, and off ground too steep.

use std::collections::HashMap;
use std::sync::Arc;

use glam::{DMat3, DQuat, DVec2, DVec3};
use open_racing_sim::{GroundMesh, Surface};
use open_racing_track::Mesh;

use crate::model::Model;
use crate::project::{Brush, Scatter};
use crate::road::RoadBuild;
use crate::terrain::Lookup;

/// Copies merged into one mesh stand within tiles of this size, m, so that far ones are
/// culled.
const TILE: f64 = 120.0;

/// One copy of a scatter's model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Copy {
    /// Which of the scatter's models.
    pub model: usize,
    pub pos: DVec3,
    /// Turn about its up, radians.
    pub yaw: f64,
    pub scale: f64,
    /// Which way its up points.
    pub up: DVec3,
}

impl Copy {
    /// Its turn: upright to its `up`, then about it.
    pub fn rotation(&self) -> DMat3 {
        DMat3::from_quat(DQuat::from_rotation_arc(
            DVec3::Z,
            self.up.normalize_or(DVec3::Z),
        )) * DMat3::from_rotation_z(self.yaw)
    }
}

/// A number in [0, 1) for a place of a scatter, the same every time.
fn hash(seed: u64, i: i64, j: i64, what: u64) -> f64 {
    let mut h = seed
        ^ (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ (j as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f)
        ^ what.wrapping_mul(0x1656_67b1_9e37_79f9);
    h ^= h >> 30;
    h = h.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^= h >> 31;
    (h >> 11) as f64 / (1u64 << 53) as f64
}

fn seed(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
    })
}

/// A place of the grid: the cell it is in, and where in it.
fn place(s: &Scatter, seed: u64, i: i64, j: i64) -> DVec2 {
    let jitter = DVec2::new(hash(seed, i, j, 1), hash(seed, i, j, 2));
    (DVec2::new(i as f64, j as f64) + DVec2::splat(0.1) + jitter * 0.8) * s.spacing
}

/// How much of the scatter each place has, by its cell, after its strokes.
pub fn coverage(s: &Scatter) -> HashMap<(i64, i64), f64> {
    let seed = seed(&s.name);
    let mut cover: HashMap<(i64, i64), f64> = HashMap::new();
    for stroke in &s.strokes {
        let (lo, hi) = stroke.bounds();
        let (a, b) = ((lo / s.spacing).floor(), (hi / s.spacing).floor());
        if !(a.is_finite() && b.is_finite()) {
            continue;
        }
        for j in a.y as i64..=b.y as i64 {
            for i in a.x as i64..=b.x as i64 {
                let k = stroke.strength * stroke.weight(place(s, seed, i, j));
                if k <= 0.0 {
                    continue;
                }
                let c = cover.entry((i, j)).or_insert(0.0);
                match stroke.brush {
                    Brush::Erase => *c -= *c * k,
                    _ => *c += (1.0 - *c) * k,
                }
            }
        }
    }
    cover.retain(|_, c| *c > 0.0);
    cover
}

/// The copies of a scatter as painted, before they are put on the ground: where each
/// stands in the plane, and which model it is, its turn and size.
pub fn planned(s: &Scatter) -> Vec<(DVec2, usize, f64, f64)> {
    let seed = seed(&s.name);
    let total: f64 = s.models.iter().map(|m| m.weight).sum();
    let mut out: Vec<_> = coverage(s)
        .into_iter()
        .filter(|&((i, j), c)| c > hash(seed, i, j, 3))
        .map(|((i, j), _)| {
            let mut pick = hash(seed, i, j, 4) * total;
            let model = s
                .models
                .iter()
                .position(|m| {
                    pick -= m.weight;
                    pick < 0.0
                })
                .unwrap_or(s.models.len() - 1);
            let yaw = hash(seed, i, j, 5) * std::f64::consts::TAU;
            let scale = s.scale[0] + (s.scale[1] - s.scale[0]) * hash(seed, i, j, 6);
            (place(s, seed, i, j), model, yaw, scale)
        })
        .collect();
    // In a fixed order, whatever the map's.
    out.sort_by(|a, b| (a.0.y, a.0.x).partial_cmp(&(b.0.y, b.0.x)).expect("finite"));
    out
}

/// What lies in the way of copies: the roads (and their strips), to keep `clearance`
/// from.
pub struct Keepout<'a> {
    roads: &'a [RoadBuild],
    lookup: Option<Lookup>,
}

impl<'a> Keepout<'a> {
    pub fn new(roads: &'a [RoadBuild]) -> Self {
        let (mut lo, mut hi) = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
        for f in roads.iter().flat_map(|b| &b.sampled.frames) {
            lo = lo.min(f.pos.truncate());
            hi = hi.max(f.pos.truncate());
        }
        Self {
            roads,
            lookup: lo.is_finite().then(|| Lookup::new(roads, lo, hi)),
        }
    }

    /// How far `p` is beyond the outer edges of the roads, m (negative on them).
    fn beyond(&self, p: DVec2) -> f64 {
        self.lookup
            .as_ref()
            .and_then(|l| l.beyond(self.roads, p))
            .unwrap_or(f64::INFINITY)
    }
}

/// The copies of a scatter, stood on `ground`: none on the roads or within its
/// clearance of them, on drivable splines, run-off or gravel, or on ground steeper than
/// it allows.
pub fn copies(s: &Scatter, keepout: &Keepout, ground: &GroundMesh) -> Vec<Copy> {
    let steepest = s.max_slope.to_radians().cos();
    planned(s)
        .into_iter()
        .filter_map(|(p, model, yaw, scale)| {
            if keepout.beyond(p) < s.clearance.max(0.0) {
                return None;
            }
            let hit = ground.raycast_down(p.extend(1e4), 2e4)?;
            if matches!(
                hit.surface.kind,
                Surface::Asphalt | Surface::Kerb | Surface::Runoff | Surface::Gravel
            ) || hit.normal.z < steepest
            {
                return None;
            }
            let up = DVec3::Z.lerp(hit.normal, s.tilt).normalize_or(DVec3::Z);
            Some(Copy {
                model,
                pos: hit.point,
                yaw,
                scale,
                up,
            })
        })
        .collect()
}

/// Copies of `models` merged into meshes, by model and by tile: (the model, a mesh of
/// its materials).
pub fn meshes(models: &[Arc<Model>], copies: &[Copy]) -> Vec<(usize, Mesh)> {
    let mut tiles: HashMap<(usize, i64, i64), Vec<&Copy>> = HashMap::new();
    for c in copies {
        let t = (c.pos.truncate() / TILE).floor();
        tiles
            .entry((c.model, t.x as i64, t.y as i64))
            .or_default()
            .push(c);
    }
    let mut keys: Vec<_> = tiles.keys().copied().collect();
    keys.sort_unstable();
    let mut out = Vec::new();
    for key in keys {
        let Some(model) = models.get(key.0) else {
            continue;
        };
        let group = &tiles[&key];
        for part in &model.meshes {
            let mut mesh = Mesh {
                material: part.material,
                cast_shadows: part.cast_shadows,
                ..Default::default()
            };
            for c in group {
                let turn = c.rotation();
                let base = mesh.positions.len() as u32;
                for (p, n) in part.positions.iter().zip(&part.normals) {
                    let p = c.pos + turn * (DVec3::from(p.map(f64::from)) * c.scale);
                    let n = turn * DVec3::from(n.map(f64::from));
                    mesh.positions.push(p.as_vec3().to_array());
                    mesh.normals.push(n.as_vec3().to_array());
                }
                mesh.uvs.extend(&part.uvs);
                mesh.indices.extend(part.indices.iter().map(|i| i + base));
            }
            out.push((key.0, mesh));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{Project, ScatterModel, Stroke};

    fn woods(strokes: Vec<Stroke>) -> Scatter {
        Scatter {
            name: "woods".into(),
            models: vec![
                ScatterModel {
                    model: "builtin:pine".into(),
                    weight: 3.0,
                },
                ScatterModel {
                    model: "builtin:tree".into(),
                    weight: 1.0,
                },
            ],
            spacing: 6.0,
            scale: [0.8, 1.2],
            tilt: 0.0,
            clearance: 5.0,
            max_slope: 35.0,
            collide: false,
            strokes,
            group: None,
        }
    }

    fn stroke(brush: Brush, strength: f64, at: (f64, f64), radius: f64) -> Stroke {
        Stroke {
            brush,
            radius,
            strength,
            points: vec![DVec2::new(at.0, at.1)],
        }
    }

    #[test]
    fn strokes_paint_copies_in_and_wipe_them_out() {
        let full = woods(vec![stroke(Brush::Paint, 1.0, (0.0, 0.0), 40.0)]);
        let n = planned(&full).len();
        // About one per cell over the middle of the brush, fewer to its edge.
        let area = std::f64::consts::PI * 40.0 * 40.0 / 36.0;
        assert!(n as f64 > 0.25 * area && (n as f64) < area, "{n} of {area}");
        assert_eq!(planned(&full), planned(&full));
        // A light stroke gives fewer; the same again, more.
        let light = woods(vec![stroke(Brush::Paint, 0.3, (0.0, 0.0), 40.0)]);
        let twice = woods(vec![stroke(Brush::Paint, 0.3, (0.0, 0.0), 40.0); 2]);
        let (l, t) = (planned(&light).len(), planned(&twice).len());
        assert!(l < n && l < t && t < n, "{l} {t} {n}");
        // Wiped out in the middle: none left there.
        let mut wiped = full.clone();
        wiped
            .strokes
            .push(stroke(Brush::Erase, 1.0, (0.0, 0.0), 15.0));
        assert!(planned(&wiped).iter().all(|(p, ..)| p.length() > 7.0));
        // Both models, the pines three times as often.
        let pines = planned(&full).iter().filter(|c| c.1 == 0).count();
        assert!(pines > 2 * (n - pines), "{pines} of {n}");
    }

    #[test]
    fn copies_keep_off_the_roads_and_stand_on_the_ground() {
        let p = Project::new("t");
        let scene = crate::bake::build(&p);
        let surfaces: Vec<_> = p.surfaces.iter().map(|s| s.props).collect();
        let ground = scene.ground.build(&surfaces);
        let keepout = Keepout::new(&scene.roads);
        // Across the first straight, 12 m wide with 18 m of grass each side.
        let s = woods(vec![Stroke {
            brush: Brush::Paint,
            radius: 30.0,
            strength: 1.0,
            points: vec![DVec2::new(120.0, -60.0), DVec2::new(120.0, 60.0)],
        }]);
        let copies = copies(&s, &keepout, &ground);
        assert!(!copies.is_empty());
        for c in &copies {
            assert!(keepout.beyond(c.pos.truncate()) >= 5.0, "{:?}", c.pos);
            let under = ground.raycast_down(c.pos + DVec3::Z, 2.0).unwrap();
            assert!((under.point.z - c.pos.z).abs() < 1e-6);
        }
        let models = [
            Arc::new(crate::shapes::model("pine").unwrap()),
            Arc::new(crate::shapes::model("tree").unwrap()),
        ];
        let meshes = meshes(&models, &copies);
        let triangles: usize = meshes.iter().map(|(_, m)| m.indices.len() / 3).sum();
        let expected: usize = copies.iter().map(|c| models[c.model].triangles).sum();
        assert_eq!(triangles, expected);
    }
}
