//! Engine, clutch, sequential gearbox and limited-slip differentials, driving the front,
//! rear or all wheels.

use crate::params::{CarParams, DifferentialParams, Drive, lookup};

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

/// Inputs to one drivetrain step, gathered from the wheels in the simulation's order.
pub struct DriveInput {
    pub throttle: f64,
    pub clutch_pedal: f64,
    /// Full-throttle torque relative to the engine's curve: the air's density.
    pub power: f64,
    /// Spin rates of the wheels, rad/s.
    pub wheel_speed: [f64; 4],
    /// Net torque on each wheel from the road, excluding the drivetrain, N·m.
    pub wheel_torque: [f64; 4],
    /// Rotational inertia of each wheel, kg·m².
    pub wheel_inertia: [f64; 4],
    pub dt: f64,
}

/// Wheel indices of the front or rear axle (left, right).
fn axle(front: bool) -> [usize; 2] {
    if front { [0, 1] } else { [2, 3] }
}

/// Sum of a per-wheel value over the front or rear axle.
fn axle_sum(v: &[f64; 4], front: bool) -> f64 {
    axle(front).iter().map(|&i| v[i]).sum()
}

/// Advances the engine and returns the drive torque applied to each wheel.
pub fn step(state: &mut DrivetrainState, p: &CarParams, input: &DriveInput) -> [f64; 4] {
    let dt = input.dt;
    if state.shift_timer > 0.0 {
        state.shift_timer -= dt;
        if state.shift_timer <= 0.0 {
            state.shift_timer = 0.0;
            state.gear = state.target_gear;
        }
    }

    // Speed of the gearbox output: the axles' speeds weighted by their torque shares,
    // which is how fast a centre differential's carrier turns.
    let share = p.drive.front_share();
    let driveline_speed = 0.5
        * (share * axle_sum(&input.wheel_speed, true)
            + (1.0 - share) * axle_sum(&input.wheel_speed, false));

    let e = &p.engine;
    let rpm = state.rpm();
    // Sequential gearbox electronics: ignition cut on upshifts, throttle blip to
    // match revs on downshifts.
    let driver_throttle = if state.shift_timer > 0.0 {
        if state.target_gear > state.gear {
            0.0
        } else {
            let target_rpm = driveline_speed * gear_ratio(p, state.target_gear) * RPM_PER_RAD_S;
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
        let full = lookup(&e.torque_curve, rpm) * input.power;
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
        // Driveline seen from the engine: the driven wheels reflected through the ratio.
        let driven = |v: &[f64; 4]| {
            [true, false]
                .into_iter()
                .filter(|&front| p.drive.drives(front))
                .map(|front| axle_sum(v, front))
                .sum::<f64>()
        };
        let i_d = driven(&input.wheel_inertia) / (ratio * ratio);
        let t_d = driven(&input.wheel_torque) / ratio;
        let slip = state.engine_speed - driveline_speed * ratio;
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

    let input_torque = clutch_torque
        * ratio
        * if clutch_torque * ratio >= 0.0 {
            eff
        } else {
            1.0 / eff
        };
    let power = clutch_torque >= 0.0;
    // An axle's differential: equal shares for the two wheels.
    let across = |d: &DifferentialParams, torque: f64, front: bool| {
        let [l, r] = axle(front);
        let pair = |v: &[f64; 4]| [v[l], v[r]];
        split(
            d,
            torque,
            0.5,
            power,
            pair(&input.wheel_speed),
            pair(&input.wheel_torque),
            pair(&input.wheel_inertia),
            dt,
        )
    };
    match &p.drive {
        Drive::Rear => {
            let [l, r] = across(&p.differential, input_torque, false);
            [0.0, 0.0, l, r]
        }
        Drive::Front => {
            let [l, r] = across(&p.differential, input_torque, true);
            [l, r, 0.0, 0.0]
        }
        Drive::All {
            front_share,
            centre_differential,
            front_differential,
        } => {
            // The centre differential sees each axle as one shaft turning at the mean of
            // its wheels' speeds.
            let axles = |v: &[f64; 4]| [axle_sum(v, true), axle_sum(v, false)];
            let [front, rear] = split(
                centre_differential,
                input_torque,
                *front_share,
                power,
                axles(&input.wheel_speed).map(|s| 0.5 * s),
                axles(&input.wheel_torque),
                axles(&input.wheel_inertia),
                dt,
            );
            let [fl, fr] = across(front_differential, front, true);
            let [rl, rr] = across(&p.differential, rear, false);
            [fl, fr, rl, rr]
        }
    }
}

/// Splits `torque` between two shafts in the ratio `share : 1 − share`, then transfers up
/// to the differential's locking torque from the faster shaft to the slower one: as much
/// as makes them turn equally at the end of the step. `road` is the other torque on each
/// shaft and `inertia` the inertia each one drives.
#[allow(clippy::too_many_arguments)]
fn split(
    d: &DifferentialParams,
    torque: f64,
    share: f64,
    power: bool,
    speed: [f64; 2],
    road: [f64; 2],
    inertia: [f64; 2],
    dt: f64,
) -> [f64; 2] {
    let (a, b) = (share * torque, (1.0 - share) * torque);
    let ramp = if power { d.power_ramp } else { d.coast_ramp };
    let lock_capacity = d.preload + ramp * torque.abs();
    let acc_a = (a + road[0]) / inertia[0];
    let acc_b = (b + road[1]) / inertia[1];
    let equalize =
        (speed[0] - speed[1] + dt * (acc_a - acc_b)) / (dt * (1.0 / inertia[0] + 1.0 / inertia[1]));
    let transfer = equalize.clamp(-lock_capacity, lock_capacity);
    [a - transfer, b + transfer]
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f64 = 1e-3;

    fn diff(preload: f64, ramp: f64) -> DifferentialParams {
        DifferentialParams {
            preload,
            power_ramp: ramp,
            coast_ramp: ramp,
        }
    }

    #[test]
    fn open_differential_keeps_the_torque_split() {
        // However differently the shafts turn, an open differential splits by its shares.
        let [a, b] = split(
            &diff(0.0, 0.0),
            1000.0,
            0.3,
            true,
            [50.0, 10.0],
            [-100.0, -400.0],
            [2.0, 3.0],
            DT,
        );
        assert!(
            (a - 300.0).abs() < 1e-9 && (b - 700.0).abs() < 1e-9,
            "{a} / {b}"
        );
    }

    #[test]
    fn locking_moves_torque_to_the_slower_shaft_up_to_its_capacity() {
        let run = |preload| {
            split(
                &diff(preload, 0.0),
                1000.0,
                0.5,
                true,
                [12.0, 10.0],
                [0.0, 0.0],
                [2.0, 2.0],
                DT,
            )
        };
        // A little locking torque all goes to the slower shaft.
        let [a, b] = run(100.0);
        assert!(
            (a - 400.0).abs() < 1e-9 && (b - 600.0).abs() < 1e-9,
            "{a} / {b}"
        );
        // With plenty, the shafts end the step turning equally.
        let [a, b] = run(1e6);
        let (end_a, end_b) = (12.0 + DT * a / 2.0, 10.0 + DT * b / 2.0);
        assert!((end_a - end_b).abs() < 1e-9, "{end_a} / {end_b}");
        assert!((a + b - 1000.0).abs() < 1e-9);
    }
}
