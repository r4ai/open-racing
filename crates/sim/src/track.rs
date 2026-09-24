//! Track model: a closed centreline with width and banking, resampled into a
//! uniform table for O(1) hinted queries.

use std::sync::Arc;

use glam::{DVec2, DVec3};
use serde::{Deserialize, Serialize};

use crate::ground::{GroundMesh, SurfaceProps};

/// One control point of the centreline, as authored in `assets/tracks/*.ron`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct TrackPoint {
    /// Centreline position (x, y, z) in metres, Z up.
    pub pos: (f64, f64, f64),
    /// Distance from the centreline to the left / right track edge in metres.
    pub width_left: f64,
    pub width_right: f64,
    /// Banking in radians. Positive tilts the surface normal to the left
    /// (right edge higher), which suits left-hand corners.
    #[serde(default)]
    pub bank: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrackDef {
    pub name: String,
    /// Control points of a closed loop in driving order. The start/finish line is at
    /// the first point.
    pub points: Vec<TrackPoint>,
    /// Width of the kerbs outside each track edge in metres.
    #[serde(default = "default_kerb_width")]
    pub kerb_width: f64,
    /// Height of the kerb crown in metres; the ridges across it rise a fifth higher.
    #[serde(default = "default_kerb_height")]
    pub kerb_height: f64,
    /// Width of the grass run-off outside each kerb in metres. An invisible
    /// barrier stands at its outer edge.
    #[serde(default = "default_runoff_width")]
    pub runoff_width: f64,
    /// Spacing of the resampled centreline table in metres.
    #[serde(default = "default_spacing")]
    pub spacing: f64,
}

fn default_kerb_width() -> f64 {
    1.2
}
fn default_kerb_height() -> f64 {
    0.03
}
fn default_runoff_width() -> f64 {
    30.0
}
fn default_spacing() -> f64 {
    1.0
}

/// Kind of ground under a tyre.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Surface {
    /// The paved racing surface (also concrete and painted areas that count as track).
    Asphalt,
    Kerb,
    /// Paved ground outside the track limits.
    Runoff,
    Grass,
    /// Artificial turf strips beside kerbs.
    Turf,
    /// Gravel and sand traps.
    Gravel,
    /// Bare earth.
    Dirt,
}

/// Loose material a tyre picks up off the road.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coat {
    /// Grass clippings and the soil under them.
    Grass = 0,
    /// Earth and mud.
    Soil = 1,
    /// Sand and gravel.
    Grit = 2,
}

impl Coat {
    pub const ALL: [Self; 3] = [Self::Grass, Self::Soil, Self::Grit];
}

impl Surface {
    /// Grip multiplier relative to the tyre's nominal μ.
    pub fn grip(self) -> f64 {
        match self {
            Self::Asphalt => 1.0,
            Self::Kerb => 0.92,
            Self::Runoff => 0.9,
            Self::Grass => 0.55,
            Self::Turf => 0.7,
            Self::Gravel => 0.5,
            Self::Dirt => 0.6,
        }
    }

    /// Extra rolling resistance coefficient on top of the tyre's own.
    pub fn drag(self) -> f64 {
        match self {
            Self::Asphalt | Self::Kerb | Self::Runoff => 0.0,
            Self::Turf => 0.02,
            Self::Grass => 0.06,
            Self::Dirt => 0.08,
            Self::Gravel => 0.2,
        }
    }

    /// How readily a tyre picks up loose material here, 0..1 (Assetto Corsa's
    /// `DIRT_ADDITIVE`).
    pub fn dirt(self) -> f64 {
        match self {
            Self::Asphalt | Self::Kerb | Self::Runoff => 0.0,
            Self::Turf => 0.1,
            Self::Grass | Self::Gravel | Self::Dirt => 1.0,
        }
    }

    /// What sticks to a tyre rolling here.
    pub fn coat(self) -> Option<Coat> {
        match self {
            Self::Asphalt | Self::Kerb | Self::Runoff => None,
            Self::Grass | Self::Turf => Some(Coat::Grass),
            Self::Dirt => Some(Coat::Soil),
            Self::Gravel => Some(Coat::Grit),
        }
    }

