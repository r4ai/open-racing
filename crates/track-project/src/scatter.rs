//! Models scattered over the ground where brush strokes painted them: woods, bushes,
//! rocks. Each scatter has a grid of places `spacing` apart, each jittered within its
//! cell; its strokes paint how much of the scatter each place has, in order, and a
//! place has a copy where that is above a threshold of its own. So a stroke painted
//! lightly gives fewer copies, a stroke painted again more, and the same strokes the
//! same copies every build. Copies keep off the roads and their strips, off drivable
//! splines, and off ground too steep. A painted copy is known by its cell, so that it
//! can be taken out on its own; copies planted one by one stand where they were put.
//!
//! Copies are merged into meshes by square tiles, one for each model part and level of
//! detail: the models in full near the camera, their far models (or pictures of them on
//! crossed cards, see `impostor`) beyond the scatter's detail distance, and nothing
//! beyond its draw distance, each faded in and out by the distance to the tile's middle.

use std::collections::HashMap;
use std::sync::Arc;

use glam::{DMat3, DQuat, DVec2, DVec3};
use open_racing_sim::{GroundMesh, Surface};
use open_racing_track::{Lod, Mesh};
use serde::{Deserialize, Serialize};

use crate::model::Model;
use crate::project::{Brush, Plant, Scatter};
use crate::road::RoadBuild;
use crate::terrain::Lookup;

/// Edge of the tiles copies are merged in, m: small enough for a tile's middle to say
/// how far its copies are, large enough for few meshes.
pub const TILE: f64 = 64.0;

/// Which copy of a scatter: a painted one by its cell of the grid, or one planted on
/// its own by its place in `Scatter::placed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CopyId {
    Cell([i64; 2]),
    Placed(usize),
}

/// One copy of a scatter's model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Copy {
    pub id: CopyId,
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

    /// The copy as one planted on its own where it stands.
    pub fn plant(&self) -> Plant {
        Plant {
            model: self.model,
            pos: self.pos.truncate(),
            yaw: self.yaw,
            scale: self.scale,
        }
    }
}

/// A copy as painted or planted, before it is put on the ground.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Planned {
    pub id: CopyId,
    pub pos: DVec2,
    pub model: usize,
    pub yaw: f64,
    pub scale: f64,
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

/// A place of the grid: the cell it is in, and where in it.
fn place(s: &Scatter, seed: u64, i: i64, j: i64) -> DVec2 {
    let jitter = DVec2::new(hash(seed, i, j, 1), hash(seed, i, j, 2));
    (DVec2::new(i as f64, j as f64) + DVec2::splat(0.1) + jitter * 0.8) * s.spacing
}

