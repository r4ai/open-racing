//! Track evolution: rubber the cars lay on the racing surface, and dirt dragged onto
//! it from off the track.
//!
//! An unused track is dusty. Cars clean the asphalt where they drive and lay rubber
//! into it, so grip builds up along the racing line while the rest of the road stays
//! dirty. Rubber comes from the tread's frictional work, so most of it goes down in
//! braking zones, through corners and where cars accelerate out of them; straights
//! gain little.
//!
//! The state is a grid in track coordinates (distance along the centreline × lateral
//! offset) holding the rubber level of each patch: 0 is dusty ([`DUSTY_GRIP`]), 1 fully
//! rubbered in (the tyre's nominal grip). Named starting conditions put rubber along a
//! racing line estimated from the track geometry (the minimum-curvature line, driven
//! at an estimated speed); from there every tyre lays rubber where it actually rolls.
//! Only asphalt carries rubber: kerbs and run-off keep their own grip.
//!
//! Tyres coated with grass, soil or grit off the track shed it where they rejoin, and
//! it costs grip on the road until tyres rolling over it have swept it away, within a
//! few dozen metres of rolling.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::Arc;

use glam::DVec3;

use crate::track::{Surface, Track};

/// Grip of dusty asphalt relative to a fully rubbered-in surface.
pub const DUSTY_GRIP: f64 = 0.90;
/// Grip off the racing line once a line is rubbered in: dust collects where cars do
/// not drive.
pub const OFF_LINE_GRIP: f64 = 0.94;

/// Grip lost on asphalt fully covered, per kind of coat ([`crate::Coat`] order).
const ROAD_COAT_GRIP_LOSS: [f64; 3] = [0.3, 0.3, 0.35];
/// Cover a cell gets per full tyre coat shed onto it.
const DIRT_PER_COAT: f64 = 6.0;
/// Tyre rolling over which the cover on the asphalt is mostly swept away, m, per kind
/// of coat: grass and soil smear into the surface, grit is flung aside.
const COAT_SWEEP_LENGTH: [f64; 3] = [25.0, 30.0, 10.0];
/// Share of the swept cover that sticks to the tyre sweeping it.
const SWEEP_PICKUP: f64 = 0.5;
/// Cover below which a cell counts as clean again.
const CLEAN: f32 = 1e-4;

/// Rubber a tyre lays, relative to one at the grip limit: a share even when it just
/// rolls, the rest growing with the square of how much of its grip it uses.
const RUBBER_ROLLING: f64 = 0.2;
/// Mean of that over a lap at racing pace, which `gain_per_lap` refers to.
const RUBBER_LAP_MEAN: f64 = 0.45;

/// Grid cell along the centreline and across it, m.
const CELL_S: f64 = 4.0;
const CELL_D: f64 = 0.5;
/// Spacing of the points of the estimated racing line, m.
const LINE_SPACING: f64 = 10.0;
/// Distance the racing line keeps from the track edges (half a car and a little), m.
const LINE_MARGIN: f64 = 1.3;
/// The rubbered band: full rubber within this distance of the line, none beyond the
/// outer one, m. Cars do not all drive the same line, so the band is wider than a car.
const BAND_INNER: f64 = 1.0;
const BAND_OUTER: f64 = 2.5;
/// Share of the line's rubber on straights, where tyres do little work.
const STRAIGHT_RUBBER: f64 = 0.35;
/// Speed profile of the estimated line: lateral grip, braking and acceleration in
/// m/s², top speed in m/s (a GT car).
const LINE_LATERAL: f64 = 15.0;
const LINE_BRAKING: f64 = 14.0;
const LINE_ACCELERATION: f64 = 6.0;
const LINE_TOP_SPEED: f64 = 75.0;

/// Named starting conditions, from a dirty track to a fully rubbered-in one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackCondition {
    /// Unused for a long time: dust over the whole road.
    Dusty,
    /// Cleaned, or washed by rain: no rubber anywhere.
    Green,
    /// Some rubber down on the racing line, as after practice.
    Fast,
    /// Racing line fully rubbered in, as late in a race.
    Optimum,
}

impl TrackCondition {
    pub const ALL: [Self; 4] = [Self::Dusty, Self::Green, Self::Fast, Self::Optimum];