    /// Paved: sheds the coat of a tyre rolling on it, and holds what is shed.
    pub fn paved(self) -> bool {
        matches!(self, Self::Asphalt | Self::Kerb | Self::Runoff)
    }

    /// Outside the track limits.
    pub fn off_track(self) -> bool {
        !matches!(self, Self::Asphalt | Self::Kerb)
    }
}

/// Resampled centreline sample.
#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub pos: DVec3,
    /// Unit tangent in driving direction.
    pub tangent: DVec3,
    /// Unit lateral (to the left) lying in the banked surface.
    pub lateral: DVec3,
    /// Unit surface normal.
    pub normal: DVec3,
    pub width_left: f64,
    pub width_right: f64,
    /// Signed horizontal curvature in 1/m, positive = turning left.
    pub curvature: f64,
}

/// Result of a surface query.
#[derive(Clone, Copy, Debug)]
pub struct TrackQuery {
    /// Segment index; feed back as the hint of the next query.
    pub index: usize,
    /// Distance along the centreline from the start line, in [0, length).
    pub s: f64,
    /// Signed lateral offset from the centreline, positive = left.
    pub d: f64,
    /// Point on the road surface under the query point.
    pub surface_point: DVec3,
    pub normal: DVec3,
    pub tangent: DVec3,
    /// Unit lateral (to the left) lying in the surface.
    pub lateral: DVec3,
    pub surface: Surface,
    /// Grip multiplier of the surface relative to the tyre's nominal μ.
    pub grip: f64,
    /// Extra rolling resistance coefficient of the surface.
    pub drag: f64,
    /// How readily a tyre picks up loose material from the surface, 0..1.
    pub dirt: f64,
    pub width_left: f64,
    pub width_right: f64,
}

impl TrackQuery {
    /// Signed distance from the edge of the drivable area (track + kerbs + run-off)
    /// towards the outside; positive means beyond the barrier.
    pub fn beyond_barrier(&self, track: &Track) -> f64 {
        track.beyond_barrier(self.d, self.width_left, self.width_right)
    }
}

/// Position of a point relative to the centreline.
#[derive(Clone, Copy, Debug)]
pub struct TrackCoords {
    /// Segment index; feed back as the hint of the next query.
    pub index: usize,
    /// Distance along the centreline from the start line, in [0, length).
    pub s: f64,
    /// Signed lateral offset from the centreline, positive = left.
    pub d: f64,
    /// Centreline sample at `s`.
    pub sample: Sample,
}

impl TrackCoords {
    /// Signed distance from the edge of the drivable area (track + kerbs + run-off)
    /// towards the outside; positive means beyond the barrier.
    pub fn beyond_barrier(&self, track: &Track) -> f64 {
        track.beyond_barrier(self.d, self.sample.width_left, self.sample.width_right)
    }
}

#[derive(Debug)]
pub enum TrackError {
    Io(std::io::Error),
    Parse(ron::error::SpannedError),
    Invalid(&'static str),
}

impl std::fmt::Display for TrackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "track: {e}"),
            Self::Parse(e) => write!(f, "track: {e}"),
            Self::Invalid(msg) => write!(f, "track: {msg}"),
        }
    }
}

impl std::error::Error for TrackError {}

/// Immutable, shareable track.
#[derive(Clone, Debug)]
pub struct Track {
    pub name: String,
    pub samples: Vec<Sample>,
    pub spacing: f64,
    pub length: f64,
    pub kerb_width: f64,
    pub kerb_height: f64,
    pub runoff_width: f64,
    /// Road meshes and walls. When present the tyres ride on the meshes and the
    /// centreline only provides track coordinates (s, d).
    pub ground: Option<Arc<GroundMesh>>,
}

/// Ridges across the kerbs: their height as a share of the kerb's crown, and the
/// distance between them along the track in m.
const KERB_RIDGE_SHARE: f64 = 0.2;
const KERB_RIDGE_PITCH: f64 = 0.9;

/// How far above a query point ground rays start, in m. Keeps wheels on their deck
/// under an overpass.
const RAY_UP: f64 = 1.0;

impl Track {
    pub fn from_ron(src: &str) -> Result<Self, TrackError> {
        Self::new(&ron::from_str(src).map_err(TrackError::Parse)?)
    }

