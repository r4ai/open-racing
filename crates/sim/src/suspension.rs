//! Suspension kinematics from the linkage's hardpoints.
//!
//! Each axle's linkage (double wishbones, a MacPherson strut, five links or a trailing
//! arm) guides the upright, the wheel carrier, with one degree of freedom left: its
//! travel. Steered axles have a second input, the rack. The linkage is solved ahead of
//! time over the travel and the rack's range into a table of the upright's pose and of
//! how it moves: the wheel centre's path and the upright's rotation per metre of travel
//! and of rack. The car reads the table each step.
//!
//! How the upright moves decides how forces reach the body. A force on the tyre does
//! work along the travel in proportion to how far its point of action moves with it, so
//! a braking force on a contact patch that moves forward as the wheel rises pulls the
//! wheel down (anti-dive), a cornering force on a patch that moves outward pushes it
//! down and the body up (the roll centre's jacking), and the rack feels the tyre's
//! forces through the steering axis the linkage makes (trail, scrub, the lift of the
//! kingpin inclination). The springs, dampers and bars work through their actuation,
//! directly or through a push- or pullrod and a rocker, whose motion ratio follows the
//! travel.
//!
//! Hardpoints are given for the left wheel relative to its wheel centre at static ride
//! height, in body axes (x forward, y left, z up), in m; the right wheel mirrors them.

use glam::{DQuat, DVec3, DVec4};
use serde::{Deserialize, Serialize};

use crate::params::ParamsError;

/// A point relative to the wheel centre at static ride height, or a direction: forward,
/// left, up, m.
pub type Point = [f64; 3];

/// An A-arm: two pivots on the body and a ball joint on the upright.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Wishbone {
    pub front: Point,
    pub rear: Point,
    pub outer: Point,
}

/// A rod between a ball joint on the body (or the steering rack) and one on the upright.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Link {
    pub inner: Point,
    pub outer: Point,
}

/// How the upright is guided.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Linkage {
    /// Upper and lower A-arms and a tie rod (the toe link on an unsteered axle).
    DoubleWishbone {
        upper: Wishbone,
        lower: Wishbone,
        tie_rod: Link,
    },
    /// A lower A-arm, and a strut fixed to the upright that slides through its top mount
    /// on the body; the steering axis runs from the top mount to the lower ball joint.
    MacPherson {
        lower: Wishbone,
        /// Top mount on the body.
        strut_top: Point,
        /// A point on the strut's axis on the upright.
        strut_bottom: Point,
        tie_rod: Link,
    },
    /// Four links and a tie rod (or toe link), each with its own ball joints.
    MultiLink { links: [Link; 4], tie_rod: Link },
    /// The upright is fixed to an arm that pivots on the body about the axis through
    /// two points: a trailing arm when the axis runs across the car, a semi-trailing arm
    /// when it is swept. A twist beam behaves close to one. It cannot steer.
    TrailingArm { pivots: [Point; 2] },
}

/// The part of the linkage an actuating rod or coil-over is attached to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mount {
    #[default]
    Upright,
    LowerArm,
    UpperArm,
}

/// How the wheel works the springs, dampers, anti-roll bar and heave spring.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Actuation {
    /// They act at the wheel along its travel: their rates are wheel rates.
    #[default]
    Wheel,
    /// A coil-over from the body to a point on the linkage: rates are per metre of its
    /// own compression.
    Direct {
        chassis: Point,
        outer: Point,
        #[serde(default)]
        on: Mount,
    },
    /// A pushrod (from low on the linkage up to the rocker) or a pullrod (from high on
    /// the linkage down to it) turns a rocker on the body, which compresses a coil-over
    /// from the rocker to the body. Rates are per metre of the coil-over's compression.
    Rocker {
        /// The rod's lower (push) or upper (pull) end, on the linkage.
        rod: Point,
        #[serde(default)]
        on: Mount,
        /// The rocker's pivot and axis.
        pivot: Point,
        axis: Point,
        /// Where the rod and the coil-over meet the rocker.
        rod_end: Point,
        spring_end: Point,
        /// The coil-over's end on the body.
        chassis: Point,
    },
}

/// Setup of the static wheel alignment: the hub turned on the upright.
#[derive(Clone, Copy, Debug)]
pub struct Alignment {
    /// Camber, rad, negative = top leaning inwards.
    pub camber: f64,
    /// Toe, rad, positive = toe-in.
    pub toe: f64,
}

/// One node of the kinematics table, for the left wheel.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pose {
    /// Wheel centre relative to its static position, m.
    pub centre: DVec3,
    /// The upright's rotation from its static attitude.
    pub rotation: DQuat,
    /// The wheel's spin axis, pointing left (for both wheels), with the alignment.
    pub axis: DVec3,
    /// Wheel centre's velocity and the upright's angular velocity per m/s of travel
    /// (bump positive).
    pub centre_travel: DVec3,
    pub spin_travel: DVec3,
    /// The same per m/s of the rack moving left.
    pub centre_rack: DVec3,
    pub spin_rack: DVec3,
    /// Compression of the actuation from static (the coil-over's, or the wheel's
    /// travel), m, and its rate per m of travel (the motion ratio) and of rack.
    pub actuation: f64,
    pub motion_ratio: f64,
    pub actuation_rack: f64,
    /// Rocker angle from static, rad (0 without a rocker).
    pub rocker: f64,
}

