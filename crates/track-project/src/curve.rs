//! Road splines: cubic Bézier segments through the nodes, and their resampling into
//! evenly spaced frames that carry the road's profile values.

use glam::DVec3;

use crate::project::{Node, Range, Road};

/// Subdivisions per segment for measuring arc length.
const SUBDIVISIONS: usize = 64;

/// The Bézier control points of segment `i` (from node `i` to the next).
pub fn segment(nodes: &[Node], closed: bool, i: usize) -> [DVec3; 4] {
    let n = nodes.len();
    let j = (i + 1) % n;
    let (p0, p3) = (nodes[i].pos, nodes[j].pos);
    let (_, out) = handles(nodes, closed, i);
    let (inc, _) = handles(nodes, closed, j);
    [p0, p0 + out, p3 + inc, p3]
}

/// Offsets of node `i`'s incoming and outgoing handles.
pub fn handles(nodes: &[Node], closed: bool, i: usize) -> (DVec3, DVec3) {
    if let Some(h) = nodes[i].handle {
        return (-h, h);
    }
    let n = nodes.len();
    let p = nodes[i].pos;
    let prev = if i > 0 {
        Some(nodes[i - 1].pos)
    } else if closed {
        Some(nodes[n - 1].pos)
    } else {
        None
    };
    let next = if i + 1 < n {
        Some(nodes[i + 1].pos)
    } else if closed {
        Some(nodes[0].pos)
    } else {
        None
    };
    // Catmull-Rom's direction, each side scaled to its own chord so that uneven node
    // spacing does not make the road overshoot.
    let (dir, d_in, d_out) = match (prev, next) {
        (Some(a), Some(b)) => ((b - a).normalize_or_zero(), p.distance(a), p.distance(b)),
        (None, Some(b)) => ((b - p).normalize_or_zero(), 0.0, p.distance(b)),
        (Some(a), None) => ((p - a).normalize_or_zero(), p.distance(a), 0.0),
        (None, None) => (DVec3::ZERO, 0.0, 0.0),
    };
    (-dir * d_in / 3.0, dir * d_out / 3.0)
}

pub fn bezier(c: &[DVec3; 4], t: f64) -> DVec3 {
    let s = 1.0 - t;
    c[0] * (s * s * s) + c[1] * (3.0 * s * s * t) + c[2] * (3.0 * s * t * t) + c[3] * (t * t * t)
}

/// Number of segments of a spline through `n` nodes.
pub fn segments(n: usize, closed: bool) -> usize {
    match (n, closed) {
        (0 | 1, _) => 0,
        (n, true) => n,
        (n, false) => n - 1,
    }
}

/// Position on the road's spline at parameter `u`.
pub fn point(road: &Road, u: f64) -> DVec3 {
    point_on(&road.nodes, road.closed, u)
}

/// Position on the spline through `nodes` at parameter `u`.
pub fn point_on(nodes: &[Node], closed: bool, u: f64) -> DVec3 {
    let segs = segments(nodes.len(), closed);
    if segs == 0 {
        return nodes.first().map_or(DVec3::ZERO, |n| n.pos);
    }
    let u = if closed {
        u.rem_euclid(segs as f64)
    } else {
        u.clamp(0.0, segs as f64)
    };
    let i = (u.floor() as usize).min(segs - 1);
    bezier(&segment(nodes, closed, i), u - i as f64)
}

/// A cross-section of a road: where it is and how it lies.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    /// Spline parameter.
    pub u: f64,
    /// Distance along the road from its first node, m.
    pub s: f64,
    pub pos: DVec3,
    /// Unit direction of travel.
    pub tangent: DVec3,
    /// Unit lateral to the left, in the banked surface.
    pub lateral: DVec3,
    /// Unit surface normal.
    pub normal: DVec3,
    pub width_left: f64,
    pub width_right: f64,
}

/// A road resampled every `spacing` metres or so.
#[derive(Clone, Debug)]
pub struct Sampled {
    pub frames: Vec<Frame>,
    pub length: f64,
    pub closed: bool,
    /// (u, s) along the spline, for converting between the two.
    table: Vec<(f64, f64)>,
}

impl Sampled {
    pub fn new(road: &Road, spacing: f64) -> Self {
        let mut smp = Self::line(&road.nodes, road.closed, spacing, |p| p);
        let period = road.period();
        for f in &mut smp.frames {
            let bank = road.bank.eval(f.u, period, road.closed);
            // As the simulation's centreline: the level lateral rotated by the bank.
            let flat_normal = f.normal;
            f.lateral = f.lateral * bank.cos() - flat_normal * bank.sin();
            f.normal = f.tangent.cross(f.lateral);
            f.width_left = road.width_left.eval(f.u, period, road.closed).max(0.1);
            f.width_right = road.width_right.eval(f.u, period, road.closed).max(0.1);
        }
        smp
    }