    pub fn load(path: impl AsRef<std::path::Path>) -> Result<Self, TrackError> {
        Self::from_ron(&std::fs::read_to_string(path).map_err(TrackError::Io)?)
    }

    /// The bundled default circuit.
    pub fn default_circuit() -> Self {
        Self::from_ron(include_str!("../../../assets/tracks/lakeside.ron"))
            .expect("bundled track is valid")
    }

    pub fn new(def: &TrackDef) -> Result<Self, TrackError> {
        if def.points.len() < 4 {
            return Err(TrackError::Invalid(
                "a track needs at least 4 control points",
            ));
        }
        if def.runoff_width < 0.0 {
            return Err(TrackError::Invalid("runoff width must not be negative"));
        }
        if def.spacing <= 0.0 {
            return Err(TrackError::Invalid("spacing must be positive"));
        }
        if def
            .points
            .iter()
            .any(|p| p.width_left <= 0.0 || p.width_right <= 0.0)
        {
            return Err(TrackError::Invalid("widths must be positive"));
        }

        let dense = densify(&def.points);
        let (samples, length) = resample(&dense, def.spacing);
        Ok(Self {
            name: def.name.clone(),
            spacing: length / samples.len() as f64,
            length,
            samples,
            kerb_width: def.kerb_width,
            kerb_height: def.kerb_height,
            runoff_width: def.runoff_width,
            ground: None,
        })
    }

    /// Makes the tyres ride on `ground` instead of the surface implied by the centreline.
    pub fn with_ground(mut self, ground: GroundMesh) -> Self {
        self.ground = Some(Arc::new(ground));
        self
    }

    /// Deepest penetration of a sphere into the walls: (unit push-out direction, depth).
    pub fn wall_contact(&self, center: DVec3, radius: f64) -> Option<(DVec3, f64)> {
        self.ground.as_ref()?.wall_contact(center, radius)
    }

    /// Index of the sample after `i`, wrapping round the loop.
    #[inline]
    fn next(&self, i: usize) -> usize {
        if i + 1 == self.samples.len() {
            0
        } else {
            i + 1
        }
    }

    /// Index of the sample before `i`, wrapping round the loop.
    #[inline]
    fn prev(&self, i: usize) -> usize {
        if i == 0 {
            self.samples.len() - 1
        } else {
            i - 1
        }
    }

    /// Wraps a distance into [0, length).
    #[inline]
    pub fn wrap_s(&self, s: f64) -> f64 {
        s.rem_euclid(self.length)
    }

    /// Signed shortest distance from `from` to `to` along the loop.
    #[inline]
    pub fn delta_s(&self, from: f64, to: f64) -> f64 {
        let d = (to - from).rem_euclid(self.length);
        if d > 0.5 * self.length {
            d - self.length
        } else {
            d
        }
    }

    /// Interpolated centreline sample at distance `s`.
    pub fn sample_at(&self, s: f64) -> Sample {
        let x = self.wrap_s(s) / self.spacing;
        let i = x.floor() as usize % self.samples.len();
        let u = x - x.floor();
        lerp_sample(&self.samples[i], &self.samples[self.next(i)], u)
    }

    /// Index of the closest sample, by brute force. Use to seed a hint.
    pub fn nearest_index(&self, p: DVec3) -> usize {
        let mut best = (0, f64::INFINITY);
        for (i, smp) in self.samples.iter().enumerate() {
            let d2 = (smp.pos - p).length_squared();
            if d2 < best.1 {
                best = (i, d2);
            }
        }
        best.0
    }