impl Pose {
    fn lerp(&self, other: &Self, t: f64) -> Self {
        let l = |a: DVec3, b: DVec3| a + (b - a) * t;
        let s = |a: f64, b: f64| a + (b - a) * t;
        Self {
            centre: l(self.centre, other.centre),
            rotation: DQuat::from_vec4(DVec4::from(self.rotation).lerp(other.rotation.into(), t))
                .normalize(),
            axis: l(self.axis, other.axis).normalize(),
            centre_travel: l(self.centre_travel, other.centre_travel),
            spin_travel: l(self.spin_travel, other.spin_travel),
            centre_rack: l(self.centre_rack, other.centre_rack),
            spin_rack: l(self.spin_rack, other.spin_rack),
            actuation: s(self.actuation, other.actuation),
            motion_ratio: s(self.motion_ratio, other.motion_ratio),
            actuation_rack: s(self.actuation_rack, other.actuation_rack),
            rocker: s(self.rocker, other.rocker),
        }
    }

    /// The right wheel's pose, mirrored from the left wheel's.
    pub fn mirrored(&self) -> Self {
        let m = |v: DVec3| DVec3::new(v.x, -v.y, v.z);
        // Axial vectors (rotations) mirror the other way.
        let a = |v: DVec3| DVec3::new(-v.x, v.y, -v.z);
        let q = self.rotation;
        Self {
            centre: m(self.centre),
            rotation: DQuat::from_xyzw(-q.x, q.y, -q.z, q.w),
            // The mirrored axis points right; turned round, left.
            axis: -m(self.axis),
            centre_travel: m(self.centre_travel),
            spin_travel: a(self.spin_travel),
            // The right wheel at rack r is the mirror of the left at −r.
            centre_rack: -m(self.centre_rack),
            spin_rack: -a(self.spin_rack),
            actuation_rack: -self.actuation_rack,
            ..*self
        }
    }

    /// Road wheel angle in the body, rad, positive to the left.
    pub fn steer(&self) -> f64 {
        let forward = self.axis.cross(DVec3::Z);
        forward.y.atan2(forward.x)
    }

    /// Camber of a wheel on side `side` (+1 left, −1 right) against the body, rad,
    /// negative = top leaning inwards.
    pub fn camber(&self, side: f64) -> f64 {
        -side * self.axis.z.clamp(-1.0, 1.0).asin()
    }
}

/// Kinematic figures of an axle at static ride height, for setup and checks.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Summary {
    /// Camber change per m of bump, rad/m (negative: more negative camber in bump).
    pub camber_gain: f64,
    /// Toe-in change per m of bump, rad/m.
    pub bump_steer: f64,
    /// Height of the roll centre above the ground, m.
    pub roll_centre: f64,
    /// Forward motion per m of bump of the contact patch and of the wheel centre: the
    /// side-view angles that give anti-dive, anti-lift and anti-squat.
    pub patch_pitch: f64,
    pub centre_pitch: f64,
    /// Motion ratio of the actuation: its compression per m of bump.
    pub motion_ratio: f64,
    /// The steering axis (steered axles): caster and kingpin inclination, rad;
    /// mechanical trail and scrub radius at the ground, m.
    pub caster: f64,
    pub kingpin_inclination: f64,
    pub trail: f64,
    pub scrub_radius: f64,
}

/// A linkage solved over its travel and rack range.
#[derive(Clone, Debug)]
pub struct Kinematics {
    pub linkage: Linkage,
    pub actuation: Actuation,
    /// Travel range, m (droop negative), and the table's spacing.
    pub travel: (f64, f64),
    travel_step: f64,
    travel_nodes: usize,
    /// Rack travel, m, per rad of steering wheel angle (0 on an unsteered axle).
    pub rack_gain: f64,
    rack_range: f64,
    rack_nodes: usize,
    /// `travel_nodes × rack_nodes`, the rack the slower index.
    table: Vec<Pose>,
    pub summary: Summary,
}

/// Newton iterations to converge a pose.
const MAX_ITERATIONS: usize = 60;
/// Residual at which a pose counts as converged, m.
const TOLERANCE: f64 = 1e-11;
/// Step of the finite differences taken for the pose's rates, m.
const DIFF_STEP: f64 = 1e-5;
/// Spacing of the table along the travel, m.
const TRAVEL_STEP: f64 = 0.004;
/// Rack nodes on each side of the centre.
const RACK_HALF_NODES: usize = 12;
/// Rack range beyond full lock, so the table covers it.
const RACK_MARGIN: f64 = 1.05;

fn v(p: &Point) -> DVec3 {
    DVec3::from_array(*p)
}

