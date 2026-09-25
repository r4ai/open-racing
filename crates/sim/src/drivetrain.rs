//! Engine, clutch, gearbox and limited-slip differentials, driving the front, rear or all
//! wheels.
//!
//! Three bodies turn: the engine, the gearbox input shaft (clutch disc and the gears
//! turning with it) and the driveline (the driven wheels, seen at the gearbox output).
//! Friction couplings join them — the clutch, a synchroniser, or a dual-clutch gearbox's
//! two clutches — each transmitting what makes its two sides turn together at the end of
//! the step, up to its capacity. A meshed gear joins the input shaft rigidly to the
//! driveline.

use crate::controls::Shift;
use crate::params::{CarParams, DifferentialParams, Drive, DualClutchControl, GearboxKind, lookup};

pub const RPM_PER_RAD_S: f64 = 60.0 / std::f64::consts::TAU;

/// Speed difference between the engine and the input shaft within which a sequential
/// gearbox's clutch counts as stuck as its dogs mesh, rad/s.
const MESH_LOCK_WINDOW: f64 = 3.0;
/// Gauss-Seidel passes over the couplings per step.
const ITERATIONS: usize = 8;

/// Stage of a gear change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShiftPhase {
    #[default]
    None,
    /// Sequential: waiting for the torque through the dogs to fall enough to let go.
    Unloading,
    /// Sequential barrel turning, or the driver's hand moving the H-pattern lever.
    Moving,
    /// H-pattern: the synchroniser matching the input shaft to the selected gear.
    Synchronising,
    /// Dual clutch: the torque passing from one clutch to the other.
    Handover,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrivetrainState {
    /// Engine speed in rad/s.
    pub engine_speed: f64,
    /// Gearbox input shaft speed in rad/s.
    pub input_speed: f64,
    /// Meshed gear: −1 = reverse, 0 = none (neutral or between gears), 1.. = forward. On a
    /// dual-clutch gearbox, the gear whose clutch carries the drive.
    pub gear: i32,
    /// Gear selected: the lever's gate, the barrel's next gear or the dual clutch's
    /// incoming one. Equals `gear` when no shift is in progress.
    pub target_gear: i32,
    pub phase: ShiftPhase,
    /// Time spent in `phase`, s.
    pub phase_time: f64,
    pub stalled: bool,
    /// Whether each clutch is stuck rather than slipping: the clutch, or a dual clutch's
    /// odd and even ones.
    pub clutch_locked: [bool; 2],
    /// Torque the clutches took from the engine in the last step, N·m.
    pub clutch_torque: f64,
    /// A synchroniser has been fighting a turning engine: the gear grinds.
    pub grinding: bool,
}

impl DrivetrainState {
    pub fn new(p: &CarParams, gear: i32) -> Self {
        let engine_speed = p.engine.idle_rpm / RPM_PER_RAD_S;
        Self {
            engine_speed,
            input_speed: engine_speed,
            gear,
            target_gear: gear,
            phase: ShiftPhase::None,
            phase_time: 0.0,
            stalled: false,
            clutch_locked: [true; 2],
            clutch_torque: 0.0,
            grinding: false,
        }
    }

    pub fn rpm(&self) -> f64 {
        self.engine_speed * RPM_PER_RAD_S
    }

    /// Overall ratio engine / wheel of the meshed gear (0 in neutral or between gears).
    pub fn ratio(&self, p: &CarParams) -> f64 {
        gear_ratio(p, self.gear)
    }

    /// Whether a gear change is in progress.
    pub fn shifting(&self) -> bool {
        self.phase != ShiftPhase::None
    }

    pub fn restart(&mut self, p: &CarParams) {
        self.stalled = false;
        self.engine_speed = p.engine.idle_rpm / RPM_PER_RAD_S;
    }
}

/// Overall ratio engine / wheel for `gear`.
pub fn gear_ratio(p: &CarParams, gear: i32) -> f64 {
    let g = &p.gearbox;
    match gear {
        0 => 0.0,
        -1 => -g.reverse * g.final_drive,
        n => g.ratios[(n - 1) as usize] * g.final_drive,
    }
}

