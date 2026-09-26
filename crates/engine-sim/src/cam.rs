//! Valve lift from the cam, and the effective flow area it opens.
//!
//! A head may have a second, higher lobe per valve that rocker pins lock in (Honda's VTEC,
//! Mitsubishi's MIVEC): the pins can only slide when the rockers sit on the base circle,
//! so each cylinder changes lobes when its valves are shut. A cam phaser turns the whole
//! cam against the crank, moving every event earlier (advance) or later together.
//!
//! Angles here are crank angles within the 720° cycle, with the firing TDC at 0° and the
//! gas-exchange TDC at 360°.

use std::f64::consts::PI;

use crate::spec::{Head, Profile};
use crate::table::lookup;

/// The 4-5-6-7 polynomial rising from 0 to 1 over 0..1 with zero velocity and
/// acceleration at both ends.
#[inline]
fn rise(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    let x4 = x * x * x * x;
    x4 * (35.0 - 84.0 * x + 70.0 * x * x - 20.0 * x * x * x)
}

/// A lobe's events.
#[derive(Clone, Debug)]
struct Lobe {
    open_deg: f64,
    duration_deg: f64,
    lift: f64,
    profile: Profile,
}

impl Lobe {
    fn new(cam: &crate::spec::Cam, intake: bool) -> Self {
        let peak = if intake {
            360.0 + cam.centreline_deg
        } else {
            360.0 - cam.centreline_deg
        };
        Self {
            open_deg: (peak - 0.5 * cam.duration_deg).rem_euclid(720.0),
            duration_deg: cam.duration_deg,
            lift: cam.lift,
            profile: cam.profile.clone(),
        }
    }

    fn lift_at(&self, cycle_deg: f64) -> f64 {
        let a = (cycle_deg - self.open_deg).rem_euclid(720.0);
        if a >= self.duration_deg {
            return 0.0;
        }
        let f = a / self.duration_deg;
        let s = match &self.profile {
            Profile::Polynomial => {
                if f < 0.5 {
                    rise(2.0 * f)
                } else {
                    rise(2.0 * (1.0 - f))
                }
            }
            Profile::Table(t) => lookup(t, f).clamp(0.0, 1.0),
        };
        s * self.lift
    }
}

/// A side's valves as the simulation uses them.
#[derive(Clone, Debug)]
pub struct Valvetrain {
    /// Crank angle at which the valve leaves its seat on the (low) lobe, unphased,
    /// degrees in 0..720.
    pub open_deg: f64,
    pub duration_deg: f64,
    pub lift: f64,
    low: Lobe,
    high: Option<Lobe>,
    diameter: f64,
    count: f64,
    /// Area of the throats past the stems, m².
    throat: f64,
    cd: Vec<(f64, f64)>,
}

impl Valvetrain {
    /// The intake (`intake`) or exhaust side of a head.
    pub fn new(head: &Head, intake: bool) -> Self {
        let low = Lobe::new(&head.cam, intake);
        let v = &head.valves;
        Self {
            open_deg: low.open_deg,
            duration_deg: low.duration_deg,
            lift: low.lift,
            low,
            high: head.high_cam.as_ref().map(|c| Lobe::new(c, intake)),
            diameter: v.diameter,
            count: v.count as f64,
            throat: PI * 0.25 * (v.diameter * v.diameter - v.stem * v.stem),
            cd: v.cd.clone(),
        }
    }

    /// Crank angle at which the valve closes, degrees in 0..720.
    pub fn close_deg(&self) -> f64 {
        (self.open_deg + self.duration_deg).rem_euclid(720.0)
    }

    /// Whether it has a high lobe to switch to.
    pub fn switchable(&self) -> bool {
        self.high.is_some()
    }

    /// Crank angle at which the valve closes on a lobe (`high`) with the cam advanced by
    /// `phase` crank degrees, degrees in 0..720.
    pub fn close_deg_on(&self, high: bool, phase: f64) -> f64 {
        let l = self.lobe(high);
        (l.open_deg + l.duration_deg - phase).rem_euclid(720.0)
    }

    fn lobe(&self, high: bool) -> &Lobe {
        match (&self.high, high) {
            (Some(h), true) => h,
            _ => &self.low,
        }
    }

    /// Valve lift at a crank angle in the cycle on the low lobe, unphased, m.
    pub fn lift_at(&self, cycle_deg: f64) -> f64 {
        self.low.lift_at(cycle_deg)
    }

    /// Valve lift on a lobe (`high`) with the cam advanced by `phase` crank degrees, m.
    pub fn lift_on(&self, cycle_deg: f64, high: bool, phase: f64) -> f64 {
        self.lobe(high).lift_at(cycle_deg + phase)
    }

    /// Effective flow area (discharge coefficient × geometric area) of all the side's
    /// valves at a lift, m².
    pub fn area_at_lift(&self, lift: f64) -> f64 {
        if lift <= 0.0 {
            return 0.0;
        }
        let curtain = PI * self.diameter * lift;
        let cd = lookup(&self.cd, lift / self.diameter);
        self.count * cd * curtain.min(self.throat)
    }

    /// Effective flow area at a crank angle, m².
    pub fn area_at(&self, cycle_deg: f64) -> f64 {
        self.area_at_lift(self.lift_at(cycle_deg))
    }

    /// Largest effective area, m² (at full lift, of the higher lobe).
    pub fn max_area(&self) -> f64 {
        let high = self.high.as_ref().map_or(0.0, |h| h.lift);
        self.area_at_lift(self.lift.max(high))
    }
}

/// Crank degrees in the cycle (0..720) of a crank angle `theta` (rad) for a cylinder
/// firing at `firing` (rad).
#[inline]
pub fn cycle_deg(theta: f64, firing: f64) -> f64 {
    (theta - firing).rem_euclid(4.0 * PI).to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lift_is_smooth_and_timed() {
        let e = crate::samples::i4();
        let iv = Valvetrain::new(&e.intake, true);
        let peak = 360.0 + e.intake.cam.centreline_deg;
        assert!((iv.lift_at(peak) - e.intake.cam.lift).abs() < 1e-12);
        assert_eq!(iv.lift_at(iv.open_deg - 1.0), 0.0);
        assert!(iv.lift_at(iv.open_deg + 1.0) < 1e-5);
        let ev = Valvetrain::new(&e.exhaust, false);
        // Overlap round the gas-exchange TDC.
        assert!(iv.lift_at(360.0) > 0.0 && ev.lift_at(360.0) > 0.0);
        assert_eq!(ev.lift_at(0.0), 0.0);
    }
}