    /// Where `p` lies relative to the centreline, without looking at the ground.
    /// `hint` should be the `index` of a recent query near `p`; the search walks from
    /// there so the cost is O(1) while tracking.
    pub fn locate(&self, p: DVec3, hint: usize) -> TrackCoords {
        let n = self.samples.len();
        let mut i = hint % n;
        let mut last_step = 0isize;
        let mut u;
        let mut steps = 0;
        loop {
            let a = &self.samples[i];
            let b = &self.samples[self.next(i)];
            let seg = (b.pos - a.pos).truncate();
            u = (p - a.pos).truncate().dot(seg) / seg.length_squared();
            let step = if u < 0.0 {
                -1
            } else if u > 1.0 {
                1
            } else {
                0
            };
            // Stop when inside the segment, or when the walk reverses (inside of a
            // sharp corner where the point projects past both neighbouring segments).
            if step == 0 || step == -last_step || steps > n {
                break;
            }
            last_step = step;
            i = if step > 0 { self.next(i) } else { self.prev(i) };
            steps += 1;
            if steps == 64 {
                // Far from the hint: restart from the global nearest sample.
                i = self.nearest_index(p);
                last_step = 0;
            }
        }
        let u = u.clamp(0.0, 1.0);
        let sample = lerp_sample(&self.samples[i], &self.samples[self.next(i)], u);
        TrackCoords {
            index: i,
            s: self.wrap_s((i as f64 + u) * self.spacing),
            d: (p - sample.pos).dot(sample.lateral),
            sample,
        }
    }

    /// How far `p` lies beyond the barrier at the edge of the run-off: (unit direction
    /// back towards the track, depth). See [`Self::locate`] for `hint`.
    pub fn barrier_contact(&self, p: DVec3, hint: usize) -> Option<(DVec3, f64)> {
        let c = self.locate(p, hint);
        let excess = c.beyond_barrier(self);
        (excess > 0.0).then(|| (c.sample.lateral * -c.d.signum(), excess))
    }

    /// Surface information under `p`. See [`Self::locate`] for `hint`.
    pub fn query(&self, p: DVec3, hint: usize) -> TrackQuery {
        let TrackCoords {
            index,
            s,
            d,
            sample: smp,
        } = self.locate(p, hint);
        let hit = self.ground.as_ref().and_then(|g| g.raycast_down(p, RAY_UP));
        let (surface_point, normal, props) = match hit {
            Some(hit) => (hit.point, hit.normal, hit.surface),
            None => {
                let (mut surface, kerb_rise) = self.classify(s, d, smp.width_left, smp.width_right);
                if self.ground.is_some() {
                    // Off the meshes: keep the car up on the centreline plane, as grass.
                    surface = Surface::Grass;
                }
                // Surface plane through the centreline point, offset by the kerb profile.
                let height_along_normal = (p - smp.pos).dot(smp.normal);
                (
                    p - smp.normal * (height_along_normal - kerb_rise),
                    smp.normal,
                    SurfaceProps::of(surface),
                )
            }
        };

        TrackQuery {
            index,
            s,
            d,
            surface_point,
            normal,
            tangent: smp.tangent,
            lateral: smp.lateral,
            surface: props.kind,
            grip: props.grip,
            drag: props.drag,
            dirt: props.dirt(),
            width_left: smp.width_left,
            width_right: smp.width_right,
        }
    }

    #[inline]
    fn beyond_barrier(&self, d: f64, width_left: f64, width_right: f64) -> f64 {
        let edge = self.kerb_width + self.runoff_width;
        if d >= 0.0 {
            d - width_left - edge
        } else {
            -d - width_right - edge
        }
    }

    #[inline]
    fn classify(&self, s: f64, d: f64, wl: f64, wr: f64) -> (Surface, f64) {
        let outside = if d >= 0.0 { d - wl } else { -d - wr };
        if outside <= 0.0 {
            (Surface::Asphalt, 0.0)
        } else if outside <= self.kerb_width {
            // Kerb crown: rises from the track edge, peaks in the middle, with ridges
            // across it.
            let x = outside / self.kerb_width;
            let ridge = (std::f64::consts::PI * s / KERB_RIDGE_PITCH).sin().powi(2);
            (
                Surface::Kerb,
                self.kerb_height
                    * (std::f64::consts::PI * x).sin()
                    * (1.0 + KERB_RIDGE_SHARE * ridge),
            )
        } else {
            (Surface::Grass, 0.0)
        }
    }

    /// World pose (position on the surface, heading tangent) at track coordinates.
    pub fn pose_at(&self, s: f64, d: f64) -> (DVec3, DVec3, DVec3) {
        let smp = self.sample_at(s);
        let pos = smp.pos + smp.lateral * d;
        // The centreline may float a little above or below the meshes; start higher.
        match self.ground.as_ref().and_then(|g| g.raycast_down(pos, 5.0)) {
            Some(hit) => (hit.point, smp.tangent, hit.normal),
            None => (pos, smp.tangent, smp.normal),
        }
    }
}

