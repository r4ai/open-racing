//! A running engine: its pipes, volumes and cylinders, stepped in time together.
//!
//! The step is fixed and locked to the audio clock (a quality's rate: 24, 48, 96 or
//! 192 kHz), so sound comes out at an integer ratio of 48 kHz without drifting; the cells
//! are sized for it. A step splits in two only when the flow is fast enough to break the
//! CFL condition.

use std::f64::consts::PI;

use crate::boundary::{
    Radiation, Reservoir, pipe_open_to_reservoir, pipe_radiating, pipe_to_pipe, pipe_to_reservoir,
    reservoir_to_reservoir,
};
use crate::cam::Valvetrain;
use crate::combustion::{self, Phase, Rng};
use crate::crank::{SliderCrank, firing_angles};
use crate::gas::Gas;
use crate::pipe::{End, Pipe};
use crate::spec::{EngineSpec, throttle_area};

/// Most parts a step is split into for the CFL condition.
const MAX_SPLIT: usize = 16;

/// rad/s per rpm.
pub const RAD_PER_RPM: f64 = PI / 30.0;

/// How finely the engine is simulated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Quality {
    /// 24 kHz, ≈ 60 mm cells: for playing in real time.
    Draft,
    /// 48 kHz, ≈ 30 mm cells: for the dyno and baking.
    #[default]
    Normal,
    /// 96 kHz, ≈ 15 mm cells: for recordings.
    High,
    /// 192 kHz, ≈ 7.5 mm cells.
    Ultra,
}

impl Quality {
    /// Solver steps per second.
    pub fn rate(self) -> u32 {
        match self {
            Quality::Draft => 24_000,
            Quality::Normal => 48_000,
            Quality::High => 96_000,
            Quality::Ultra => 192_000,
        }
    }

    /// Cell length the step allows with gas up to 1300 m/s (sound plus flow), m.
    pub fn cell_length(self) -> f64 {
        1300.0 / self.rate() as f64 / 0.9
    }
}

/// The air round the engine.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Ambient {
    /// Pa.
    pub pressure: f64,
    /// K.
    pub temperature: f64,
}

impl Default for Ambient {
    fn default() -> Self {
        Self {
            pressure: crate::gas::P_STANDARD,
            temperature: crate::gas::T_STANDARD,
        }
    }
}

/// What the engine drives.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Load {
    /// An ideal dynamometer holding the speed, rpm.
    Speed(f64),
    /// A flywheel added to the engine's own, kg·m², and a steady torque, N·m.
    Inertia { inertia: f64, torque: f64 },
}

/// The driver's and the test bench's inputs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Controls {
    /// Accelerator pedal, 0..1.
    pub pedal: f64,
    pub ignition: bool,
    pub starter: bool,
    pub load: Load,
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            pedal: 0.0,
            ignition: true,
            starter: false,
            load: Load::Inertia {
                inertia: 0.0,
                torque: 0.0,
            },
        }
    }
}

/// A 0D body of gas: a volume or a cylinder.
#[derive(Clone, Debug)]
pub struct Lump {
    pub name: String,
    /// m³.
    pub volume: f64,
    pub mass: f64,
    /// Internal energy, J.
    pub energy: f64,
    /// Burned-gas mass, kg.
    pub burned: f64,
    pub wall_temperature: f64,
    /// Whether it is a cylinder's (advanced with the crank, not with the volumes).
    pub cylinder: bool,
    // Derived.
    pub p: f64,
    pub t: f64,
    pub y: f64,
    // Flows in over the current step, per second.
    dm: f64,
    de: f64,
    dmy: f64,
}

impl Lump {
    pub fn new(gas: &Gas, name: String, volume: f64, p: f64, t: f64, y: f64, wall: f64) -> Self {
        let mass = p * volume / (gas.r(y) * t);
        let mut l = Self {
            name,
            volume,
            mass,
            energy: mass * gas.energy(t, y),
            burned: mass * y,
            wall_temperature: wall,
            cylinder: false,
            p,
            t,
            y,
            dm: 0.0,
            de: 0.0,
            dmy: 0.0,
        };
        l.derive(gas);
        l
    }

