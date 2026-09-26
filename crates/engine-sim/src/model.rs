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
use crate::chem::Chemistry;
use crate::combustion::{self, Phase, Rng};
use crate::crank::{SliderCrank, firing_angles};
use crate::gas::Gas;
use crate::pipe::{End, Pipe};
use crate::spec::{Cut, EngineSpec, throttle_area};
use crate::turbo::Turbo;

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
    /// Unburned fuel, kg (a part of the fresh charge).
    pub fuel: f64,
    /// Heat released by fuel burning outside a flame since it was built, J.
    pub heat_released: f64,
    pub wall_temperature: f64,
    /// Whether it is a cylinder's (advanced with the crank, not with the volumes).
    pub cylinder: bool,
    /// A flame crossing it (a volume's afterfire), and whether gas hot enough to light
    /// one came in over the step.
    flame: Option<Flame>,
    hot_inflow: bool,
    // Derived.
    pub p: f64,
    pub t: f64,
    pub y: f64,
    // Flows in over the current step, per second.
    dm: f64,
    de: f64,
    dmy: f64,
    dmf: f64,
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
            fuel: 0.0,
            heat_released: 0.0,
            wall_temperature: wall,
            cylinder: false,
            flame: None,
            hot_inflow: false,
            p,
            t,
            y,
            dm: 0.0,
            de: 0.0,
            dmy: 0.0,
            dmf: 0.0,
        };
        l.derive(gas);
        l
    }

    pub fn derive(&mut self, gas: &Gas) {
        self.mass = self.mass.max(1e-12);
        self.burned = self.burned.clamp(0.0, self.mass);
        self.fuel = self.fuel.clamp(0.0, self.mass - self.burned);
        self.y = self.burned / self.mass;
        self.t = gas
            .temperature_near(self.energy / self.mass, self.y, self.t)
            .max(150.0);
        self.p = self.mass * gas.r(self.y) * self.t / self.volume;
    }

    fn reservoir(&self, gas: &Gas) -> Reservoir {
        Reservoir {
            f: self.fuel / self.mass,
            ..Reservoir::new(gas, self.p, self.t, self.y)
        }
    }

    fn apply(&mut self, dt: f64) {
        self.mass += self.dm * dt;
        self.energy += self.de * dt;
        self.burned += self.dmy * dt;
        self.fuel += self.dmf * dt;
        self.dm = 0.0;
        self.de = 0.0;
        self.dmy = 0.0;
        self.dmf = 0.0;
    }

    /// Adds flows in (negative: out), per second, over the current step: mass, energy,
    /// burned mass and fuel.
    pub(crate) fn add_rates(&mut self, dm: f64, de: f64, dmy: f64, dmf: f64) {
        self.dm += dm;
        self.de += de;
        self.dmy += dmy;
        self.dmf += dmf;
    }

    /// Burns its unburned fuel as a flame crossing it would, over `dt` (call `derive`
    /// after); returns the heat released, J.
    ///
    /// A volume full of unburned mixture — fuel from misfires, late burns and rich
    /// running collected in a collector or a silencer, too cool to ignite by itself — is
    /// lit when gas hotter than `IGNITION` comes in (a burning or freshly burned slug of
    /// exhaust), if it is flammable: its laminar flame speed (Metghalchi & Keck, at the
    /// mixture's λ, burned-gas dilution and its temperature compressed isentropically
    /// since it was lit) above the quench speed. The turbulent flame then crosses it at
    /// S_T = S_L + √(S_L·u′) (the thin-flame limit, u′ the exhaust's turbulence) in the
    /// time the volume's size takes, burning along a Wiebe curve. A deflagration in a
    /// closed box, vented through its pipes: the afterfire's bang.
    fn deflagrate(&mut self, chem: &Chemistry, dt: f64) -> f64 {
        let hot = std::mem::take(&mut self.hot_inflow);
        let air = (self.mass - self.burned - self.fuel).max(0.0);
        if self.fuel <= 1e-5 * self.mass || air <= 0.0 {
            self.flame = None;
            return 0.0;
        }
        let lambda = air / (chem.afr * self.fuel);
        let mut flame = match self.flame.take() {
            Some(f) => f,
            None if hot => Flame {
                p0: self.p,
                t0: self.t,
                residual: self.y,
                progress: 0.0,
                done: 0.0,
            },
            None => return 0.0,
        };
        let t_u = flame.t0 * (self.p / flame.p0).max(0.1).powf(0.25);
        let s_l = combustion::laminar_flame_speed(t_u, self.p, lambda, flame.residual);
        if s_l < combustion::QUENCH_SPEED {
            return 0.0;
        }
        let size = 2.0 * (3.0 * self.volume / (4.0 * PI)).cbrt();
        let s_t = s_l + (s_l * TURBULENCE).sqrt();
        flame.progress += dt * s_t / size;
        let x = combustion::wiebe(5.0, 2.0, flame.progress);
        let share = ((x - flame.done) / (1.0 - flame.done).max(1e-9)).clamp(0.0, 1.0);
        flame.done = flame.done.max(x);
        let dm = share * self.fuel.min(air / chem.afr);
        self.fuel -= dm;
        self.burned += dm * (1.0 + chem.afr);
        self.energy += dm * chem.heat;
        self.heat_released += dm * chem.heat;
        if flame.progress < 1.2 {
            self.flame = Some(flame);
        }
        dm * chem.heat
    }

    /// Burns its unburned fuel as the chemistry lets it over `dt` (call `derive` after);
    /// returns the heat released, J.
    fn react(&mut self, chem: &Chemistry, dt: f64) -> f64 {
        if self.fuel <= 1e-4 * self.mass {
            return 0.0;
        }
        let rho = self.mass / self.volume;
        let f = self.fuel / self.mass;
        let dm = chem.burn(rho, self.t, self.y, f, dt) * self.mass;
        self.fuel -= dm;
        self.burned += dm * (1.0 + chem.afr);
        self.energy += dm * chem.heat;
        self.heat_released += dm * chem.heat;
        dm * chem.heat
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
    /// A turbocharger's wastegate: its area when fully open, m², opened as far as the
    /// turbocharger's `wastegate` says.
    Wastegate { turbo: usize, area: f64 },
    /// A blow-off valve: its area when fully open, m², opened as far as the link's
    /// `position` says; it opens `opens` Pa (fully `span` Pa further) over the
    /// `reference` volume's pressure.
    BlowOff {
        area: f64,
        reference: usize,
        opens: f64,
        span: f64,
    },
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
    /// How far open a valve worked by the gas (a blow-off valve) is, 0..1.
    pub position: f64,
}