    /// Grip where the racing line is rubbered most (braking zones and corners),
    /// relative to a fully rubbered-in surface.
    pub fn grip(self) -> f64 {
        match self {
            Self::Dusty => DUSTY_GRIP,
            Self::Green => OFF_LINE_GRIP,
            Self::Fast => 0.97,
            Self::Optimum => 1.0,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Dusty => "dusty",
            Self::Green => "green",
            Self::Fast => "fast",
            Self::Optimum => "optimum",
        }
    }

    /// The named condition closest to a racing-line grip.
    pub fn nearest(grip: f64) -> Self {
        Self::ALL
            .into_iter()
            .min_by(|a, b| (a.grip() - grip).abs().total_cmp(&(b.grip() - grip).abs()))
            .expect("conditions exist")
    }
}

/// Parses a racing-line grip level: a condition name (`green`) or a number, as a
/// fraction (`0.97`) or in percent (`97`).
pub fn parse_grip(s: &str) -> Result<f64, String> {
    let s = s.trim();
    if let Some(c) = TrackCondition::ALL
        .into_iter()
        .find(|c| c.name().eq_ignore_ascii_case(s))
    {
        return Ok(c.grip());
    }
    let value: f64 = s.trim_end_matches('%').parse().map_err(|_| {
        format!("grip level `{s}`: expected dusty, green, fast, optimum or a number such as 0.97")
    })?;
    let grip = if value > 1.5 { value / 100.0 } else { value };
    if (DUSTY_GRIP..=1.0).contains(&grip) {
        Ok(grip)
    } else {
        Err(format!(
            "grip level `{s}` is outside {:.0}–100 %",
            DUSTY_GRIP * 100.0
        ))
    }
}

fn rubber_of(grip: f64) -> f64 {
    ((grip - DUSTY_GRIP) / (1.0 - DUSTY_GRIP)).clamp(0.0, 1.0)
}

