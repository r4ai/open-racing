//! Mean-value model of a four-stroke engine: air through the throttle fills the intake
//! manifold, the cylinders draw from it, and the torque follows the air they trap, less
//! the work of pumping it and the engine's friction. A turbocharger's shaft, driven by
//! the exhaust and loaded by its compressor, sets the pressure ahead of the throttle.
//!
//! The model is calibrated to the car's torque curves at build time: in standard air at
//! a steady speed, the engine gives its `torque_curve` with the throttle wide open (and
//! no boost) and its `drag_curve` with the throttle shut and the fuel cut. Everything
//! between and around those points — part throttle, the manifold filling and emptying,
//! turbo lag, the wastegate, thin air — comes from the flows.

use std::f64::consts::PI;

use crate::drivetrain::RPM_PER_RAD_S;
use crate::engine_thermal::{EngineHeat, EngineThermal};
use crate::params::{EngineParams, ThrottleKind, TurboParams, lookup};
use crate::{AMBIENT_TEMPERATURE, DT};

/// Specific gas constant of air, J/(kg·K).
const R_AIR: f64 = 287.05;
/// Sea-level standard pressure, Pa.
pub const STANDARD_PRESSURE: f64 = 101_325.0;
/// 0 °C in K.
const KELVIN: f64 = 273.15;
/// Pa per bar.
const BAR: f64 = 1e5;
/// Pressure ratio across the throttle below which its flow is choked (Hendricks' fit
/// of the compressible orifice flow).
const CRITICAL_RATIO: f64 = 0.4404;
/// Volumetric efficiency at the speed of peak indicated torque.
const PEAK_VOLUMETRIC_EFFICIENCY: f64 = 0.95;
/// Least volumetric efficiency the tables give, however low the torque curve drops.
const MIN_VOLUMETRIC_EFFICIENCY: f64 = 0.2;
/// Indicated mean effective pressure at the torque peak, which sizes the displacement
/// when the car does not give it, Pa.
const PEAK_IMEP: f64 = 14.0e5;
/// Manifold volume relative to the displacement when the car does not give it.
const MANIFOLD_VOLUME_RATIO: f64 = 1.5;
/// Manifold pressure relative to the ambient at the rev limit with the throttle wide
/// open: sizes the throttle bore.
const WOT_PRESSURE_RATIO: f64 = 0.97;
/// Manifold pressure relative to the ambient of an engine before it has run.
const CLOSED_PRESSURE_RATIO: f64 = 0.25;
/// Torque short of holding idle with the plate shut, relative to the drag there: the
/// air leaking past the plate is sized so that the idle control has to add the rest.
const IDLE_SHORTFALL: f64 = 0.2;
/// Angle of a shut throttle plate from the bore's cross-section, rad.
const PLATE_CLOSED_ANGLE: f64 = 0.12;
/// Share of the closed-throttle drag at least left to friction once pumping is taken out.
const MIN_FRICTION_SHARE: f64 = 0.3;
/// Air-fuel mass ratio of a firing engine.
const AIR_FUEL_RATIO: f64 = 12.8;
/// Turbocharger bearing and windage losses relative to the compressor's power at the
/// reference point.
const TURBO_FRICTION: f64 = 0.2;
/// Power the exhaust gives the turbine with the fuel cut (cool air, no combustion),
/// relative to a firing engine at the same flow.
const COLD_EXHAUST: f64 = 0.2;
/// Manifold pressure relative to the ambient below which a blow-off valve opens.
const BLOW_OFF_VACUUM: f64 = 0.8;
/// (γ − 1) / γ for air.
const HEAT_RATIO_EXPONENT: f64 = 0.2857;
/// Engine speeds tabulated from 0 to 110 % of the rev limit.
const TABLE_POINTS: usize = 128;
/// Time constant of a drive-by-wire throttle's motor following the pedal, s.
const THROTTLE_MOTOR_TIME: f64 = 0.025;
/// How much of its lag the throttle motor keeps over one physics step:
/// exp(−DT / THROTTLE_MOTOR_TIME), to 16 digits.
const MOTOR_DECAY_PER_STEP: f64 = 0.960_789_439_152_323_2;
/// Newton iterations at most for the manifold pressure in a step.
const MANIFOLD_ITERATIONS: usize = 12;

