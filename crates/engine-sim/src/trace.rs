//! One engine cycle recorded in detail: a cylinder's pressure, volume, temperature and
//! burn, its valves' lifts and flows, and the pressure along chosen pipes — the p–V and
//! p–θ diagrams and the wave plots of an engine test cell.

use serde::{Deserialize, Serialize};

use crate::build::Build;
use crate::dyno::{DynoOptions, point};
use crate::model::{Controls, Load, Opening, Port};

/// A cycle of one cylinder, every crank degree.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CycleTrace {
    pub rpm: f64,
    pub cylinder: usize,
    /// Degrees from the firing TDC, 0..720.
    pub deg: Vec<f64>,
    /// Pa, m³, K.
    pub pressure: Vec<f64>,
    pub volume: Vec<f64>,
    pub temperature: Vec<f64>,
    /// Burned-gas share of the charge.
    pub burned: Vec<f64>,
    /// Valve lifts, m.
    pub intake_lift: Vec<f64>,
    pub exhaust_lift: Vec<f64>,
    /// Mass flow into the cylinder through its intake and exhaust valves, kg/s.
    pub intake_flow: Vec<f64>,
    pub exhaust_flow: Vec<f64>,
    /// For each recorded pipe: its name, cell centres (m from its start) and the pressure
    /// of each cell at each degree.
    pub pipes: Vec<PipeTrace>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PipeTrace {
    pub name: String,
    pub x: Vec<f64>,
    pub pressure: Vec<Vec<f64>>,
}

/// Runs the engine at a speed until it settles, then records a cycle of `cylinder` (from
/// 0) and the pipes whose names contain one of `pipes`.
pub fn cycle(
    b: &Build,
    rpm: f64,
    throttle: f64,
    cylinder: usize,
    pipes: &[String],
) -> Result<CycleTrace, String> {
    let (mut m, _) = b.build()?;
    if cylinder >= m.cylinders.len() {
        return Err(format!("the engine has {} cylinders", m.cylinders.len()));
    }
    point(
        &mut m,
        rpm,
        &DynoOptions {
            throttle,
            ..Default::default()
        },
    );
    let c = Controls {
        pedal: throttle,
        load: Load::Speed(rpm),
        ..Default::default()
    };
    let chosen: Vec<usize> = (0..m.pipes.len())
        .filter(|&i| pipes.iter().any(|p| m.pipes[i].name.contains(p.as_str())))
        .collect();
    let valve = |intake: bool| {
        m.links.iter().position(|l| {
            matches!(l.opening, Opening::Valves { cylinder: cc, intake: i } if cc == cylinder && i == intake)
        })
    };
    let (iv, ev) = (valve(true), valve(false));
    let mut t = CycleTrace {
        rpm,
        cylinder,
        pipes: chosen
            .iter()
            .map(|&i| {
                let p = &m.pipes[i];
                PipeTrace {
                    name: p.name.clone(),
                    x: (0..p.cells()).map(|k| (k as f64 + 0.5) * p.dx).collect(),
                    pressure: Vec::new(),
                }
            })
            .collect(),
        ..Default::default()
    };
    // Start at the firing TDC.
    let mut last = m.cycle_deg(cylinder);
    let mut started = false;
    let mut next = 0.0;
    for _ in 0..(m.quality.rate() as f64 * 240.0 / rpm) as usize * 4 {
        m.step(&c);
        let deg = m.cycle_deg(cylinder);
        if !started {
            if deg < last {
                started = true;
            } else {
                last = deg;
                continue;
            }
        }
        if deg < last && next > 360.0 {
            break;
        }
        last = deg;
        if deg + 1e-9 < next {
            continue;
        }
        next = deg.floor() + 1.0;
        let g = &m.lumps[m.cylinders[cylinder].gas];
        let flow_into = |li: Option<usize>| {
            li.map_or(0.0, |li| {
                let l = &m.links[li];
                if matches!(l.b, Port::Cylinder(_)) {
                    l.flow
                } else {
                    -l.flow
                }
            })
        };
        t.deg.push(deg);
        t.pressure.push(g.p);
        t.volume.push(g.volume);
        t.temperature.push(g.t);
        t.burned.push(g.y);
        t.intake_lift.push(m.intake_valves.lift_at(deg));
        t.exhaust_lift.push(m.exhaust_valves.lift_at(deg));
        t.intake_flow.push(flow_into(iv));
        t.exhaust_flow.push(flow_into(ev));
        for (k, &pi) in chosen.iter().enumerate() {
            t.pipes[k]
                .pressure
                .push(m.pipes[pi].s.iter().map(|s| s.w.p).collect());
        }
    }
    Ok(t)
}