/// Unit vectors square to `d` and to each other.
fn perpendiculars(d: DVec3) -> (DVec3, DVec3) {
    let a = d.any_orthonormal_vector();
    (a, d.cross(a))
}

/// Rotation of `p` about the axis through `origin` along unit `k` by `angle`.
fn rotate_about(p: DVec3, origin: DVec3, k: DVec3, angle: f64) -> DVec3 {
    origin + DQuat::from_axis_angle(k, angle) * (p - origin)
}

/// Angle, about the unit axis `k` through `origin`, that turns `from` onto `to`.
fn angle_about(from: DVec3, to: DVec3, origin: DVec3, k: DVec3) -> f64 {
    let flat = |p: DVec3| {
        let r = p - origin;
        r - k * r.dot(k)
    };
    let (a, b) = (flat(from), flat(to));
    k.dot(a.cross(b)).atan2(a.dot(b))
}

/// The upright's pose: wheel centre offset and rotation vector.
#[derive(Clone, Copy, Debug, Default)]
struct State([f64; 6]);

impl State {
    fn centre(&self) -> DVec3 {
        DVec3::new(self.0[0], self.0[1], self.0[2])
    }

    fn rotation(&self) -> DQuat {
        DQuat::from_scaled_axis(DVec3::new(self.0[3], self.0[4], self.0[5]))
    }

    /// Where the upright's point `p` (static position) is.
    fn point(&self, p: DVec3) -> DVec3 {
        self.centre() + self.rotation() * p
    }
}

/// Residual of a rod of its static length between `inner` and the upright's `outer`,
/// in m.
fn rod(s: &State, inner: DVec3, outer: DVec3, length: f64) -> f64 {
    (s.point(outer) - inner).length() - length
}

impl Linkage {
    /// The six equations the upright's pose meets at travel `q` and rack `r`: the
    /// linkage's five, and the wheel centre's height.
    fn residuals(&self, s: &State, q: f64, r: f64) -> [f64; 6] {
        let rack = DVec3::Y * r;
        let link = |l: &Link, moves: bool| {
            let inner = v(&l.inner) + if moves { rack } else { DVec3::ZERO };
            rod(s, inner, v(&l.outer), (v(&l.outer) - v(&l.inner)).length())
        };
        let arm = |w: &Wishbone| {
            let o = v(&w.outer);
            [
                rod(s, v(&w.front), o, (o - v(&w.front)).length()),
                rod(s, v(&w.rear), o, (o - v(&w.rear)).length()),
            ]
        };
        let height = s.0[2] - q;
        match self {
            Self::DoubleWishbone {
                upper,
                lower,
                tie_rod,
            } => {
                let [a, b] = arm(upper);
                let [c, d] = arm(lower);
                [a, b, c, d, link(tie_rod, true), height]
            }
            Self::MacPherson {
                lower,
                strut_top,
                strut_bottom,
                tie_rod,
            } => {
                let [c, d] = arm(lower);
                let (top, bottom) = (v(strut_top), v(strut_bottom));
                let (e1, e2) = perpendiculars((top - bottom).normalize());
                let rot = s.rotation();
                let off = top - s.point(bottom);
                [
                    c,
                    d,
                    off.dot(rot * e1),
                    off.dot(rot * e2),
                    link(tie_rod, true),
                    height,
                ]
            }
            Self::MultiLink { links, tie_rod } => [
                link(&links[0], false),
                link(&links[1], false),
                link(&links[2], false),
                link(&links[3], false),
                link(tie_rod, true),
                height,
            ],
            Self::TrailingArm { pivots } => {
                let (a, b) = (v(&pivots[0]), v(&pivots[1]));
                let (e1, e2) = perpendiculars((b - a).normalize());
                let pa = s.point(a) - a;
                let pb = s.point(b) - b;
                [pa.x, pa.y, pa.z, pb.dot(e1), pb.dot(e2), height]
            }
        }
    }

    fn steerable(&self) -> bool {
        !matches!(self, Self::TrailingArm { .. })
    }

    /// The arm `mount` names, if the linkage has it.
    fn arm(&self, mount: Mount) -> Option<&Wishbone> {
        match (self, mount) {
            (Self::DoubleWishbone { lower, .. }, Mount::LowerArm)
            | (Self::MacPherson { lower, .. }, Mount::LowerArm) => Some(lower),
            (Self::DoubleWishbone { upper, .. }, Mount::UpperArm) => Some(upper),
            _ => None,
        }
    }

    /// Where the point `p` (static position) on `mount` is with the upright at `s`.
    fn mounted(&self, mount: Mount, s: &State, p: DVec3) -> DVec3 {
        match self.arm(mount) {
            None => s.point(p),
            Some(w) => {
                let (front, outer) = (v(&w.front), v(&w.outer));
                let k = (v(&w.rear) - front).normalize();
                let angle = angle_about(outer, s.point(outer), front, k);
                rotate_about(p, front, k, angle)
            }
        }
    }