impl Link {
    pub fn new(a: Port, b: Port, opening: Opening) -> Self {
        Self {
            a,
            b,
            opening,
            flow: 0.0,
            guess: 0.0,
            position: 0.0,
        }
    }
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
    /// Outward gas velocity, m/s, and the gas's density there, kg/m³.
    pub velocity: f64,
    pub density: f64,
    /// Area of the mouth, m².
    pub area: f64,
    pub radiation: Radiation,
}

impl Cylinder {
    /// Laminar flame speed in the unburned charge at cylinder pressure `p`, m/s: its
    /// temperature is the trapped charge's, compressed isentropically since the inlet
    /// valve shut.
    fn flame_speed(&self, p: f64, lambda: f64, residual: f64) -> f64 {
        let Some((pr, _, tr)) = self.ivc else {
            return 1.0;
        };
        let t_u = tr * (p / pr).powf(0.25);
        combustion::laminar_flame_speed(t_u, p, lambda, residual)
    }
}

impl Mouth {
    pub fn new(name: String, link: usize, position: [f64; 3], area: f64) -> Self {
        Self {
            name,
            link,
            position,
            flow: 0.0,
            prev_flow: 0.0,
            velocity: 0.0,
            density: 0.0,
            area,
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
    /// Burned-gas share of the charge trapped then.
    residual: f64,
    knock: f64,
    /// Accumulators over the current cycle.
    acc: CycleAcc,
    /// The last completed cycle.
    pub last: CycleStats,
    /// Valve lifts at the previous step, for seating.
    lift_prev: [f64; 2],
    /// Whether its intake and exhaust valves run on the high lobes.
    pub high: [bool; 2],
    /// Seating velocity of a valve that closed this step, m/s.
    pub seated: f64,
    /// dp/dt of the cylinder this step, Pa/s.
    pub dpdt: f64,
}

/// Gas coming into a volume hotter than this lights the unburned mixture there, K: the
/// temperature at which the fuel burns within a fraction of a millisecond (see
/// [`crate::chem`]), so that the gas is itself burning.
const IGNITION: f64 = 1200.0;

/// Turbulence intensity u′ in the exhaust's volumes, m/s: a tenth or so of the gas's
/// speed in the pipes feeding them.
const TURBULENCE: f64 = 5.0;

/// A flame crossing a volume.
#[derive(Clone, Debug)]
struct Flame {
    /// Pressure and temperature of the mixture when it was lit, and its burned share.
    p0: f64,
    t0: f64,
    residual: f64,
    /// Share of the crossing done, and of the fuel burned.
    progress: f64,
    done: f64,
}

/// A flame, burning the cylinder's fuel along its Wiebe function.
#[derive(Clone, Debug)]
struct Burn {
    start_deg: f64,
    duration_deg: f64,
    /// Share of the duration run, which the flame speed paces.
    progress: f64,
    /// Share of the charge burned so far.
    done: f64,
    /// Excess-air ratio of the charge and its residual gas round the flame (which varies
    /// from cycle to cycle), and its laminar flame speed at the spark, m/s.
    lambda: f64,
    residual: f64,
    speed0: f64,
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
    /// The high lobes asked for, and how long the oil has been changing over.
    cam_want: bool,
    cam_timer: f64,
}

/// Time the oil takes to move the rocker pins once the ECU switches the spool valve, s.
const CAM_SWITCH_TIME: f64 = 0.08;

/// Fastest a cam phaser turns, crank degrees per second.
const PHASER_RATE: f64 = 250.0;

/// What the ECU does to the cylinders this step.
#[derive(Clone, Copy, Debug)]
struct Firing {
    fuel: bool,
    spark: bool,
    /// Advance before the firing TDC, crank degrees.
    spark_deg: f64,
    lambda: f64,
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
    pub turbos: Vec<Turbo>,
    /// The fuel's burning outside the flame.
    pub chem: Chemistry,
    /// Heat released by fuel burning outside the cylinders' flames (in the pipes and
    /// volumes) over the last step, J: the afterfire.
    pub afterfire: f64,
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
    /// Oil pressure at the rocker pins: valves go over to the high lobes as they shut.
    pub cam_oil: bool,
    /// Advance of the intake and exhaust cams, crank degrees.
    pub phase: [f64; 2],
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
            chem: Chemistry::new(&spec.combustion),
            afterfire: 0.0,
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
            turbos: Vec::new(),
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
            ecu: EcuState::default(),
            cam_oil: false,
            phase: [0.0; 2],
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
                residual: 0.0,
                knock: 0.0,
                acc: CycleAcc::default(),
                last: CycleStats::default(),
                lift_prev: [0.0; 2],
                high: [false; 2],
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
            m.density = 0.0;
        }
        for c in &mut self.cylinders {
            c.seated = 0.0;
            c.dpdt = 0.0;
        }
        self.afterfire = 0.0;
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
        // Only with the pedal up: revving, the speed is the driver's, and an integral
        // wound down meanwhile would let the engine dip under idle when it comes back.
        if c.pedal < 0.01 {
            self.ecu.idle = (self.ecu.idle + e.idle_gain * err * self.dt).clamp(lo, hi);
        }
        let idle = e.idle_opening + (self.ecu.idle + 0.5 * e.idle_gain * err).clamp(lo, hi);
        self.throttle = c.pedal.clamp(0.0, 1.0).max(idle);
        if let Some(pops) = &e.pops
            && c.pedal < 0.01
            && rpm > pops.above_rpm
        {
            self.throttle = self.throttle.max(pops.throttle);
        }
        // Wastegates' diaphragms, pushed by the boost at their compressors' outlets
        // against their springs; they settle in some 50 ms.
        let amb = self.ambient.pressure;
        let lag = 1.0 - (-self.dt / 0.05).exp();
        for t in &mut self.turbos {
            let gate = t.turbine.as_ref().and_then(|t| t.wastegate.as_ref());
            let target = gate.filter(|_| t.compressor.is_some()).map_or(0.0, |w| {
                let boost = self.lumps[t.compressor_ports[1]].p - amb;
                ((boost - w.opens) / (w.open - w.opens).max(1.0)).clamp(0.0, 1.0)
            });
            t.wastegate += lag * (target - t.wastegate);
        }
        // Blow-off valves: light poppets, open in a few milliseconds.
        let lag = 1.0 - (-self.dt / 0.005).exp();
        for li in 0..self.links.len() {
            if let Opening::BlowOff {
                reference,
                opens,
                span,
                ..
            } = self.links[li].opening
            {
                let from = match self.links[li].a {
                    Port::Volume(v) => self.lumps[v].p,
                    _ => amb,
                };
                let target = ((from - self.lumps[reference].p - opens) / span).clamp(0.0, 1.0);
                self.links[li].position += lag * (target - self.links[li].position);
            }
        }
        // Cam lobes: the spool valve follows the ECU, the oil after it.
        if let Some(sw) = &e.cam_switch {
            let want = if self.ecu.cam_want {
                rpm > sw.rpm - sw.hysteresis_rpm && self.throttle >= 0.5 * sw.min_load
            } else {
                rpm > sw.rpm && self.throttle >= sw.min_load
            };
            self.ecu.cam_want = want;
            if want != self.cam_oil {
                self.ecu.cam_timer += self.dt;
                if self.ecu.cam_timer >= CAM_SWITCH_TIME {
                    self.cam_oil = want;
                    self.ecu.cam_timer = 0.0;
                }
            } else {
                self.ecu.cam_timer = 0.0;
            }
        }
        // Cam phasers, as fast as they turn.
        for (k, map) in [&e.intake_phase, &e.exhaust_phase].into_iter().enumerate() {
            let target = map.as_ref().map_or(0.0, |m| m.at(rpm, self.throttle));
            let most = PHASER_RATE * self.dt;
            self.phase[k] += (target - self.phase[k]).clamp(-most, most);
        }
        if rpm > e.limiter_rpm {
            self.ecu.limiter_cut = true;
        } else if rpm < e.limiter_rpm - e.limiter_hysteresis_rpm {
            self.ecu.limiter_cut = false;
        }
    }