/// Speed of the gearbox output: the axles' speeds weighted by their torque shares, which
/// is how fast a centre differential's carrier turns.
pub fn driveline_speed(p: &CarParams, wheel_speed: &[f64; 4]) -> f64 {
    let share = p.drive.front_share();
    0.5 * (share * axle_sum(wheel_speed, true) + (1.0 - share) * axle_sum(wheel_speed, false))
}

/// Throttle that brings the engine up to `target_rpm` from `rpm` for a rev-matched
/// downshift, opening fully `band_rpm` short of it; nothing when it already turns faster.
pub fn blip_throttle(target_rpm: f64, rpm: f64, band_rpm: f64) -> f64 {
    ((target_rpm - rpm) / band_rpm).clamp(0.0, 1.0)
}

/// Which of a dual clutch's clutches drives `gear`: odd gears on one, even gears and
/// reverse on the other.
fn clutch_of(gear: i32) -> usize {
    if gear > 0 && gear % 2 == 1 { 0 } else { 1 }
}

/// Inputs to one drivetrain step, gathered from the wheels in the simulation's order.
pub struct DriveInput {
    pub throttle: f64,
    pub brake: f64,
    pub clutch_pedal: f64,
    /// Sequential gear change request, one step long.
    pub shift: Shift,
    /// H-pattern lever gate held by the driver, if a shifter is used.
    pub selector: Option<i32>,
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

/// Moves the lever, barrel or clutches on by one step: takes the driver's request and
/// advances the gear change in progress. `out` is the gearbox output speed.
fn advance_shift(s: &mut DrivetrainState, p: &CarParams, input: &DriveInput, out: f64) {
    let top = p.gearbox.ratios.len() as i32;
    s.phase_time += input.dt;
    let request = match input.shift {
        Shift::None => None,
        Shift::Up => Some(true),
        Shift::Down => Some(false),
    };
    let next = |from: i32, up: bool| (from + if up { 1 } else { -1 }).clamp(-1, top);
    let begin = |s: &mut DrivetrainState, phase| {
        s.phase = phase;
        s.phase_time = 0.0;
    };
    // Downshift protection: no gear that would spin the engine past its limit.
    let allowed = |from: i32, to: i32| {
        let (r_from, r_to) = (gear_ratio(p, from), gear_ratio(p, to));
        r_to.abs() <= r_from.abs()
            || p.electronics
                .downshift_protection_rpm
                .is_none_or(|max| r_to * out * RPM_PER_RAD_S <= max)
    };
    match p.gearbox.kind {
        GearboxKind::HPattern {
            lever_time,
            sync_window,
            grind_time,
            ..
        } => {
            // A shifter puts the lever straight into its gate; up / down requests move it
            // one gate along the sequence R, N, 1, 2, ...
            let gate = match input.selector {
                Some(gate) => Some((gate.clamp(-1, top), 0.0)),
                None if s.phase == ShiftPhase::Moving => None,
                None => request.map(|up| (next(s.target_gear, up), lever_time)),
            };
            if let Some((gate, travel)) = gate.filter(|&(gate, _)| gate != s.target_gear) {
                s.gear = 0;
                s.target_gear = gate;
                begin(s, ShiftPhase::Moving);
                if travel == 0.0 {
                    s.phase_time = f64::INFINITY;
                }
            }
            if s.phase == ShiftPhase::Moving && s.phase_time >= lever_time {
                if s.target_gear == 0 {
                    begin(s, ShiftPhase::None);
                } else {
                    begin(s, ShiftPhase::Synchronising);
                }
            }
            if s.phase == ShiftPhase::Synchronising
                && (s.input_speed - gear_ratio(p, s.target_gear) * out).abs() < sync_window
            {
                s.gear = s.target_gear;
                begin(s, ShiftPhase::None);
            }
            s.grinding = s.phase == ShiftPhase::Synchronising && s.phase_time > grind_time;
        }
        GearboxKind::Sequential {
            shift_time,
            dog_release_torque,
        } => {
            if let Some(up) = request.filter(|_| s.phase == ShiftPhase::None) {
                let to = next(s.gear, up);
                if to != s.gear && allowed(s.gear, to) {
                    s.target_gear = to;
                    if s.gear == 0 {
                        begin(s, ShiftPhase::Moving);
                    } else {
                        begin(s, ShiftPhase::Unloading);
                    }
                }
            }
            if s.phase == ShiftPhase::Unloading && s.clutch_torque.abs() < dog_release_torque {
                s.gear = 0;
                begin(s, ShiftPhase::Moving);
            }
            if s.phase == ShiftPhase::Moving && s.phase_time >= shift_time {
                // The dogs mesh at once: the light input shaft jumps to the gear's speed and
                // the clutch slips until the engine matches it.
                s.gear = s.target_gear;
                s.input_speed = gear_ratio(p, s.gear) * out;
                s.clutch_locked[0] = (s.engine_speed - s.input_speed).abs() < MESH_LOCK_WINDOW;
                begin(s, ShiftPhase::None);
            }
        }
        GearboxKind::DualClutch {
            shift_time,
            ref control,
            ..
        } => {
            if let Some(up) = request.filter(|_| s.phase == ShiftPhase::None) {
                let to = next(s.gear, up);
                if to != s.gear && allowed(s.gear, to) {
                    s.target_gear = to;
                    if s.gear == 0 || to == 0 {
                        // Into or out of neutral: nothing to hand over.
                        s.gear = to;
                    } else {
                        begin(s, ShiftPhase::Handover);
                    }
                }
            }
            // The control unit drops a gear once the car slows too much for it.
            if s.phase == ShiftPhase::None
                && s.gear > 1
                && gear_ratio(p, s.gear) * out * RPM_PER_RAD_S
                    < p.engine.idle_rpm + control.downshift_rpm
            {
                s.target_gear = s.gear - 1;
                begin(s, ShiftPhase::Handover);
            }
            if s.phase == ShiftPhase::Handover && s.phase_time >= shift_time {
                s.gear = s.target_gear;
                begin(s, ShiftPhase::None);
            }
        }
    }
}

/// A rotating body: its speed, inverse inertia and the torque on it this step.
#[derive(Clone, Copy, Default)]
struct Body {
    speed: f64,
    inv_inertia: f64,
    torque: f64,
}

impl Body {
    fn new(speed: f64, inertia: f64, torque: f64) -> Self {
        Self {
            speed,
            inv_inertia: 1.0 / inertia,
            torque,
        }
    }
}

/// A friction coupling between bodies `a` and `b` through `ratio`: it holds
/// `a.speed = ratio · b.speed` with up to `capacity` N·m on `a`. It takes `torque` from `a`
/// and gives `ratio · torque` to `b`.
#[derive(Clone, Copy, Default)]
struct Coupling {
    a: usize,
    b: usize,
    ratio: f64,
    capacity: f64,
    torque: f64,
}

impl Coupling {
    fn new(a: usize, b: usize, ratio: f64, capacity: f64) -> Self {
        Self {
            a,
            b,
            ratio,
            capacity,
            torque: 0.0,
        }
    }
}

/// The couplings active in a step, with the `clutch_locked` entry each clutch keeps its
/// state in.
#[derive(Default)]
struct CouplingSet {
    list: [Coupling; 2],
    clutch: [Option<usize>; 2],
    len: usize,
}

impl CouplingSet {
    fn push(&mut self, c: Coupling, clutch: Option<usize>) {
        self.list[self.len] = c;
        self.clutch[self.len] = clutch;
        self.len += 1;
    }
}

const ENGINE: usize = 0;
const DRIVELINE: usize = 1;
const INPUT: usize = 2;

/// Finds the coupling torques by projected Gauss-Seidel: each coupling in turn takes the
/// torque that makes its sides turn together at the end of the step, within its capacity.
fn solve(bodies: &mut [Body; 3], couplings: &mut [Coupling], dt: f64) {
    for _ in 0..ITERATIONS {
        for c in couplings.iter_mut() {
            let (a, b) = (bodies[c.a], bodies[c.b]);
            let slip = a.speed + dt * a.torque * a.inv_inertia
                - c.ratio * (b.speed + dt * b.torque * b.inv_inertia);
            let mass = dt * (a.inv_inertia + c.ratio * c.ratio * b.inv_inertia);
            let torque = (c.torque + slip / mass).clamp(-c.capacity, c.capacity);
            let delta = torque - c.torque;
            c.torque = torque;
            bodies[c.a].torque -= delta;
            bodies[c.b].torque += c.ratio * delta;
        }
    }
}

/// Torque a dual-clutch control unit asks of the clutch driving the gear: all it has once
/// the clutch is stuck at speed, otherwise a slip that holds the engine near a speed set
/// by the throttle to pull away, plus a creep with neither pedal pressed.
fn dual_clutch_capacity(
    s: &DrivetrainState,
    p: &CarParams,
    input: &DriveInput,
    out: f64,
    creep_torque: f64,
    launch_rpm: f64,
    control: &DualClutchControl,
) -> f64 {
    let e = &p.engine;
    let rpm = s.rpm();
    let gear_rpm = gear_ratio(p, s.gear) * out * RPM_PER_RAD_S;
    let locked = s.clutch_locked[clutch_of(s.gear)];
    if (locked && gear_rpm > e.idle_rpm)
        || ((rpm - gear_rpm).abs() < control.lock_slip_rpm
            && gear_rpm > e.idle_rpm + control.lock_rpm)
    {
        return p.clutch.max_torque;
    }
    let closed_bite = e.idle_rpm + control.bite_rpm;
    let bite = closed_bite + (launch_rpm - closed_bite) * input.throttle;
    let creep = if input.throttle == 0.0 && input.brake > 0.0 {
        0.0
    } else {
        creep_torque
    };
    creep + p.clutch.max_torque * ((rpm - bite) / control.engage_band_rpm).clamp(0.0, 1.0)
}

/// Advances the engine and gearbox and returns the drive torque applied to each wheel.
pub fn step(state: &mut DrivetrainState, p: &CarParams, input: &DriveInput) -> [f64; 4] {
    let dt = input.dt;
    let out = driveline_speed(p, &input.wheel_speed);
    advance_shift(state, p, input, out);
    let s = state;

    let e = &p.engine;
    let el = &p.electronics;
    let rpm = s.rpm();
    // Gearbox electronics: ignition cut to unload the dogs on sequential upshifts, a
    // throttle blip to match revs on downshifts.
    let target_rpm = gear_ratio(p, s.target_gear) * out * RPM_PER_RAD_S;
    let upshift = target_rpm < rpm;
    let mut throttle = input.throttle;
    if el.ignition_cut
        && matches!(p.gearbox.kind, GearboxKind::Sequential { .. })
        && matches!(s.phase, ShiftPhase::Unloading | ShiftPhase::Moving)
        && upshift
    {
        throttle = 0.0;
    }
    // The blip waits for the dogs to let go: while they carry it, it only loads them.
    if el.auto_blip
        && !matches!(s.phase, ShiftPhase::None | ShiftPhase::Unloading)
        && s.target_gear != 0
    {
        throttle = throttle.max(blip_throttle(target_rpm, rpm, el.blip_band_rpm));
    }
    let engine_torque = if s.stalled {
        -lookup(&e.drag_curve, rpm)
    } else {
        // Idle control: opens the throttle just enough to hold idle.
        let idle_throttle = ((e.idle_rpm - rpm) / e.idle_band_rpm).clamp(0.0, e.idle_authority);
        let mut throttle = throttle.max(idle_throttle);
        if rpm >= e.limiter_rpm {
            throttle = 0.0;
        }
        let full = lookup(&e.torque_curve, rpm) * input.power;
        let drag = lookup(&e.drag_curve, rpm);
        throttle * full - (1.0 - throttle) * drag
    };

    // The driven wheels seen at the gearbox output.
    let driven = |v: &[f64; 4]| {
        [true, false]
            .into_iter()
            .filter(|&front| p.drive.drives(front))
            .map(|front| axle_sum(v, front))
            .sum::<f64>()
    };
    let (out_inertia, out_torque) = (driven(&input.wheel_inertia), driven(&input.wheel_torque));
    let input_inertia = p.gearbox.input_inertia;
    let friction = |locked: bool, capacity: f64| {
        if locked {
            capacity
        } else {
            capacity * p.clutch.kinetic_ratio
        }
    };

    // Shaft inertia meshed with the driveline, at the gearbox output.
    let mut meshed_inertia = 0.0;
    let mut set = CouplingSet::default();
    match p.gearbox.kind {
        GearboxKind::HPattern { .. } | GearboxKind::Sequential { .. } => {
            // Anti-stall opens the clutch as the gear drags the engine down.
            let dragged = (gear_ratio(p, s.gear) * out * RPM_PER_RAD_S) < rpm;
            let anti_stall = match &el.anti_stall {
                Some(a) if dragged => ((rpm - a.rpm) / a.band_rpm).clamp(0.0, 1.0),
                _ => 1.0,
            };
            let capacity = friction(
                s.clutch_locked[0],
                p.clutch.max_torque * p.clutch.engagement(input.clutch_pedal) * anti_stall,
            );
            if s.gear != 0 {
                let ratio = gear_ratio(p, s.gear);
                meshed_inertia = input_inertia * ratio * ratio;
                set.push(Coupling::new(ENGINE, DRIVELINE, ratio, capacity), Some(0));
            } else {
                set.push(Coupling::new(ENGINE, INPUT, 1.0, capacity), Some(0));
                if let (
                    GearboxKind::HPattern {
                        synchro_torque,
                        reverse_synchro,
                        ..
                    },
                    ShiftPhase::Synchronising,
                ) = (&p.gearbox.kind, s.phase)
                {
                    // Without a synchroniser, only the gear's teeth meet the input shaft.
                    let torque = if s.target_gear == -1 && !reverse_synchro {
                        0.0
                    } else {
                        *synchro_torque
                    };
                    let ratio = gear_ratio(p, s.target_gear);
                    set.push(Coupling::new(INPUT, DRIVELINE, ratio, torque), None);
                }
            }
        }
        GearboxKind::DualClutch {
            shift_time,
            creep_torque,
            launch_rpm,
            ref control,
        } => {
            if s.gear != 0 {
                let capacity =
                    dual_clutch_capacity(s, p, input, out, creep_torque, launch_rpm, control);
                let incoming = if s.phase == ShiftPhase::Handover {
                    (s.phase_time / shift_time).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let mut engage = |gear: i32, share: f64| {
                    let ratio = gear_ratio(p, gear);
                    let k = clutch_of(gear);
                    meshed_inertia += 0.5 * input_inertia * ratio * ratio;
                    let capacity = friction(s.clutch_locked[k], capacity * share);
                    set.push(Coupling::new(ENGINE, DRIVELINE, ratio, capacity), Some(k));
                };
                engage(s.gear, 1.0 - incoming);
                if s.phase == ShiftPhase::Handover {
                    engage(s.target_gear, incoming);
                }
            }
        }
    }

    let mut bodies = [
        Body::new(s.engine_speed, e.inertia, engine_torque),
        Body::new(out, out_inertia + meshed_inertia, out_torque),
        Body::new(
            s.input_speed,
            input_inertia,
            -p.gearbox.input_drag * (s.input_speed / 1.0).tanh(),
        ),
    ];
    let couplings = &mut set.list[..set.len];
    solve(&mut bodies, couplings, dt);

    s.clutch_torque = couplings
        .iter()
        .filter(|c| c.a == ENGINE)
        .map(|c| c.torque)
        .sum();
    for (c, k) in couplings.iter().zip(set.clutch) {
        if let Some(k) = k {
            s.clutch_locked[k] = c.torque.abs() < c.capacity;
        }
    }
    let accel = |b: &Body| b.torque * b.inv_inertia;
    s.engine_speed += dt * accel(&bodies[ENGINE]);
    if !s.stalled && s.rpm() < e.stall_rpm * 0.5 {
        s.stalled = true;
    } else if s.stalled && s.rpm() > e.idle_rpm {
        // Bump start: the wheels spun the engine up through the clutch.
        s.stalled = false;
    }
    s.engine_speed = s.engine_speed.max(0.0);
    let out_accel = accel(&bodies[DRIVELINE]);
    s.input_speed = if s.gear != 0 {
        gear_ratio(p, s.gear) * (out + dt * out_accel)
    } else {
        s.input_speed + dt * accel(&bodies[INPUT])
    };

    // What the couplings gave the driveline, less what spun up the meshed shafts.
    let coupled = bodies[DRIVELINE].torque - out_torque - meshed_inertia * out_accel;
    let power = s.clutch_torque >= 0.0;
    let eff = p.gearbox.efficiency;
    let input_torque = coupled * if power { eff } else { 1.0 / eff };
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
