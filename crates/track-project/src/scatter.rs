//! Models scattered over the ground where brush strokes painted them: woods, bushes,
//! rocks. Each scatter has a grid of places `spacing` apart, each jittered within its
//! cell; its strokes paint how much of the scatter each place has, in order, and a
//! place has a copy where that is above a threshold of its own. So a stroke painted
//! lightly gives fewer copies, a stroke painted again more, and the same strokes the
//! same copies every build. Copies keep off the roads and their strips, off drivable
//! splines, and off ground too steep. A painted copy is known by its cell, so that it
//! can be taken out on its own; copies planted one by one stand where they were put.
//!
//! Each model is kept once, and its copies as a list of where each stands, drawn by
//! instancing: the model in full near the camera, its far model (or pictures of it on
//! crossed cards, see `impostor`) beyond the scatter's detail distance, and nothing
//! beyond its draw distance, each copy faded in and out by its own distance.
//!
//! Each copy of a plant has leaves of its own colour, as the season and the scatter's
//! variety make them: greens differing from tree to tree, turning yellow, orange and red
//! each at its own time in autumn, and bare in winter (see `leaves`).

use std::collections::HashMap;
use std::sync::Arc;

use glam::{DMat3, DQuat, DVec2, DVec3};
use open_racing_sim::{GroundMesh, Surface};
use open_racing_track::{BARE, FAR_AWAY, Instance, Level, Mesh, PlantLook, Visual};
use serde::{Deserialize, Serialize};

use crate::model::Model;
use crate::project::{Brush, Foliage, Plant, Scatter};
use crate::road::RoadBuild;
use crate::terrain::Lookup;

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
    pub fn rotation(&self) -> DQuat {
        DQuat::from_rotation_arc(DVec3::Z, self.up.normalize_or(DVec3::Z))
            * DQuat::from_rotation_z(self.yaw)
    }

    /// Where the renderer draws it, with its leaves' look.
    pub fn instance(&self, leaves: [u8; 4]) -> Instance {
        Instance {
            pos: self.pos.as_vec3().to_array(),
            rotation: self.rotation().as_quat().to_array(),
            scale: self.scale as f32,
            leaves,
        }
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

/// The distances over which a scatter's levels fade in and out, m: the models in full
/// out to `detail`, the far models (if `far`) from there out to `draw`. Their shapes
/// are left to be filled in.
pub fn fades(s: &Scatter, far: bool) -> [Level; 2] {
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
        Level {
            shape: 0,
            fade_in: [0.0; 2],
            fade_out: end(if far { detail } else { draw }),
        },
        Level {
            shape: 0,
            fade_in: end(detail),
            fade_out: end(draw),
        },
    ]
}

/// Each of the scatter's models' copies, where the renderer draws them, with their
/// leaves as they look at the plant month `month` (see `Environment::plant_month`).
pub fn instances(s: &Scatter, month: f64, copies: &[Copy]) -> Vec<Vec<Instance>> {
    let mut out = vec![Vec::new(); s.models.len()];
    for c in copies {
        if let Some(list) = out.get_mut(c.model) {
            let foliage = s.models[c.model].foliage();
            list.push(c.instance(leaves(foliage, month, s.variety, c.pos.truncate())));
        }
    }
    out
}

/// How a model's materials move in the wind as a plant of `foliage`, if it is one: its
/// wood's and its leaves'. The top of the model bends so far in a 10 m/s wind: the
/// taller a copy, the farther.
pub fn plant(foliage: Foliage, model: &Model) -> Option<[PlantLook; 2]> {
    let (top, flutter) = match foliage {
        Foliage::Rigid => return None,
        Foliage::Evergreen => (0.25, 0.012),
        Foliage::Deciduous => (0.35, 0.03),
        Foliage::Grass => (0.12, 0.015),
    };
    let height = model.bounds[1].z.max(0.3);
    let sway = top / (height * height);
    Some([
        PlantLook {
            sway,
            flutter: 0.0,
            leaves: false,
        },
        PlantLook {
            sway,
            flutter,
            leaves: true,
        },
    ])
}

/// A model's materials as a plant of `foliage`: those of its leaves as leaves, all
/// moving in the wind; none a plant's when it is not one.
pub fn plant_look(model: &Model, foliage: Foliage) -> Visual {
    let plants = plant(foliage, model);
    let mut look = model.look.clone();
    for m in &mut look.materials {
        let leaves = m.plant.is_some_and(|p| p.leaves);
        m.plant = plants.map(|[wood, leaf]| if leaves { leaf } else { wood });
    }
    look
}