/// The air the engine breathes.
#[derive(Clone, Copy, Debug)]
pub struct Ambient {
    /// Pressure, Pa.
    pub pressure: f64,
    /// Temperature, °C.
    pub temperature: f64,
    /// Full-throttle torque relative to standard conditions at the same manifold
    /// pressure: the dry air's share and the charge temperature (SAE J1349 with the
    /// pressure taken out, as the manifold carries it).
    pub charge: f64,
}

impl Ambient {
    /// Standard air: 1013.25 hPa and the ambient temperature.
    pub const STANDARD: Self = Self {
        pressure: STANDARD_PRESSURE,
        temperature: AMBIENT_TEMPERATURE,
        charge: 1.0,
    };
}

/// What changes in the engine from step to step, beyond its speed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EngineState {
    /// Throttle the plate is set to, 0..1: the pedal, or where a throttle motor has got
    /// to following it.
    pub throttle: f64,
    /// Absolute pressure in the intake manifold, Pa.
    pub manifold_pressure: f64,
    /// Kinetic energy of the turbocharger's shaft relative to that at which it gives
    /// its maximum boost in standard air (0 without a turbocharger).
    pub turbo_spool: f64,
    /// Gauge pressure ahead of the throttle, Pa: the boost.
    pub boost: f64,
    /// Wastegate opening, 0 = shut, 1 = fully open.
    pub wastegate: f64,
    /// Air drawn into the cylinders, kg/s.
    pub air_flow: f64,
    /// Fuel injected, kg/s.
    pub fuel_flow: f64,
    /// Torque at the crank, N·m.
    pub torque: f64,
    /// Temperatures and wear of the engine's parts.
    pub heat: EngineHeat,
}

impl Default for EngineState {
    fn default() -> Self {
        Self {
            throttle: 0.0,
            manifold_pressure: CLOSED_PRESSURE_RATIO * STANDARD_PRESSURE,
            turbo_spool: 0.0,
            boost: 0.0,
            wastegate: 0.0,
            air_flow: 0.0,
            fuel_flow: 0.0,
            torque: 0.0,
            heat: EngineHeat::WARM,
        }
    }
}

/// Calibration at one engine speed.
#[derive(Clone, Copy, Debug, Default)]
struct Point {
    /// Volumetric efficiency.
    volumetric_efficiency: f64,
    /// Indicated torque per Pa of manifold pressure while firing, N·m/Pa.
    combustion: f64,
    /// Friction torque, N·m.
    friction: f64,
    /// Steady manifold pressure in standard air with the throttle shut / wide open, Pa.
    closed: f64,
    open: f64,
}

impl Point {
    fn lerp(a: &Self, b: &Self, t: f64) -> Self {
        let l = |x: f64, y: f64| x + (y - x) * t;
        Self {
            volumetric_efficiency: l(a.volumetric_efficiency, b.volumetric_efficiency),
            combustion: l(a.combustion, b.combustion),
            friction: l(a.friction, b.friction),
            closed: l(a.closed, b.closed),
            open: l(a.open, b.open),
        }
    }
}

#[derive(Clone, Debug)]
struct Turbo {
    /// Wastegate setpoint and the boost range over which it opens, Pa.
    max_boost: f64,
    band: f64,
    /// PR^((γ−1)/γ) − 1 of the compressor at unit spool: its ideal work per unit mass.
    work: f64,
    /// Air flow at the reference speed with the wastegate shut at max boost, kg/s.
    reference_flow: f64,
    /// Time constant of the spool at the reference flow, s, times (1 + friction).
    tau: f64,
    exponent: f64,
    blow_off_valve: bool,
}

impl Turbo {
    /// Compressor pressure ratio at `spool`: its ideal work grows with the square of the
    /// shaft speed.
    #[inline]
    fn pressure_ratio(&self, spool: f64) -> f64 {
        // (1 + y)^(γ/(γ−1)) with γ/(γ−1) = 3.5.
        let y = 1.0 + spool.max(0.0) * self.work;
        y * y * y * y.sqrt()
    }
}

