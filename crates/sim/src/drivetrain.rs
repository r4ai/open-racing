//! Engine, clutch, sequential gearbox and limited-slip differential.

use crate::params::{CarParams, lookup};

const RPM_PER_RAD_S: f64 = 60.0 / std::f64::consts::TAU;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrivetrainState {
    /// Engine speed in rad/s.
    pub engine_speed: f64,
    /// −1 = reverse, 0 = neutral, 1.. = forward gears.
    pub gear: i32,
    /// Gear being engaged while a shift is in progress.
    pub target_gear: i32,
    /// Remaining time of an ongoing shift in s (no drive while > 0).
    pub shift_timer: f64,
    pub stalled: bool,
    /// Torque transmitted by the clutch in the last step, N·m (engine side).
    pub clutch_torque: f64,
}

impl DrivetrainState {
    pub fn new(p: &CarParams, gear: i32) -> Self {
        Self {
            engine_speed: p.engine.idle_rpm / RPM_PER_RAD_S,
            gear,
            target_gear: gear,
            shift_timer: 0.0,
            stalled: false,
            clutch_torque: 0.0,
        }
    }

    pub fn rpm(&self) -> f64 {
        self.engine_speed * RPM_PER_RAD_S
    }

    /// Overall ratio engine / wheel for the current gear (0 in neutral or mid-shift).
    pub fn ratio(&self, p: &CarParams) -> f64 {
        if self.shift_timer > 0.0 {
            return 0.0;
        }
        gear_ratio(p, self.gear)
    }

    pub fn request_shift(&mut self, p: &CarParams, up: bool) {
        if self.shift_timer > 0.0 {
            return;
        }
        let top = p.gearbox.ratios.len() as i32;
        let next = (self.gear + if up { 1 } else { -1 }).clamp(-1, top);
        if next != self.gear {
            self.target_gear = next;
            self.shift_timer = p.gearbox.shift_time;
        }
    }

    pub fn restart(&mut self, p: &CarParams) {
        self.stalled = false;
        self.engine_speed = p.engine.idle_rpm / RPM_PER_RAD_S;
    }
}

/// Overall ratio engine / wheel for `gear`.
fn gear_ratio(p: &CarParams, gear: i32) -> f64 {
    let g = &p.gearbox;
    match gear {
        0 => 0.0,
        -1 => -g.reverse * g.final_drive,
        n => g.ratios[(n - 1) as usize] * g.final_drive,
    }
}

/// Inputs to one drivetrain step, gathered from the wheels.
pub struct DriveInput {
    pub throttle: f64,
    pub clutch_pedal: f64,
    /// Spin rates of the two driven wheels (left, right), rad/s.
    pub wheel_speed: [f64; 2],
    /// Net torque on each driven wheel from the road and brakes excluding the drivetrain, N·m.
    pub wheel_torque: [f64; 2],
    pub wheel_inertia: f64,
    pub dt: f64,
}

/// Advances the engine and returns the drive torque applied to each driven wheel.
pub fn step(state: &mut DrivetrainState, p: &CarParams, input: &DriveInput) -> [f64; 2] {
    let dt = input.dt;
    if state.shift_timer > 0.0 {
        state.shift_timer -= dt;
        if state.shift_timer <= 0.0 {
            state.shift_timer = 0.0;
            state.gear = state.target_gear;
        }
    }

    let e = &p.engine;
    let rpm = state.rpm();
    let wheel_avg = 0.5 * (input.wheel_speed[0] + input.wheel_speed[1]);
    // Sequential gearbox electronics: ignition cut on upshifts, throttle blip to
    // match revs on downshifts.
    let driver_throttle = if state.shift_timer > 0.0 {
        if state.target_gear > state.gear {
            0.0
        } else {
            let target_rpm = wheel_avg * gear_ratio(p, state.target_gear) * RPM_PER_RAD_S;
            ((target_rpm - rpm) / 500.0).clamp(0.0, 1.0)
        }
    } else {
        input.throttle
    };
    let engine_torque = if state.stalled {
        -lookup(&e.drag_curve, rpm)
    } else {
        // Idle governor: opens the throttle just enough to hold idle.
        let idle_throttle = ((e.idle_rpm - rpm) / 300.0).clamp(0.0, 0.35);
        let mut throttle = driver_throttle.max(idle_throttle);
        if rpm >= e.limiter_rpm {
            throttle = 0.0;
        }
        let full = lookup(&e.torque_curve, rpm);
        let drag = lookup(&e.drag_curve, rpm);
        throttle * full - (1.0 - throttle) * drag
    };

    let ratio = state.ratio(p);
    let eff = p.gearbox.efficiency;

    // Clutch: the torque that would lock engine and driveline together this step,
    // limited by capacity. Anti-stall fades capacity out near the stall speed.
    let clutch_torque = if ratio == 0.0 {
        0.0
    } else {
        let anti_stall =
            ((rpm - e.stall_rpm) / (p.clutch.anti_stall_rpm - e.stall_rpm)).clamp(0.0, 1.0);
        let capacity = p.clutch.max_torque * (1.0 - input.clutch_pedal) * anti_stall;
        // Driveline seen from the engine: two wheels reflected through the ratio.
        let i_d = 2.0 * input.wheel_inertia / (ratio * ratio);
        let t_d = (input.wheel_torque[0] + input.wheel_torque[1]) / ratio;
        let slip = state.engine_speed - wheel_avg * ratio;
        let lock = (slip + dt * (engine_torque / e.inertia - t_d / i_d))
            / (dt * (1.0 / e.inertia + 1.0 / i_d));
        lock.clamp(-capacity, capacity)
    };
    state.clutch_torque = clutch_torque;

    state.engine_speed += dt * (engine_torque - clutch_torque) / e.inertia;
    if !state.stalled && state.engine_speed * RPM_PER_RAD_S < e.stall_rpm * 0.5 {
        state.stalled = true;
    }
    state.engine_speed = state.engine_speed.max(0.0);

    // Differential: split input torque, then transfer up to the locking torque from
    // the faster to the slower wheel.
    let input_torque = clutch_torque
        * ratio
        * if clutch_torque * ratio >= 0.0 {
            eff
        } else {
            1.0 / eff
        };
    let half = 0.5 * input_torque;
    let d = &p.differential;
    let ramp = if clutch_torque >= 0.0 {
        d.power_ramp
    } else {
        d.coast_ramp
    };
    let lock_capacity = d.preload + ramp * input_torque.abs();
    let iw = input.wheel_inertia;
    let acc_l = (half + input.wheel_torque[0]) / iw;
    let acc_r = (half + input.wheel_torque[1]) / iw;
    let equalize =
        (input.wheel_speed[0] - input.wheel_speed[1] + dt * (acc_l - acc_r)) * iw / (2.0 * dt);
    let transfer = equalize.clamp(-lock_capacity, lock_capacity);
    [half - transfer, half + transfer]
}