/// A colour for leaves, linear, scaled to `brightness` times their own, and mixed in
/// by `amount` (0 to 1): see `open_racing_track::Instance::leaves`.
fn tint(rgb: [f64; 3], brightness: f64, amount: f64) -> [u8; 4] {
    let luminance = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
    let scale = 0.2 * brightness / luminance.max(1e-6);
    let srgb = |c: f64| {
        let c = (c * scale).clamp(0.0, 1.0);
        let s = if c <= 0.003_130_8 {
            c * 12.92
        } else {
            1.055 * c.powf(1.0 / 2.4) - 0.055
        };
        (s * 255.0).round() as u8
    };
    let a = (amount.clamp(0.0, 1.0) * 254.0).round() as u8;
    [srgb(rgb[0]), srgb(rgb[1]), srgb(rgb[2]), a]
}

fn smooth(a: f64, b: f64, x: f64) -> f64 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn mix3(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

/// Autumn's colours, in the order leaves turn: (colour, brightness).
const AUTUMN: [([f64; 3], f64); 5] = [
    ([0.35, 0.55, 0.08], 1.1),
    ([0.8, 0.6, 0.05], 1.7),
    ([0.9, 0.33, 0.03], 1.4),
    ([0.7, 0.1, 0.03], 0.95),
    ([0.35, 0.17, 0.06], 0.7),
];

/// How the leaves of a copy of a plant standing at `at` look at the plant month
/// `month` (0 to 12, 0 the start of January, in a northern year), with the scatter's
/// `variety`: each copy's green differs a little from the others', and each turns,
/// falls and comes out at a time of its own round the season's.
pub fn leaves(foliage: Foliage, month: f64, variety: f64, at: DVec2) -> [u8; 4] {
    let cell = (at * 100.0).round();
    let h = |k: u64| hash(0x1eaf, cell.x as i64, cell.y as i64, k);
    let variety = variety.clamp(0.0, 1.0);
    // Summer: greens from yellowish to bluish, lighter and darker.
    let green = mix3([0.3, 0.6, 0.08], [0.12, 0.55, 0.25], h(1));
    let summer = (green, 1.0 + (h(2) - 0.5) * 0.7 * variety, variety);
    let (rgb, brightness, amount) = match foliage {
        Foliage::Rigid => return [0; 4],
        Foliage::Evergreen => {
            // Darker and bluer in the cold months.
            let cold = 1.0 - smooth(1.5, 3.5, month) + smooth(10.5, 12.0, month);
            let winter = ([0.1, 0.4, 0.3], 0.8);
            (
                mix3(summer.0, winter.0, cold),
                summer.1 + (winter.1 - summer.1) * cold,
                summer.2.max(0.5 * cold),
            )
        }
        Foliage::Grass => {
            // Green in spring, drying through late summer, straw from late autumn.
            let straw = ([0.6, 0.5, 0.2], 1.4);
            let dry = (smooth(7.0, 9.0, month) * 0.5 + smooth(10.0, 11.5, month) * 0.5)
                .max(1.0 - smooth(2.0, 3.5, month));
            let dry = (dry + (h(3) - 0.5) * 0.3).clamp(0.0, 1.0);
            (
                mix3(summer.0, straw.0, dry),
                summer.1 + (straw.1 - summer.1) * dry,
                summer.2.max(0.85 * dry),
            )
        }
        Foliage::Deciduous => {
            // Each tree comes out and turns up to a few weeks from the others.
            let spring = (month - 3.3) / 0.8 + (h(4) - 0.5) * 1.2;
            let autumn = (month - 8.8) / 2.2 + (h(5) - 0.5) * 0.5;
            if spring < 0.0 || autumn > 1.1 {
                return BARE;
            }
            if month < 6.0 {
                // Fresh, light green, darkening into summer's.
                let fresh = 1.0 - spring.clamp(0.0, 1.0);
                (
                    mix3(summer.0, [0.4, 0.7, 0.1], fresh),
                    summer.1 + 0.4 * fresh,
                    summer.2.max(0.8 * fresh),
                )
            } else if autumn <= 0.0 {
                summer
            } else {
                // Some turn only yellow, others on to red before they brown and fall.
                let reach = 0.3 + 0.7 * h(6);
                let t = if autumn > 1.0 {
                    1.0
                } else {
                    autumn.min(reach)
                } * (AUTUMN.len() - 1) as f64;
                let (i, f) = ((t.floor() as usize).min(AUTUMN.len() - 2), t.fract());
                let (a, b) = (AUTUMN[i], AUTUMN[i + 1]);
                let f = if t >= (AUTUMN.len() - 1) as f64 { 1.0 } else { f };
                (
                    mix3(a.0, b.0, f),
                    a.1 + (b.1 - a.1) * f,
                    summer.2.max(smooth(0.0, 0.15, autumn) * 0.95),
                )
            }
        }
    };
    tint(rgb, brightness, amount)
}

/// A model's part at each of `copies`, merged in the world: what cars hit when the
/// scatter is solid.
pub fn world_mesh(part: &Mesh, copies: &[&Copy]) -> Mesh {
    let mut mesh = Mesh {
        material: part.material,
        cast_shadows: part.cast_shadows,
        ..Default::default()
    };
    for c in copies {
        let turn = DMat3::from_quat(c.rotation());
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

/// Which of the scatter's models each copy is, grouped: the copies of model `i`.
pub fn of_model<'a>(copies: &'a [Copy], i: usize) -> Vec<&'a Copy> {
    copies.iter().filter(|c| c.model == i).collect()
}

/// How many triangles the copies' models have in full.
pub fn triangles(near: &[Arc<Model>], copies: &[Copy]) -> usize {
    copies
        .iter()
        .filter_map(|c| near.get(c.model))
        .map(|m| m.triangles)
        .sum()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::project::{HARDNESS, Project, ScatterModel, Stroke, VARIETY};

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
            variety: VARIETY,
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

        // Each copy is drawn where it stands, turned and sized as it is.
        let lists = instances(&s, 6.5, &copies);
        assert_eq!(lists[0].len() + lists[1].len(), copies.len());
        let c = copies.iter().find(|c| c.model == 1).unwrap();
        let i = lists[1][0];
        assert_eq!(i.pos, c.pos.as_vec3().to_array());
        assert_eq!(i.scale, c.scale as f32);
        let turn = glam::Quat::from_rotation_z(c.yaw as f32);
        assert!(glam::Quat::from_array(i.rotation).dot(turn).abs() > 0.9999);
        // Solid copies put their model's triangles in the world.
        let pine = crate::shapes::model("pine").unwrap();
        let pines = of_model(&copies, 0);
        let solid = world_mesh(&pine.meshes[0], &pines);
        assert_eq!(solid.indices.len(), pine.meshes[0].indices.len() * pines.len());
    }

    #[test]
    fn leaves_turn_in_autumn_fall_in_winter_and_differ_from_copy_to_copy() {
        let at = |k: usize| DVec2::new(k as f64 * 7.3, 3.1);
        let look = |f, month, k| leaves(f, month, VARIETY, at(k));
        // Rocks have no leaves to colour.
        assert_eq!(look(Foliage::Rigid, 10.0, 0), [0; 4]);
        // Broadleaf trees: bare in January, all in leaf in July, most turned in late
        // October, and not all alike in summer.
        let n = 200;
        let count = |month, f: &dyn Fn([u8; 4]) -> bool| {
            (0..n)
                .filter(|&k| f(look(Foliage::Deciduous, month, k)))
                .count()
        };
        assert_eq!(count(0.5, &|l| l == BARE), n);
        assert_eq!(count(6.5, &|l| l == BARE), 0);
        let red_or_yellow = |l: [u8; 4]| l != BARE && l[0] > l[1] / 2 + 40 && l[3] > 200;
        assert!(count(9.8, &red_or_yellow) > n / 2, "{}", count(9.8, &red_or_yellow));
        assert!(count(11.8, &|l| l == BARE) > n * 9 / 10);
        let summer: std::collections::HashSet<_> =
            (0..n).map(|k| look(Foliage::Deciduous, 6.5, k)).collect();
        assert!(summer.len() > n / 2);
        // Without variety, summer leaves keep their own colour.
        assert_eq!(leaves(Foliage::Deciduous, 6.5, 0.0, at(3))[3], 0);
        // Conifers stay in needles; grass is straw in winter.
        assert!((0..n).all(|k| look(Foliage::Evergreen, 0.5, k) != BARE));
        let straw = look(Foliage::Grass, 0.5, 1);
        assert!(straw[0] > straw[2] + 60 && straw[3] > 150, "{straw:?}");
        // South of the equator the seasons are the other way round.
        let south = open_racing_track::Environment {
            month: 7,
            ..Default::default()
        };
        assert!(south.plant_month(-35.0) < 1.0);
        assert_eq!(south.plant_month(48.0), 6.5);
    }

    #[test]
    fn far_models_take_over_at_the_detail_distance() {
        let s = woods(vec![]);
        let [near, far] = fades(&s, true);
        assert_eq!(near.fade_out, far.fade_in);
        assert_eq!(near.fade_out[0], 150.0);
        assert_eq!(far.fade_out[0], 2000.0);
        // Without far models the models in full show out to the draw distance.
        let [near, _] = fades(&s, false);
        assert_eq!(near.fade_out[0], 2000.0);
        // Drawn however far: never faded out.
        let [_, far] = fades(&Scatter { draw: 0.0, ..s }, true);
        assert_eq!(far.fade_out, [FAR_AWAY; 2]);
    }
}