    pub fn derive(&mut self, gas: &Gas) {
        self.mass = self.mass.max(1e-12);
        self.burned = self.burned.clamp(0.0, self.mass);
        self.y = self.burned / self.mass;
        self.t = gas
            .temperature_near(self.energy / self.mass, self.y, self.t)
            .max(150.0);
        self.p = self.mass * gas.r(self.y) * self.t / self.volume;
    }

    fn reservoir(&self, gas: &Gas) -> Reservoir {
        Reservoir::new(gas, self.p, self.t, self.y)
    }

    fn apply(&mut self, dt: f64) {
        self.mass += self.dm * dt;
        self.energy += self.de * dt;
        self.burned += self.dmy * dt;
        self.dm = 0.0;
        self.de = 0.0;
        self.dmy = 0.0;
    }
}

/// One side of a link.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Port {
    Pipe(usize, End),
    /// A volume, by index.
    Volume(usize),
    /// A cylinder, by index.
    Cylinder(usize),
    Ambient,
    Closed,
}

/// How much a link lets through.
#[derive(Clone, Debug, PartialEq)]
pub enum Opening {
    /// No restriction: a pipe's own section.
    Open,
    /// Effective area, m².
    Fixed(f64),
    Throttle {
        bore: f64,
        shaft: f64,
        closed_angle_deg: f64,
    },
    /// A cylinder's intake (true) or exhaust valves.
    Valves { cylinder: usize, intake: bool },
}

#[derive(Clone, Debug)]
pub struct Link {
    pub a: Port,
    pub b: Port,
    pub opening: Opening,
    /// Mass flow from `a` to `b` in the last step, kg/s.
    pub flow: f64,
    /// Face velocity at a pipe end the step before, the next search's start, m/s.
    pub guess: f64,
}

/// A pipe's mouth to the air, a source of sound.
#[derive(Clone, Debug)]
pub struct Mouth {
    pub name: String,
    pub link: usize,
    /// Where it is, m.
    pub position: [f64; 3],
    /// Outward volume flow, m³/s, this step and the step before.
    pub flow: f64,
    pub prev_flow: f64,
    /// Outward gas velocity, m/s.
    pub velocity: f64,
    pub radiation: Radiation,
}

impl Mouth {
    pub fn new(name: String, link: usize, position: [f64; 3]) -> Self {
        Self {
            name,
            link,
            position,
            flow: 0.0,
            prev_flow: 0.0,
            velocity: 0.0,
            radiation: Radiation::default(),
        }
    }
}

/// A cylinder's state over its cycle.
#[derive(Clone, Debug)]
pub struct Cylinder {
    /// Crank angle of its firing TDC, rad.
    pub firing: f64,
    pub gas: usize,
    /// Cycle angle at the previous step, degrees.
    cycle_prev: f64,
    burn: Option<Burn>,
    /// State at inlet valve closing, for Woschni's motored pressure.
    ivc: Option<(f64, f64, f64)>,
    knock: f64,
    /// Accumulators over the current cycle.
    acc: CycleAcc,
    /// The last completed cycle.
    pub last: CycleStats,
    /// Valve lifts at the previous step, for seating.
    lift_prev: [f64; 2],
    /// Seating velocity of a valve that closed this step, m/s.
    pub seated: f64,
    /// dp/dt of the cylinder this step, Pa/s.
    pub dpdt: f64,
}

#[derive(Clone, Debug)]
struct Burn {
    start_deg: f64,
    duration_deg: f64,
    heat: f64,
    /// Fresh mass that burns.
    fresh: f64,
    done: f64,
}

