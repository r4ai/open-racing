//! Optional driver aids that act like a human driver on top of the physics.
//! They only produce `Controls`; the car model itself is never altered.

use crate::DT;
use crate::car::Car;
use crate::controls::{Controls, Shift};
use crate::drivetrain::{RPM_PER_RAD_S, ShiftPhase, blip_throttle, driveline_speed, gear_ratio};
use crate::params::GearboxKind;

/// Automatic gear selection (the driver still owns the pedals).
#[derive(Clone, Copy, Debug, Default)]
pub struct AutoShift;

impl AutoShift {
    pub fn shift(&self, car: &Car) -> Shift {
        let p = &car.model.params;
        let dt = &car.state.drivetrain;
        if dt.shifting() {
            return Shift::None;
        }
        let top = p.gearbox.ratios.len() as i32;
        if dt.gear <= 0 {
            return Shift::Up;
        }
        // The speed the gear turns the engine at, which a slipping clutch or an engine
        // revving between gears does not change.
        let out = driveline_speed(p, &car.state.wheels.map(|w| w.spin));
        let rpm = gear_ratio(p, dt.gear) * out * RPM_PER_RAD_S;
        let limiter = p.engine.limiter_rpm;
        if rpm > 0.96 * limiter && dt.gear < top {
            return Shift::Up;
        }
        if dt.gear > 1 {
            let g = &p.gearbox.ratios;
            let idx = (dt.gear - 1) as usize;
            // Locked wheels do not fool it into a gear the car's speed would over-rev.
            let driven = if p.drive.drives(false) { 2 } else { 0 };
            let rolling = car.local_velocity().x.abs() / car.model.tire(driven).p.radius;
            let rpm_after =
                rpm.max(gear_ratio(p, dt.gear) * rolling * RPM_PER_RAD_S) * g[idx - 1] / g[idx];
            if rpm_after < 0.85 * limiter && rpm < 0.55 * limiter {
                return Shift::Down;
            }
        }
        Shift::None
    }
}

/// Fastest the clutch assist lets the clutch in, engagement per second.
const CLUTCH_RELEASE_RATE: f64 = 4.0;

/// Works the clutch pedal the way a driver would, for gearboxes that have one:
///
/// - pulling away, it lets the clutch bite as the engine revs above idle and in fully as
///   the car picks up speed, so the car rests with the throttle closed and pulls away as
///   it opens;
/// - it opens the clutch before the engine stalls, whether the car has anti-stall or not;
/// - it holds the clutch down while an H-pattern lever moves and its synchroniser works,
///   and lets it back in at a driver's pace.
///
/// The driver's own pedal still wins where it opens the clutch further.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClutchAssist {
    /// Engagement asked of the clutch, 0..1.
    engagement: f64,
    /// The car is too slow for its gear to turn the engine: slipping the clutch to pull
    /// away.
    launching: bool,
}

impl ClutchAssist {
    pub fn apply(&mut self, car: &Car, c: &mut Controls) {
        let p = &car.model.params;
        let d = &car.state.drivetrain;
        if matches!(p.gearbox.kind, GearboxKind::DualClutch { .. }) {
            return;
        }
        let e = &p.engine;
        let out = driveline_speed(p, &car.state.wheels.map(|w| w.spin));
        let rpm = d.rpm();
        let gear_rpm = gear_ratio(p, d.gear) * out * RPM_PER_RAD_S;
        let bite = e.idle_rpm + 150.0;
        // Engine speed over which the clutch goes from biting to fully in.
        let span = e.limiter_rpm - 2.0 * bite;
        let lever_moving =
            matches!(p.gearbox.kind, GearboxKind::HPattern { .. }) && d.target_gear != d.gear;
        let target = if d.stalled || lever_moving {
            0.0
        } else if d.gear == 0 {
            // Neutral holds nothing; mid-shift in a dog box, keep the pedal where it is.
            if d.shifting() { self.engagement } else { 1.0 }
        } else {
            if gear_rpm < e.idle_rpm + 50.0 {
                self.launching = true;
            }
            if self.launching
                && gear_rpm > bite
                && ((rpm - gear_rpm).abs() < 100.0 || gear_rpm > bite + 0.5 * span)
            {
                self.launching = false;
            }
            if self.launching {
                // Like a centrifugal clutch: the faster the engine, the harder it bites;
                // and the faster the car, the further in it goes.
                let engine = (rpm - bite) / span;
                let car = (gear_rpm - bite) / (0.5 * span);
                engine.max(car).clamp(0.0, 1.0)
            } else {
                1.0
            }
        };
        // Let in no faster than a driver's foot; open at once.
        self.engagement = if target > self.engagement {
            (self.engagement + CLUTCH_RELEASE_RATE * DT).min(target)
        } else {
            target
        };
        c.clutch = c.clutch.max(p.clutch.pedal_for(self.engagement));
    }
}

/// How far below the incoming gear's speed the blip assist opens the throttle fully, rpm.
const BLIP_BAND_RPM: f64 = 500.0;

/// Blips the throttle on downshifts to bring the engine to the speed of the incoming gear,
/// as heel-and-toe does, on cars whose electronics do not already.
#[derive(Clone, Copy, Debug, Default)]
pub struct BlipAssist;

impl BlipAssist {
    pub fn apply(&self, car: &Car, c: &mut Controls) {
        let p = &car.model.params;
        let d = &car.state.drivetrain;
        if p.electronics.auto_blip {
            return;
        }
        let gear = match p.gearbox.kind {
            // The lever is on its way to a gear with the clutch open, or the gear is in and
            // the clutch still slipping as it comes back.
            GearboxKind::HPattern { .. } => {
                if d.target_gear != d.gear {
                    d.target_gear
                } else if p.clutch.engagement(c.clutch) < 1.0 && !d.clutch_locked[0] {
                    d.gear
                } else {
                    0
                }
            }
            // In a dog box, once the dogs are out: while they carry it, a blip only loads
            // them.
            GearboxKind::Sequential { .. } => {
                if d.phase == ShiftPhase::Moving {
                    d.target_gear
                } else {
                    0
                }
            }
            GearboxKind::DualClutch { .. } => {
                if d.shifting() {
                    d.target_gear
                } else {
                    0
                }
            }
        };
        if gear != 0 {
            let out = driveline_speed(p, &car.state.wheels.map(|w| w.spin));
            let target_rpm = gear_ratio(p, gear) * out * RPM_PER_RAD_S;
            c.throttle = c
                .throttle
                .max(blip_throttle(target_rpm, d.rpm(), BLIP_BAND_RPM));
        }
    }
}
