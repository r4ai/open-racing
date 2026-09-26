//! A simulated engine dynamometer: the engine held at a speed until its cycles repeat,
//! and the figures of the last cycle.
//!
//! A sweep runs each speed as its own model, in parallel. Motoring (fuel cut) gives the
//! torque it takes to turn the engine over: friction plus pumping, what the game calls the
//! engine's drag curve.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::build::Build;
use crate::model::{Controls, Load, Model, Quality};

/// How to run a point.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DynoOptions {
    /// Accelerator pedal, 0..1.
    pub throttle: f64,
    /// Fire the engine (false: motoring, fuel cut).
    pub firing: bool,
    pub min_cycles: u32,
    pub max_cycles: u32,
    /// Relative change of the indicated work and trapped air between cycles below which
    /// the point has converged.
    pub tolerance: f64,
}

impl Default for DynoOptions {
    fn default() -> Self {
        Self {
            throttle: 1.0,
            firing: true,
            min_cycles: 6,
            max_cycles: 40,
            tolerance: 0.003,
        }
    }
}

/// The figures of one speed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DynoPoint {
    pub rpm: f64,
    /// At the flywheel, N·m.
    pub brake_torque: f64,
    /// Of the gas on the pistons (net indicated), N·m.
    pub indicated_torque: f64,
    /// W.
    pub power: f64,
    /// Mean effective pressures, Pa: brake, net indicated, pumping (negative: a loss) and
    /// friction.
    pub bmep: f64,
    pub imep: f64,
    pub pmep: f64,
    pub fmep: f64,
    /// Air trapped over the air the swept volume holds at ambient density.
    pub volumetric_efficiency: f64,
    /// kg/s.
    pub air_flow: f64,
    pub fuel_flow: f64,
    /// Brake specific fuel consumption, g/kWh.
    pub bsfc: f64,
    pub lambda: f64,
    /// Spark advance, degrees before TDC.
    pub spark_deg: f64,
    /// Highest cylinder pressure, Pa, and where, degrees after TDC.
    pub peak_pressure: f64,
    pub peak_deg: f64,
    /// Knock integral (≥ 1: the end gas autoignites before the flame reaches it).
    pub knock: f64,
    /// Burned gas left in the cylinders at inlet valve closing, share of the charge.
    pub residual: f64,
    /// Heat lost to the cylinder walls over the fuel's heat.
    pub wall_heat_share: f64,
    pub cycles: u32,
    pub converged: bool,
}

/// A whole sweep.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DynoRun {
    pub quality: Quality,
    pub throttle: f64,
    pub points: Vec<DynoPoint>,
    /// Torque to motor the engine with its throttle shut and the fuel cut, N·m (positive).
    pub drag: Vec<(f64, f64)>,
}

impl DynoRun {
    pub fn peak_torque(&self) -> Option<&DynoPoint> {
        self.points
            .iter()
            .max_by(|a, b| a.brake_torque.total_cmp(&b.brake_torque))
    }

    pub fn peak_power(&self) -> Option<&DynoPoint> {
        self.points
            .iter()
            .max_by(|a, b| a.power.total_cmp(&b.power))
    }
}

/// Speeds from `from` to `to` every `step`, rpm.
pub fn speeds(from: f64, to: f64, step: f64) -> Vec<f64> {
    let n = ((to - from) / step).floor().max(0.0) as usize;
    (0..=n).map(|i| from + i as f64 * step).collect()
}

/// Runs a model at a speed until it repeats, and measures its last cycle.
pub fn point(m: &mut Model, rpm: f64, o: &DynoOptions) -> DynoPoint {
    m.set_crank(m.angle, rpm);
    let c = Controls {
        pedal: o.throttle,
        ignition: o.firing,
        starter: false,
        load: Load::Speed(rpm),
    };
    let cycle = 120.0 / rpm;
    let mut prev: Option<(f64, f64)> = None;
    let mut settled = 0;
    let mut cycles = 0;
    let mut converged = false;
    let (mut t_sum, mut f_sum);
    loop {
        let end = m.time + cycle;
        t_sum = 0.0;
        f_sum = 0.0;
        let mut dt_sum = 0.0;
        while m.time < end - 0.5 * m.dt {
            m.step(&c);
            t_sum += m.gas_torque * m.dt;
            f_sum += m.friction_torque * m.dt;
            dt_sum += m.dt;
        }
        t_sum /= dt_sum;
        f_sum /= dt_sum;
        cycles += 1;
        let work: f64 = m.cylinders.iter().map(|c| c.last.work).sum();
        let air: f64 = m.cylinders.iter().map(|c| c.last.intake_mass).sum();
        if let Some((w0, a0)) = prev {
            let dw = (work - w0).abs() / w0.abs().max(1e-3 * m.displacement() * 1e5);
            let da = (air - a0).abs() / a0.abs().max(1e-9);
            if dw < o.tolerance && da < o.tolerance {
                settled += 1;
            } else {
                settled = 0;
            }
        }
        prev = Some((work, air));
        if cycles >= o.min_cycles && settled >= 2 {
            converged = true;
            break;
        }
        if cycles >= o.max_cycles {
            break;
        }
    }
    measure(m, rpm, o, t_sum, f_sum, cycles, converged)
}