#[derive(Clone, Debug, Default)]
struct CycleAcc {
    work: f64,
    pump_work: f64,
    intake_mass: f64,
    exhaust_mass: f64,
    fuel: f64,
    heat_released: f64,
    wall_heat: f64,
    peak_pressure: f64,
    peak_deg: f64,
    knock: f64,
    trapped: f64,
    residual: f64,
}

/// Figures of one cylinder's last complete cycle.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct CycleStats {
    /// Net indicated work (whole cycle), J.
    pub work: f64,
    /// Work of the gas-exchange strokes (negative: pumping loss), J.
    pub pump_work: f64,
    /// Fresh charge that entered through the intake valves (net), kg.
    pub intake_mass: f64,
    pub exhaust_mass: f64,
    pub fuel: f64,
    pub heat_released: f64,
    pub wall_heat: f64,
    /// Pa, and crank degrees after the firing TDC.
    pub peak_pressure: f64,
    pub peak_deg: f64,
    /// Largest Livengood–Wu integral of the end gas (≥ 1: knock).
    pub knock: f64,
    /// Mass in the cylinder at inlet valve closing, and its burned share.
    pub trapped: f64,
    pub residual: f64,
}

/// The ECU's state.
#[derive(Clone, Debug, Default)]
struct EcuState {
    idle: f64,
    limiter_cut: bool,
}

/// The engine model.
pub struct Model {
    pub spec: EngineSpec,
    pub gas: Gas,
    pub quality: Quality,
    pub ambient: Ambient,
    ambient_res: Reservoir,
    pub pipes: Vec<Pipe>,
    pub lumps: Vec<Lump>,
    pub links: Vec<Link>,
    pub cylinders: Vec<Cylinder>,
    pub mouths: Vec<Mouth>,
    pub kinematics: SliderCrank,
    pub intake_valves: Valvetrain,
    pub exhaust_valves: Valvetrain,
    /// Crank angle, rad (0..4π), and speed, rad/s.
    pub angle: f64,
    pub omega: f64,
    /// Crank train's own inertia, kg·m².
    pub inertia: f64,
    /// Time since the start, s.
    pub time: f64,
    /// Step, s.
    pub dt: f64,
    /// Throttle opening (0..1) the ECU commands.
    pub throttle: f64,
    /// Torque of the gas and the reciprocating masses, and of friction, this step, N·m.
    pub gas_torque: f64,
    pub friction_torque: f64,
    /// Torque the load takes, N·m.
    pub load_torque: f64,
    /// Steps that had to be split for the CFL condition.
    pub split_steps: u64,
    /// Set when the solution breaks down (a step would need too many splits).
    pub fault: Option<String>,
    ecu: EcuState,
    rng: Rng,
    peak_pressure: f64,
    /// The speed below which fuel is cut and the engine is taken as stopped, rad/s.
    stall: f64,
}

