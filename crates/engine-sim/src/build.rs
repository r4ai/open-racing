//! Assembling a model from an engine and the intake and exhaust systems joined to it.

use crate::model::{Ambient, Link, Lump, Model, Mouth, Opening, Port, Quality};
use crate::pipe::{End, Pipe, PipeGeometry, Prim};
use crate::spec::{self, EngineSpec, Network, Restriction};
use crate::turbo::Turbo;

/// An intake or exhaust system fitted to the engine, under an instance name (terminals
/// are `name:terminal`), at a position in the engine's frame (for the sound).
#[derive(Clone, Debug, PartialEq)]
pub struct System {
    pub name: String,
    pub network: Network,
    pub offset: [f64; 3],
}

/// Everything a model is built from.
#[derive(Clone, Debug, PartialEq)]
pub struct Build {
    pub engine: EngineSpec,
    pub systems: Vec<System>,
    /// Pairs of joined terminals, `instance:terminal` (the engine's instance is `engine`).
    /// Engine terminals not listed join the one system terminal of the same name, if there
    /// is exactly one.
    pub connections: Vec<(String, String)>,
    pub quality: Quality,
    pub ambient: Ambient,
}

impl Build {
    pub fn new(engine: &EngineSpec) -> Self {
        Self {
            engine: engine.clone(),
            systems: Vec::new(),
            connections: Vec::new(),
            quality: Quality::Normal,
            ambient: Ambient::default(),
        }
    }

    pub fn system(mut self, name: &str, network: &Network) -> Self {
        self.systems.push(System {
            name: name.into(),
            network: network.clone(),
            offset: [0.0; 3],
        });
        self
    }

    pub fn quality(mut self, q: Quality) -> Self {
        self.quality = q;
        self
    }

    /// Builds the model; also returns warnings (unconnected terminals and the like).
    pub fn build(&self) -> Result<(Model, Vec<String>), String> {
        build(self)
    }
}

/// Every terminal of the engine: its two ports per cylinder.
pub fn engine_terminals(e: &EngineSpec) -> Vec<String> {
    let n = e.layout.cylinders.len();
    (1..=n)
        .map(|i| format!("intake.{i}"))
        .chain((1..=n).map(|i| format!("exhaust.{i}")))
        .collect()
}

/// Every terminal of a network.
pub fn network_terminals(n: &Network) -> Vec<String> {
    let mut t = Vec::new();
    for p in &n.pipes {
        for end in [&p.a, &p.b] {
            if let spec::End::Terminal(name) = end {
                t.push(name.clone());
            }
        }
    }
    t
}

fn opening(r: &Option<Restriction>) -> Opening {
    match r {
        None => Opening::Open,
        Some(Restriction::Fixed { cd, diameter }) => {
            Opening::Fixed(cd * std::f64::consts::PI * 0.25 * diameter * diameter)
        }
        // Only between volumes, where the build makes it.
        Some(Restriction::BlowOff { diameter, .. }) => {
            Opening::Fixed(0.8 * std::f64::consts::PI * 0.25 * diameter * diameter)
        }
        Some(Restriction::Throttle {
            bore,
            shaft,
            closed_angle_deg,
        }) => Opening::Throttle {
            bore: *bore,
            shaft: *shaft,
            closed_angle_deg: *closed_angle_deg,
        },
    }
}

fn add(v: [f64; 3], w: [f64; 3]) -> [f64; 3] {
    [v[0] + w[0], v[1] + w[1], v[2] + w[2]]
}