fn smoothstep(lo: f64, hi: f64, x: f64) -> f64 {
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Grid over a track, where its racing line lies and how hard cars work their tyres
/// along it. Built once per track and shared.
#[derive(Debug)]
pub struct RubberMap {
    rows: usize,
    cols: usize,
    /// Lateral offset of the right edge of column 0, m.
    d0: f64,
    /// Rubber share of each cell when the line is rubbered in, 0..1, row-major
    /// (row = cell along the centreline).
    band: Vec<f32>,
    /// Lateral offset of the racing line at the centre of each row, m (positive = left).
    line: Vec<f32>,
}

impl RubberMap {
    pub fn new(track: &Track) -> Self {
        let rows = (track.length / CELL_S).ceil().max(1.0) as usize;
        let reach = |f: fn(&crate::track::Sample) -> f64| {
            track.samples.iter().map(f).fold(0.0, f64::max) + CELL_D
        };
        let (left, right) = (reach(|s| s.width_left), reach(|s| s.width_right));
        let cols = ((left + right) / CELL_D).ceil() as usize;
        let d0 = -right;
        let line = RacingLine::new(track);
        let step = track.length / line.offset.len() as f64;
        let at = |values: &[f64], s: f64| {
            let x = s / step;
            let (i, u) = (x.floor() as usize, x - x.floor());
            let n = values.len();
            values[i % n] * (1.0 - u) + values[(i + 1) % n] * u
        };
        let mut band = vec![0.0; rows * cols];
        let mut offsets = vec![0.0; rows];
        for r in 0..rows {
            let s = (r as f64 + 0.5) * CELL_S;
            let d_line = at(&line.offset, s);
            let work = at(&line.work, s);
            let strength = STRAIGHT_RUBBER + (1.0 - STRAIGHT_RUBBER) * work;
            offsets[r] = d_line as f32;
            for c in 0..cols {
                let d = d0 + (c as f64 + 0.5) * CELL_D;
                let from_line = d - d_line;
                band[r * cols + c] =
                    (strength * (1.0 - smoothstep(BAND_INNER, BAND_OUTER, from_line.abs()))) as f32;
            }
        }
        Self {
            rows,
            cols,
            d0,
            band,
            line: offsets,
        }
    }

    /// Lateral offset of the estimated racing line at distance `s`, m (positive = left).
    pub fn racing_line(&self, s: f64) -> f64 {
        f64::from(self.line[self.row(s)])
    }

    #[inline]
    fn row(&self, s: f64) -> usize {
        ((s / CELL_S).max(0.0) as usize).min(self.rows - 1)
    }

    /// The two columns straddling `d` and the weight of the second.
    #[inline]
    fn columns(&self, d: f64) -> (usize, usize, f64) {
        let x = ((d - self.d0) / CELL_D - 0.5).clamp(0.0, (self.cols - 1) as f64);
        let c = x.floor() as usize;
        (c, (c + 1).min(self.cols - 1), x - x.floor())
    }
}

/// Minimum-curvature line through a track and how hard a car driving it works its
/// tyres, at points spaced about [`LINE_SPACING`] apart.
struct RacingLine {
    /// Lateral offset from the centreline, m.
    offset: Vec<f64>,
    /// Share of the tyres' grip used, squared, 0..1.
    work: Vec<f64>,
}

impl RacingLine {
    fn new(track: &Track) -> Self {
        let n = ((track.length / LINE_SPACING).round() as usize).max(8);
        let ds = track.length / n as f64;
        let points: Vec<_> = (0..n).map(|i| track.sample_at(i as f64 * ds)).collect();
        let offset = Self::min_curvature(&points);
        let pos: Vec<DVec3> = (0..n)
            .map(|i| points[i].pos + points[i].lateral * offset[i])
            .collect();
        let curvature: Vec<f64> = (0..n)
            .map(|i| {
                let (a, b, c) = (pos[(i + n - 1) % n], pos[i], pos[(i + 1) % n]);
                let (u, v) = ((b - a).truncate(), (c - b).truncate());
                let turn = u.perp_dot(v).atan2(u.dot(v));
                2.0 * turn / (u.length() + v.length()).max(1e-6)
            })
            .collect();
        // Speed profile: cornering limit, then braking into and accelerating out of
        // each corner, twice round the loop so the start line joins up.
        let mut v: Vec<f64> = curvature
            .iter()
            .map(|k| {
                (LINE_LATERAL / k.abs().max(1e-6))
                    .sqrt()
                    .min(LINE_TOP_SPEED)
            })
            .collect();
        for _ in 0..2 {
            for i in (0..n).rev() {
                let next = v[(i + 1) % n];
                v[i] = v[i].min((next * next + 2.0 * LINE_BRAKING * ds).sqrt());
            }
            for i in 0..n {
                let prev = v[(i + n - 1) % n];
                v[i] = v[i].min((prev * prev + 2.0 * LINE_ACCELERATION * ds).sqrt());
            }
        }
        let raw: Vec<f64> = (0..n)
            .map(|i| {
                let next = v[(i + 1) % n];
                let long = (next * next - v[i] * v[i]) / (2.0 * ds);
                let long = long
                    / if long < 0.0 {
                        LINE_BRAKING
                    } else {
                        LINE_ACCELERATION
                    };
                let lat = v[i] * v[i] * curvature[i] / LINE_LATERAL;
                (long * long + lat * lat).min(1.0)
            })
            .collect();
        // Rubber spreads a little along the track.
        let work = (0..n)
            .map(|i| (raw[(i + n - 1) % n] + 2.0 * raw[i] + raw[(i + 1) % n]) / 4.0)
            .collect();
        Self { offset, work }
    }

    /// Projected gradient descent on the squared second differences of the line, kept
    /// [`LINE_MARGIN`] inside the edges.
    fn min_curvature(points: &[crate::track::Sample]) -> Vec<f64> {
        const ITERATIONS: usize = 20_000;
        const RATE: f64 = 0.05;
        let n = points.len();
        let limits: Vec<(f64, f64)> = points
            .iter()
            .map(|p| {
                let (lo, hi) = (LINE_MARGIN - p.width_right, p.width_left - LINE_MARGIN);
                if lo <= hi {
                    (lo, hi)
                } else {
                    let mid = 0.5 * (lo + hi);
                    (mid, mid)
                }
            })
            .collect();
        let mut d = vec![0.0; n];
        let mut k = vec![DVec3::ZERO; n];
        for _ in 0..ITERATIONS {
            let pos = |i: usize, d: &[f64]| points[i].pos + points[i].lateral * d[i];
            for (i, k) in k.iter_mut().enumerate() {
                *k = pos((i + n - 1) % n, &d) - 2.0 * pos(i, &d) + pos((i + 1) % n, &d);
            }
            let mut moved: f64 = 0.0;
            for i in 0..n {
                let g = (k[(i + n - 1) % n] - 2.0 * k[i] + k[(i + 1) % n]).dot(points[i].lateral);
                let next = (d[i] - RATE * g).clamp(limits[i].0, limits[i].1);
                moved = moved.max((next - d[i]).abs());
                d[i] = next;
            }
            if moved < 1e-5 {
                break;
            }
        }
        d
    }
}

/// Hashes grid cell indices with one multiply.
#[derive(Default)]
struct CellHasher(u64);

impl Hasher for CellHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 << 8 | u64::from(b)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
    }
    fn write_u32(&mut self, i: u32) {
        self.0 = u64::from(i).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

type CellMap<V> = HashMap<u32, V, BuildHasherDefault<CellHasher>>;

/// Rubber on a track: how much lies on each patch of asphalt, and how fast the cars
/// add to it; and the dirt they drag onto it.
#[derive(Clone, Debug)]
pub struct TrackEvolution {
    /// `None`: the whole asphalt has the tyre's nominal grip and nothing changes.
    map: Option<Arc<RubberMap>>,
    /// Rubber level on the racing line and off it at the start.
    line: f64,
    off: f64,
    /// Rubber laid by the cars since the start, per cell (empty without evolution).
    laid: Vec<f32>,
    /// Loose material on the asphalt by kind ([`crate::Coat`] order; 1 covers the
    /// asphalt), only where there is some.
    cover: CellMap<[f32; 3]>,
    /// Rubber level one tyre at the grip limit adds per metre it rolls over a cell.
    rate: f64,
}

impl TrackEvolution {
    /// Default rise where the line is worked hardest: +1 % grip every 5 laps of one
    /// car, so a green line is rubbered in after about 30 laps.
    pub const DEFAULT_GAIN_PER_LAP: f64 = 0.002;
    /// Uniform grip over the whole asphalt, with no rubber model.
    pub const UNIFORM: Self = Self {
        map: None,
        line: 1.0,
        off: 1.0,
        laid: Vec::new(),
        cover: HashMap::with_hasher(BuildHasherDefault::new()),
        rate: 0.0,
    };

    /// Rubber on the racing line of `map` giving `line_grip` where it is worked
    /// hardest, off the line at most [`OFF_LINE_GRIP`]. Every lap one car drives adds
    /// about `gain_per_lap` of grip where its tyres roll.
    pub fn new(map: Arc<RubberMap>, line_grip: f64, gain_per_lap: f64) -> Self {
        let mut e = Self {
            line: 0.0,
            off: 0.0,
            laid: Vec::new(),
            cover: CellMap::default(),
            map: Some(map),
            rate: 0.0,
        };
        e.restart(line_grip, gain_per_lap);
        e
    }

    /// Starts over from `line_grip` on the racing line, now gaining `gain_per_lap`.
    pub fn restart(&mut self, line_grip: f64, gain_per_lap: f64) {
        let cells = match &self.map {
            Some(map) if gain_per_lap > 0.0 => map.band.len(),
            _ => 0,
        };
        self.laid.resize(cells, 0.0);
        // Front and rear tyres each roll over a cell once a lap.
        self.rate = gain_per_lap.max(0.0) / (1.0 - DUSTY_GRIP) / (2.0 * CELL_S) / RUBBER_LAP_MEAN;
        self.reset(line_grip);
    }

    /// Starts over from `line_grip` on the racing line, without the rubber and dirt
    /// the cars left since.
    pub fn reset(&mut self, line_grip: f64) {
        self.line = rubber_of(line_grip);
        self.off = rubber_of(line_grip.min(OFF_LINE_GRIP));
        self.laid.fill(0.0);
        self.cover.clear();
    }

    pub fn map(&self) -> Option<&RubberMap> {
        self.map.as_deref()
    }

    /// Grip on the racing line (where it is worked hardest) and off it at the start.
    pub fn start_grip(&self) -> (f64, f64) {
        let grip = |r: f64| DUSTY_GRIP + (1.0 - DUSTY_GRIP) * r;
        (grip(self.line), grip(self.off))
    }

    /// Rubber level of cell `i` of `map`.
    #[inline]
    fn cell_rubber(&self, map: &RubberMap, i: usize) -> f64 {
        let laid = self.laid.get(i).copied().unwrap_or(0.0);
        let band = f64::from(map.band[i]);
        (self.off + (self.line - self.off) * band + f64::from(laid)).min(1.0)
    }

    /// Grip of cell `i` of `map` from its rubber and cover.
    #[inline]
    fn cell_grip(&self, map: &RubberMap, i: usize) -> f64 {
        let rubber = self.cell_rubber(map, i);
        let covered: f64 = match self.cover.get(&(i as u32)) {
            Some(cover) => (0..3)
                .map(|k| ROAD_COAT_GRIP_LOSS[k] * f64::from(cover[k]))
                .sum(),
            None => 0.0,
        };
        (DUSTY_GRIP + (1.0 - DUSTY_GRIP) * rubber) * (1.0 - covered.min(0.6))
    }

    /// Grip multiplier the rubber and dirt give the surface at track coordinates (s, d).
    #[inline]
    pub fn grip_at(&self, surface: Surface, s: f64, d: f64) -> f64 {
        if surface != Surface::Asphalt {
            return 1.0;
        }
        let Some(map) = self.map.as_deref() else {
            return DUSTY_GRIP + (1.0 - DUSTY_GRIP) * self.line;
        };
        let base = map.row(s) * map.cols;
        let (a, b, t) = map.columns(d);
        self.cell_grip(map, base + a) * (1.0 - t) + self.cell_grip(map, base + b) * t
    }

    /// Rubber level at track coordinates (s, d), 0 on dusty asphalt, 1 where it is
    /// fully rubbered in.
    pub fn rubber_at(&self, s: f64, d: f64) -> f64 {
        let Some(map) = self.map.as_deref() else {
            return self.line;
        };
        let base = map.row(s) * map.cols;
        let (a, b, t) = map.columns(d);
        self.cell_rubber(map, base + a) * (1.0 - t) + self.cell_rubber(map, base + b) * t
    }

    /// Loose material on the asphalt at track coordinates (s, d) by kind ([`crate::Coat`]
    /// order), 1 covering it.
    pub fn cover_at(&self, s: f64, d: f64) -> [f64; 3] {
        let Some(map) = self.map.as_deref() else {
            return [0.0; 3];
        };
        let base = map.row(s) * map.cols;
        let (a, b, t) = map.columns(d);
        let cover = |i: usize| self.cover.get(&(i as u32)).copied().unwrap_or_default();
        let (ca, cb) = (cover(base + a), cover(base + b));
        std::array::from_fn(|k| f64::from(ca[k]) * (1.0 - t) + f64::from(cb[k]) * t)
    }

    /// A tyre rolls `distance` metres over the asphalt at (s, d), using `grip_use`
    /// (0..1) of its grip and shedding `shed` of a full coat: lays rubber, leaves the
    /// dirt, and sweeps up dirt already there. Returns the share of a full coat the
    /// tyre picks up from the road, by kind.
    #[inline]
    pub fn roll(
        &mut self,
        s: f64,
        d: f64,
        distance: f64,
        grip_use: f64,
        shed: [f64; 3],
    ) -> [f64; 3] {
        let Some(map) = self.map.as_deref() else {
            return [0.0; 3];
        };
        let base = map.row(s) * map.cols;
        let (a, b, t) = map.columns(d);
        let mut picked = [0.0; 3];
        let shedding = shed.iter().any(|&x| x > 0.0);
        let rubber = self.rate
            * distance
            * (RUBBER_ROLLING + (1.0 - RUBBER_ROLLING) * grip_use.min(1.0).powi(2));
        for (i, w) in [(base + a, 1.0 - t), (base + b, t)] {
            if let Some(laid) = self.laid.get_mut(i) {
                *laid += (rubber * w) as f32;
            }
            let key = i as u32;
            if !shedding && !self.cover.contains_key(&key) {
                continue;
            }
            let cover = self.cover.entry(key).or_default();
            let mut left = 0.0;
            for k in 0..3 {
                let swept = f64::from(cover[k]) * (distance / COAT_SWEEP_LENGTH[k]).min(1.0) * w;
                cover[k] = (f64::from(cover[k]) - swept + shed[k] * DIRT_PER_COAT * w) as f32;
                picked[k] += swept * SWEEP_PICKUP / DIRT_PER_COAT;
                left += cover[k];
            }
            if left < CLEAN {
                self.cover.remove(&key);
            }
        }
        picked
    }
}

impl Default for TrackEvolution {
    fn default() -> Self {
        Self::UNIFORM
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_fractions_and_percent() {
        assert_eq!(parse_grip("Optimum"), Ok(1.0));
        assert_eq!(parse_grip("0.97"), Ok(0.97));
        assert_eq!(parse_grip("96%"), Ok(0.96));
        assert!(parse_grip("0.5").is_err());
        assert!(parse_grip("wet").is_err());
    }

    fn assert_close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-6, "{a} vs {b}");
    }

    #[test]
    fn racing_line_is_rubbered_most_where_cars_brake_and_corner() {
        let track = Track::default_circuit();
        let map = Arc::new(RubberMap::new(&track));
        let e = TrackEvolution::new(map.clone(), 1.0, 0.0);
        let (mut apexes, mut straight, mut corner) = (0, f64::INFINITY, 0.0_f64);
        for i in 0..track.samples.len() / 4 {
            let s = i as f64 * 4.0 + 2.0;
            let line = map.racing_line(s);
            let smp = track.sample_at(s);
            assert!(line < smp.width_left && -line < smp.width_right);
            let on_line = e.grip_at(Surface::Asphalt, s, line);
            assert!(on_line > OFF_LINE_GRIP - 1e-9 && on_line <= 1.0);
            if smp.curvature.abs() < 1e-4 {
                straight = straight.min(on_line);
            } else {
                corner = corner.max(on_line);
            }
            // Far off the line: dust.
            let off = if line > 0.0 { line - 4.0 } else { line + 4.0 };
            assert_close(e.grip_at(Surface::Asphalt, s, off), OFF_LINE_GRIP);
            assert_close(e.grip_at(Surface::Kerb, s, off), 1.0);
            // The line cuts to the inside of corners.
            if smp.curvature.abs() > 0.01 && line * smp.curvature.signum() > 1.0 {
                apexes += 1;
            }
        }
        assert!(apexes > 0);
        assert!(corner > 0.995, "corners {corner}");
        assert!(straight < 0.97, "straights {straight}");
    }

    #[test]
    fn tyres_lay_rubber_where_they_work() {
        let track = Track::default_circuit();
        let map = Arc::new(RubberMap::new(&track));
        let green = TrackCondition::Green.grip();
        let mut e = TrackEvolution::new(map, green, 0.01);
        let (s, d) = (100.0, e.map().unwrap().racing_line(100.0));
        let before = e.grip_at(Surface::Asphalt, s, d);
        // Five laps of front and rear tyres over one cell at the grip limit.
        for _ in 0..10 {
            e.roll(s, d, CELL_S, 1.0, [0.0; 3]);
        }
        let hard = e.grip_at(Surface::Asphalt, s, d) - before;
        e.reset(green);
        for _ in 0..10 {
            e.roll(s, d, CELL_S, 0.0, [0.0; 3]);
        }
        let cruising = e.grip_at(Surface::Asphalt, s, d) - before;
        assert!(hard > 0.03, "gained {hard}");
        assert!(cruising < hard * 0.3, "cruising {cruising} vs {hard}");
        assert_close(e.grip_at(Surface::Asphalt, s, d + 4.0), green);
    }

    #[test]
    fn dirt_shed_on_the_road_costs_grip_until_swept_away() {
        let track = Track::default_circuit();
        let map = Arc::new(RubberMap::new(&track));
        let mut e = TrackEvolution::new(map, 1.0, 0.0);
        let (s, d) = (100.0, 0.0);
        let clean = e.grip_at(Surface::Asphalt, s, d);
        assert_eq!(e.roll(s, d, 1.0, 0.0, [0.05, 0.0, 0.0]), [0.0; 3]);
        let dirty = e.grip_at(Surface::Asphalt, s, d);
        assert!(dirty < clean - 0.02, "{dirty} vs {clean}");
        let picked: f64 = (0..3000).map(|_| e.roll(s, d, 1.0, 0.0, [0.0; 3])[0]).sum();
        assert!(
            (picked - 0.05 * SWEEP_PICKUP).abs() < 2e-3,
            "picked {picked}"
        );
        assert!(e.grip_at(Surface::Asphalt, s, d) > clean - 1e-6);
        assert!(e.cover.is_empty(), "swept cells are dropped");
    }
}