impl Model {
    /// A model of `spec` with nothing built yet; `build` adds its pipes and volumes.
    pub(crate) fn empty(
        spec: EngineSpec,
        quality: Quality,
        ambient: Ambient,
    ) -> Result<Self, String> {
        let gas = Gas::new();
        let firing = firing_angles(&spec.layout)?;
        let kin = SliderCrank::new(&spec);
        let ambient_res = Reservoir::new(&gas, ambient.pressure, ambient.temperature, 0.0);
        let cr = &spec.crank;
        let n = firing.len() as f64;
        let r = kin.r;
        let inertia = cr.flywheel_inertia
            + cr.crank_inertia
            + n * (cr.rotating_mass * r * r + 0.5 * cr.reciprocating_mass * r * r);
        let mut m = Self {
            intake_valves: Valvetrain::new(&spec.intake, true),
            exhaust_valves: Valvetrain::new(&spec.exhaust, false),
            rng: Rng::new(spec.combustion.seed),
            stall: spec.ecu.stall_rpm * RAD_PER_RPM,
            spec,
            gas,
            quality,
            ambient,
            ambient_res,
            pipes: Vec::new(),
            lumps: Vec::new(),
            links: Vec::new(),
            cylinders: Vec::new(),
            mouths: Vec::new(),
            kinematics: kin,
            angle: 0.0,
            omega: 0.0,
            inertia,
            time: 0.0,
            dt: 1.0 / quality.rate() as f64,
            throttle: 0.0,
            gas_torque: 0.0,
            friction_torque: 0.0,
            load_torque: 0.0,
            split_steps: 0,
            fault: None,
            ecu: EcuState {
                idle: 0.0,
                limiter_cut: false,
            },
            peak_pressure: 50e5,
        };
        for (i, f) in firing.into_iter().enumerate() {
            let v = m.kinematics.at(-f).volume;
            let mut lump = Lump::new(
                &m.gas,
                format!("cylinder {}", i + 1),
                v,
                ambient.pressure,
                ambient.temperature,
                0.0,
                m.spec.heat_transfer.liner,
            );
            lump.cylinder = true;
            m.lumps.push(lump);
            m.cylinders.push(Cylinder {
                firing: f,
                gas: m.lumps.len() - 1,
                cycle_prev: 0.0,
                burn: None,
                ivc: None,
                knock: 0.0,
                acc: CycleAcc::default(),
                last: CycleStats::default(),
                lift_prev: [0.0; 2],
                seated: 0.0,
                dpdt: 0.0,
            });
        }
        Ok(m)
    }

    /// Engine speed, rpm.
    pub fn rpm(&self) -> f64 {
        self.omega / RAD_PER_RPM
    }

    /// Total swept volume, m³.
    pub fn displacement(&self) -> f64 {
        self.kinematics.swept() * self.cylinders.len() as f64
    }

    /// Mean piston speed, m/s.
    pub fn mean_piston_speed(&self) -> f64 {
        2.0 * self.spec.stroke * self.omega.abs() / (2.0 * PI)
    }

    /// A cylinder's cycle angle, degrees (0 = firing TDC).
    pub fn cycle_deg(&self, cylinder: usize) -> f64 {
        crate::cam::cycle_deg(self.angle, self.cylinders[cylinder].firing)
    }

    /// Pressure in a cylinder, Pa.
    pub fn cylinder_pressure(&self, cylinder: usize) -> f64 {
        self.lumps[self.cylinders[cylinder].gas].p
    }

    /// Sets the crank angle and speed and puts every cylinder's volume where the crank has it.
    pub fn set_crank(&mut self, angle: f64, rpm: f64) {
        self.angle = angle.rem_euclid(4.0 * PI);
        self.omega = rpm * RAD_PER_RPM;
        for c in 0..self.cylinders.len() {
            let deg = self.cycle_deg(c);
            let g = self.cylinders[c].gas;
            self.lumps[g].volume = self.kinematics.at(deg.to_radians()).volume;
            self.lumps[g].derive(&self.gas);
            self.cylinders[c].cycle_prev = deg;
        }
    }

    /// Mass of gas in the whole model, kg.
    pub fn total_mass(&self) -> f64 {
        self.pipes.iter().map(|p| p.mass()).sum::<f64>()
            + self.lumps.iter().map(|l| l.mass).sum::<f64>()
    }

    /// Advances one step.
    pub fn step(&mut self, c: &Controls) {
        self.control(c);
        let mut max_dt = f64::INFINITY;
        let mut worst = 0;
        for (i, p) in self.pipes.iter().enumerate() {
            let d = p.max_dt(0.9);
            if d < max_dt {
                max_dt = d;
                worst = i;
            }
        }
        let n = (self.dt / max_dt).ceil().max(1.0);
        if (n.is_nan() || n > MAX_SPLIT as f64) && self.fault.is_none() {
            self.fault = Some(format!(
                "the flow in {} broke down at {:.3} s ({:.0} rpm)",
                self.pipes[worst].name,
                self.time,
                self.rpm()
            ));
        }
        let n = if n.is_finite() {
            (n as usize).min(MAX_SPLIT)
        } else {
            MAX_SPLIT
        };
        if n > 1 {
            self.split_steps += 1;
        }
        let h = self.dt / n as f64;
        for m in &mut self.mouths {
            m.prev_flow = m.flow;
            m.flow = 0.0;
            m.velocity = 0.0;
        }
        for c in &mut self.cylinders {
            c.seated = 0.0;
            c.dpdt = 0.0;
        }
        for _ in 0..n {
            self.advance(h, c);
        }
        for c in &mut self.cylinders {
            c.dpdt /= n as f64;
        }
    }