/// Simulation-ready engine: the calibration tables of an [`EngineParams`].
#[derive(Clone, Debug)]
pub struct EngineModel {
    /// Swept volume, m³.
    pub displacement: f64,
    /// Intake manifold volume, m³.
    pub manifold_volume: f64,
    /// Flow gain of the throttle wide open and of the leak past the shut plate, in
    /// standard air: ṁ = gain · p_up · β(p / p_up), kg/(s·Pa).
    throttle_gain: f64,
    leak_gain: f64,
    throttle: ThrottleKind,
    rpm_step: f64,
    table: Box<[Point]>,
    turbo: Option<Turbo>,
    /// Cooling system, and the limits of the parts.
    pub thermal: EngineThermal,
}

/// Flow through the throttle relative to choked flow at pressure ratio `pr`, and its
/// derivative.
#[inline]
fn beta(pr: f64) -> (f64, f64) {
    if pr <= CRITICAL_RATIO {
        return (1.0, 0.0);
    }
    let u = ((pr - CRITICAL_RATIO) / (1.0 - CRITICAL_RATIO)).min(1.0);
    let b = (1.0 - u * u).max(0.0).sqrt();
    (b, -u / ((1.0 - CRITICAL_RATIO) * b.max(1e-6)))
}

/// Mass flow into the manifold at pressure `p` from `up` through `gain`, and its
/// derivative with `p`. It reverses when the manifold holds the higher pressure.
#[inline]
fn throttle_flow(gain: f64, up: f64, p: f64) -> (f64, f64) {
    if p <= up {
        let (b, db) = beta(p / up);
        (gain * up * b, gain * db)
    } else {
        let r = up / p;
        let (b, db) = beta(r);
        (-gain * p * b, -gain * (b - r * db))
    }
}