    /// Segments between the body's and the upright's joints with the upright at `s`,
    /// for drawing.
    fn segments(&self, s: &State, r: f64, out: &mut Vec<(DVec3, DVec3)>) {
        let rack = DVec3::Y * r;
        let link = |l: &Link, moves: bool| {
            (
                v(&l.inner) + if moves { rack } else { DVec3::ZERO },
                s.point(v(&l.outer)),
            )
        };
        match self {
            Self::DoubleWishbone {
                upper,
                lower,
                tie_rod,
            } => {
                for w in [upper, lower] {
                    let o = s.point(v(&w.outer));
                    out.push((v(&w.front), o));
                    out.push((v(&w.rear), o));
                }
                out.push(link(tie_rod, true));
            }
            Self::MacPherson {
                lower,
                strut_top,
                strut_bottom,
                tie_rod,
            } => {
                let o = s.point(v(&lower.outer));
                out.push((v(&lower.front), o));
                out.push((v(&lower.rear), o));
                out.push((v(strut_top), s.point(v(strut_bottom))));
                out.push(link(tie_rod, true));
            }
            Self::MultiLink { links, tie_rod } => {
                for l in links {
                    out.push(link(l, false));
                }
                out.push(link(tie_rod, true));
            }
            Self::TrailingArm { pivots } => {
                for p in pivots {
                    out.push((v(p), s.point(DVec3::ZERO)));
                }
            }
        }
    }
}

/// Solves the 6×6 system `a x = b` by Gaussian elimination with partial pivoting.
fn solve6(mut a: [[f64; 6]; 6], mut b: [f64; 6]) -> Option<[f64; 6]> {
    for col in 0..6 {
        let pivot = (col..6).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col].abs() < 1e-14 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        let (top, rest) = a.split_at_mut(col + 1);
        let pivot_row = &top[col];
        for (k, row) in rest.iter_mut().enumerate() {
            let f = row[col] / pivot_row[col];
            for (x, p) in row[col..].iter_mut().zip(&pivot_row[col..]) {
                *x -= f * p;
            }
            b[col + 1 + k] -= f * b[col];
        }
    }
    let mut x = [0.0; 6];
    for row in (0..6).rev() {
        let sum: f64 = (row + 1..6).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - sum) / a[row][row];
    }
    Some(x)
}

impl Actuation {
    fn check(&self, linkage: &Linkage) -> Result<(), ParamsError> {
        let mount = match self {
            Self::Wheel => return Ok(()),
            Self::Direct { on, .. } | Self::Rocker { on, .. } => *on,
        };
        if mount != Mount::Upright && linkage.arm(mount).is_none() {
            return Err(ParamsError::Invalid(
                "actuation: mounted on an arm the linkage does not have",
            ));
        }
        if let Self::Rocker { axis, .. } = self
            && v(axis).length() < 1e-9
        {
            return Err(ParamsError::Invalid("actuation: rocker axis is zero"));
        }
        Ok(())
    }

    /// Compression from static and rocker angle with the upright at `s` and travel `q`;
    /// `rocker` is where the rocker's angle search starts.
    fn compression(&self, linkage: &Linkage, s: &State, q: f64, rocker: f64) -> (f64, f64) {
        match self {
            Self::Wheel => (q, 0.0),
            Self::Direct { chassis, outer, on } => {
                let (c, o) = (v(chassis), v(outer));
                let now = linkage.mounted(*on, s, o);
                ((o - c).length() - (now - c).length(), 0.0)
            }
            Self::Rocker {
                rod,
                on,
                pivot,
                axis,
                rod_end,
                spring_end,
                chassis,
            } => {
                let (pivot, k) = (v(pivot), v(axis).normalize());
                let (e0, s0, c) = (v(rod_end), v(spring_end), v(chassis));
                let rod0 = v(rod);
                let length = (e0 - rod0).length();
                let end = linkage.mounted(*on, s, rod0);
                // The rocker turns until the rod is its length again.
                let g = |a: f64| (rotate_about(e0, pivot, k, a) - end).length() - length;
                let mut a = rocker;
                for _ in 0..MAX_ITERATIONS {
                    let h = 1e-7;
                    let slope = (g(a + h) - g(a - h)) / (2.0 * h);
                    if slope.abs() < 1e-12 {
                        break;
                    }
                    let step = g(a) / slope;
                    a -= step;
                    if step.abs() < 1e-13 {
                        break;
                    }
                }
                let spring = rotate_about(s0, pivot, k, a);
                ((s0 - c).length() - (spring - c).length(), a)
            }
        }
    }

    /// Segments of the actuation for drawing: the rod and the rocker's arms, and the
    /// coil-over.
    fn segments(&self, linkage: &Linkage, s: &State, rocker: f64, out: &mut Vec<(DVec3, DVec3)>) {
        match self {
            Self::Wheel => {}
            Self::Direct { chassis, outer, on } => {
                out.push((v(chassis), linkage.mounted(*on, s, v(outer))));
            }
            Self::Rocker {
                rod,
                on,
                pivot,
                axis,
                rod_end,
                spring_end,
                chassis,
            } => {
                let (p, k) = (v(pivot), v(axis).normalize());
                let e = rotate_about(v(rod_end), p, k, rocker);
                let sp = rotate_about(v(spring_end), p, k, rocker);
                out.push((linkage.mounted(*on, s, v(rod)), e));
                out.push((p, e));
                out.push((p, sp));
                out.push((sp, v(chassis)));
            }
        }
    }
}