    /// The ECU: throttle, idle control and the rev limiter.
    fn control(&mut self, c: &Controls) {
        let e = &self.spec.ecu;
        let rpm = self.rpm();
        // Idle speed control: a PI controller on the throttle, its proportional part half a
        // second of the integral's rate.
        let err = e.idle_rpm - rpm;
        // Never below half the idle air: a shut plate would starve the manifold and stall.
        let (lo, hi) = (-0.5 * e.idle_opening, e.idle_authority);
        self.ecu.idle = (self.ecu.idle + e.idle_gain * err * self.dt).clamp(lo, hi);
        let idle = e.idle_opening + (self.ecu.idle + 0.5 * e.idle_gain * err).clamp(lo, hi);
        self.throttle = c.pedal.clamp(0.0, 1.0).max(idle);
        if rpm > e.limiter_rpm {
            self.ecu.limiter_cut = true;
        } else if rpm < e.limiter_rpm - e.limiter_hysteresis_rpm {
            self.ecu.limiter_cut = false;
        }
    }

    fn fuel_cut(&self, c: &Controls) -> bool {
        let e = &self.spec.ecu;
        let rpm = self.rpm();
        !c.ignition
            || self.omega < self.stall
            || self.ecu.limiter_cut
            || e.overrun_cut_rpm
                .is_some_and(|cut| rpm > cut && c.pedal < 0.01)
    }

    fn reservoir(&self, port: Port) -> Reservoir {
        match port {
            Port::Volume(i) => self.lumps[i].reservoir(&self.gas),
            Port::Cylinder(i) => self.lumps[self.cylinders[i].gas].reservoir(&self.gas),
            _ => self.ambient_res,
        }
    }

    fn lump_of(&self, port: Port) -> Option<usize> {
        match port {
            Port::Volume(i) => Some(i),
            Port::Cylinder(i) => Some(self.cylinders[i].gas),
            _ => None,
        }
    }

    fn cda(&self, o: &Opening, pipe_area: f64) -> f64 {
        match *o {
            Opening::Open => pipe_area,
            Opening::Fixed(a) => a,
            Opening::Throttle {
                bore,
                shaft,
                closed_angle_deg,
            } => 0.85 * throttle_area(bore, shaft, closed_angle_deg, self.throttle),
            Opening::Valves { cylinder, intake } => {
                let deg = self.cycle_deg(cylinder);
                if intake {
                    self.intake_valves.area_at(deg)
                } else {
                    self.exhaust_valves.area_at(deg)
                }
            }
        }
    }

