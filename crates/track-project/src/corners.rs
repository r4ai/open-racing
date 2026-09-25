//! A road's corners, found from its curvature, numbered from the start line as
//! circuits number their turns, and kerbs laid round each: on the outside where cars
//! turn in and run out, on the inside at the apex.

use crate::curve::Sampled;
use crate::ops::Op;
use crate::project::{Profile, Project, Range, Side, Strip};

/// A corner of a road. Places are distances along the road, m, in order: on a loop a
/// corner across the start runs on past the loop's length.
#[derive(Clone, Debug, PartialEq)]
pub struct Corner {
    /// 1 for the first corner after the start line.
    pub number: usize,
    /// The way it turns: left is anticlockwise seen from above.
    pub dir: Side,
    pub entry: f64,
    /// Where it turns tightest.
    pub apex: f64,
    pub exit: f64,
    /// Tightest radius, m.
    pub radius: f64,
    /// How far it turns, radians.
    pub angle: f64,
}

impl Corner {
    /// The side on the inside of the turn.
    pub fn inside(&self) -> Side {
        self.dir
    }

    pub fn outside(&self) -> Side {
        match self.dir {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }

    /// The corner's length along the road, m, on a road `length` long.
    pub fn length(&self, length: f64) -> f64 {
        (self.exit - self.entry).rem_euclid(length.max(1e-9))
    }

    /// Whether `s` lies within the corner, or `margin` metres either side.
    pub fn covers(&self, s: f64, margin: f64, length: f64, closed: bool) -> bool {
        let (a, b) = (self.entry - margin, self.exit + margin);
        if closed {
            (s - a).rem_euclid(length) <= (b - a).rem_euclid(length)
        } else {
            (a..=b).contains(&s)
        }
    }
}

/// Tighter than this radius, m, a road is turning.
const TURNING: f64 = 350.0;
/// Least turn that makes a corner.
const LEAST_ANGLE: f64 = 0.35;
/// Length curvature is averaged over, m, so that a wobble is not a corner.
const WINDOW: f64 = 20.0;

/// The corners of a sampled road, numbered from `start` (m along it).
pub fn find(smp: &Sampled, start: f64) -> Vec<Corner> {
    let f = &smp.frames;
    let n = f.len();
    if n < 5 {
        return vec![];
    }
    let idx = |k: isize| -> Option<usize> {
        if smp.closed {
            Some(k.rem_euclid(n as isize) as usize)
        } else {
            (0..n as isize).contains(&k).then_some(k as usize)
        }
    };
    let gap = |a: usize, b: usize| {
        let d = f[b].s - f[a].s;
        if smp.closed {
            d.rem_euclid(smp.length)
        } else {
            d
        }
    };
    // Signed curvature at each frame, positive to the left, then averaged.
    let raw: Vec<f64> = (0..n as isize)
        .map(|k| {
            let (Some(a), Some(b)) = (idx(k - 1).or(idx(k)), idx(k + 1).or(idx(k))) else {
                return 0.0;
            };
            let ds = gap(a, b);
            if ds < 1e-9 {
                return 0.0;
            }
            let (ta, tb) = (f[a].tangent.truncate(), f[b].tangent.truncate());
            ta.normalize_or_zero().angle_to(tb.normalize_or_zero()) / ds
        })
        .collect();
    let spacing = smp.length / n as f64;
    let reach = ((0.5 * WINDOW / spacing.max(1e-9)).round() as isize).max(1);
    let kappa: Vec<f64> = (0..n as isize)
        .map(|k| {
            let ks: Vec<usize> = (-reach..=reach).filter_map(|d| idx(k + d)).collect();
            ks.iter().map(|&j| raw[j]).sum::<f64>() / ks.len() as f64
        })
        .collect();
    let turning = |k: usize| kappa[k].abs() > 1.0 / TURNING;
    // Runs of frames turning the same way; on a loop, begin where it is not turning.
    let first = if smp.closed {
        match (0..n).find(|&k| !turning(k)) {
            Some(k) => k,
            None => return vec![],
        }
    } else {
        0
    };
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut open: Option<usize> = None;
    for step in 0..=n {
        let k = (first + step) % n;
        let end = step == n || (!smp.closed && k < first);
        let same = open.is_some_and(|o| turning(k) && kappa[k].signum() == kappa[o].signum());
        if let Some(o) = open
            && (end || !same)
        {
            runs.push((o, (k + n - 1) % n));
            open = None;
        }
        if end {
            break;
        }
        if open.is_none() && turning(k) {
            open = Some(k);
        }
    }
    let mut corners: Vec<Corner> = runs
        .into_iter()
        .filter_map(|(a, b)| {
            let len = (b + n - a) % n + 1;
            let ks = (0..len).map(|i| (a + i) % n);
            let apex = ks
                .clone()
                .max_by(|&x, &y| kappa[x].abs().total_cmp(&kappa[y].abs()))?;
            let angle: f64 = ks.map(|k| raw[k] * spacing).sum::<f64>().abs();
            // Across the start of a loop, the later places run on past its length.
            let (entry, mut apex, mut exit) = (f[a].s, f[apex].s, f[b].s);
            if smp.closed {
                if apex < entry {
                    apex += smp.length;
                }
                if exit < apex {
                    exit += smp.length;
                }
            }
            let tightest = kappa[(0..len)
                .map(|i| (a + i) % n)
                .max_by(|&x, &y| kappa[x].abs().total_cmp(&kappa[y].abs()))?];
            (angle >= LEAST_ANGLE).then(|| Corner {
                number: 0,
                dir: if tightest > 0.0 {
                    Side::Left
                } else {
                    Side::Right
                },
                entry,
                apex,
                exit,
                radius: 1.0 / tightest.abs(),
                angle,
            })
        })
        .collect();
    let from_start = |c: &Corner| {
        if smp.closed {
            (c.entry - start).rem_euclid(smp.length)
        } else {
            c.entry
        }
    };
    corners.sort_by(|a, b| from_start(a).total_cmp(&from_start(b)));
    for (i, c) in corners.iter_mut().enumerate() {
        c.number = i + 1;
    }
    corners
}

/// Which kerbs a corner has.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Kerbs {
    /// On the outside where cars turn in.
    pub entry: bool,
    /// On the inside round the apex.
    pub apex: bool,
    /// On the outside where cars run out.
    pub exit: bool,
    pub width: f64,
    pub profile: Profile,
}