impl Kinematics {
    /// Solves `linkage` over the travel from `droop` (negative) to `bump`, m. A steered
    /// axle's rack is sized so that `steering_ratio` of steering wheel angle turns the
    /// wheels by one near the centre, up to `lock` (rad of steering wheel). `radius` is
    /// the loaded tyre radius and `half_track` the wheel centre's distance from the
    /// car's centreline, m.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        linkage: Linkage,
        actuation: Actuation,
        alignment: Alignment,
        droop: f64,
        bump: f64,
        steering: Option<(f64, f64)>,
        radius: f64,
        half_track: f64,
    ) -> Result<Self, ParamsError> {
        actuation.check(&linkage)?;
        if steering.is_some() && !linkage.steerable() {
            return Err(ParamsError::Invalid(
                "a trailing arm cannot carry a steered wheel",
            ));
        }
        let setup =
            DQuat::from_rotation_z(-alignment.toe) * DQuat::from_rotation_x(-alignment.camber);
        let hub_axis = setup * DVec3::Y;

        let solver = Solver {
            linkage: &linkage,
            actuation: &actuation,
            hub_axis,
        };
        let design = solver
            .pose(State::default(), 0.0, 0.0, 0.0)
            .ok_or(ParamsError::Invalid(
                "linkage: cannot be solved at static ride height",
            ))?;

        // The rack's gain: the steering ratio near the centre.
        let (rack_gain, rack_range) = match steering {
            Some((ratio, lock)) => {
                let steer_per_rack = design.0.spin_rack.dot(DVec3::Z);
                if steer_per_rack.abs() < 1e-6 {
                    return Err(ParamsError::Invalid(
                        "linkage: the tie rod does not steer the wheel",
                    ));
                }
                let gain = 1.0 / (ratio * steer_per_rack);
                (gain, (gain * lock).abs() * RACK_MARGIN)
            }
            None => (0.0, 0.0),
        };

        let (lo, hi) = (
            (droop / TRAVEL_STEP).floor() as i64,
            (bump / TRAVEL_STEP).ceil() as i64,
        );
        let travel_nodes = (hi - lo + 1) as usize;
        let rack_nodes = if steering.is_some() {
            2 * RACK_HALF_NODES + 1
        } else {
            1
        };
        let rack_step = if rack_nodes > 1 {
            rack_range / RACK_HALF_NODES as f64
        } else {
            0.0
        };
        let rack_at = |j: usize| (j as f64 - (rack_nodes / 2) as f64) * rack_step;
        let travel_at = |k: usize| (lo + k as i64) as f64 * TRAVEL_STEP;
        let k0 = (-lo) as usize;
        let j0 = rack_nodes / 2;

        let mut table = vec![Pose::default(); travel_nodes * rack_nodes];
        let mut states = vec![None; travel_nodes * rack_nodes];
        // From the centre outwards, each node starting from its solved neighbour.
        let mut order_j: Vec<usize> = (0..rack_nodes).collect();
        order_j.sort_by_key(|&j| j.abs_diff(j0));
        let mut order_k: Vec<usize> = (0..travel_nodes).collect();
        order_k.sort_by_key(|&k| k.abs_diff(k0));
        for &j in &order_j {
            for &k in &order_k {
                let seed = if j == j0 {
                    if k == k0 {
                        Some((State::default(), 0.0))
                    } else {
                        let kk = if k > k0 { k - 1 } else { k + 1 };
                        states[j * travel_nodes + kk]
                    }
                } else {
                    let jj = if j > j0 { j - 1 } else { j + 1 };
                    states[jj * travel_nodes + k]
                };
                let (start, rocker) = seed.unwrap_or_default();
                let (pose, state) = solver
                    .pose(start, travel_at(k), rack_at(j), rocker)
                    .map(|(p, s)| (p, (s, p.rocker)))
                    .ok_or(ParamsError::Invalid(
                        "linkage: cannot reach its full travel or steering lock",
                    ))?;
                table[j * travel_nodes + k] = pose;
                states[j * travel_nodes + k] = Some(state);
            }
        }

        let summary = summarise(&design.0, radius, half_track);
        Ok(Self {
            linkage,
            actuation,
            travel: (travel_at(0), travel_at(travel_nodes - 1)),
            travel_step: TRAVEL_STEP,
            travel_nodes,
            rack_gain,
            rack_range,
            rack_nodes,
            table,
            summary,
        })
    }

    /// The left wheel's pose at travel `q` (bump positive) and rack `r` (left), m,
    /// interpolated in the table and clamped to its range.
    pub fn pose(&self, q: f64, r: f64) -> Pose {
        let (tq, kq) = cell(q - self.travel.0, self.travel_step, self.travel_nodes);
        let row = |j: usize| {
            let b = j * self.travel_nodes + kq;
            self.table[b].lerp(&self.table[b + 1], tq)
        };
        if self.rack_nodes == 1 {
            return row(0);
        }
        let step = self.rack_range / RACK_HALF_NODES as f64;
        let (tr, jr) = cell(r + self.rack_range, step, self.rack_nodes);
        row(jr).lerp(&row(jr + 1), tr)
    }

    /// Lines between the joints of the left wheel's linkage and actuation at travel `q`
    /// and rack `r`, relative to its static wheel centre, appended to `out`.
    pub fn segments(&self, q: f64, r: f64, out: &mut Vec<(DVec3, DVec3)>) {
        let p = self.pose(q, r);
        let mut s = State::default();
        s.0[..3].copy_from_slice(&p.centre.to_array());
        s.0[3..].copy_from_slice(&p.rotation.to_scaled_axis().to_array());
        self.linkage.segments(&s, r, out);
        self.actuation.segments(&self.linkage, &s, p.rocker, out);
    }
}