    fn advance(&mut self, h: f64, c: &Controls) {
        let gas = &self.gas;
        for p in &mut self.pipes {
            p.predict(gas, h);
        }
        // Links.
        for li in 0..self.links.len() {
            let (a, b, opening) = {
                let l = &self.links[li];
                (l.a, l.b, l.opening.clone())
            };
            let flow = match (a, b) {
                (Port::Pipe(pa, ea), Port::Pipe(pb, eb)) => {
                    let (sa, sb) = (*self.pipes[pa].end(ea), *self.pipes[pb].end(eb));
                    let (aa, ab) = (self.pipes[pa].end_area(ea), self.pipes[pb].end_area(eb));
                    let (fa, fb) = pipe_to_pipe(&sa, ea.sign(), aa, &sb, eb.sign(), ab);
                    self.pipes[pa].set_end_flux(ea, fa);
                    self.pipes[pb].set_end_flux(eb, fb);
                    fa[0]
                }
                (Port::Pipe(p, e), other) | (other, Port::Pipe(p, e)) => {
                    let s = *self.pipes[p].end(e);
                    let area = self.pipes[p].end_area(e);
                    let cda = if other == Port::Closed {
                        0.0
                    } else {
                        self.cda(&opening, area)
                    };
                    let res = self.reservoir(other);
                    let mouth = if other == Port::Ambient {
                        self.mouths.iter().position(|m| m.link == li)
                    } else {
                        None
                    };
                    let f = if let (Opening::Open, Some(k)) = (&opening, mouth) {
                        let rad = &mut self.mouths[k].radiation;
                        pipe_radiating(&self.gas, &s, e.sign(), area, &res, rad, h)
                    } else if opening == Opening::Open && other != Port::Closed {
                        pipe_open_to_reservoir(&self.gas, &s, e.sign(), area, &res)
                    } else {
                        let mut guess = self.links[li].guess;
                        let f =
                            pipe_to_reservoir(&self.gas, &s, e.sign(), area, cda, &res, &mut guess);
                        self.links[li].guess = guess;
                        f
                    };
                    self.pipes[p].set_end_flux(e, f);
                    if let Some(l) = self.lump_of(other) {
                        let lump = &mut self.lumps[l];
                        lump.dm += f[0];
                        lump.de += f[2];
                        lump.dmy += f[3];
                    }
                    if let Some(k) = mouth {
                        let m = &mut self.mouths[k];
                        let rho = s.w.rho;
                        m.flow += f[0] / rho * h / self.dt;
                        m.velocity += f[0] / (rho * area) * h / self.dt;
                    }
                    // Positive from a to b: out of the pipe when the pipe is `a`.
                    if matches!(a, Port::Pipe(..)) {
                        f[0]
                    } else {
                        -f[0]
                    }
                }
                (x, y) => {
                    let cda = self.cda(&opening, f64::INFINITY);
                    let (rx, ry) = (self.reservoir(x), self.reservoir(y));
                    let f = reservoir_to_reservoir(cda, &rx, &ry);
                    for (port, sign) in [(x, -1.0), (y, 1.0)] {
                        if let Some(l) = self.lump_of(port) {
                            let lump = &mut self.lumps[l];
                            lump.dm += sign * f[0];
                            lump.de += sign * f[1];
                            lump.dmy += sign * f[2];
                        }
                    }
                    f[0]
                }
            };
            self.links[li].flow = flow;
            if let Opening::Valves { cylinder, intake } = opening {
                // Positive into the cylinder.
                let into = if matches!(b, Port::Cylinder(_)) {
                    flow
                } else {
                    -flow
                };
                let acc = &mut self.cylinders[cylinder].acc;
                if intake {
                    acc.intake_mass += into * h;
                } else {
                    acc.exhaust_mass -= into * h;
                }
            }
        }
        for p in &mut self.pipes {
            p.update(&self.gas, h);
        }
        // Volumes (cylinders follow with the crank).
        for l in &mut self.lumps {
            if l.cylinder {
                continue;
            }
            l.apply(h);
            l.derive(&self.gas);
        }
        self.advance_cylinders(h, c);
    }