fn measure(
    m: &Model,
    rpm: f64,
    o: &DynoOptions,
    _gas_torque: f64,
    friction: f64,
    cycles: u32,
    converged: bool,
) -> DynoPoint {
    let vd = m.displacement();
    let n = m.cylinders.len() as f64;
    let per_cycle = rpm / 120.0;
    let sum = |f: &dyn Fn(&crate::model::CycleStats) -> f64| {
        m.cylinders.iter().map(|c| f(&c.last)).sum::<f64>()
    };
    let work = sum(&|s| s.work);
    let pump = sum(&|s| s.pump_work);
    let air = sum(&|s| s.intake_mass);
    let fuel = sum(&|s| s.fuel);
    let heat = sum(&|s| s.heat_released);
    let wall = sum(&|s| s.wall_heat);
    let (lhv, afr) = m.spec.combustion.fuel.properties();
    let _ = afr;
    let indicated_torque = work / (4.0 * std::f64::consts::PI);
    let brake_torque = indicated_torque - friction;
    let power = brake_torque * rpm * crate::model::RAD_PER_RPM;
    let rho = m.ambient.pressure / (crate::gas::R_AIR * m.ambient.temperature);
    let fuel_flow = fuel * per_cycle;
    let throttle = o.throttle;
    DynoPoint {
        rpm,
        brake_torque,
        indicated_torque,
        power,
        bmep: brake_torque * 4.0 * std::f64::consts::PI / vd,
        imep: work / vd,
        pmep: pump / vd,
        fmep: friction * 4.0 * std::f64::consts::PI / vd,
        volumetric_efficiency: air / (rho * vd),
        air_flow: air * per_cycle,
        fuel_flow,
        bsfc: if power > 0.0 {
            fuel_flow / power * 3.6e9
        } else {
            0.0
        },
        lambda: if o.firing {
            m.spec.ecu.lambda.at(rpm, throttle)
        } else {
            0.0
        },
        spark_deg: m.spec.ecu.spark_deg.at(rpm, throttle),
        peak_pressure: m
            .cylinders
            .iter()
            .map(|c| c.last.peak_pressure)
            .fold(0.0, f64::max),
        peak_deg: m.cylinders.iter().map(|c| c.last.peak_deg).sum::<f64>() / n,
        knock: m.cylinders.iter().map(|c| c.last.knock).fold(0.0, f64::max),
        residual: m.cylinders.iter().map(|c| c.last.residual).sum::<f64>() / n,
        wall_heat_share: if fuel > 0.0 { wall / (fuel * lhv) } else { 0.0 },
        cycles,
        converged: converged && heat.is_finite(),
    }
}

/// A full-load sweep and the motoring drag, each speed in parallel.
pub fn sweep(b: &Build, rpms: &[f64], throttle: f64) -> Result<DynoRun, String> {
    b.build()?;
    let fired = DynoOptions {
        throttle,
        ..Default::default()
    };
    let motored = DynoOptions {
        throttle: 0.0,
        firing: false,
        ..Default::default()
    };
    let jobs: Vec<(f64, bool)> = rpms.iter().flat_map(|&r| [(r, true), (r, false)]).collect();
    let results: Vec<(f64, bool, DynoPoint)> = jobs
        .par_iter()
        .map(|&(rpm, fire)| {
            let (mut m, _) = b.build().expect("checked above");
            let p = point(&mut m, rpm, if fire { &fired } else { &motored });
            (rpm, fire, p)
        })
        .collect();
    let mut run = DynoRun {
        quality: b.quality,
        throttle,
        ..Default::default()
    };
    for (rpm, fire, p) in results {
        if fire {
            run.points.push(p);
        } else {
            run.drag.push((rpm, -p.brake_torque));
        }
    }
    run.points.sort_by(|a, b| a.rpm.total_cmp(&b.rpm));
    run.drag.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(run)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::samples;

    /// The sample inline four at 3000 rpm, full load, lands in the ranges textbooks give for
    /// a naturally aspirated four-valve engine (Heywood, ch. 13 and 15).
    #[test]
    fn inline_four_is_plausible_at_full_load() {
        let (e, i, x) = (samples::i4(), samples::i4_intake(), samples::i4_exhaust());
        let b = Build::new(&e)
            .system("intake", &i)
            .system("exhaust", &x)
            .quality(Quality::Draft);
        let (mut m, w) = b.build().unwrap();
        assert!(w.is_empty(), "{w:?}");
        let p = point(&mut m, 3000.0, &DynoOptions::default());
        assert!(p.converged, "{p:?}");
        assert!((10.0e5..14.0e5).contains(&p.bmep), "bmep {}", p.bmep);
        assert!(
            (0.85..1.05).contains(&p.volumetric_efficiency),
            "ve {}",
            p.volumetric_efficiency
        );
        assert!((230.0..320.0).contains(&p.bsfc), "bsfc {}", p.bsfc);
        assert!(
            (50e5..90e5).contains(&p.peak_pressure),
            "pmax {}",
            p.peak_pressure
        );
        assert!((0.5e5..2.0e5).contains(&p.fmep), "fmep {}", p.fmep);
    }

    #[test]
    fn motoring_takes_torque() {
        let (e, i, x) = (samples::i4(), samples::i4_intake(), samples::i4_exhaust());
        let b = Build::new(&e)
            .system("intake", &i)
            .system("exhaust", &x)
            .quality(Quality::Draft);
        let (mut m, _) = b.build().unwrap();
        let o = DynoOptions {
            throttle: 0.0,
            firing: false,
            ..Default::default()
        };
        let p = point(&mut m, 4000.0, &o);
        // Friction and the pumping loss against a shut throttle.
        assert!(
            p.brake_torque < -10.0 && p.brake_torque > -60.0,
            "{}",
            p.brake_torque
        );
        assert!(p.pmep < -0.3e5, "{}", p.pmep);
    }
}