/// Interpolation weight and lower node of `x` on a grid of `n` nodes `step` apart from 0.
fn cell(x: f64, step: f64, n: usize) -> (f64, usize) {
    let f = (x / step).clamp(0.0, (n - 1) as f64);
    let k = (f.floor() as usize).min(n - 2);
    (f - k as f64, k)
}

struct Solver<'a> {
    linkage: &'a Linkage,
    actuation: &'a Actuation,
    hub_axis: DVec3,
}

impl Solver<'_> {
    /// The upright's state at travel `q` and rack `r`, by Newton's method from `start`.
    fn state(&self, start: State, q: f64, r: f64) -> Option<State> {
        let mut s = start;
        for _ in 0..MAX_ITERATIONS {
            let f = self.linkage.residuals(&s, q, r);
            if f.iter().all(|x| x.abs() < TOLERANCE) {
                return Some(s);
            }
            let mut jac = [[0.0; 6]; 6];
            let h = 1e-7;
            for c in 0..6 {
                let mut sp = s;
                sp.0[c] += h;
                let mut sm = s;
                sm.0[c] -= h;
                let (fp, fm) = (
                    self.linkage.residuals(&sp, q, r),
                    self.linkage.residuals(&sm, q, r),
                );
                for (row, (p, m)) in jac.iter_mut().zip(fp.iter().zip(&fm)) {
                    row[c] = (p - m) / (2.0 * h);
                }
            }
            let dx = solve6(jac, f.map(|x| -x))?;
            for (x, d) in s.0.iter_mut().zip(dx) {
                *x += d;
            }
        }
        let f = self.linkage.residuals(&s, q, r);
        f.iter().all(|x| x.abs() < 1e-8).then_some(s)
    }

    /// The pose at travel `q` and rack `r` with its rates, and the state it solved to.
    fn pose(&self, start: State, q: f64, r: f64, rocker: f64) -> Option<(Pose, State)> {
        let s = self.state(start, q, r)?;
        let h = DIFF_STEP;
        let (qp, qm) = (self.state(s, q + h, r)?, self.state(s, q - h, r)?);
        let (rp, rm) = (self.state(s, q, r + h)?, self.state(s, q, r - h)?);
        let rate = |a: &State, b: &State| {
            (
                (a.centre() - b.centre()) / (2.0 * h),
                (a.rotation() * b.rotation().inverse()).to_scaled_axis() / (2.0 * h),
            )
        };
        let (centre_travel, spin_travel) = rate(&qp, &qm);
        let (centre_rack, spin_rack) = rate(&rp, &rm);
        let (u, angle) = self.actuation.compression(self.linkage, &s, q, rocker);
        let act = |st: &State, q: f64| self.actuation.compression(self.linkage, st, q, angle);
        let pose = Pose {
            centre: s.centre(),
            rotation: s.rotation(),
            axis: s.rotation() * self.hub_axis,
            centre_travel,
            spin_travel,
            centre_rack,
            spin_rack,
            actuation: u,
            motion_ratio: (act(&qp, q + h).0 - act(&qm, q - h).0) / (2.0 * h),
            actuation_rack: (act(&rp, q).0 - act(&rm, q).0) / (2.0 * h),
            rocker: angle,
        };
        Some((pose, s))
    }
}