    fn advance_cylinders(&mut self, h: f64, c: &Controls) {
        let rpm = self.rpm();
        let cut = self.fuel_cut(c);
        let dtheta = self.omega * h;
        let new_angle = self.angle + dtheta;
        let sp = self.mean_piston_speed();
        let bore = self.spec.bore;
        let kin = self.kinematics;
        let ht = self.spec.heat_transfer.clone();
        let comb = self.spec.combustion.clone();
        let (lhv, afr) = comb.fuel.properties();
        let ivc_deg = self.intake_valves.close_deg();
        let m_rec = self.spec.crank.reciprocating_mass;
        let mut gas_torque = 0.0;
        for ci in 0..self.cylinders.len() {
            let firing = self.cylinders[ci].firing;
            let deg0 = crate::cam::cycle_deg(self.angle, firing);
            let deg1 = crate::cam::cycle_deg(new_angle, firing);
            let wrapped = deg1 < deg0;
            let (p0, v0) = {
                let l = &self.lumps[self.cylinders[ci].gas];
                (l.p, l.volume)
            };
            let piston = kin.at(deg1.to_radians());
            let v1 = piston.volume;
            // Spark: the ECU's advance before the firing TDC.
            let advance = self.spec.ecu.spark_deg.at(rpm, self.throttle);
            let spark = 720.0 - advance;
            let crossed = |at: f64| -> bool {
                let at = at.rem_euclid(720.0);
                if wrapped {
                    at > deg0 || at <= deg1
                } else {
                    at > deg0 && at <= deg1
                }
            };
            // Inlet valve closing: the charge is trapped.
            if crossed(ivc_deg) {
                let l = &self.lumps[self.cylinders[ci].gas];
                let (m, y, p, t, v) = (l.mass, l.y, l.p, l.t, l.volume);
                let cyl = &mut self.cylinders[ci];
                cyl.ivc = Some((p, v, t));
                cyl.knock = 0.0;
                cyl.acc.trapped = m;
                cyl.acc.residual = y;
            }
            if crossed(spark) {
                let lambda = self.spec.ecu.lambda.at(rpm, self.throttle).max(0.5);
                let l = &self.lumps[self.cylinders[ci].gas];
                let fresh = l.mass - l.burned;
                if !cut && fresh > 0.0 {
                    // Port-injected: the fresh charge is air and fuel at this λ.
                    let fuel = fresh / (1.0 + afr * lambda);
                    let spread = 1.0 + comb.variation * self.rng.normal();
                    let dur = combustion::duration_deg(&comb, rpm, lambda)
                        * spread.clamp(0.5, 2.0)
                        * (1.0 + 1.5 * l.y);
                    let heat = comb.efficiency * combustion::burnable(lambda) * fuel * lhv;
                    let cyl = &mut self.cylinders[ci];
                    cyl.burn = Some(Burn {
                        start_deg: spark,
                        duration_deg: dur,
                        heat,
                        fresh,
                        done: 0.0,
                    });
                    cyl.acc.fuel += fuel;
                }
            }
            // Heat released this step.
            let mut dq = 0.0;
            let mut dburned = 0.0;
            let mut burning = false;
            if let Some(b) = &mut self.cylinders[ci].burn {
                let since = (deg1 - b.start_deg).rem_euclid(720.0);
                let f = since / b.duration_deg;
                let x = combustion::wiebe(comb.wiebe_a, comb.wiebe_m, f);
                let dx = (x - b.done).max(0.0);
                b.done = x;
                dq = dx * b.heat;
                dburned = dx * b.fresh;
                burning = f < 1.0;
                if f >= 1.2 {
                    self.cylinders[ci].burn = None;
                }
            }
            // Woschni.
            let (p, t) = {
                let l = &self.lumps[self.cylinders[ci].gas];
                (l.p, l.t)
            };
            let li = self.intake_valves.lift_at(deg1);
            let le = self.exhaust_valves.lift_at(deg1);
            let phase = if burning {
                Phase::Combustion
            } else if li > 0.0 || le > 0.0 {
                Phase::GasExchange
            } else {
                Phase::Compression
            };
            let (p_mot, reference) = match self.cylinders[ci].ivc {
                Some((pr, vr, tr)) => (pr * (vr / v1).powf(1.32), kin.swept() * tr / (pr * vr)),
                None => (p, 0.0),
            };
            let hw = ht.scale * combustion::woschni(p, t, bore, sp, phase, p_mot, reference);
            let a_head = 1.1 * kin.area;
            let a_liner = std::f64::consts::PI * bore * (v1 - kin.clearance) / kin.area;
            let q_wall = hw
                * (a_head * (t - ht.head) + kin.area * (t - ht.piston) + a_liner * (t - ht.liner))
                * h;
            // Knock: the end gas compressed isentropically from inlet valve closing.
            if burning && let Some((pr, _, tr)) = self.cylinders[ci].ivc {
                let tu = tr * (p / pr).powf(0.25);
                let tau = combustion::ignition_delay(comb.octane, p, tu);
                self.cylinders[ci].knock += h / tau;
            }
            // Energy balance.
            let work = p0 * (v1 - v0);
            {
                let g = &self.gas;
                let l = &mut self.lumps[self.cylinders[ci].gas];
                l.volume = v1;
                l.apply(h);
                l.energy += dq - q_wall - work;
                l.burned += dburned;
                l.derive(g);
            }
            let p1 = self.lumps[self.cylinders[ci].gas].p;
            // Torque on the crank: the gas on the piston, and the reciprocating mass.
            let omega = self.omega;
            let ddx = piston.ddx * omega * omega;
            let t_gas = (p1 - self.ambient.pressure) * piston.dv - m_rec * ddx * piston.dx;
            gas_torque += t_gas;
            // Valve seating.
            let cyl = &mut self.cylinders[ci];
            for (k, lift) in [li, le].into_iter().enumerate() {
                if lift == 0.0 && cyl.lift_prev[k] > 0.0 {
                    cyl.seated = cyl.seated.max(cyl.lift_prev[k] / h);
                }
                cyl.lift_prev[k] = lift;
            }
            cyl.dpdt += (p1 - p0) / h;
            // Cycle bookkeeping.
            let acc = &mut cyl.acc;
            acc.work += work;
            if (180.0..540.0).contains(&deg1) {
                acc.pump_work += work;
            }
            acc.heat_released += dq;
            acc.wall_heat += q_wall;
            if p1 > acc.peak_pressure {
                acc.peak_pressure = p1;
                acc.peak_deg = if deg1 > 360.0 { deg1 - 720.0 } else { deg1 };
            }
            acc.knock = acc.knock.max(cyl.knock);
            if wrapped {
                let a = std::mem::take(&mut cyl.acc);
                cyl.last = CycleStats {
                    work: a.work,
                    pump_work: a.pump_work,
                    intake_mass: a.intake_mass,
                    exhaust_mass: a.exhaust_mass,
                    fuel: a.fuel,
                    heat_released: a.heat_released,
                    wall_heat: a.wall_heat,
                    peak_pressure: a.peak_pressure,
                    peak_deg: a.peak_deg,
                    knock: a.knock,
                    trapped: a.trapped,
                    residual: a.residual,
                };
                self.peak_pressure = self.peak_pressure.max(0.0) * 0.5 + 0.5 * a.peak_pressure;
            }
            cyl.cycle_prev = deg1;
        }
        // Friction: Chen–Flynn on the last peak pressure, smoothed through zero speed.
        let fmep = combustion::fmep(&self.spec.friction, self.peak_pressure, sp, 1.0);
        let friction = fmep * self.displacement() / (4.0 * PI) * (self.omega / 5.0).tanh();
        self.gas_torque = gas_torque;
        self.friction_torque = friction;
        let starter = if c.starter && self.omega < 2.0 * self.stall {
            25.0 * self.cylinders.len() as f64
        } else {
            0.0
        };
        match c.load {
            Load::Speed(rpm) => {
                self.omega = rpm * RAD_PER_RPM;
                self.load_torque = gas_torque - friction;
            }
            Load::Inertia { inertia, torque } => {
                self.load_torque = torque * (self.omega / 5.0).tanh();
                let net = gas_torque + starter - friction - self.load_torque;
                self.omega = (self.omega + net / (self.inertia + inertia) * h).max(0.0);
            }
        }
        self.angle = new_angle.rem_euclid(4.0 * PI);
        self.time += h;
    }
}