fn lerp_sample(a: &Sample, b: &Sample, u: f64) -> Sample {
    let tangent = a.tangent.lerp(b.tangent, u).normalize();
    let normal = a.normal.lerp(b.normal, u);
    let lateral = normal.cross(tangent).normalize();
    Sample {
        pos: a.pos.lerp(b.pos, u),
        tangent,
        lateral,
        normal: tangent.cross(lateral),
        width_left: a.width_left + (b.width_left - a.width_left) * u,
        width_right: a.width_right + (b.width_right - a.width_right) * u,
        curvature: a.curvature + (b.curvature - a.curvature) * u,
    }
}

/// Dense point on the spline before uniform resampling: (position, width_left, width_right, bank).
type DensePoint = (DVec3, f64, f64, f64);

/// Centripetal Catmull–Rom through the (closed) control points.
fn densify(points: &[TrackPoint]) -> Vec<DensePoint> {
    const SUB: usize = 64;
    let n = points.len();
    let get = |i: isize| points[i.rem_euclid(n as isize) as usize];
    let pos = |p: &TrackPoint| DVec3::new(p.pos.0, p.pos.1, p.pos.2);
    let mut out = Vec::with_capacity(n * SUB);
    for i in 0..n as isize {
        let (p0, p1, p2, p3) = (get(i - 1), get(i), get(i + 1), get(i + 2));
        let (x0, x1, x2, x3) = (pos(&p0), pos(&p1), pos(&p2), pos(&p3));
        let knot = |a: DVec3, b: DVec3| (b - a).length().sqrt().max(1e-6);
        let t0 = 0.0;
        let t1 = t0 + knot(x0, x1);
        let t2 = t1 + knot(x1, x2);
        let t3 = t2 + knot(x2, x3);
        for k in 0..SUB {
            let u = k as f64 / SUB as f64;
            let t = t1 + (t2 - t1) * u;
            let a1 = x0 * ((t1 - t) / (t1 - t0)) + x1 * ((t - t0) / (t1 - t0));
            let a2 = x1 * ((t2 - t) / (t2 - t1)) + x2 * ((t - t1) / (t2 - t1));
            let a3 = x2 * ((t3 - t) / (t3 - t2)) + x3 * ((t - t2) / (t3 - t2));
            let b1 = a1 * ((t2 - t) / (t2 - t0)) + a2 * ((t - t0) / (t2 - t0));
            let b2 = a2 * ((t3 - t) / (t3 - t1)) + a3 * ((t - t1) / (t3 - t1));
            let c = b1 * ((t2 - t) / (t2 - t1)) + b2 * ((t - t1) / (t2 - t1));
            // Smoothstep for scalar properties so width/bank changes have no kinks.
            let w = u * u * (3.0 - 2.0 * u);
            let lerp = |a: f64, b: f64| a + (b - a) * w;
            out.push((
                c,
                lerp(p1.width_left, p2.width_left),
                lerp(p1.width_right, p2.width_right),
                lerp(p1.bank, p2.bank),
            ));
        }
    }
    out
}