/// The figures of the static pose `p` of a wheel `radius` below its centre and
/// `half_track` from the centreline.
fn summarise(p: &Pose, radius: f64, half_track: f64) -> Summary {
    // The contact patch under the wheel centre, and how it moves with the travel.
    let patch = -DVec3::Z * radius;
    let patch_travel = p.centre_travel + p.spin_travel.cross(patch);
    // Front view: the patch moves square to the line to the roll centre.
    let roll_centre = half_track * patch_travel.y / patch_travel.z;
    let mut s = Summary {
        // Camber is −asin(axis.z) on the left; the axis turns with the upright.
        camber_gain: -p.spin_travel.cross(p.axis).z / p.axis.z.asin().cos(),
        // The left wheel toes in turning right.
        bump_steer: -p.spin_travel.z,
        roll_centre,
        patch_pitch: patch_travel.x / patch_travel.z,
        centre_pitch: p.centre_travel.x / p.centre_travel.z,
        motion_ratio: p.motion_ratio,
        ..Default::default()
    };
    // The steering axis: the upright's screw axis as the rack moves.
    let w = p.spin_rack;
    if w.length() > 1e-9 {
        let k = if w.z < 0.0 { -w } else { w }.normalize();
        let point = w.cross(p.centre_rack) / w.length_squared();
        let ground = point + k * ((-radius - point.z) / k.z);
        s.caster = (-k.x).atan2(k.z);
        s.kingpin_inclination = (-k.y).atan2(k.z);
        s.trail = ground.x - patch.x;
        s.scrub_radius = patch.y - ground.y;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const RADIUS: f64 = 0.33;

    fn wishbones(upper_inner_y: f64) -> Linkage {
        Linkage::DoubleWishbone {
            upper: Wishbone {
                front: [0.15, upper_inner_y, 0.15],
                rear: [-0.15, upper_inner_y, 0.15],
                outer: [-0.02, -0.10, 0.16],
            },
            lower: Wishbone {
                front: [0.2, -0.55, -0.13],
                rear: [-0.2, -0.55, -0.13],
                outer: [0.0, -0.06, -0.14],
            },
            tie_rod: Link {
                inner: [-0.13, -0.5, 0.0],
                outer: [-0.13, -0.09, 0.0],
            },
        }
    }

    fn solve(linkage: Linkage, steer: bool) -> Kinematics {
        Kinematics::new(
            linkage,
            Actuation::Wheel,
            Alignment {
                camber: -0.05,
                toe: 0.002,
            },
            -0.08,
            0.06,
            steer.then_some((12.0, 4.0)),
            RADIUS,
            0.8,
        )
        .unwrap()
    }

    #[test]
    fn the_static_pose_is_the_design_with_the_alignment() {
        let k = solve(wishbones(-0.4), true);
        let p = k.pose(0.0, 0.0);
        assert!(p.centre.length() < 1e-9, "{:?}", p.centre);
        assert!((p.camber(1.0) + 0.05).abs() < 1e-9);
        // Toe-in: the left wheel points right.
        assert!((p.steer() + 0.002).abs() < 1e-6, "{}", p.steer());
        assert!((p.actuation).abs() < 1e-12 && (p.motion_ratio - 1.0).abs() < 1e-6);
        assert!((k.pose(0.03, 0.0).centre.z - 0.03).abs() < 1e-9);
    }

    #[test]
    fn arms_converging_inboard_gain_negative_camber_and_raise_the_roll_centre() {
        // Upper arm shorter than the lower, its inner end inboard: negative camber gain.
        let short = solve(wishbones(-0.35), false).summary;
        // Parallel arms of equal length: the upright only translates.
        let parallel = solve(
            Linkage::DoubleWishbone {
                upper: Wishbone {
                    front: [0.15, -0.55, 0.15],
                    rear: [-0.15, -0.55, 0.15],
                    outer: [0.0, -0.06, 0.15],
                },
                lower: Wishbone {
                    front: [0.2, -0.55, -0.13],
                    rear: [-0.2, -0.55, -0.13],
                    outer: [0.0, -0.06, -0.13],
                },
                tie_rod: Link {
                    inner: [-0.13, -0.55, 0.0],
                    outer: [-0.13, -0.06, 0.0],
                },
            },
            false,
        )
        .summary;
        assert!(short.camber_gain < -0.2, "{short:?}");
        assert!(parallel.camber_gain.abs() < 1e-3, "{parallel:?}");
        // Parallel horizontal arms put the roll centre on the ground.
        assert!(parallel.roll_centre.abs() < 1e-3, "{parallel:?}");
        assert!(short.roll_centre > 0.0, "{short:?}");
        // The front view swing arm's length, from the camber gain, matches the arms'
        // intersection found by hand to within a few per cent.
        let upper_slope = (0.16 - 0.15) / (-0.10 + 0.35);
        let lower_slope = (-0.14 + 0.13) / (-0.06 + 0.55);
        // Intersection of z = 0.16 + u (y + 0.10) and z = −0.14 + l (y + 0.06).
        let y =
            (-0.14 - 0.16 + upper_slope * 0.10 - lower_slope * 0.06) / (upper_slope - lower_slope);
        assert!(
            ((-1.0 / short.camber_gain) - (-y)).abs() / -y < 0.05,
            "{y} {short:?}"
        );
    }

    #[test]
    fn the_rates_match_the_poses() {
        let k = solve(wishbones(-0.4), true);
        let (q, r, h) = (0.02, 0.01, 1e-4);
        let p = k.pose(q, r);
        let dq = (k.pose(q + h, r).centre - k.pose(q - h, r).centre) / (2.0 * h);
        let dr = (k.pose(q, r + h).centre - k.pose(q, r - h).centre) / (2.0 * h);
        assert!(
            (dq - p.centre_travel).length() < 1e-3,
            "{dq} {}",
            p.centre_travel
        );
        assert!(
            (dr - p.centre_rack).length() < 1e-2,
            "{dr} {}",
            p.centre_rack
        );
        // The mirrored wheel steers the same way under the same rack.
        let right = k.pose(q, -r).mirrored();
        assert!(right.steer() * p.steer() > 0.0);
        assert!(right.spin_rack.z * p.spin_rack.z > 0.0);
    }

    #[test]
    fn the_steering_axis_runs_through_the_ball_joints() {
        let k = solve(wishbones(-0.4), true);
        let s = k.summary;
        let (lo, up) = (
            DVec3::new(0.0, -0.06, -0.14),
            DVec3::new(-0.02, -0.10, 0.16),
        );
        let d = up - lo;
        assert!((s.caster - (-d.x).atan2(d.z)).abs() < 0.01, "{s:?}");
        assert!(
            (s.kingpin_inclination - (-d.y).atan2(d.z)).abs() < 0.01,
            "{s:?}"
        );
        let ground = lo + d * ((-RADIUS - lo.z) / d.z);
        assert!((s.trail - ground.x).abs() < 3e-3, "{s:?} {ground}");
        assert!((s.scrub_radius + ground.y).abs() < 3e-3, "{s:?} {ground}");
        // The steering ratio holds near the centre.
        let steer = k.pose(0.0, k.rack_gain * 0.01).steer() - k.pose(0.0, 0.0).steer();
        assert!((steer / 0.01 - 1.0 / 12.0).abs() < 1e-3, "{steer}");
    }

    #[test]
    fn a_trailing_arm_swings_the_wheel_about_its_pivots() {
        let k = solve(
            Linkage::TrailingArm {
                pivots: [[0.45, -0.6, 0.0], [0.45, -0.3, 0.0]],
            },
            false,
        );
        let p = k.pose(0.048, 0.0);
        // On a circle of 0.45 m about the pivot axis.
        let r = (p.centre - DVec3::new(0.45, p.centre.y, 0.0)).length();
        assert!((r - 0.45).abs() < 1e-6, "{r}");
        assert!((p.centre.x - (0.45 - (0.45f64.powi(2) - 0.048f64.powi(2)).sqrt())).abs() < 1e-6);
        // The toe-in tilts the axle a little out of the arc's plane.
        assert!(k.summary.camber_gain.abs() < 1e-2 && k.summary.roll_centre.abs() < 1e-9);
    }

    #[test]
    fn a_rocker_works_the_coil_over_with_its_motion_ratio() {
        // A pushrod from the lower arm's ball joint to a rocker on a fore-aft axis.
        let k = Kinematics::new(
            wishbones(-0.4),
            Actuation::Rocker {
                rod: [0.0, -0.08, -0.12],
                on: Mount::LowerArm,
                pivot: [0.0, -0.55, 0.2],
                axis: [1.0, 0.0, 0.0],
                rod_end: [0.0, -0.48, 0.25],
                spring_end: [0.0, -0.5, 0.12],
                chassis: [0.0, -0.2, 0.14],
            },
            Alignment {
                camber: 0.0,
                toe: 0.0,
            },
            -0.05,
            0.05,
            None,
            RADIUS,
            0.8,
        )
        .unwrap();
        let (a, b) = (k.pose(-0.02, 0.0), k.pose(0.02, 0.0));
        assert!(
            b.actuation > a.actuation,
            "the coil-over compresses in bump"
        );
        let mean = (b.actuation - a.actuation) / 0.04;
        assert!(
            (mean - k.pose(0.0, 0.0).motion_ratio).abs() < 0.05,
            "{mean}"
        );
        assert!(b.rocker != 0.0);
        // A direct coil-over on the upright under the wheel centre moves with it one
        // to one.
        let direct = Kinematics::new(
            wishbones(-0.4),
            Actuation::Direct {
                chassis: [0.0, 0.0, 0.5],
                outer: [0.0, 0.0, 0.0],
                on: Mount::Upright,
            },
            Alignment {
                camber: 0.0,
                toe: 0.0,
            },
            -0.05,
            0.05,
            None,
            RADIUS,
            0.8,
        )
        .unwrap();
        assert!((direct.summary.motion_ratio - 1.0).abs() < 0.01);
    }

    #[test]
    fn mounts_must_exist() {
        let err = Kinematics::new(
            Linkage::TrailingArm {
                pivots: [[0.45, -0.6, 0.0], [0.45, -0.3, 0.0]],
            },
            Actuation::Direct {
                chassis: [0.0, 0.0, 0.5],
                outer: [0.0, 0.0, 0.0],
                on: Mount::LowerArm,
            },
            Alignment {
                camber: 0.0,
                toe: 0.0,
            },
            -0.05,
            0.05,
            None,
            RADIUS,
            0.8,
        );
        assert!(err.is_err());
    }
}