    /// The spline through `nodes` resampled every `spacing` metres or so, with level
    /// frames and no width. `place` moves each sample (onto the ground, say) before the
    /// frames' directions are taken from them.
    pub fn line(
        nodes: &[Node],
        closed: bool,
        spacing: f64,
        place: impl Fn(DVec3) -> DVec3,
    ) -> Self {
        let segs = segments(nodes.len(), closed);
        let period = segs as f64;
        // Dense points with their parameter and cumulative length.
        let mut table = Vec::with_capacity(segs * SUBDIVISIONS + 1);
        let mut dense = Vec::with_capacity(segs * SUBDIVISIONS + 1);
        let mut s = 0.0;
        for i in 0..segs {
            let c = segment(nodes, closed, i);
            for k in 0..SUBDIVISIONS {
                let t = k as f64 / SUBDIVISIONS as f64;
                let p = bezier(&c, t);
                if let Some(&last) = dense.last() {
                    s += p.distance(last);
                }
                dense.push(p);
                table.push((i as f64 + t, s));
            }
        }
        let end = point_on(nodes, closed, period);
        if let Some(&last) = dense.last() {
            s += end.distance(last);
        }
        dense.push(end);
        table.push((period, s));
        let length = s;

        let count = if closed {
            (length / spacing).round().max(8.0) as usize
        } else {
            (length / spacing).round().max(1.0) as usize + 1
        };
        let ds = if closed {
            length / count as f64
        } else {
            length / (count - 1) as f64
        };
        let mut j = 0;
        let raw: Vec<(f64, f64, DVec3)> = (0..count)
            .map(|k| {
                let s = (k as f64 * ds).min(length);
                while j + 2 < table.len() && table[j + 1].1 < s {
                    j += 1;
                }
                let (a, b) = (table[j], table[j + 1]);
                let t = if b.1 > a.1 {
                    ((s - a.1) / (b.1 - a.1)).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                (
                    a.0 + (b.0 - a.0) * t,
                    s,
                    place(dense[j].lerp(dense[j + 1], t)),
                )
            })
            .collect();

        let frames = (0..count)
            .map(|k| {
                let (u, s, pos) = raw[k];
                let (prev, next) = if closed {
                    (raw[(k + count - 1) % count].2, raw[(k + 1) % count].2)
                } else {
                    (raw[k.saturating_sub(1)].2, raw[(k + 1).min(count - 1)].2)
                };
                let tangent = (next - prev).normalize_or(DVec3::X);
                let lateral = DVec3::Z.cross(tangent).normalize_or(DVec3::Y);
                Frame {
                    u,
                    s,
                    pos,
                    tangent,
                    lateral,
                    normal: tangent.cross(lateral),
                    width_left: 0.0,
                    width_right: 0.0,
                }
            })
            .collect();
        Self {
            frames,
            length,
            closed,
            table,
        }
    }

    /// Distance along the road at parameter `u`.
    pub fn s_at(&self, u: f64) -> f64 {
        let period = self.table.last().map_or(0.0, |t| t.0);
        let u = if self.closed {
            u.rem_euclid(period.max(1e-9))
        } else {
            u.clamp(0.0, period)
        };
        let i = self
            .table
            .partition_point(|t| t.0 <= u)
            .clamp(1, self.table.len() - 1);
        let (a, b) = (self.table[i - 1], self.table[i]);
        let t = if b.0 > a.0 {
            (u - a.0) / (b.0 - a.0)
        } else {
            0.0
        };
        a.1 + (b.1 - a.1) * t
    }

    /// Parameter at distance `s` along the road.
    pub fn u_at(&self, s: f64) -> f64 {
        let s = if self.closed {
            s.rem_euclid(self.length.max(1e-9))
        } else {
            s.clamp(0.0, self.length)
        };
        let i = self
            .table
            .partition_point(|t| t.1 <= s)
            .clamp(1, self.table.len() - 1);
        let (a, b) = (self.table[i - 1], self.table[i]);
        let t = if b.1 > a.1 {
            (s - a.1) / (b.1 - a.1)
        } else {
            0.0
        };
        a.0 + (b.0 - a.0) * t
    }

    /// Frame at distance `s`, interpolated.
    pub fn frame_at(&self, s: f64) -> Frame {
        let n = self.frames.len();
        let spacing = if self.closed {
            self.length / n as f64
        } else {
            self.length / (n - 1).max(1) as f64
        };
        let s = if self.closed {
            s.rem_euclid(self.length)
        } else {
            s.clamp(0.0, self.length)
        };
        let x = s / spacing;
        let i = (x.floor() as usize).min(n - 1);
        let j = if self.closed {
            (i + 1) % n
        } else {
            (i + 1).min(n - 1)
        };
        let t = x - i as f64;
        let (a, b) = (&self.frames[i], &self.frames[j]);
        let tangent = a.tangent.lerp(b.tangent, t).normalize();
        let normal = a.normal.lerp(b.normal, t);
        let lateral = normal.cross(tangent).normalize();
        Frame {
            u: self.u_at(s),
            s,
            pos: a.pos.lerp(b.pos, t),
            tangent,
            lateral,
            normal: tangent.cross(lateral),
            width_left: a.width_left + (b.width_left - a.width_left) * t,
            width_right: a.width_right + (b.width_right - a.width_right) * t,
        }
    }

    /// Nearest frame to `p` in the horizontal plane, by brute force.
    pub fn nearest(&self, p: DVec3) -> usize {
        let mut best = (0, f64::INFINITY);
        for (i, f) in self.frames.iter().enumerate() {
            let d = (f.pos - p).truncate().length_squared();
            if d < best.1 {
                best = (i, d);
            }
        }
        best.0
    }

    /// How much of something limited to `ranges` is present at `s`, 0..1, easing in and
    /// out over `fade` metres at the ends of each range. Everywhere when there are no
    /// ranges.
    pub fn presence(&self, ranges: &[Range], fade: f64, s: f64) -> f64 {
        if ranges.is_empty() {
            return 1.0;
        }
        ranges
            .iter()
            .map(|r| {
                let (a, b) = (self.s_at(r.from), self.s_at(r.to));
                let (from_start, to_end) = if self.closed {
                    let span = (b - a).rem_euclid(self.length);
                    let x = (s - a).rem_euclid(self.length);
                    if x > span {
                        return 0.0;
                    }
                    (x, span - x)
                } else {
                    if s < a || s > b {
                        return 0.0;
                    }
                    (s - a, b - s)
                };
                if fade <= 0.0 {
                    return 1.0;
                }
                let t = (from_start.min(to_end) / fade).clamp(0.0, 1.0);
                t * t * (3.0 - 2.0 * t)
            })
            .fold(0.0, f64::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{Project, StationCurve};

    fn circle(n: usize, r: f64) -> Road {
        let mut road = Project::new("t").roads.remove(0);
        road.nodes = (0..n)
            .map(|i| {
                let a = std::f64::consts::TAU * i as f64 / n as f64;
                Node::at(r * a.cos(), r * a.sin(), 0.0)
            })
            .collect();
        road
    }

    #[test]
    fn circle_length_and_frames() {
        let road = circle(8, 100.0);
        let smp = Sampled::new(&road, 2.0);
        let expected = std::f64::consts::TAU * 100.0;
        assert!(
            (smp.length - expected).abs() / expected < 0.01,
            "{}",
            smp.length
        );
        for f in &smp.frames {
            // Anticlockwise: left points to the centre, up is up.
            assert!(f.lateral.dot(-f.pos.normalize()) > 0.99);
            assert!(f.normal.z > 0.99);
            assert!((f.pos.length() - 100.0).abs() < 1.5);
        }
    }

    #[test]
    fn parameter_and_distance_round_trip() {
        let road = circle(6, 80.0);
        let smp = Sampled::new(&road, 2.0);
        for u in [0.0, 0.3, 1.0, 2.7, 5.9] {
            assert!((smp.u_at(smp.s_at(u)) - u).abs() < 1e-3, "{u}");
        }
        // Nodes are on the spline.
        assert!(point(&road, 2.0).distance(road.nodes[2].pos) < 1e-9);
    }

    #[test]
    fn curves_wrap_on_closed_roads() {
        let mut c = StationCurve::constant(1.0);
        c.set(2.0, 3.0);
        assert_eq!(c.eval(2.0, 4.0, true), 3.0);
        assert!((c.eval(3.0, 4.0, true) - 2.0).abs() < 1e-9);
        assert_eq!(c.eval(3.0, 4.0, false), 3.0);
        assert!((c.eval(1.0, 4.0, true) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn presence_fades_at_range_ends() {
        let road = circle(8, 100.0);
        let smp = Sampled::new(&road, 2.0);
        let r = [Range { from: 7.5, to: 0.5 }];
        let a = smp.s_at(7.5);
        assert_eq!(smp.presence(&r, 0.0, 0.0), 1.0);
        assert_eq!(smp.presence(&r, 0.0, smp.s_at(4.0)), 0.0);
        assert!((smp.presence(&r, 10.0, a + 5.0) - 0.5).abs() < 1e-9);
    }
}
