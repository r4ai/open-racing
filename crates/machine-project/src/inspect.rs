//! Facts about parts and machines, for `machinectl info` (and its `--json`).

use serde::Serialize;

use open_racing_engine_sim::build::network_terminals;
use open_racing_engine_sim::cam::Valvetrain;
use open_racing_engine_sim::crank::{SliderCrank, firing_intervals};
use open_racing_engine_sim::{EngineSpec, Network};

use crate::library::Library;
use crate::mass::{MachineMass, part_props};
use crate::part::{Design, Part, PartRef};

#[derive(Clone, Debug, Serialize)]
pub struct PartSummary {
    pub part: String,
    pub mass: f64,
    pub centre: [f64; 3],
    pub shapes: usize,
    pub mounts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<EngineSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkSummary>,
    pub used_by: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct EngineSummary {
    /// l.
    pub displacement: f64,
    pub cylinders: usize,
    pub bore_stroke: (f64, f64),
    pub compression_ratio: f64,
    pub firing_order: Vec<usize>,
    /// Degrees between firings, in firing order.
    pub firing_intervals: Vec<f64>,
    /// Valve events in the usual terms: intake opens before TDC, closes after BDC; exhaust
    /// opens before BDC, closes after TDC; overlap. Degrees.
    pub intake_opens_btdc: f64,
    pub intake_closes_abdc: f64,
    pub exhaust_opens_bbdc: f64,
    pub exhaust_closes_atdc: f64,
    pub overlap: f64,
    pub limiter_rpm: f64,
    pub idle_rpm: f64,
    /// Mean piston speed at the limiter, m/s.
    pub piston_speed_at_limiter: f64,
    pub terminals: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct NetworkSummary {
    pub pipes: usize,
    /// m.
    pub pipe_length: f64,
    /// l.
    pub volume: f64,
    pub volumes: Vec<(String, f64)>,
    pub terminals: Vec<String>,
    pub mouths: Vec<String>,
}

pub fn engine(e: &EngineSpec) -> EngineSummary {
    let k = SliderCrank::new(e);
    let iv = Valvetrain::new(&e.intake, true);
    let ev = Valvetrain::new(&e.exhaust, false);
    let ivo = 360.0 - iv.open_deg;
    let ivc = iv.close_deg() - 540.0;
    let evo = 180.0 - ev.open_deg;
    let evc = ev.close_deg() - 360.0;
    let n = e.layout.cylinders.len();
    EngineSummary {
        displacement: k.swept() * n as f64 * 1e3,
        cylinders: n,
        bore_stroke: (e.bore, e.stroke),
        compression_ratio: e.compression_ratio,
        firing_order: e.layout.firing_order.clone(),
        firing_intervals: firing_intervals(&e.layout).unwrap_or_default(),
        intake_opens_btdc: ivo,
        intake_closes_abdc: ivc,
        exhaust_opens_bbdc: evo,
        exhaust_closes_atdc: evc,
        overlap: ivo + evc,
        limiter_rpm: e.ecu.limiter_rpm,
        idle_rpm: e.ecu.idle_rpm,
        piston_speed_at_limiter: 2.0 * e.stroke * e.ecu.limiter_rpm / 60.0,
        terminals: open_racing_engine_sim::build::engine_terminals(e),
    }
}

pub fn network(n: &Network) -> NetworkSummary {
    let pipe_volume: f64 = n
        .pipes
        .iter()
        .map(|p| {
            let d = p.diameter.iter().map(|d| d.1).sum::<f64>() / p.diameter.len().max(1) as f64;
            std::f64::consts::PI * 0.25 * d * d * p.length
        })
        .sum();
    NetworkSummary {
        pipes: n.pipes.len(),
        pipe_length: n.pipes.iter().map(|p| p.length).sum(),
        volume: (pipe_volume + n.volumes.iter().map(|v| v.volume).sum::<f64>()) * 1e3,
        volumes: n
            .volumes
            .iter()
            .map(|v| (v.name.clone(), v.volume * 1e3))
            .collect(),
        terminals: network_terminals(n),
        mouths: n
            .pipes
            .iter()
            .filter(|p| {
                matches!(p.a, open_racing_engine_sim::spec::End::Ambient { .. })
                    || matches!(p.b, open_racing_engine_sim::spec::End::Ambient { .. })
            })
            .map(|p| p.name.clone())
            .collect(),
    }
}

pub fn part(lib: &Library, r: &PartRef, p: &Part) -> PartSummary {
    let m = part_props(p);
    let s = r.to_string();
    PartSummary {
        part: s.clone(),
        mass: m.mass,
        centre: m.centre.to_array(),
        shapes: p.physical.shapes.len(),
        mounts: p.physical.mounts.iter().map(|m| m.name.clone()).collect(),
        engine: match &p.design {
            Design::Engine(e) => Some(engine(&e.spec)),
            _ => None,
        },
        network: match &p.design {
            Design::Intake(i) => Some(network(&i.network)),
            Design::Exhaust(x) => Some(network(&x.network)),
            _ => None,
        },
        used_by: lib
            .machines
            .iter()
            .filter(|(_, m)| m.parts.iter().any(|x| x.part == s))
            .map(|(n, _)| n.clone())
            .collect(),
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct MachineSummary {
    pub machine: String,
    pub mass: MachineMass,
    /// CG height above the ground, m.
    pub cg_height: f64,
    pub parts: Vec<(String, String)>,
    /// Joined gas terminals, the machine's and those joined by name.
    pub gas: Vec<(String, String)>,
    pub warnings: Vec<String>,
}

pub fn machine(lib: &Library, name: &str) -> Result<MachineSummary, String> {
    let m = lib.machine(name)?;
    let asm = crate::assembly::assemble(lib, m)?;
    let mass = crate::mass::machine_mass(lib, m, &asm)?;
    let mut gas = m.gas.clone();
    if let Some(b) = crate::engines::for_machine(lib, m, open_racing_engine_sim::Quality::Draft)? {
        let (model, _) = b.build()?;
        gas = model
            .links
            .iter()
            .filter_map(|l| match (l.a, l.b) {
                (
                    open_racing_engine_sim::model::Port::Pipe(a, _),
                    open_racing_engine_sim::model::Port::Pipe(b, _),
                ) => Some((model.pipes[a].name.clone(), model.pipes[b].name.clone())),
                _ => None,
            })
            .collect();
    }
    Ok(MachineSummary {
        machine: name.into(),
        cg_height: mass.centre[2],
        mass,
        parts: m
            .parts
            .iter()
            .map(|p| (p.name.clone(), p.part.clone()))
            .collect(),
        gas,
        warnings: crate::validate::machine_warnings(m, lib),
    })
}