/// Steady manifold pressure where the throttle's flow from `up` through `gain` meets
/// the cylinders' draw of `pumping` · p.
fn steady_pressure(gain: f64, up: f64, pumping: f64) -> f64 {
    let (mut lo, mut hi) = (0.0, up);
    for _ in 0..80 {
        let mid = 0.5 * (lo + hi);
        if throttle_flow(gain, up, mid).0 > pumping * mid {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// Manifold pressure at the end of a step from `p0` (backward Euler): the root of
/// p − p0 − k·(ṁ_throttle(p) − pumping·p), which rises monotonically with p.
#[inline]
fn solve_manifold(p0: f64, up: f64, gain: f64, pumping: f64, k: f64) -> f64 {
    let (mut lo, mut hi) = (0.0, p0.max(up));
    let mut p = p0.clamp(lo, hi);
    for _ in 0..MANIFOLD_ITERATIONS {
        let (flow, dflow) = throttle_flow(gain, up, p);
        let f = p - p0 - k * (flow - pumping * p);
        if f > 0.0 {
            hi = p;
        } else {
            lo = p;
        }
        let df = 1.0 - k * (dflow - pumping);
        let mut next = p - f / df;
        if !(next > lo && next < hi) {
            next = 0.5 * (lo + hi);
        }
        if (next - p).abs() < 0.1 {
            return next;
        }
        p = next;
    }
    p
}

/// Share of the bore a butterfly plate uncovers at `opening` of its travel, from shut
/// (tilted by the closed angle) to square to the bore.
#[inline]
fn plate_area(opening: f64) -> f64 {
    let angle = PLATE_CLOSED_ANGLE + opening.clamp(0.0, 1.0) * (0.5 * PI - PLATE_CLOSED_ANGLE);
    1.0 - angle.cos() / PLATE_CLOSED_ANGLE.cos()
}

impl EngineModel {
    pub fn new(e: &EngineParams) -> Self {
        let rt = R_AIR * (AMBIENT_TEMPERATURE + KELVIN);
        let indicated = |rpm: f64| lookup(&e.torque_curve, rpm) + lookup(&e.drag_curve, rpm);
        let peak = e
            .torque_curve
            .iter()
            .map(|&(rpm, _)| indicated(rpm))
            .fold(1.0, f64::max);
        let displacement = e
            .displacement
            .map_or(4.0 * PI * peak / PEAK_IMEP, |litres| litres * 1e-3);
        let manifold_volume = e
            .manifold_volume
            .map_or(MANIFOLD_VOLUME_RATIO * displacement, |litres| litres * 1e-3);
        let volumetric_efficiency = |rpm: f64| {
            (PEAK_VOLUMETRIC_EFFICIENCY * indicated(rpm) / peak).max(MIN_VOLUMETRIC_EFFICIENCY)
        };
        // Cylinder draw per Pa of manifold pressure at `rpm`, kg/(s·Pa).
        let pumping = |rpm: f64| {
            volumetric_efficiency(rpm) * displacement * (rpm / RPM_PER_RAD_S) / (4.0 * PI * rt)
        };
        let p0 = STANDARD_PRESSURE;
        // The throttle bore passes the air at the limiter with a small loss.
        let limiter_flow = pumping(e.limiter_rpm) * WOT_PRESSURE_RATIO * p0;
        let full_gain = limiter_flow / (p0 * beta(WOT_PRESSURE_RATIO).0);
        let pump_torque = |p: f64| (p0 - p) * displacement / (4.0 * PI);
        // The leak past the shut plate lets a firing engine at idle speed fall just short
        // of holding it: indicated torque ∝ p, calibrated wide open, against the drag.
        let idle = e.idle_rpm;
        let open = steady_pressure(full_gain, p0, pumping(idle));
        let (drag, full) = (lookup(&e.drag_curve, idle), lookup(&e.torque_curve, idle));
        let shortfall = |p: f64| {
            let friction = (drag - pump_torque(p)).max(MIN_FRICTION_SHARE * drag);
            (full + friction + pump_torque(open)) / open * p - pump_torque(p) - friction
                + IDLE_SHORTFALL * drag
        };
        let (mut lo, mut hi) = (0.0, open);
        for _ in 0..60 {
            let mid = 0.5 * (lo + hi);
            if shortfall(mid) < 0.0 {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let idle_pressure = 0.5 * (lo + hi);
        let leak_gain = pumping(idle) * idle_pressure / (p0 * beta(idle_pressure / p0).0.max(1e-6));
        let throttle_gain = (full_gain - leak_gain).max(0.0);

        let rpm_step = 1.1 * e.limiter_rpm / (TABLE_POINTS - 1) as f64;
        let table = (0..TABLE_POINTS)
            .map(|k| {
                let rpm = k as f64 * rpm_step;
                let c = pumping(rpm);
                let open = steady_pressure(throttle_gain + leak_gain, p0, c);
                let closed = steady_pressure(leak_gain, p0, c);
                let drag = lookup(&e.drag_curve, rpm);
                let friction = (drag - pump_torque(closed)).max(MIN_FRICTION_SHARE * drag);
                Point {
                    volumetric_efficiency: volumetric_efficiency(rpm),
                    combustion: (lookup(&e.torque_curve, rpm) + friction + pump_torque(open))
                        / open,
                    friction,
                    closed,
                    open,
                }
            })
            .collect::<Box<[Point]>>();

        let peak_power = e
            .torque_curve
            .iter()
            .map(|&(rpm, torque)| torque * rpm / RPM_PER_RAD_S)
            .fold(0.0, f64::max)
            * e.turbo
                .as_ref()
                .map_or(1.0, |t| 1.0 + t.max_boost * BAR / STANDARD_PRESSURE);
        let mut model = Self {
            thermal: EngineThermal::new(e, displacement, peak_power),
            displacement,
            manifold_volume,
            throttle_gain,
            leak_gain,
            throttle: e.throttle.clone(),
            rpm_step,
            table,
            turbo: None,
        };
        model.turbo = e.turbo.as_ref().map(|t| model.turbo_model(t, &pumping));
        model
    }

    fn turbo_model(&self, t: &TurboParams, pumping: &dyn Fn(f64) -> f64) -> Turbo {
        let p0 = STANDARD_PRESSURE;
        let ratio = (p0 + t.max_boost * BAR) / p0;
        let open = self.at(t.reference_rpm).open;
        Turbo {
            max_boost: t.max_boost * BAR,
            band: t.wastegate_band * BAR,
            work: ratio.powf(HEAT_RATIO_EXPONENT) - 1.0,
            reference_flow: pumping(t.reference_rpm) * open * ratio,
            tau: t.spool_time * (1.0 + TURBO_FRICTION),
            exponent: t.flow_exponent,
            blow_off_valve: t.blow_off_valve,
        }
    }

    /// Calibration at `rpm`, interpolated.
    #[inline]
    fn at(&self, rpm: f64) -> Point {
        let x = (rpm.max(0.0) / self.rpm_step).min((TABLE_POINTS - 1) as f64 - 1e-9);
        let i = x as usize;
        Point::lerp(&self.table[i], &self.table[i + 1], x - i as f64)
    }

    /// Whether a turbocharger feeds the engine.
    pub fn turbocharged(&self) -> bool {
        self.turbo.is_some()
    }

    /// The engine running steadily at `rpm` in standard air with the throttle at
    /// `throttle` (firing) and no boost.
    pub fn settled(&self, rpm: f64, throttle: f64) -> EngineState {
        let p = self.at(rpm);
        let throttle = throttle.clamp(0.0, 1.0);
        EngineState {
            throttle,
            manifold_pressure: p.closed + throttle * (p.open - p.closed),
            ..Default::default()
        }
    }

    /// Advances the manifold and the turbocharger by `dt` at engine speed `speed` (rad/s),
    /// heats the engine's parts and returns the torque at the crank. `firing` is false
    /// while the fuel or the ignition is cut (stalled, overrun, rev limiter, a gearbox's
    /// ignition cut); a broken engine does not fire.
    pub fn step(
        &self,
        s: &mut EngineState,
        speed: f64,
        throttle: f64,
        firing: bool,
        air: &Ambient,
        dt: f64,
    ) -> f64 {
        let firing = firing && !s.heat.failed();
        let rt = R_AIR * (air.temperature + KELVIN);
        let point = self.at(speed * RPM_PER_RAD_S);
        let pumping =
            point.volumetric_efficiency * self.displacement * speed.max(0.0) / (4.0 * PI * rt);
        let ambient = air.pressure;
        let upstream = match &self.turbo {
            // Manifold vacuum opens the blow-off valve, which vents the boost.
            Some(t) if !(t.blow_off_valve && s.manifold_pressure < BLOW_OFF_VACUUM * ambient) => {
                ambient * t.pressure_ratio(s.turbo_spool)
            }
            _ => ambient,
        };
        // The throttle's flow gain in this air: ṁ ∝ A / √(R·T).
        let air_gain = (R_AIR * (AMBIENT_TEMPERATURE + KELVIN) / rt).sqrt();
        let pedal = throttle.clamp(0.0, 1.0);
        s.throttle = match self.throttle {
            ThrottleKind::Cable => pedal,
            ThrottleKind::DriveByWire => {
                let decay = if dt == DT {
                    MOTOR_DECAY_PER_STEP
                } else {
                    (-dt / THROTTLE_MOTOR_TIME).exp()
                };
                pedal + (s.throttle - pedal) * decay
            }
        };
        let throttle = s.throttle;
        let gain = air_gain
            * match self.throttle {
                ThrottleKind::Cable => self.leak_gain + self.throttle_gain * plate_area(throttle),
                ThrottleKind::DriveByWire => {
                    // The control unit opens the plate as far as makes the manifold settle
                    // at the pedal's share of the way from shut to wide open.
                    let open = point.open * upstream / STANDARD_PRESSURE;
                    let target = point.closed + throttle * (open - point.closed);
                    let (b, _) = beta(target / upstream);
                    let flow = pumping * target * air_gain.recip();
                    (flow / (upstream * b.max(1e-6)))
                        .clamp(self.leak_gain, self.leak_gain + self.throttle_gain)
                }
            };
        let k = dt * rt / self.manifold_volume;
        let p = solve_manifold(s.manifold_pressure, upstream, gain, pumping, k);
        s.manifold_pressure = p;
        s.boost = upstream - ambient;
        s.air_flow = pumping * p;
        s.fuel_flow = if firing {
            s.air_flow / AIR_FUEL_RATIO
        } else {
            0.0
        };

        if let Some(t) = &self.turbo {
            // Shaft power: the turbine takes the exhaust the wastegate lets through, the
            // compressor works on the engine's air and bearings rub; the kinetic energy
            // settles exponentially at the flow of this step.
            let x = s.air_flow / t.reference_flow;
            s.wastegate = ((s.boost - t.max_boost) / t.band + 0.5).clamp(0.0, 1.0);
            let exhaust = if firing { 1.0 } else { COLD_EXHAUST };
            let through = ((1.0 - s.wastegate) * x).max(0.0);
            let turbine = exhaust
                * (1.0 + TURBO_FRICTION)
                * if t.exponent == 2.0 {
                    through * through * through
                } else {
                    through.powf(t.exponent + 1.0)
                };
            let load = x.max(0.0) + TURBO_FRICTION;
            let settled = turbine / load;
            s.turbo_spool = settled + (s.turbo_spool - settled) * (-load * dt / t.tau).exp();
        }

        let combustion = if firing {
            point.combustion * p * air.charge * s.heat.power
        } else {
            0.0
        };
        let pumping_loss = (ambient - p) * self.displacement / (4.0 * PI);
        let friction = point.friction * s.heat.friction;
        s.torque = combustion - pumping_loss - friction;
        self.thermal
            .heat(&mut s.heat, s.fuel_flow, friction * speed.abs(), dt);
        s.torque
    }

    /// Torque and boost (bar) of the engine held at `rpm` with the throttle wide open in
    /// standard air, once the manifold and the turbocharger have settled.
    pub fn full_load(&self, rpm: f64) -> (f64, f64) {
        let mut s = self.settled(rpm, 1.0);
        let speed = rpm / RPM_PER_RAD_S;
        for _ in 0..30_000 {
            self.step(&mut s, speed, 1.0, true, &Ambient::STANDARD, 1e-3);
        }
        (s.torque, s.boost / BAR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CarModel;

    fn gt3() -> EngineParams {
        CarModel::gt3().params.engine
    }

    fn turbo(e: &mut EngineParams) {
        e.turbo = Some(TurboParams {
            max_boost: 0.8,
            reference_rpm: 4000.0,
            spool_time: 0.4,
            flow_exponent: 2.0,
            wastegate_band: 0.1,
            blow_off_valve: true,
        });
    }

    /// Runs the engine at `rpm` for `seconds` and returns its state.
    fn run(m: &EngineModel, s: &mut EngineState, rpm: f64, throttle: f64, seconds: f64) {
        let firing = throttle > 0.0;
        for _ in 0..(seconds * 1000.0) as usize {
            m.step(
                s,
                rpm / RPM_PER_RAD_S,
                throttle,
                firing,
                &Ambient::STANDARD,
                1e-3,
            );
        }
    }

    #[test]
    fn the_motor_decay_matches_its_time_constant() {
        assert!((MOTOR_DECAY_PER_STEP - (-DT / THROTTLE_MOTOR_TIME).exp()).abs() < 1e-15);
    }

    #[test]
    fn steady_state_reproduces_the_torque_curves() {
        let e = gt3();
        let m = EngineModel::new(&e);
        for rpm in [2000.0, 4000.0, 6000.0, 8000.0, 9000.0] {
            let (torque, _) = m.full_load(rpm);
            let want = lookup(&e.torque_curve, rpm);
            assert!(
                (torque - want).abs() < 0.01 * want,
                "{rpm}: {torque} vs {want}"
            );
            let mut s = m.settled(rpm, 0.0);
            run(&m, &mut s, rpm, 0.0, 2.0);
            let drag = lookup(&e.drag_curve, rpm);
            assert!(
                s.torque <= -drag + 0.5 && s.torque > -drag * 1.6,
                "{rpm}: overrun {} vs drag {drag}",
                s.torque
            );
        }
    }

    #[test]
    fn drive_by_wire_torque_follows_the_pedal_and_a_cable_throttle_leads_it() {
        let mut e = gt3();
        let dbw = EngineModel::new(&e);
        e.throttle = ThrottleKind::Cable;
        let cable = EngineModel::new(&e);
        let rpm = 3000.0;
        let at = |m: &EngineModel, throttle| {
            let mut s = m.settled(rpm, 0.0);
            run(m, &mut s, rpm, throttle, 2.0);
            s.torque
        };
        let full = at(&dbw, 1.0);
        let (quarter, half, three_quarters) = (at(&dbw, 0.25), at(&dbw, 0.5), at(&dbw, 0.75));
        assert!(quarter < half && half < three_quarters && three_quarters < full);
        assert!(
            (half - 0.5 * (quarter + three_quarters)).abs() < 0.01 * full,
            "{quarter} / {half} / {three_quarters}"
        );
        // A butterfly plate uncovers most of the flow a low speed needs early in its travel.
        assert!(at(&cable, 0.3) > at(&dbw, 0.3) + 0.1 * full);
    }

    #[test]
    fn the_manifold_fills_in_tens_of_milliseconds() {
        let m = EngineModel::new(&gt3());
        let rpm = 5000.0;
        let mut s = m.settled(rpm, 0.0);
        run(&m, &mut s, rpm, 1.0, 0.005);
        let early = s.torque;
        run(&m, &mut s, rpm, 1.0, 0.3);
        let (full, _) = m.full_load(rpm);
        assert!(early < 0.7 * full, "{early} of {full} after 5 ms");
        assert!(s.torque > 0.98 * full, "{} of {full} after 0.3 s", s.torque);
    }

    #[test]
    fn a_turbo_spools_up_holds_its_boost_and_lags() {
        let mut e = gt3();
        turbo(&mut e);
        let m = EngineModel::new(&e);
        let na = EngineModel::new(&gt3());
        // Above the reference speed the wastegate holds the boost near its setpoint.
        let (torque, boost) = m.full_load(6000.0);
        assert!((0.7..0.9).contains(&boost), "boost {boost}");
        assert!(torque > 1.6 * na.full_load(6000.0).0, "{torque}");
        // Well below it the exhaust cannot spin the turbo up to full boost.
        let (_, low) = m.full_load(2000.0);
        assert!(low < 0.4, "boost {low} at 2000 rpm");
        // Floored from part throttle, the boost builds over the spool time.
        let mut s = m.settled(5000.0, 0.2);
        run(&m, &mut s, 5000.0, 0.2, 3.0);
        run(&m, &mut s, 5000.0, 1.0, 0.1);
        let lagging = s.boost / BAR;
        run(&m, &mut s, 5000.0, 1.0, 3.0);
        assert!(
            lagging < 0.6 * s.boost / BAR,
            "{lagging} then {}",
            s.boost / BAR
        );
        // Lifting opens the blow-off valve, and the shaft keeps turning.
        let spool = s.turbo_spool;
        run(&m, &mut s, 5000.0, 0.0, 0.3);
        assert!(s.boost.abs() < 1.0, "boost {} after the lift", s.boost);
        assert!(s.turbo_spool > 0.5 * spool);
    }

    #[test]
    fn thin_air_weakens_a_naturally_aspirated_engine() {
        let m = EngineModel::new(&gt3());
        let rpm = 6000.0;
        let torque = |air: Ambient| {
            let mut s = m.settled(rpm, 1.0);
            for _ in 0..2000 {
                m.step(&mut s, rpm / RPM_PER_RAD_S, 1.0, true, &air, 1e-3);
            }
            s.torque
        };
        let sea = torque(Ambient::STANDARD);
        let high = torque(Ambient {
            pressure: 0.8 * STANDARD_PRESSURE,
            ..Ambient::STANDARD
        });
        assert!(high < 0.85 * sea && high > 0.7 * sea, "{high} vs {sea}");
    }
}
