//! Optional driver aids that act like a human driver on top of the physics.
//! They only produce `Controls`; the car model itself is never altered.

use crate::car::Car;
use crate::controls::Shift;

/// Automatic gear selection for a sequential gearbox (the driver still owns the pedals).
#[derive(Clone, Copy, Debug, Default)]
pub struct AutoShift;

impl AutoShift {
    pub fn shift(&self, car: &Car) -> Shift {
        let p = &car.model.params;
        let dt = &car.state.drivetrain;
        if dt.shift_timer > 0.0 {
            return Shift::None;
        }
        let top = p.gearbox.ratios.len() as i32;
        if dt.gear <= 0 {
            return Shift::Up;
        }
        let rpm = dt.rpm();
        let limiter = p.engine.limiter_rpm;
        if rpm > 0.96 * limiter && dt.gear < top {
            return Shift::Up;
        }
        if dt.gear > 1 {
            let g = &p.gearbox.ratios;
            let idx = (dt.gear - 1) as usize;
            let rpm_after = rpm * g[idx - 1] / g[idx];
            if rpm_after < 0.85 * limiter && rpm < 0.55 * limiter {
                return Shift::Down;
            }
        }
        Shift::None
    }
}