impl Default for Kerbs {
    fn default() -> Self {
        Self {
            entry: true,
            apex: true,
            exit: true,
            width: 1.5,
            profile: Profile::Crown(0.03),
        }
    }
}

/// Which kerb of a corner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kerb {
    Entry,
    Apex,
    Exit,
}

impl Kerb {
    pub const ALL: [Kerb; 3] = [Kerb::Entry, Kerb::Apex, Kerb::Exit];

    pub fn label(self) -> &'static str {
        match self {
            Kerb::Entry => "entry",
            Kerb::Apex => "apex",
            Kerb::Exit => "exit",
        }
    }

    /// The name of this kerb's strip for corner `number`.
    pub fn strip_name(self, number: usize) -> String {
        format!("T{number} {}", self.label())
    }

    /// Which side it is on, and where it runs (m along the road) round a corner.
    pub fn place(self, c: &Corner) -> (Side, f64, f64) {
        let before = c.apex - c.entry;
        let after = c.exit - c.apex;
        match self {
            Kerb::Entry => (c.outside(), c.entry - 25.0, c.entry + 0.4 * before),
            Kerb::Apex => (
                c.inside(),
                c.apex - 0.5 * before - 5.0,
                c.apex + 0.5 * after + 5.0,
            ),
            Kerb::Exit => (c.outside(), c.apex + 0.5 * after, c.exit + 30.0),
        }
    }
}

/// The kerb material and surface the project has, or its first.
fn named(names: &[&str], wanted: &str) -> String {
    names
        .iter()
        .find(|n| **n == wanted)
        .or(names.first())
        .map_or(wanted.to_string(), |n| n.to_string())
}