/// Uniform arc-length resampling of the dense closed polyline.
fn resample(dense: &[DensePoint], spacing: f64) -> (Vec<Sample>, f64) {
    let n = dense.len();
    let mut cum = Vec::with_capacity(n + 1);
    cum.push(0.0);
    for i in 0..n {
        let d = (dense[(i + 1) % n].0 - dense[i].0).length();
        cum.push(cum[i] + d);
    }
    let length = cum[n];
    let count = (length / spacing).round().max(8.0) as usize;
    let ds = length / count as f64;

    let mut j = 0;
    let mut raw: Vec<DensePoint> = Vec::with_capacity(count);
    for k in 0..count {
        let s = k as f64 * ds;
        while cum[j + 1] < s {
            j += 1;
        }
        let u = (s - cum[j]) / (cum[j + 1] - cum[j]);
        let (a, b) = (dense[j], dense[(j + 1) % n]);
        raw.push((
            a.0.lerp(b.0, u),
            a.1 + (b.1 - a.1) * u,
            a.2 + (b.2 - a.2) * u,
            a.3 + (b.3 - a.3) * u,
        ));
    }

    let samples = (0..count)
        .map(|k| {
            let prev = raw[(k + count - 1) % count].0;
            let next = raw[(k + 1) % count].0;
            let (pos, wl, wr, bank) = raw[k];
            let tangent = (next - prev).normalize();
            let flat_left = DVec3::Z.cross(tangent).normalize();
            let flat_normal = tangent.cross(flat_left);
            let lateral = flat_left * bank.cos() - flat_normal * bank.sin();
            let t_in = (pos - prev).truncate().normalize();
            let t_out = (next - pos).truncate().normalize();
            let curvature = t_in.perp_dot(t_out).asin_clamped() / ds;
            Sample {
                pos,
                tangent,
                lateral,
                normal: tangent.cross(lateral),
                width_left: wl,
                width_right: wr,
                curvature,
            }
        })
        .collect();
    (samples, length)
}

trait AsinClamped {
    fn asin_clamped(self) -> f64;
}

impl AsinClamped for f64 {
    fn asin_clamped(self) -> f64 {
        self.clamp(-1.0, 1.0).asin()
    }
}

/// Horizontal heading (radians, atan2(y, x)) of a direction vector.
#[inline]
pub fn heading(v: DVec3) -> f64 {
    let h: DVec2 = v.truncate();
    h.y.atan2(h.x)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn circle(radius: f64) -> Track {
        let points = (0..16)
            .map(|i| {
                let a = i as f64 / 16.0 * std::f64::consts::TAU;
                TrackPoint {
                    pos: (radius * a.cos(), radius * a.sin(), 0.0),
                    width_left: 5.0,
                    width_right: 5.0,
                    bank: 0.0,
                }
            })
            .collect();
        Track::new(&TrackDef {
            name: "circle".into(),
            points,
            kerb_width: 1.0,
            kerb_height: 0.0,
            runoff_width: 30.0,
            spacing: 1.0,
        })
        .unwrap()
    }

    #[test]
    fn circle_geometry() {
        let t = circle(100.0);
        assert!(
            (t.length - std::f64::consts::TAU * 100.0).abs() < 0.5,
            "{}",
            t.length
        );
        // Counter-clockwise circle turns left everywhere.
        for s in &t.samples {
            assert!((s.curvature - 0.01).abs() < 2.5e-3, "{}", s.curvature);
        }
        let total: f64 = t.samples.iter().map(|s| s.curvature * t.spacing).sum();
        assert!((total - std::f64::consts::TAU).abs() < 0.01, "{total}");
    }

    #[test]
    fn query_lateral_offset_and_surface() {
        let t = circle(100.0);
        // Point inside the circle = left of a CCW centreline.
        let q = t.query(DVec3::new(97.0, 0.0, 0.5), 0);
        assert!((q.d - 3.0).abs() < 0.05, "{}", q.d);
        assert_eq!(q.surface, Surface::Asphalt);
        assert!(q.surface_point.z.abs() < 1e-6);
        let q = t.query(DVec3::new(94.5, 0.0, 0.0), 0);
        assert_eq!(q.surface, Surface::Kerb);
        let q = t.query(DVec3::new(90.0, 0.0, 0.0), 0);
        assert_eq!(q.surface, Surface::Grass);
    }

    #[test]
    fn hinted_query_matches_bruteforce() {
        let t = circle(100.0);
        let mut hint = 0;
        for k in 0..2000 {
            let a = k as f64 * 0.003;
            let p = DVec3::new(101.0 * a.cos(), 101.0 * a.sin(), 0.0);
            let q = t.query(p, hint);
            hint = q.index;
            let expect = t.wrap_s(a * 100.0 * t.length / (std::f64::consts::TAU * 100.0));
            assert!(
                t.delta_s(q.s, expect).abs() < 0.5,
                "k={k} s={} expect={expect}",
                q.s
            );
        }
    }

    #[test]
    fn bundled_track_loads() {
        let t = Track::default_circuit();
        assert!(t.length > 2000.0);
    }
}