/// How much of the scatter each place has, by its cell, after its strokes.
pub fn coverage(s: &Scatter) -> HashMap<(i64, i64), f64> {
    let seed = s.seed();
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

/// The copies of a scatter as painted and planted, before they are put on the ground:
/// the painted ones in a fixed order, then the planted ones.
pub fn planned(s: &Scatter) -> Vec<Planned> {
    let seed = s.seed();
    let total: f64 = s.models.iter().map(|m| m.weight).sum();
    let mut out: Vec<_> = coverage(s)
        .into_iter()
        .filter(|&((i, j), c)| c > hash(seed, i, j, 3) && !s.removed.contains(&[i, j]))
        .map(|((i, j), _)| {
            let mut pick = hash(seed, i, j, 4) * total;
            let model = s
                .models
                .iter()
                .position(|m| {
                    pick -= m.weight;
                    pick < 0.0
                })
                .unwrap_or(s.models.len().saturating_sub(1));
            Planned {
                id: CopyId::Cell([i, j]),
                pos: place(s, seed, i, j),
                model,
                yaw: hash(seed, i, j, 5) * std::f64::consts::TAU,
                scale: s.scale[0] + (s.scale[1] - s.scale[0]) * hash(seed, i, j, 6),
            }
        })
        .collect();
    // In a fixed order, whatever the map's.
    out.sort_by(|a, b| {
        (a.pos.y, a.pos.x)
            .partial_cmp(&(b.pos.y, b.pos.x))
            .expect("finite")
    });
    out.extend(s.placed.iter().enumerate().map(|(k, p)| Planned {
        id: CopyId::Placed(k),
        pos: p.pos,
        model: p.model,
        yaw: p.yaw,
        scale: p.scale,
    }));
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

/// The copies of a scatter, stood on `ground`. Painted ones keep off the roads and
/// their clearance of them, drivable splines, run-off and gravel, and ground steeper
/// than it allows; planted ones stand where they were put, unless that is on a road
/// or a kerb.
pub fn copies(s: &Scatter, keepout: &Keepout, ground: &GroundMesh) -> Vec<Copy> {
    let steepest = s.max_slope.to_radians().cos();
    planned(s)
        .into_iter()
        .filter_map(|c| {
            let painted = matches!(c.id, CopyId::Cell(_));
            if painted && keepout.beyond(c.pos) < s.clearance.max(0.0) {
                return None;
            }
            let hit = ground.raycast_down(c.pos.extend(1e4), 2e4)?;
            let off = if painted {
                matches!(
                    hit.surface.kind,
                    Surface::Asphalt | Surface::Kerb | Surface::Runoff | Surface::Gravel
                ) || hit.normal.z < steepest
            } else {
                matches!(hit.surface.kind, Surface::Asphalt | Surface::Kerb)
            };
            if off {
                return None;
            }
            let up = DVec3::Z.lerp(hit.normal, s.tilt).normalize_or(DVec3::Z);
            Some(Copy {
                id: c.id,
                model: c.model,
                pos: hit.point,
                yaw: c.yaw,
                scale: c.scale,
                up,
            })
        })
        .collect()
}

/// Which level of detail a mesh of copies is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Level {
    /// The models in full, near the camera.
    Near,
    /// Their far models, beyond the detail distance.
    Far,
}

/// Distances standing for "however far".
const FAR_AWAY: f32 = 1e9;

/// The distances over which a scatter's levels fade in and out, m: the models in full
/// out to `detail`, the far models (if `far`) from there out to `draw`.
pub fn fades(s: &Scatter, far: bool) -> [Lod; 2] {
    let draw = if s.draw > 0.0 {
        s.draw as f32
    } else {
        FAR_AWAY
    };
    let detail = (s.detail as f32).min(draw);
    let end = |d: f32| {
        if d >= FAR_AWAY {
            [FAR_AWAY; 2]
        } else {
            [d, d + (0.1 * d).clamp(4.0, 40.0)]
        }
    };
    [
        Lod {
            center: [0.0; 3],
            fade_in: [0.0; 2],
            fade_out: end(if far { detail } else { draw }),
        },
        Lod {
            center: [0.0; 3],
            fade_in: end(detail),
            fade_out: end(draw),
        },
    ]
}

/// A mesh of copies of a model's part.
fn copies_mesh(part: &Mesh, group: &[&Copy], lod: Lod, shadows: bool) -> Mesh {
    let mut mesh = Mesh {
        material: part.material,
        cast_shadows: part.cast_shadows && shadows,
        lod: Some(lod),
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
    mesh
}

/// Copies of a scatter's models merged into meshes by tile and level of detail: (the
/// model, its level, a mesh of one of its materials). `far` has each model's far model,
/// if it has one.
pub fn meshes(
    s: &Scatter,
    near: &[Arc<Model>],
    far: &[Option<Arc<Model>>],
    copies: &[Copy],
) -> Vec<(usize, Level, Mesh)> {
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
        let group = &tiles[&key];
        // Every level of the tile fades by the distance to the same point.
        let z = group.iter().map(|c| c.pos.z).sum::<f64>() / group.len() as f64;
        let middle = (DVec2::new(key.1 as f64, key.2 as f64) + 0.5) * TILE;
        let center = middle.extend(z).as_vec3().to_array();
        let lighter = far.get(key.0).cloned().flatten();
        let [near_lod, far_lod] = fades(s, lighter.is_some()).map(|l| Lod { center, ..l });
        if let Some(model) = near.get(key.0) {
            for part in &model.meshes {
                out.push((
                    key.0,
                    Level::Near,
                    copies_mesh(part, group, near_lod, s.shadows),
                ));
            }
        }
        if let Some(model) = lighter {
            for part in &model.meshes {
                out.push((
                    key.0,
                    Level::Far,
                    copies_mesh(part, group, far_lod, s.shadows),
                ));
            }
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::project::{HARDNESS, Project, ScatterModel, Stroke};

    pub(crate) fn woods(strokes: Vec<Stroke>) -> Scatter {
        Scatter {
            name: "woods".into(),
            seed: 0,
            models: vec![
                ScatterModel::new("builtin:pine".into(), 3.0),
                ScatterModel::new("builtin:tree".into(), 1.0),
            ],
            spacing: 6.0,
            scale: [0.8, 1.2],
            tilt: 0.0,
            clearance: 5.0,
            max_slope: 35.0,
            collide: false,
            shadows: true,
            detail: 150.0,
            draw: 2000.0,
            strokes,
            removed: vec![],
            placed: vec![],
            group: None,
        }
    }

    pub(crate) fn stroke(brush: Brush, strength: f64, at: (f64, f64), radius: f64) -> Stroke {
        Stroke {
            brush,
            radius,
            strength,
            points: vec![DVec2::new(at.0, at.1)],
            fill: false,
            hardness: HARDNESS,
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
        assert!(planned(&wiped).iter().all(|c| c.pos.length() > 7.0));
        // Both models, the pines three times as often.
        let pines = planned(&full).iter().filter(|c| c.model == 0).count();
        assert!(pines > 2 * (n - pines), "{pines} of {n}");
        // A hard brush acts fully out to its edge: more copies than a soft one.
        let mut hard = stroke(Brush::Paint, 1.0, (0.0, 0.0), 40.0);
        hard.hardness = 1.0;
        let hard = planned(&woods(vec![hard])).len();
        assert!(hard as f64 > 0.9 * area && hard > n, "{hard} of {area}");
    }

    #[test]
    fn single_copies_are_taken_out_and_planted_and_keep_their_places_through_a_rename() {
        let mut s = woods(vec![stroke(Brush::Paint, 1.0, (0.0, 0.0), 30.0)]);
        let before = planned(&s);
        let CopyId::Cell(cell) = before[3].id else {
            panic!("painted")
        };
        s.removed.push(cell);
        s.placed.push(Plant {
            model: 1,
            pos: DVec2::new(500.0, 5.0),
            yaw: 1.0,
            scale: 2.0,
        });
        let after = planned(&s);
        assert_eq!(after.len(), before.len());
        assert!(!after.iter().any(|c| c.id == before[3].id));
        let last = after.last().unwrap();
        assert_eq!(last.id, CopyId::Placed(0));
        assert_eq!((last.model, last.scale), (1, 2.0));
        // Renamed with its seed kept, the painted copies stay where they are.
        s.seed = s.seed();
        s.name = "forest".into();
        assert_eq!(planned(&s), after);
    }

    #[test]
    fn copies_keep_off_the_roads_and_stand_on_the_ground() {
        let p = Project::new("t");
        let scene = crate::bake::build(&p);
        let surfaces: Vec<_> = p.surfaces.iter().map(|s| s.props).collect();
        let ground = scene.ground.build(&surfaces);
        let keepout = Keepout::new(&scene.roads);
        // Across the first straight, 12 m wide with 18 m of grass each side.
        let mut s = woods(vec![Stroke {
            points: vec![DVec2::new(120.0, -60.0), DVec2::new(120.0, 60.0)],
            ..stroke(Brush::Paint, 1.0, (0.0, 0.0), 30.0)
        }]);
        // Planted beside the road, inside the clearance, it stands; on the road (which
        // bows south between its nodes), not.
        s.placed = [(120.0, 2.5), (120.0, -6.0)]
            .map(|(x, y)| Plant {
                model: 0,
                pos: DVec2::new(x, y),
                yaw: 0.0,
                scale: 1.0,
            })
            .to_vec();
        let copies = copies(&s, &keepout, &ground);
        assert!(!copies.is_empty());
        for c in &copies {
            if matches!(c.id, CopyId::Cell(_)) {
                assert!(keepout.beyond(c.pos.truncate()) >= 5.0, "{:?}", c.pos);
            }
            let under = ground.raycast_down(c.pos + DVec3::Z, 2.0).unwrap();
            assert!((under.point.z - c.pos.z).abs() < 1e-6);
        }
        let planted: Vec<_> = copies
            .iter()
            .filter(|c| matches!(c.id, CopyId::Placed(_)))
            .collect();
        assert_eq!(planted.len(), 1);
        assert_eq!(planted[0].id, CopyId::Placed(0));

        let models = [
            Arc::new(crate::shapes::model("pine").unwrap()),
            Arc::new(crate::shapes::model("tree").unwrap()),
        ];
        let meshes = meshes(&s, &models, &[None, None], &copies);
        let triangles: usize = meshes.iter().map(|(_, _, m)| m.indices.len() / 3).sum();
        let expected: usize = copies.iter().map(|c| models[c.model].triangles).sum();
        assert_eq!(triangles, expected);
        // Without far models the models in full show out to the draw distance.
        for (_, level, m) in &meshes {
            assert_eq!(*level, Level::Near);
            let lod = m.lod.unwrap();
            assert_eq!(lod.fade_out[0], 2000.0);
        }
    }

    #[test]
    fn far_models_take_over_at_the_detail_distance_in_the_same_tiles() {
        let s = woods(vec![]);
        let pine = Arc::new(crate::shapes::model("pine").unwrap());
        let copies: Vec<Copy> = (0..20)
            .map(|k| Copy {
                id: CopyId::Placed(k),
                model: 0,
                pos: DVec3::new(k as f64 * 10.0, 3.0, 1.0),
                yaw: 0.0,
                scale: 1.0,
                up: DVec3::Z,
            })
            .collect();
        let meshes = meshes(
            &s,
            std::slice::from_ref(&pine),
            &[Some(pine.clone())],
            &copies,
        );
        let near: Vec<_> = meshes.iter().filter(|m| m.1 == Level::Near).collect();
        let far: Vec<_> = meshes.iter().filter(|m| m.1 == Level::Far).collect();
        assert_eq!(near.len(), far.len());
        for (a, b) in near.iter().zip(&far) {
            let (a, b) = (a.2.lod.unwrap(), b.2.lod.unwrap());
            assert_eq!(a.center, b.center);
            assert_eq!(a.fade_out, b.fade_in);
            assert_eq!(a.fade_out[0], 150.0);
            assert_eq!(b.fade_out[0], 2000.0);
        }
    }
}
