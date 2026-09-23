use serde::{Deserialize, Serialize};

/// Gear change request for a sequential gearbox.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Shift {
    #[default]
    None,
    Up,
    Down,
}

/// Driver inputs expressed in physical units, exactly as a real car receives them.
///
/// Input devices (keyboard, gamepad, steering wheel, AI policy) are responsible for
/// producing these values; the simulation never filters or assists them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Controls {
    /// Steering wheel angle in radians, positive = turn left. Clamped to the car's lock.
    pub steer_wheel_angle: f64,
    /// Throttle pedal, 0..1.
    pub throttle: f64,
    /// Brake pedal, 0..1.
    pub brake: f64,
    /// Clutch pedal, 0 = engaged (released pedal), 1 = fully disengaged.
    pub clutch: f64,
    /// Edge-triggered gear change request.
    pub shift: Shift,
}

impl Controls {
    pub(crate) fn sanitized(&self, steer_lock: f64) -> Self {
        Self {
            steer_wheel_angle: self.steer_wheel_angle.clamp(-steer_lock, steer_lock),
            throttle: self.throttle.clamp(0.0, 1.0),
            brake: self.brake.clamp(0.0, 1.0),
            clutch: self.clutch.clamp(0.0, 1.0),
            shift: self.shift,
        }
    }
}
