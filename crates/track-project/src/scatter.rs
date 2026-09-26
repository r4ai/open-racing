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
use open_racing_track::{BARE, FAR_AWAY, Instance, Level, Mesh, Varies, Visual};
use serde::{Deserialize, Serialize};

use crate::model::Model;
use crate::project::{Brush, Kind, Layout, Plant, Scatter};
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
    pub fn instance(&self, tint: [u8; 4]) -> Instance {
        Instance {
            pos: self.pos.as_vec3().to_array(),
            rotation: self.rotation().as_quat().to_array(),
            scale: self.scale as f32,
            tint,
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

//// For `Layout::Even`: the edge of a cell of the candidates, against the spacing. One
/// candidate a cell kept where no other within a cell's edge of it goes first (Matérn's
/// second process) leaves 0.305 of them, which are then `spacing` apart on the whole.
const EVEN_CELL: f64 = 0.552;

/// The edge of the cells a scatter's places are in, m.
fn cell(s: &Scatter) -> f64 {
    match s.layout {
        Layout::Even => s.spacing * EVEN_CELL,
        _ => s.spacing,
    }
}

/// A place of the grid: the cell it is in, and where in it.
fn place(s: &Scatter, seed: u64, i: i64, j: i64) -> DVec2 {
    let jitter = DVec2::new(hash(seed, i, j, 1), hash(seed, i, j, 2));
    let (margin, spread) = match s.layout {
        Layout::Even => (0.0, 1.0),
        _ => (0.1, 0.8),
    };
    (DVec2::new(i as f64, j as f64) + DVec2::splat(margin) + jitter * spread) * cell(s)
}

/// For `Layout::Even`: whether the place of cell (i, j) is kept, none of those near it
/// going first. It depends on the places round it only, painted or not, so painting
/// more never moves those already there.
fn kept_evenly(s: &Scatter, seed: u64, i: i64, j: i64) -> bool {
    let (p, first) = (place(s, seed, i, j), hash(seed, i, j, 8));
    let reach = cell(s);
    !(-1..=1).any(|dj| {
        (-1..=1).any(|di| {
            (di, dj) != (0, 0)
                && hash(seed, i + di, j + dj, 8) > first
                && place(s, seed, i + di, j + dj).distance(p) < reach
        })
    })
}

/// How much of the scatter each place has, by its cell, after its strokes.
pub fn coverage(s: &Scatter) -> HashMap<(i64, i64), f64> {
    let seed = s.seed();
    let size = cell(s);
    let mut cover: HashMap<(i64, i64), f64> = HashMap::new();
    for stroke in &s.strokes {
        let (lo, hi) = stroke.bounds();
        let (a, b) = ((lo / size).floor(), (hi / size).floor());
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

/// How much of the scatter its strokes paint at `p`, of those whose boxes (`bounds`)
/// hold it.
fn painted_at(s: &Scatter, bounds: &[(DVec2, DVec2)], p: DVec2) -> f64 {
    let mut c = 0.0;
    for (stroke, (lo, hi)) in s.strokes.iter().zip(bounds) {
        if p.cmplt(*lo).any() || p.cmpgt(*hi).any() {
            continue;
        }
        let k = stroke.strength * stroke.weight(p);
        if k > 0.0 {
            match stroke.brush {
                Brush::Erase => c -= c * k,
                _ => c += (1.0 - c) * k,
            }
        }
    }
    c
}

/// Which of the scatter's models `pick` (0 to 1) picks by their weights.
fn pick_model(s: &Scatter, pick: f64) -> usize {
    let total: f64 = s.models.iter().map(|m| m.weight).sum();
    let mut pick = pick * total;
    s.models
        .iter()
        .position(|m| {
            pick -= m.weight;
            pick < 0.0
        })
        .unwrap_or(s.models.len().saturating_sub(1))
}

/// For `Layout::Rows`: how far rows reach beyond the roads' clearance, m.
const ROWS_DEPTH: f64 = 200.0;

/// For `Layout::Rows`: which row a copy is in, as the first number of its cell: the
/// road, the side (left or right) and the row outwards from the road.
fn row_code(road: usize, left: bool, row: usize) -> i64 {
    ((road as i64) << 20) | if left { 0 } else { 1 << 19 } | row as i64
}

/// For `Layout::Rows`: the row outwards from its road a copy's cell is in.
fn row_of(code: i64) -> i64 {
    code & ((1 << 19) - 1)
}

/// The copies of `Layout::Rows`: rows `spacing` apart beyond the roads' outer edges and
/// their clearance, where painted, each place `spacing` along from the last (every
/// other row half a place on), facing the road.
fn rows(s: &Scatter, seed: u64, edges: &Edges) -> Vec<Planned> {
    let bounds: Vec<(DVec2, DVec2)> = s.strokes.iter().map(|k| k.bounds()).collect();
    let painted: Vec<(DVec2, DVec2)> = s
        .strokes
        .iter()
        .zip(&bounds)
        .filter(|(k, _)| k.brush != Brush::Erase)
        .map(|(_, b)| *b)
        .collect();
    let spacing = s.spacing.max(0.1);
    let first = s.clearance.max(0.0);
    let depth = ((ROWS_DEPTH / spacing) as usize).min(1000);
    let mut out = Vec::new();
    for (r, road) in edges.roads.iter().enumerate() {
        let Some(length) = road.last().map(|e| e.s) else {
            continue;
        };
        let stations = (length / spacing).floor() as i64;
        for left in [true, false] {
            for j in 0..=stations {
                let along = j as f64 * spacing;
                // Only where some row of this place may be painted.
                let (Some((a, _)), Some((b, _))) = (
                    edges.beyond(r, along, left, first),
                    edges.beyond(r, along, left, first + ROWS_DEPTH),
                ) else {
                    continue;
                };
                let (lo, hi) = (a.min(b) - spacing, a.max(b) + spacing);
                if !painted
                    .iter()
                    .any(|(l, h)| l.cmple(hi).all() && h.cmpge(lo).all())
                {
                    continue;
                }
                for k in 0..depth {
                    let at = along + if k % 2 == 1 { 0.5 * spacing } else { 0.0 };
                    let Some((p, toward)) = edges.beyond(r, at, left, first + k as f64 * spacing)
                    else {
                        continue;
                    };
                    let code = row_code(r, left, k);
                    if s.removed.contains(&[code, j])
                        || painted_at(s, &bounds, p) <= hash(seed, code, j, 3)
                    {
                        continue;
                    }
                    out.push(Planned {
                        id: CopyId::Cell([code, j]),
                        pos: p,
                        model: pick_model(s, hash(seed, code, j, 4)),
                        yaw: toward.to_angle() + (hash(seed, code, j, 5) - 0.5) * 0.3,
                        scale: s.scale[0] + (s.scale[1] - s.scale[0]) * hash(seed, code, j, 6),
                    });
                }
            }
        }
    }
    out
}

/// The copies of a scatter as painted and planted, before they are put on the ground:
/// the painted ones in a fixed order, then the planted ones. Rows follow the roads'
/// `edges`.
pub fn planned(s: &Scatter, edges: &Edges) -> Vec<Planned> {
    let seed = s.seed();
    let mut out: Vec<_> = match s.layout {
        Layout::Rows => rows(s, seed, edges),
        Layout::Grid | Layout::Even => coverage(s)
            .into_iter()
            .filter(|&((i, j), c)| {
                c > hash(seed, i, j, 3)
                    && !s.removed.contains(&[i, j])
                    && (s.layout != Layout::Even || kept_evenly(s, seed, i, j))
            })
            .map(|((i, j), _)| Planned {
                id: CopyId::Cell([i, j]),
                pos: place(s, seed, i, j),
                model: pick_model(s, hash(seed, i, j, 4)),
                yaw: hash(seed, i, j, 5) * std::f64::consts::TAU,
                scale: s.scale[0] + (s.scale[1] - s.scale[0]) * hash(seed, i, j, 6),
            })
            .collect(),
    };
    // In a fixed order, whatever the map's.
    out.sort_by(|a, b| {
        (a.pos.y, a.pos.x, a.id)
            .partial_cmp(&(b.pos.y, b.pos.x, b.id))
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

/// Where the roads' outer edges (their strips included) run, for copies laid in rows
/// along them.
#[derive(Clone, Debug, Default)]
pub struct Edges {
    roads: Vec<Vec<EdgePoint>>,
}

/// A point along a road: the distance to it along the road, where it is seen from
/// above, the road's left there, and how far its outer edges are to the left (the
/// right one negative).
#[derive(Clone, Copy, Debug)]
struct EdgePoint {
    s: f64,
    pos: DVec2,
    left: DVec2,
    edges: [f64; 2],
}

impl Edges {
    pub fn new(roads: &[RoadBuild]) -> Self {
        let roads = roads
            .iter()
            .map(|b| {
                let mut s = 0.0;
                let mut last: Option<DVec2> = None;
                b.sampled
                    .frames
                    .iter()
                    .enumerate()
                    .map(|(k, f)| {
                        let pos = f.pos.truncate();
                        s += last.map_or(0.0, |l| l.distance(pos));
                        last = Some(pos);
                        let [(left, _), (right, _)] = b.edges(k);
                        EdgePoint {
                            s,
                            pos,
                            left: DVec3::Z.cross(f.tangent).truncate().normalize_or(DVec2::Y),
                            edges: [left, right],
                        }
                    })
                    .collect()
            })
            .collect();
        Self { roads }
    }

    /// The point `d` beyond road `r`'s outer edge on its left (or right) side, `s`
    /// along it, and the way from there to the road.
    fn beyond(&self, r: usize, s: f64, left: bool, d: f64) -> Option<(DVec2, DVec2)> {
        let road = self.roads.get(r)?;
        let i = road
            .partition_point(|e| e.s <= s)
            .clamp(1, road.len().max(2) - 1);
        let (a, b) = (road.get(i - 1)?, road.get(i)?);
        let t = ((s - a.s) / (b.s - a.s).max(1e-9)).clamp(0.0, 1.0);
        let pos = a.pos.lerp(b.pos, t);
        let side = a.left.lerp(b.left, t).normalize_or(DVec2::Y);
        let [l, r] = [0, 1].map(|k| a.edges[k] + (b.edges[k] - a.edges[k]) * t);
        Some(if left {
            (pos + side * (l + d), -side)
        } else {
            (pos + side * (r - d), side)
        })
    }
}

/// What lies in the way of copies: the roads (and their strips), to keep `clearance`
/// from; and where the roads' edges run.
pub struct Keepout<'a> {
    roads: &'a [RoadBuild],
    lookup: Option<Lookup>,
    pub edges: Edges,
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
            edges: Edges::new(roads),
        }
    }

    /// How far `p` is beyond the outer edges of the roads, m (negative on them).
    fn beyond(&self, p: DVec2) -> f64 {
        self.lookup
            .as_ref()
            .and_then(|l| l.beyond(self.roads, p))
            .unwrap_or(f64::INFINITY)
    }

    /// The way from `p` to the middle of the nearest road.
    fn toward_road(&self, p: DVec2) -> Option<DVec2> {
        let (r, k) = self.lookup.as_ref()?.nearest(self.roads, p)?;
        let to = self.roads[r].sampled.frames[k].pos.truncate() - p;
        (to.length() > 1e-6).then(|| to.normalize())
    }
}

/// The copies of a scatter, stood on `ground`. Painted ones keep off the roads and
/// their clearance of them, drivable splines, run-off and gravel, and ground steeper
/// than it allows; planted ones stand where they were put, unless that is on a road
/// or a kerb. Rows keep to the road they are nearest; painted spectators face the
/// nearest road.
pub fn copies(s: &Scatter, keepout: &Keepout, ground: &GroundMesh) -> Vec<Copy> {
    let steepest = s.max_slope.to_radians().cos();
    planned(s, &keepout.edges)
        .into_iter()
        .filter_map(|c| {
            let painted = matches!(c.id, CopyId::Cell(_));
            let beyond = keepout.beyond(c.pos);
            if painted && beyond < s.clearance.max(0.0) - 1e-6 {
                return None;
            }
            // A row belongs to the road it is nearest: none nearer another road.
            if let (Layout::Rows, CopyId::Cell([code, _])) = (s.layout, c.id)
                && beyond < s.clearance.max(0.0) + (row_of(code) as f64 - 0.5) * s.spacing
            {
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
            let crowd = s
                .models
                .get(c.model)
                .is_some_and(|m| m.kind() == Kind::Crowd);
            let yaw = match keepout.toward_road(c.pos) {
                Some(to) if painted && crowd && s.layout != Layout::Rows => {
                    // Each a little its own way, from its random turn.
                    to.to_angle() + (c.yaw / std::f64::consts::TAU - 0.5) * 0.6
                }
                _ => c.yaw,
            };
            let up = DVec3::Z.lerp(hit.normal, s.tilt).normalize_or(DVec3::Z);
            Some(Copy {
                id: c.id,
                model: c.model,
                pos: hit.point,
                yaw,
                scale: c.scale,
                up,
            })
        })
        .collect()
}

/// The distances over which a scatter's levels fade in and out, m: the model in full
/// near the camera, then its `lods` less detailed levels, each taking over at twice the
/// distance of the one before and the last out to `detail`, then the far model (if
/// `far`) out to `draw`. Without a far model the last level shows out to `draw`. Their
/// shapes are left to be filled in.
pub fn fades(s: &Scatter, lods: usize, far: bool) -> Vec<Level> {
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
    // Where each level of the model ends.
    let ends: Vec<f32> = (0..=lods)
        .map(|i| match lods - i {
            0 if far => detail,
            0 => draw,
            n => detail / (1 << n) as f32,
        })
        .collect();
    let mut levels: Vec<Level> = ends
        .iter()
        .enumerate()
        .map(|(i, &d)| Level {
            shape: 0,
            fade_in: if i == 0 { [0.0; 2] } else { end(ends[i - 1]) },
            fade_out: end(d),
        })
        .collect();
    if far {
        levels.push(Level {
            shape: 0,
            fade_in: end(detail),
            fade_out: end(draw),
        });
    }
    levels
}

/// Each of the scatter's models' copies, where the renderer draws them, with their
/// leaves as they look at the plant month `month` (see `Environment::plant_month`).
pub fn instances(s: &Scatter, month: f64, copies: &[Copy]) -> Vec<Vec<Instance>> {
    let mut out = vec![Vec::new(); s.models.len()];
    for c in copies {
        if let Some(list) = out.get_mut(c.model) {
            let kind = s.models[c.model].kind();
            list.push(c.instance(tint(kind, month, s.variety, c.pos.truncate())));
        }
    }
    out
}

/// How a model's materials move in the wind as a plant of `kind`, if it is one: its
/// wood's and its leaves'. The top of the model bends so far in a 10 m/s wind: the
/// taller a copy, the farther.
pub fn varies(kind: Kind, model: &Model) -> Option<[Varies; 2]> {
    let (top, flutter) = match kind {
        Kind::Rigid => return None,
        // Still, but in clothes of their own colours.
        Kind::Crowd => (0.0, 0.0),
        Kind::Evergreen => (0.25, 0.012),
        Kind::Deciduous => (0.35, 0.03),
        Kind::Grass => (0.12, 0.015),
    };
    let height = model.bounds[1].z.max(0.3);
    let sway = top / (height * height);
    Some([
        Varies {
            sway,
            flutter: 0.0,
            tinted: false,
        },
        Varies {
            sway,
            flutter,
            tinted: true,
        },
    ])
}

/// A model's materials as a plant of `kind`: those of its leaves as leaves, all
/// moving in the wind; none a plant's when it is not one.
pub fn varied_look(model: &Model, kind: Kind) -> Visual {
    let plants = varies(kind, model);
    let mut look = model.look.clone();
    for m in &mut look.materials {
        let leaves = m.varies.is_some_and(|p| p.tinted);
        m.varies = plants.map(|[wood, leaf]| if leaves { leaf } else { wood });
    }
    look
}

/// A colour for leaves, linear, scaled to `brightness` times their own, and mixed in
/// by `amount` (0 to 1): see `open_racing_track::Instance::leaves`.
fn colour(rgb: [f64; 3], brightness: f64, amount: f64) -> [u8; 4] {
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

/// The colours spectators' clothes are, as often as each other: (colour, brightness).
const CLOTHES: [([f64; 3], f64); 10] = [
    ([0.8, 0.04, 0.04], 1.0),
    ([0.05, 0.12, 0.7], 0.9),
    ([0.9, 0.75, 0.05], 2.4),
    ([1.0, 1.0, 1.0], 3.2),
    ([1.0, 1.0, 1.0], 0.25),
    ([0.9, 0.3, 0.02], 1.6),
    ([0.08, 0.45, 0.1], 1.0),
    ([0.3, 0.55, 0.9], 2.0),
    ([1.0, 1.0, 1.0], 1.0),
    ([0.5, 0.05, 0.3], 0.8),
];

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
pub fn tint(kind: Kind, month: f64, variety: f64, at: DVec2) -> [u8; 4] {
    let cell = (at * 100.0).round();
    let h = |k: u64| hash(0x1eaf, cell.x as i64, cell.y as i64, k);
    let variety = variety.clamp(0.0, 1.0);
    // Summer: greens from yellowish to bluish, lighter and darker.
    let green = mix3([0.3, 0.6, 0.08], [0.12, 0.55, 0.25], h(1));
    let summer = (green, 1.0 + (h(2) - 0.5) * 0.7 * variety, variety);
    let (rgb, brightness, amount) = match kind {
        Kind::Rigid => return [0; 4],
        Kind::Crowd => {
            let (rgb, brightness) = CLOTHES[(h(7) * CLOTHES.len() as f64) as usize % CLOTHES.len()];
            (rgb, brightness * (1.0 + (h(8) - 0.5) * 0.5 * variety), 1.0)
        }
        Kind::Evergreen => {
            // Darker and bluer in the cold months.
            let cold = 1.0 - smooth(1.5, 3.5, month) + smooth(10.5, 12.0, month);
            let winter = ([0.1, 0.4, 0.3], 0.8);
            (
                mix3(summer.0, winter.0, cold),
                summer.1 + (winter.1 - summer.1) * cold,
                summer.2.max(0.5 * cold),
            )
        }
        Kind::Grass => {
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
        Kind::Deciduous => {
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
                let t =
                    if autumn > 1.0 { 1.0 } else { autumn.min(reach) } * (AUTUMN.len() - 1) as f64;
                let (i, f) = ((t.floor() as usize).min(AUTUMN.len() - 2), t.fract());
                let (a, b) = (AUTUMN[i], AUTUMN[i + 1]);
                let f = if t >= (AUTUMN.len() - 1) as f64 {
                    1.0
                } else {
                    f
                };
                (
                    mix3(a.0, b.0, f),
                    a.1 + (b.1 - a.1) * f,
                    summer.2.max(smooth(0.0, 0.15, autumn) * 0.95),
                )
            }
        }
    };
    colour(rgb, brightness, amount)
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
            layout: Layout::Grid,
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
            stamp: None,
        }
    }

    #[test]
    fn strokes_paint_copies_in_and_wipe_them_out() {
        let full = woods(vec![stroke(Brush::Paint, 1.0, (0.0, 0.0), 40.0)]);
        let n = planned(&full, &Edges::default()).len();
        // About one per cell over the middle of the brush, fewer to its edge.
        let area = std::f64::consts::PI * 40.0 * 40.0 / 36.0;
        assert!(n as f64 > 0.25 * area && (n as f64) < area, "{n} of {area}");
        assert_eq!(
            planned(&full, &Edges::default()),
            planned(&full, &Edges::default())
        );
        // A light stroke gives fewer; the same again, more.
        let light = woods(vec![stroke(Brush::Paint, 0.3, (0.0, 0.0), 40.0)]);
        let twice = woods(vec![stroke(Brush::Paint, 0.3, (0.0, 0.0), 40.0); 2]);
        let (l, t) = (
            planned(&light, &Edges::default()).len(),
            planned(&twice, &Edges::default()).len(),
        );
        assert!(l < n && l < t && t < n, "{l} {t} {n}");
        // Wiped out in the middle: none left there.
        let mut wiped = full.clone();
        wiped
            .strokes
            .push(stroke(Brush::Erase, 1.0, (0.0, 0.0), 15.0));
        assert!(
            planned(&wiped, &Edges::default())
                .iter()
                .all(|c| c.pos.length() > 7.0)
        );
        // Both models, the pines three times as often.
        let pines = planned(&full, &Edges::default())
            .iter()
            .filter(|c| c.model == 0)
            .count();
        assert!(pines > 2 * (n - pines), "{pines} of {n}");
        // A hard brush acts fully out to its edge: more copies than a soft one.
        let mut hard = stroke(Brush::Paint, 1.0, (0.0, 0.0), 40.0);
        hard.hardness = 1.0;
        let hard = planned(&woods(vec![hard]), &Edges::default()).len();
        assert!(hard as f64 > 0.9 * area && hard > n, "{hard} of {area}");
    }

    #[test]
    fn single_copies_are_taken_out_and_planted_and_keep_their_places_through_a_rename() {
        let mut s = woods(vec![stroke(Brush::Paint, 1.0, (0.0, 0.0), 30.0)]);
        let before = planned(&s, &Edges::default());
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
        let after = planned(&s, &Edges::default());
        assert_eq!(after.len(), before.len());
        assert!(!after.iter().any(|c| c.id == before[3].id));
        let last = after.last().unwrap();
        assert_eq!(last.id, CopyId::Placed(0));
        assert_eq!((last.model, last.scale), (1, 2.0));
        // Renamed with its seed kept, the painted copies stay where they are.
        s.seed = s.seed();
        s.name = "forest".into();
        assert_eq!(planned(&s, &Edges::default()), after);
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
        assert_eq!(
            solid.indices.len(),
            pine.meshes[0].indices.len() * pines.len()
        );
    }

    #[test]
    fn leaves_turn_in_autumn_fall_in_winter_and_differ_from_copy_to_copy() {
        let at = |k: usize| DVec2::new(k as f64 * 7.3, 3.1);
        let look = |f, month, k| tint(f, month, VARIETY, at(k));
        // Rocks have no leaves to colour.
        assert_eq!(look(Kind::Rigid, 10.0, 0), [0; 4]);
        // Broadleaf trees: bare in January, all in leaf in July, most turned in late
        // October, and not all alike in summer.
        let n = 200;
        let count = |month, f: &dyn Fn([u8; 4]) -> bool| {
            (0..n)
                .filter(|&k| f(look(Kind::Deciduous, month, k)))
                .count()
        };
        assert_eq!(count(0.5, &|l| l == BARE), n);
        assert_eq!(count(6.5, &|l| l == BARE), 0);
        let red_or_yellow = |l: [u8; 4]| l != BARE && l[0] > l[1] / 2 + 40 && l[3] > 200;
        assert!(
            count(9.8, &red_or_yellow) > n / 2,
            "{}",
            count(9.8, &red_or_yellow)
        );
        assert!(count(11.8, &|l| l == BARE) > n * 9 / 10);
        let summer: std::collections::HashSet<_> =
            (0..n).map(|k| look(Kind::Deciduous, 6.5, k)).collect();
        assert!(summer.len() > n / 2);
        // Without variety, summer leaves keep their own colour.
        assert_eq!(tint(Kind::Deciduous, 6.5, 0.0, at(3))[3], 0);
        // Conifers stay in needles; grass is straw in winter.
        assert!((0..n).all(|k| look(Kind::Evergreen, 0.5, k) != BARE));
        let straw = look(Kind::Grass, 0.5, 1);
        assert!(straw[0] > straw[2] + 60 && straw[3] > 150, "{straw:?}");
        // South of the equator the seasons are the other way round.
        let south = open_racing_track::Environment {
            month: 7,
            ..Default::default()
        };
        assert!(south.plant_month(-35.0) < 1.0);
        assert_eq!(south.plant_month(48.0), 6.5);
    }

    /// A square of `side` m round (x, y), painted fully.
    fn square(x: f64, y: f64, side: f64) -> Stroke {
        let h = side / 2.0;
        Stroke {
            fill: true,
            hardness: 1.0,
            points: [(-h, -h), (h, -h), (h, h), (-h, h)]
                .map(|(a, b)| DVec2::new(x + a, y + b))
                .to_vec(),
            ..stroke(Brush::Paint, 1.0, (0.0, 0.0), 0.1)
        }
    }

    #[test]
    fn an_even_layout_keeps_copies_apart_at_the_same_density() {
        let side = 200.0;
        let mut s = woods(vec![square(0.0, 0.0, side)]);
        let grid = planned(&s, &Edges::default());
        s.layout = Layout::Even;
        let even = planned(&s, &Edges::default());
        let expected = side * side / (s.spacing * s.spacing);
        for n in [grid.len(), even.len()] {
            let n = n as f64;
            assert!(
                (0.8 * expected..1.2 * expected).contains(&n),
                "{n} of {expected}"
            );
        }
        // None nearer than about half the spacing, where the grid has some far nearer.
        let nearest = |list: &[Planned]| {
            let mut least = f64::INFINITY;
            for (i, a) in list.iter().enumerate() {
                for b in &list[i + 1..] {
                    if (a.pos.y - b.pos.y).abs() < s.spacing {
                        least = least.min(a.pos.distance(b.pos));
                    }
                }
            }
            least
        };
        assert!(
            nearest(&even) >= EVEN_CELL * s.spacing * 0.999,
            "{}",
            nearest(&even)
        );
        assert!(nearest(&grid) < 0.35 * s.spacing, "{}", nearest(&grid));
        // Painting more round them leaves those there where they are.
        let mut more = s.clone();
        more.strokes.push(square(150.0, 0.0, 100.0));
        let after = planned(&more, &Edges::default());
        assert!(even.iter().all(|c| after.contains(c)));
    }

    #[test]
    fn rows_run_along_the_roads_and_spectators_face_them() {
        let p = Project::new("t");
        let scene = crate::bake::build(&p);
        let surfaces: Vec<_> = p.surfaces.iter().map(|s| s.props).collect();
        let ground = scene.ground.build(&surfaces);
        let keepout = Keepout::new(&scene.roads);
        // Beside the first straight, north of it.
        let mut s = woods(vec![square(125.0, 45.0, 60.0)]);
        s.models = vec![ScatterModel::new(crate::shapes::path("spectator"), 1.0)];
        s.layout = Layout::Rows;
        s.spacing = 2.0;
        s.clearance = 1.0;
        let copies = copies(&s, &keepout, &ground);
        assert!(copies.len() > 100, "{}", copies.len());
        for c in &copies {
            // In rows 2 m apart from 1 m beyond the road's outer edge (as near as the
            // road's sampled frames tell, the straight bowing a little).
            let d = keepout.beyond(c.pos.truncate()) - s.clearance;
            let row = (d / s.spacing).round();
            assert!(row >= 0.0 && (d - row * s.spacing).abs() < 0.5, "{d}");
            // Facing the road, south.
            let facing = DVec2::from_angle(c.yaw);
            assert!(facing.dot(-DVec2::Y) > 0.95, "{facing}");
        }
        // Spectators painted in a crowd face the road too, each a little its own way.
        s.layout = Layout::Grid;
        let crowd = super::copies(&s, &keepout, &ground);
        assert!(!crowd.is_empty());
        for c in &crowd {
            let to = keepout.toward_road(c.pos.truncate()).unwrap();
            assert!(DVec2::from_angle(c.yaw).dot(to) > 0.9);
        }
        // Their clothes differ.
        let looks: std::collections::HashSet<_> = instances(&s, 7.0, &crowd)[0]
            .iter()
            .map(|i| i.tint)
            .collect();
        assert!(looks.len() > 5 && looks.iter().all(|t| t[3] == 254));
    }

    #[test]
    fn a_stamped_stroke_paints_in_patches() {
        let plain = woods(vec![square(0.0, 0.0, 200.0)]);
        let mut stamped = plain.clone();
        let stamp = crate::stamp::Stamp {
            shape: crate::stamp::StampShape::Spots,
            size: 30.0,
            angle: 0.0,
        };
        stamped.strokes[0].stamp = Some(stamp);
        let (all, some) = (
            planned(&plain, &Edges::default()),
            planned(&stamped, &Edges::default()),
        );
        assert!(
            some.len() > 10 && some.len() < all.len() / 3,
            "{} of {}",
            some.len(),
            all.len()
        );
        assert!(some.iter().all(|c| stamp.at(c.pos) > 0.0));
    }

    #[test]
    fn far_models_take_over_at_the_detail_distance() {
        let s = woods(vec![]);
        let [near, far] = fades(&s, 0, true)[..] else {
            panic!("two levels")
        };
        assert_eq!(near.fade_out, far.fade_in);
        assert_eq!(near.fade_out[0], 150.0);
        assert_eq!(far.fade_out[0], 2000.0);
        // Without far models the models in full show out to the draw distance.
        assert_eq!(fades(&s, 0, false)[0].fade_out[0], 2000.0);
        // Drawn however far: never faded out.
        let far = fades(&Scatter { draw: 0.0, ..s }, 0, true)[1];
        assert_eq!(far.fade_out, [FAR_AWAY; 2]);
    }

    #[test]
    fn levels_of_detail_take_over_at_doubling_distances() {
        let s = woods(vec![]);
        let levels = fades(&s, 2, true);
        let starts: Vec<f32> = levels.iter().map(|l| l.fade_in[0]).collect();
        assert_eq!(starts, [0.0, 37.5, 75.0, 150.0]);
        for pair in levels.windows(2) {
            assert_eq!(pair[0].fade_out, pair[1].fade_in);
        }
        // Without far models the last level shows out to the draw distance.
        assert_eq!(fades(&s, 2, false)[2].fade_out[0], 2000.0);
    }
}