/// The operations that give corner `c` of road `road` the kerbs `kerbs` asks for (and
/// take away those it does not), each a strip of its own against the road's edge.
pub fn kerb_ops(
    project: &Project,
    road: &str,
    smp: &Sampled,
    c: &Corner,
    kerbs: &Kerbs,
) -> Vec<Op> {
    let Some(r) = project.road(road) else {
        return vec![];
    };
    let surfaces: Vec<&str> = project.surfaces.iter().map(|s| s.name.as_str()).collect();
    let materials: Vec<&str> = project.materials.iter().map(|m| m.name.as_str()).collect();
    let u = |s: f64| {
        if smp.closed {
            smp.u_at(s.rem_euclid(smp.length))
        } else {
            smp.u_at(s.clamp(0.0, smp.length))
        }
    };
    let mut ops = Vec::new();
    for kerb in Kerb::ALL {
        let want = match kerb {
            Kerb::Entry => kerbs.entry,
            Kerb::Apex => kerbs.apex,
            Kerb::Exit => kerbs.exit,
        };
        let name = kerb.strip_name(c.number);
        let (side, from, to) = kerb.place(c);
        let has = r.strips(side).iter().any(|s| s.name == name);
        // A kerb that moved sides (the corner changed direction) goes from the other.
        let other = match side {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        };
        if r.strips(other).iter().any(|s| s.name == name) {
            ops.push(Op::RemoveStrip {
                road: road.to_string(),
                side: other,
                name: name.clone(),
            });
        }
        if want {
            ops.push(Op::PutStrip {
                road: road.to_string(),
                side,
                strip: Strip {
                    name,
                    width: kerbs.width,
                    surface: named(&surfaces, "kerb"),
                    material: named(&materials, "kerb"),
                    profile: kerbs.profile,
                    ranges: vec![Range {
                        from: u(from),
                        to: u(to),
                    }],
                    fade: 2.0,
                },
                at: Some(0),
            });
        } else if has {
            ops.push(Op::RemoveStrip {
                road: road.to_string(),
                side,
                name,
            });
        }
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::apply_all;

    #[test]
    fn finds_and_numbers_the_oval_s_corners_and_kerbs_them() {
        let mut p = Project::new("t");
        let smp = Sampled::new(&p.roads[0], 2.0);
        let start = smp.s_at(p.markers.start);
        let corners = find(&smp, start);
        // A rounded rectangle driven anticlockwise: two ends, turning left.
        assert_eq!(corners.len(), 2, "{corners:?}");
        assert!(corners.iter().all(|c| c.dir == Side::Left));
        assert_eq!(corners[0].number, 1);
        assert!(corners[0].entry > start, "numbered from the start line");
        for c in &corners {
            assert!((c.angle - std::f64::consts::PI).abs() < 0.5, "{c:?}");
            assert!(c.radius > 30.0 && c.radius < 200.0, "{c:?}");
        }
        let ops = kerb_ops(&p, "circuit", &smp, &corners[0], &Kerbs::default());
        apply_all(&mut p, &ops).unwrap();
        let r = &p.roads[0];
        assert!(r.left.iter().any(|s| s.name == "T1 apex"), "inside is left");
        assert!(r.right.iter().any(|s| s.name == "T1 entry"));
        // A corner across the start: its places still run in order.
        let mut q = Project::new("t");
        q.roads[0].nodes.rotate_left(3);
        let smp = Sampled::new(&q.roads[0], 2.0);
        let corners = find(&smp, 0.0);
        assert_eq!(corners.len(), 2, "{corners:?}");
        for c in &corners {
            assert!(c.entry <= c.apex && c.apex <= c.exit, "{c:?}");
            assert!(c.length(smp.length) < 0.5 * smp.length, "{c:?}");
        }
        // Taking one away removes its strip.
        let fewer = Kerbs {
            entry: false,
            ..Kerbs::default()
        };
        let ops = kerb_ops(&p, "circuit", &smp, &corners[0], &fewer);
        apply_all(&mut p, &ops).unwrap();
        assert!(!p.roads[0].right.iter().any(|s| s.name == "T1 entry"));
        assert!(p.roads[0].right.iter().any(|s| s.name == "T1 exit"));
    }
}