pub fn build(b: &Build) -> Result<(Model, Vec<String>), String> {
    let e = &b.engine;
    validate_engine(e)?;
    let mut m = Model::empty(e.clone(), b.quality, b.ambient)?;
    let mut warnings = Vec::new();
    let amb = b.ambient;
    let fill = Prim {
        rho: amb.pressure / (crate::gas::R_AIR * amb.temperature),
        u: 0.0,
        p: amb.pressure,
        y: 0.0,
        f: 0.0,
    };
    let cell = b.quality.cell_length();
    let geometry = |name: String,
                    length: f64,
                    d: &[(f64, f64)],
                    wall: f64,
                    rough: f64,
                    friction: f64,
                    heat: f64| {
        PipeGeometry {
            name,
            length,
            diameter: d.to_vec(),
            cell_length: cell,
            wall_temperature: wall,
            roughness: rough,
            friction_scale: friction,
            heat_scale: heat,
        }
    };
    let mut compressors = Vec::new();
    let mut turbines = Vec::new();
    // Terminal name → pipe end.
    let mut terminals: Vec<(String, usize, End)> = Vec::new();
    // Engine ports.
    for (side, head, intake) in [("intake", &e.intake, true), ("exhaust", &e.exhaust, false)] {
        for c in 0..m.cylinders.len() {
            let port = &head.port;
            let g = geometry(
                format!("engine:{side}.port.{}", c + 1),
                port.length,
                &port.diameter,
                port.wall_temperature,
                2e-4,
                1.0,
                1.0,
            );
            m.pipes.push(Pipe::new(&g, &m.gas, fill));
            let p = m.pipes.len() - 1;
            m.links.push(Link::new(
                Port::Pipe(p, End::Start),
                Port::Cylinder(c),
                Opening::Valves {
                    cylinder: c,
                    intake,
                },
            ));
            terminals.push((format!("engine:{side}.{}", c + 1), p, End::End));
        }
    }
    // Systems.
    for sys in &b.systems {
        let net = &sys.network;
        let base = m.lumps.len();
        let mut joints: Vec<(String, usize, End)> = Vec::new();
        for v in &net.volumes {
            if v.volume <= 0.0 {
                return Err(format!(
                    "{}: volume \"{}\" must be larger than zero",
                    sys.name, v.name
                ));
            }
            m.lumps.push(Lump::new(
                &m.gas,
                format!("{}:{}", sys.name, v.name),
                v.volume,
                amb.pressure,
                amb.temperature,
                0.0,
                v.wall_temperature,
            ));
        }
        let volume = |name: &str| -> Result<usize, String> {
            net.volumes
                .iter()
                .position(|v| v.name == name)
                .map(|i| base + i)
                .ok_or_else(|| format!("{}: no volume \"{name}\"", sys.name))
        };
        for ps in &net.pipes {
            if ps.length <= 0.0 || ps.diameter.is_empty() || ps.diameter.iter().any(|d| d.1 <= 0.0)
            {
                return Err(format!(
                    "{}: pipe \"{}\" needs a length and diameters",
                    sys.name, ps.name
                ));
            }
            let g = geometry(
                format!("{}:{}", sys.name, ps.name),
                ps.length,
                &ps.diameter,
                ps.wall_temperature,
                ps.roughness,
                ps.friction,
                ps.heat,
            );
            m.pipes.push(Pipe::new(&g, &m.gas, fill));
            let p = m.pipes.len() - 1;
            for (end, which) in [(&ps.a, End::Start), (&ps.b, End::End)] {
                let here = Port::Pipe(p, which);
                match end {
                    spec::End::Closed => m.links.push(Link::new(here, Port::Closed, Opening::Open)),
                    spec::End::Volume { name, restriction } => m.links.push(Link::new(
                        here,
                        Port::Volume(volume(name)?),
                        opening(restriction),
                    )),
                    spec::End::Ambient { at, restriction } => {
                        m.links
                            .push(Link::new(here, Port::Ambient, opening(restriction)));
                        m.mouths.push(Mouth::new(
                            format!("{}:{}", sys.name, ps.name),
                            m.links.len() - 1,
                            add(sys.offset, *at),
                            m.pipes[p].end_area(which),
                        ));
                    }
                    spec::End::Terminal(t) => {
                        terminals.push((format!("{}:{t}", sys.name), p, which))
                    }
                    spec::End::Join(j) => joints.push((j.clone(), p, which)),
                }
            }
        }
        joints.sort_by(|a, b| a.0.cmp(&b.0));
        let mut k = 0;
        while k < joints.len() {
            let same = joints[k..]
                .iter()
                .take_while(|j| j.0 == joints[k].0)
                .count();
            if same != 2 {
                return Err(format!(
                    "{}: joint \"{}\" must join exactly two pipe ends, not {same}",
                    sys.name, joints[k].0
                ));
            }
            m.links.push(Link::new(
                Port::Pipe(joints[k].1, joints[k].2),
                Port::Pipe(joints[k + 1].1, joints[k + 1].2),
                Opening::Open,
            ));
            k += 2;
        }
        for o in &net.orifices {
            let port = |n: &str| -> Result<Port, String> {
                if n == "ambient" {
                    Ok(Port::Ambient)
                } else {
                    volume(n).map(Port::Volume)
                }
            };
            let (a, b) = (port(&o.between.0)?, port(&o.between.1)?);
            let (op, area) = match &o.restriction {
                Restriction::BlowOff {
                    diameter,
                    reference,
                    opens,
                    span,
                } => {
                    let area = 0.8 * std::f64::consts::PI * 0.25 * diameter * diameter;
                    let op = Opening::BlowOff {
                        area,
                        reference: volume(reference)?,
                        opens: *opens,
                        span: span.max(1.0),
                    };
                    (op, area)
                }
                r => {
                    let op = opening(&Some(r.clone()));
                    let area = match op {
                        Opening::Fixed(a) => a,
                        _ => 1e-4,
                    };
                    (op, area)
                }
            };
            m.links.push(Link::new(a, b, op));
            if a == Port::Ambient || b == Port::Ambient {
                m.mouths.push(Mouth::new(
                    format!("{}:{}", sys.name, o.name),
                    m.links.len() - 1,
                    add(sys.offset, o.at),
                    area,
                ));
            }
        }
        for c in &net.compressors {
            compressors.push((
                c.clone(),
                [volume(&c.inlet)?, volume(&c.outlet)?],
                add(sys.offset, c.at),
            ));
        }
        for t in &net.turbines {
            turbines.push((t.clone(), [volume(&t.inlet)?, volume(&t.outlet)?]));
        }
    }
    // Turbochargers: the compressor and the turbine on each shaft.
    let mut shafts: Vec<String> = compressors
        .iter()
        .map(|c| c.0.shaft.clone())
        .chain(turbines.iter().map(|t| t.0.shaft.clone()))
        .collect();
    shafts.sort();
    shafts.dedup();
    for shaft in shafts {
        if compressors.iter().filter(|c| c.0.shaft == shaft).count() > 1
            || turbines.iter().filter(|t| t.0.shaft == shaft).count() > 1
        {
            return Err(format!(
                "shaft {shaft} carries more than one compressor or turbine"
            ));
        }
        let c = compressors
            .iter()
            .position(|c| c.0.shaft == shaft)
            .map(|k| compressors.remove(k));
        let t = turbines
            .iter()
            .position(|t| t.0.shaft == shaft)
            .map(|k| turbines.remove(k));
        if let Some((c, ..)) = &c {
            let ok = c.wheel > 0.0
                && c.inducer > 0.0
                && c.inducer < c.wheel
                && c.head > 0.0
                && (0.0..1.0).contains(&c.shutoff)
                && c.surge_flow > 0.0
                && c.choke_flow > c.surge_flow
                && c.efficiency > 0.0
                && c.efficiency <= 1.0
                && c.duct_length > 0.0;
            if !ok {
                return Err(format!(
                    "compressor {}: sizes and efficiency must be positive, the efficiency                      at most 1, the inducer smaller than the wheel, the shut-off head                      below 1 and the surge flow below the choke flow",
                    c.name
                ));
            }
        } else {
            warnings.push(format!(
                "shaft {shaft} has no compressor: its turbine spins free"
            ));
        }
        if let Some((t, tp)) = &t {
            if !(t.wheel > 0.0 && t.area > 0.0 && t.inertia > 0.0)
                || !(t.efficiency > 0.0 && t.efficiency <= 1.0)
            {
                return Err(format!(
                    "turbine {}: size, area, inertia and efficiency must be positive,                      the efficiency at most 1",
                    t.name
                ));
            }
            if let Some(w) = &t.wastegate {
                let area = 0.8 * std::f64::consts::PI * 0.25 * w.diameter * w.diameter;
                m.links.push(Link::new(
                    Port::Volume(tp[0]),
                    Port::Volume(tp[1]),
                    Opening::Wastegate {
                        turbo: m.turbos.len(),
                        area,
                    },
                ));
            }
        } else {
            warnings.push(format!(
                "shaft {shaft} has no turbine: its compressor spins free"
            ));
        }
        m.turbos.push(Turbo::new(shaft, c, t));
    }
    // Joins.
    let mut pairs: Vec<(String, String)> = b.connections.clone();
    let named: Vec<String> = terminals.iter().map(|t| t.0.clone()).collect();
    for t in engine_terminals(e) {
        let full = format!("engine:{t}");
        if pairs.iter().any(|(x, y)| *x == full || *y == full) {
            continue;
        }
        let matches: Vec<&String> = named
            .iter()
            .filter(|n| {
                !n.starts_with("engine:") && n.split_once(':').is_some_and(|(_, bare)| bare == t)
            })
            .collect();
        if matches.len() == 1 {
            pairs.push((full, matches[0].clone()));
        } else if matches.len() > 1 {
            return Err(format!(
                "engine port {t} matches several terminals ({}); connect it explicitly",
                matches
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    let mut used = vec![false; terminals.len()];
    for (x, y) in &pairs {
        let find = |n: &str| {
            terminals
                .iter()
                .position(|t| t.0 == *n)
                .ok_or_else(|| format!("no terminal \"{n}\""))
        };
        let (i, j) = (find(x)?, find(y)?);
        if i == j || used[i] || used[j] {
            return Err(format!("terminal joined twice: {x} – {y}"));
        }
        used[i] = true;
        used[j] = true;
        m.links.push(Link::new(
            Port::Pipe(terminals[i].1, terminals[i].2),
            Port::Pipe(terminals[j].1, terminals[j].2),
            Opening::Open,
        ));
    }
    for (k, (name, p, end)) in terminals.iter().enumerate() {
        if used[k] {
            continue;
        }
        if name.starts_with("engine:") {
            // An open port.
            m.links.push(Link::new(
                Port::Pipe(*p, *end),
                Port::Ambient,
                Opening::Open,
            ));
            m.mouths.push(Mouth::new(
                name.clone(),
                m.links.len() - 1,
                [0.0; 3],
                m.pipes[*p].end_area(*end),
            ));
            warnings.push(format!("{name} opens straight to the air"));
        } else {
            m.links
                .push(Link::new(Port::Pipe(*p, *end), Port::Closed, Opening::Open));
            warnings.push(format!("{name} is not connected: its pipe is closed there"));
        }
    }
    m.set_crank(0.0, 0.0);
    Ok((m, warnings))
}

/// Checks an engine's numbers.
pub fn validate_engine(e: &EngineSpec) -> Result<(), String> {
    let pos = |v: f64, what: &str| {
        if v > 0.0 && v.is_finite() {
            Ok(())
        } else {
            Err(format!("{what} must be positive"))
        }
    };
    pos(e.bore, "bore")?;
    pos(e.stroke, "stroke")?;
    if e.rod <= 0.5 * e.stroke + e.pin_offset.abs() {
        return Err("the rod must be longer than the crank radius".into());
    }
    if e.compression_ratio <= 1.0 {
        return Err("the compression ratio must be above 1".into());
    }
    for (side, h) in [("intake", &e.intake), ("exhaust", &e.exhaust)] {
        pos(h.cam.lift, &format!("{side} cam lift"))?;
        if !(h.cam.duration_deg > 0.0 && h.cam.duration_deg < 720.0) {
            return Err(format!("{side} cam duration must be between 0 and 720°"));
        }
        pos(h.valves.diameter, &format!("{side} valve diameter"))?;
        if h.valves.count == 0 {
            return Err(format!("{side}: at least one valve"));
        }
        if !crate::table::ascending(&h.valves.cd) {
            return Err(format!("{side} valve Cd table must ascend in L/D"));
        }
        pos(h.port.length, &format!("{side} port length"))?;
    }
    if !e.ecu.spark_deg.is_valid() || !e.ecu.lambda.is_valid() {
        return Err("ECU maps need ascending rpm and load axes and a value for each".into());
    }
    if e.ecu.limiter_rpm <= e.ecu.idle_rpm {
        return Err("the limiter must be above idle".into());
    }
    crate::crank::firing_angles(&e.layout)?;
    Ok(())
}