    /// Fuel, spark, advance and mixture this step: the maps, the limiter's cut, and on
    /// overrun either a fuel cut or a pop map's late spark.
    fn firing(&self, c: &Controls) -> Firing {
        let e = &self.spec.ecu;
        let rpm = self.rpm();
        let mut f = Firing {
            fuel: c.ignition && self.omega >= self.stall,
            spark: c.ignition && self.omega >= self.stall,
            spark_deg: e.spark_deg.at(rpm, self.throttle),
            lambda: e.lambda.at(rpm, self.throttle).max(0.5),
        };
        if self.ecu.limiter_cut {
            match e.limiter_cut {
                Cut::Fuel => f.fuel = false,
                Cut::Spark => f.spark = false,
            }
        }
        if c.pedal < 0.01 {
            if let Some(pops) = &e.pops
                && rpm > pops.above_rpm
            {
                f.spark_deg = pops.spark_deg;
                f.lambda = pops.lambda;
            } else if e.overrun_cut_rpm.is_some_and(|cut| rpm > cut) {
                f.fuel = false;
            }
        }
        f
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

    fn cda(&self, o: &Opening, pipe_area: f64, position: f64) -> f64 {
        match *o {
            Opening::Open => pipe_area,
            Opening::Fixed(a) => a,
            Opening::Throttle {
                bore,
                shaft,
                closed_angle_deg,
            } => 0.85 * throttle_area(bore, shaft, closed_angle_deg, self.throttle),
            Opening::Wastegate { turbo, area } => area * self.turbos[turbo].wastegate,
            Opening::BlowOff { area, .. } => area * position,
            Opening::Valves { cylinder, intake } => {
                let deg = self.cycle_deg(cylinder);
                let v = if intake {
                    &self.intake_valves
                } else {
                    &self.exhaust_valves
                };
                v.area_at_lift(self.valve_lift_at(cylinder, intake, deg))
            }
        }
    }

    /// A cylinder's intake (`intake`) or exhaust valve lift at a cycle angle, on the lobe
    /// it runs on and with the cam's phase, m.
    pub fn valve_lift_at(&self, cylinder: usize, intake: bool, deg: f64) -> f64 {
        let k = if intake { 0 } else { 1 };
        let v = if intake {
            &self.intake_valves
        } else {
            &self.exhaust_valves
        };
        v.lift_on(deg, self.cylinders[cylinder].high[k], self.phase[k])
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
                        self.cda(&opening, area, self.links[li].position)
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
                        lump.dmf += f[4];
                        lump.hot_inflow |= f[0] > 0.0 && s.t > IGNITION;
                    }
                    if let Some(k) = mouth {
                        let m = &mut self.mouths[k];
                        let rho = s.w.rho;
                        m.flow += f[0] / rho * h / self.dt;
                        m.velocity += f[0] / (rho * area) * h / self.dt;
                        m.density += rho * h / self.dt;
                    }
                    // Positive from a to b: out of the pipe when the pipe is `a`.
                    if matches!(a, Port::Pipe(..)) {
                        f[0]
                    } else {
                        -f[0]
                    }
                }
                (x, y) => {
                    let cda = self.cda(&opening, f64::INFINITY, self.links[li].position);
                    let (rx, ry) = (self.reservoir(x), self.reservoir(y));
                    let f = reservoir_to_reservoir(cda, &rx, &ry);
                    // A volume venting to the air (a blow-off valve) is a sound source: its
                    // jet, at the vena contracta's speed.
                    if let Some(k) = self.mouths.iter().position(|m| m.link == li) {
                        let (out, src) = if y == Port::Ambient {
                            (f[0], &rx)
                        } else {
                            (-f[0], &ry)
                        };
                        let rho = if out >= 0.0 {
                            src.density()
                        } else {
                            self.ambient_res.density()
                        };
                        let m = &mut self.mouths[k];
                        let c = (src.gamma * src.r * src.t).sqrt();
                        m.flow += out / rho * h / self.dt;
                        m.velocity += (out / (rho * cda.max(1e-9))).clamp(-c, c) * h / self.dt;
                        m.density += rho * h / self.dt;
                        m.area = cda.max(1e-7);
                    }
                    let hot = if f[0] >= 0.0 { rx.t } else { ry.t } > IGNITION;
                    for (port, sign) in [(x, -1.0), (y, 1.0)] {
                        if let Some(l) = self.lump_of(port) {
                            let lump = &mut self.lumps[l];
                            lump.dm += sign * f[0];
                            lump.de += sign * f[1];
                            lump.dmy += sign * f[2];
                            lump.dmf += sign * f[3];
                            lump.hot_inflow |= hot && sign * f[0] > 0.0;
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
        for t in &mut self.turbos {
            t.advance(&self.gas, &mut self.lumps, h);
        }
        for p in &mut self.pipes {
            self.afterfire += p.update_reacting(&self.gas, &self.chem, h);
        }
        // Volumes (cylinders follow with the crank).
        for l in &mut self.lumps {
            if l.cylinder {
                continue;
            }
            l.apply(h);
            l.derive(&self.gas);
            let q = l.react(&self.chem, h) + l.deflagrate(&self.chem, h);
            if q > 0.0 {
                self.afterfire += q;
                l.derive(&self.gas);
            }
        }
        self.advance_cylinders(h, c);
    }

    fn advance_cylinders(&mut self, h: f64, c: &Controls) {
        let rpm = self.rpm();
        let fire = self.firing(c);
        let dtheta = self.omega * h;
        let new_angle = self.angle + dtheta;
        let sp = self.mean_piston_speed();
        let bore = self.spec.bore;
        let kin = self.kinematics;
        let ht = self.spec.heat_transfer.clone();
        let comb = self.spec.combustion.clone();
        let afr = self.chem.afr;

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
            let spark = (720.0 - fire.spark_deg).rem_euclid(720.0);
            let crossed = |at: f64| -> bool {
                let at = at.rem_euclid(720.0);
                if wrapped {
                    at > deg0 || at <= deg1
                } else {
                    at > deg0 && at <= deg1
                }
            };
            // Inlet valve closing: the charge is trapped, and (port injection) the
            // fresh part of it is air and fuel at the ECU's λ.
            let ivc_deg = self
                .intake_valves
                .close_deg_on(self.cylinders[ci].high[0], self.phase[0]);
            if crossed(ivc_deg) {
                let gi = self.cylinders[ci].gas;
                let l = &mut self.lumps[gi];
                let added = if fire.fuel {
                    let fresh = l.mass - l.burned;
                    let fuel = fresh / (1.0 + afr * fire.lambda);
                    let added = (fuel - l.fuel).max(0.0);
                    l.fuel = l.fuel.max(fuel);
                    added
                } else {
                    0.0
                };
                let (m, y, p, t, v) = (l.mass, l.y, l.p, l.t, l.volume);
                let cyl = &mut self.cylinders[ci];
                cyl.ivc = Some((p, v, t));
                cyl.residual = y;
                cyl.knock = 0.0;
                cyl.acc.trapped = m;
                cyl.acc.residual = y;
                cyl.acc.fuel += added;
            }
            if crossed(spark) && fire.spark {
                let l = &self.lumps[self.cylinders[ci].gas];
                if l.fuel > 1e-4 * l.mass {
                    let lambda = (l.mass - l.burned - l.fuel) / (afr * l.fuel);
                    let dur = combustion::duration_deg(&comb, rpm, lambda) * (1.0 + 1.5 * l.y);
                    let slow = combustion::cycle_variation(&comb, l.y, self.rng.normal());
                    let cyl = &self.cylinders[ci];
                    // The kernel that grows slowly is the one in more residual gas.
                    let residual = cyl.residual * (1.0 + slow);
                    let speed0 = cyl.flame_speed(l.p, lambda, residual);
                    // Too dilute or too lean to light: a misfire.
                    if speed0 >= combustion::QUENCH_SPEED {
                        self.cylinders[ci].burn = Some(Burn {
                            start_deg: spark + 0.5 * slow * dur,
                            duration_deg: dur * (1.0 + slow),
                            progress: 0.0,
                            done: 0.0,
                            lambda,
                            residual,
                            speed0,
                        });
                    }
                }
            }
            // Heat released this step: the flame takes its share of the charge still
            // unburned, of the fuel still in the cylinder (some may have left with the
            // exhaust) and that its oxygen can burn.
            let mut dq = 0.0;
            let mut dburned = 0.0;
            let mut dfuel = 0.0;
            let mut burning = false;
            let gi = self.cylinders[ci].gas;
            if let Some(mut b) = self.cylinders[ci].burn.take() {
                // Negative until a late flame kernel starts to burn. From then the
                // turbulent flame runs at √S_L (Damköhler's thin-flame limit, as in
                // Gülder's correlation): as the expansion cools the unburned gas, a late
                // flame slows, and it goes out when S_L falls below the quench speed,
                // leaving the rest of the charge to the exhaust.
                let since = (deg1 - b.start_deg + 360.0).rem_euclid(720.0) - 360.0;
                let l = &self.lumps[gi];
                let speed = self.cylinders[ci].flame_speed(l.p, b.lambda, b.residual);
                let quenched = since > 0.0 && speed < combustion::QUENCH_SPEED;
                if since > 0.0 {
                    let step = (dtheta.to_degrees()).min(since);
                    b.progress += step / b.duration_deg * (speed / b.speed0).sqrt();
                }
                let f = b.progress;
                let x = combustion::wiebe(comb.wiebe_a, comb.wiebe_m, f);
                let share = ((x - b.done) / (1.0 - b.done).max(1e-9)).clamp(0.0, 1.0);
                b.done = b.done.max(x);
                let l = &self.lumps[gi];
                let burnable = l.fuel.min((l.mass - l.burned - l.fuel).max(0.0) / afr);
                dfuel = share * burnable;
                dq = dfuel * self.chem.heat;
                dburned = dfuel * (1.0 + afr);
                burning = f < 1.0 && !quenched;
                if f < 1.2 && !quenched {
                    self.cylinders[ci].burn = Some(b);
                }
            }
            // A charge no flame burned (unlit, quenched) is left to the exhaust: the global
            // kinetics are fitted to flames, not to the low-temperature chemistry of
            // autoignition in a cylinder.
            // Woschni.
            let (p, t) = {
                let l = &self.lumps[self.cylinders[ci].gas];
                (l.p, l.t)
            };
            let li = self.valve_lift_at(ci, true, deg1);
            let le = self.valve_lift_at(ci, false, deg1);
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
                l.fuel -= dfuel;
                l.derive(g);
            }
            let p1 = self.lumps[self.cylinders[ci].gas].p;
            // Torque on the crank: the gas on the piston, and the reciprocating mass.
            let omega = self.omega;
            let ddx = piston.ddx * omega * omega;
            let t_gas = (p1 - self.ambient.pressure) * piston.dv - m_rec * ddx * piston.dx;
            gas_torque += t_gas;
            // Valve seating. The rocker pins can move only with both lobes' rockers on
            // their base circles.
            let shut = [true, false].map(|intake| {
                let v = if intake {
                    &self.intake_valves
                } else {
                    &self.exhaust_valves
                };
                let k = if intake { 0 } else { 1 };
                [false, true]
                    .iter()
                    .all(|&hi| v.lift_on(deg1, hi, self.phase[k]) == 0.0)
            });
            let cyl = &mut self.cylinders[ci];
            for (k, lift) in [li, le].into_iter().enumerate() {
                if lift == 0.0 && cyl.lift_prev[k] > 0.0 {
                    cyl.seated = cyl.seated.max(cyl.lift_prev[k] / h);
                }
                cyl.lift_prev[k] = lift;
                if shut[k] {
                    cyl.high[k] = self.cam_oil;
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Build, samples};

    /// Above the switch speed the valves go over to the high lobes a cylinder at a time,
    /// each while its valves are shut, and the engine breathes more for it.
    #[test]
    fn cam_lobes_switch_on_the_base_circle() {
        let e = samples::i4_vtec();
        let run = |switch: f64| {
            let mut e = e.clone();
            e.ecu.cam_switch.as_mut().unwrap().rpm = switch;
            let (mut m, _) = Build::new(&e)
                .system("intake", &samples::i4_vtec_intake())
                .system("exhaust", &samples::i4_exhaust())
                .quality(Quality::Draft)
                .build()
                .unwrap();
            m.set_crank(0.0, 7000.0);
            let c = Controls {
                pedal: 1.0,
                load: Load::Speed(7000.0),
                ..Default::default()
            };
            for _ in 0..(0.4 / m.dt) as usize {
                let before: Vec<[bool; 2]> = m.cylinders.iter().map(|c| c.high).collect();
                m.step(&c);
                for (ci, cyl) in m.cylinders.iter().enumerate() {
                    for k in 0..2 {
                        if cyl.high[k] != before[ci][k] {
                            let deg = m.cycle_deg(ci);
                            assert_eq!(m.valve_lift_at(ci, k == 0, deg), 0.0);
                        }
                    }
                }
            }
            assert!(m.cylinders.iter().all(|c| c.high == [switch == 0.0; 2]));
            m.cylinders[0].last.trapped
        };
        let (low, high) = (run(1e9), run(0.0));
        assert!(high > 1.1 * low, "{low} {high}");
    }
}
