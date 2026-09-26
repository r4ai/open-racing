//! What an engine and its intake and exhaust systems are made of: the data the machine
//! tool's engine, intake and exhaust parts hold, in SI units. Angles of the crank and cam
//! are in degrees and named `_deg`, as on a cam card.
//!
//! An engine exposes one terminal per port, `intake.N` and `exhaust.N` for cylinder `N`
//! (numbered from 1): the open end of its port. Intake and exhaust systems are networks of
//! volumes and pipes whose pipes may end in terminals of their own; connecting two
//! terminals joins their pipes end to end. An engine port left unconnected opens to the
//! air.

use serde::{Deserialize, Serialize};

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// A four-stroke piston engine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EngineSpec {
    /// Cylinder bore, m.
    pub bore: f64,
    /// Piston stroke, m.
    pub stroke: f64,
    /// Connecting rod length between centres, m.
    pub rod: f64,
    /// Geometric compression ratio.
    pub compression_ratio: f64,
    /// Offset of the gudgeon pin from the bore's axis, towards the thrust side, m.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pin_offset: f64,
    pub layout: Layout,
    pub intake: Head,
    pub exhaust: Head,
    pub crank: Crank,
    pub combustion: Combustion,
    #[serde(default)]
    pub heat_transfer: HeatTransfer,
    #[serde(default)]
    pub friction: Friction,
    pub ecu: Ecu,
}

/// Where the cylinders are: banks at angles to the vertical, crank throws at angles round
/// the crank, and each cylinder on a bank and a throw.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub banks: Vec<Bank>,
    /// Angle of each crank throw (crankpin) from the first, in the direction of rotation.
    pub throws_deg: Vec<f64>,
    /// Cylinders in their numbering order (cylinder 1 first).
    pub cylinders: Vec<CylinderPlace>,
    /// Cylinder numbers in firing order, from 1.
    pub firing_order: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bank {
    pub name: String,
    /// Angle of the bank's cylinder axis from the vertical, in the crank's direction of
    /// rotation, degrees.
    pub angle_deg: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CylinderPlace {
    pub bank: String,
    /// Index into `Layout::throws_deg`.
    pub throw: usize,
}

/// One side (intake or exhaust) of the cylinder head: valves, their cam and the port.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Head {
    pub valves: Valves,
    pub cam: Cam,
    /// A second, higher lobe the ECU switches the valves to (VTEC and the like).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub high_cam: Option<Cam>,
    pub port: Port,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Valves {
    /// Valves per cylinder.
    pub count: u32,
    /// Head (seat) diameter, m.
    pub diameter: f64,
    /// Stem diameter, m.
    #[serde(default)]
    pub stem: f64,
    /// Discharge coefficient against lift over diameter, referred to the curtain area
    /// π·d·L (or the throat, when smaller).
    pub cd: Vec<(f64, f64)>,
}

/// A cam lobe, in crank degrees.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cam {
    /// Peak valve lift, m.
    pub lift: f64,
    /// From opening to closing (zero lift), crank degrees.
    pub duration_deg: f64,
    /// Angle of peak lift: after the gas-exchange TDC for an intake cam, before it for an
    /// exhaust cam, crank degrees.
    pub centreline_deg: f64,
    #[serde(default)]
    pub profile: Profile,
}

/// Shape of the lift curve.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Profile {
    /// The 4-5-6-7 polynomial on each flank: zero velocity and acceleration at the seat
    /// and at the nose.
    #[default]
    Polynomial,
    /// Lift as a share of the peak against the share of the duration, 0..1 each.
    Table(Vec<(f64, f64)>),
}

/// The port and its runner stub in the head, from the valve seat to the terminal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Port {
    pub length: f64,
    /// Diameter along the port from the valve (0) to the terminal (1), m.
    pub diameter: Vec<(f64, f64)>,
    /// Wall temperature, K.
    pub wall_temperature: f64,
}

/// Rotating and reciprocating parts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Crank {
    /// Flywheel (and clutch cover), kg·m².
    pub flywheel_inertia: f64,
    /// Crankshaft, damper and timing gear, kg·m².
    pub crank_inertia: f64,
    /// Piston, rings, pin and the small end of the rod, per cylinder, kg.
    pub reciprocating_mass: f64,
    /// Big end of the rod, per cylinder, kg.
    pub rotating_mass: f64,
}

/// The fuel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Fuel {
    #[default]
    Gasoline,
    E85,
    Methanol,
    Custom {
        /// Lower heating value, J/kg.
        lhv: f64,
        /// Stoichiometric air-fuel mass ratio.
        afr: f64,
    },
}

impl Fuel {
    /// (lower heating value J/kg, stoichiometric air-fuel ratio).
    pub fn properties(self) -> (f64, f64) {
        match self {
            Fuel::Gasoline => (43.4e6, 14.6),
            Fuel::E85 => (29.2e6, 9.8),
            Fuel::Methanol => (19.9e6, 6.45),
            Fuel::Custom { lhv, afr } => (lhv, afr),
        }
    }
}

/// Heat release: a Wiebe function from the spark.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Combustion {
    #[serde(default)]
    pub fuel: Fuel,
    /// Wiebe efficiency parameter `a` (5: 99.3 % burned at the end).
    pub wiebe_a: f64,
    /// Wiebe form factor `m`.
    pub wiebe_m: f64,
    /// Burn duration (spark to end of combustion) at `reference_rpm`, crank degrees.
    pub duration_deg: f64,
    pub reference_rpm: f64,
    /// Duration ∝ (rpm / reference)^exponent: turbulence, and so flame speed, grows with
    /// the piston speed, so the burn takes less than proportionally more crank angle.
    pub speed_exponent: f64,
    /// Share of the fuel's heat released (the rest leaves unburned or dissociated).
    pub efficiency: f64,
    /// Relative cycle-to-cycle spread of the flame's development (its start and its
    /// duration together), without residual gas; 0.03 gives a COV of IMEP of about 1–2 %
    /// at full load and near 8 % at idle.
    #[serde(default)]
    pub variation: f64,
    #[serde(default)]
    pub seed: u64,
    /// Research octane number of the fuel, for the knock margin.
    #[serde(default = "default_octane")]
    pub octane: f64,
}

fn default_octane() -> f64 {
    98.0
}

/// Woschni's in-cylinder heat transfer (SAE 670931).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeatTransfer {
    /// Multiplier on Woschni's coefficient.
    pub scale: f64,
    /// Wall temperatures, K.
    pub piston: f64,
    pub head: f64,
    pub liner: f64,
}

impl Default for HeatTransfer {
    fn default() -> Self {
        Self {
            scale: 1.0,
            piston: 560.0,
            head: 500.0,
            liner: 420.0,
        }
    }
}

/// Mechanical friction as a friction mean effective pressure, Chen & Flynn (SAE 650733):
/// `fmep = constant + peak_pressure·p_max + piston_speed·S̄p + piston_speed_sq·S̄p²`,
/// with the oil's viscosity scaling the speed terms. Pumping losses are not in it: the
/// gas dynamics give them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Friction {
    /// Pa.
    pub constant: f64,
    /// Share of the cycle's peak cylinder pressure.
    pub peak_pressure: f64,
    /// Pa per m/s of mean piston speed.
    pub piston_speed: f64,
    /// Pa per (m/s)².
    pub piston_speed_sq: f64,
    /// Accessories driven by the crank (oil, water and fuel pumps, alternator), Pa.
    pub accessories: f64,
}

impl Default for Friction {
    fn default() -> Self {
        Self {
            constant: 0.30e5,
            peak_pressure: 0.004,
            piston_speed: 0.050e5,
            piston_speed_sq: 0.0008e5,
            accessories: 0.10e5,
        }
    }
}

/// The engine's control unit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ecu {
    pub idle_rpm: f64,
    pub limiter_rpm: f64,
    /// Below this the engine does not fire (the starter's speed is above it).
    pub stall_rpm: f64,
    /// Fuel is cut above the limiter until the speed falls this far below it, rpm.
    #[serde(default = "default_hysteresis")]
    pub limiter_hysteresis_rpm: f64,
    /// Throttle opening that holds the idle speed with the pedal up (the idle air), 0..1;
    /// the idle control trims round it.
    #[serde(default = "default_idle_opening")]
    pub idle_opening: f64,
    /// Most the idle control opens beyond `idle_opening`, 0..1.
    #[serde(default = "default_idle_authority")]
    pub idle_authority: f64,
    /// Throttle the idle control opens per rpm below `idle_rpm`, per second (its integral
    /// part; the proportional part is half a second of it).
    #[serde(default = "default_idle_gain")]
    pub idle_gain: f64,
    /// What the limiter cuts. Cutting the spark leaves the fuel to go down the exhaust
    /// unburned, where it bangs.
    #[serde(default, skip_serializing_if = "is_fuel_cut")]
    pub limiter_cut: Cut,
    /// Fuel is cut with the throttle shut above this speed (overrun), rpm.
    #[serde(default)]
    pub overrun_cut_rpm: Option<f64>,
    /// A pop map: on overrun, instead of cutting the fuel, keep fuelling and fire late,
    /// so the flame is still burning when the exhaust valve opens and the charge goes on
    /// burning, and banging, in the exhaust.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pops: Option<Pops>,
    /// When the valves go over to the heads' high lobes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cam_switch: Option<CamSwitch>,
    /// Cam phasers: advance of the intake and of the exhaust cam, crank degrees, against
    /// rpm and load (none: the cam as ground).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intake_phase: Option<Map2>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exhaust_phase: Option<Map2>,
    /// Spark advance before the firing TDC, crank degrees, against rpm and load (the
    /// throttle, 0..1).
    pub spark_deg: Map2,
    /// Excess-air ratio λ against rpm and load.
    pub lambda: Map2,
}

/// Switching over to the high lobes: above a speed with enough load, back below it less
/// the hysteresis.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CamSwitch {
    pub rpm: f64,
    #[serde(default = "default_cam_hysteresis")]
    pub hysteresis_rpm: f64,
    /// Least throttle, 0..1.
    #[serde(default)]
    pub min_load: f64,
}

fn default_cam_hysteresis() -> f64 {
    200.0
}

/// What an ECU cuts to hold a speed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cut {
    #[default]
    Fuel,
    Spark,
}

/// Overrun fuelling with late spark.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pops {
    /// Above this speed, rpm.
    pub above_rpm: f64,
    /// Spark advance, crank degrees before the firing TDC (negative: after it).
    pub spark_deg: f64,
    /// Excess-air ratio.
    pub lambda: f64,
    /// Throttle the ECU opens meanwhile, 0..1: behind a shut plate the charge is so
    /// diluted with residual gas that the flame will not light, and the unburned mixture
    /// burns steadily in the hot manifold instead of popping.
    #[serde(default)]
    pub throttle: f64,
}

fn is_fuel_cut(c: &Cut) -> bool {
    *c == Cut::Fuel
}

fn is_origin(v: &[f64; 3]) -> bool {
    *v == [0.0; 3]
}

fn default_hysteresis() -> f64 {
    150.0
}
fn default_idle_opening() -> f64 {
    0.01
}
fn default_idle_authority() -> f64 {
    0.04
}
fn default_idle_gain() -> f64 {
    0.00005
}

/// A table over rpm and load: `values[i][j]` at `rpm[i]`, `load[j]`; bilinear between and
/// held beyond the axes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Map2 {
    pub rpm: Vec<f64>,
    pub load: Vec<f64>,
    pub values: Vec<Vec<f64>>,
}

impl Map2 {
    pub fn constant(v: f64) -> Self {
        Self {
            rpm: vec![0.0],
            load: vec![0.0],
            values: vec![vec![v]],
        }
    }

    pub fn at(&self, rpm: f64, load: f64) -> f64 {
        let (i, fi) = axis(&self.rpm, rpm);
        let (j, fj) = axis(&self.load, load);
        let v = |a: usize, b: usize| self.values[a][b];
        let i1 = (i + 1).min(self.rpm.len() - 1);
        let j1 = (j + 1).min(self.load.len() - 1);
        let a = v(i, j) + fj * (v(i, j1) - v(i, j));
        let b = v(i1, j) + fj * (v(i1, j1) - v(i1, j));
        a + fi * (b - a)
    }

    pub fn is_valid(&self) -> bool {
        !self.rpm.is_empty()
            && !self.load.is_empty()
            && self.values.len() == self.rpm.len()
            && self.values.iter().all(|r| r.len() == self.load.len())
            && self.rpm.windows(2).all(|w| w[1] > w[0])
            && self.load.windows(2).all(|w| w[1] > w[0])
    }
}

/// Interval and fraction of `x` on an ascending axis.
fn axis(a: &[f64], x: f64) -> (usize, f64) {
    if a.len() < 2 || x <= a[0] {
        return (0, 0.0);
    }
    for i in 0..a.len() - 1 {
        if x <= a[i + 1] {
            return (i, (x - a[i]) / (a[i + 1] - a[i]));
        }
    }
    (a.len() - 1, 0.0)
}

/// An intake or exhaust system: volumes joined by pipes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Network {
    #[serde(default)]
    pub volumes: Vec<Volume>,
    #[serde(default)]
    pub pipes: Vec<PipeSpec>,
    /// Restrictions straight between two volumes (or a volume and the air).
    #[serde(default)]
    pub orifices: Vec<Orifice>,
    /// Turbochargers' compressors (in an intake) and turbines (in an exhaust); a
    /// compressor and a turbine on the same `shaft` name, in whichever systems, make one
    /// turbocharger.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compressors: Vec<Compressor>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turbines: Vec<Turbine>,
}

/// A centrifugal compressor between two volumes: air drawn from `inlet` through the
/// impeller and diffuser into `outlet` (the volute and the charge pipe's start).
///
/// Its characteristic is set by five figures of the map in dimensionless form, with the
/// impeller's tip speed U = ω·D/2 and the flow coefficient φ = ṁ/(ρ₀·U·D²): the
/// isentropic head coefficient Δh_s/U² at the surge line (`head`, the top of each speed
/// line) and at no flow (`shutoff` of it), the flow coefficients at the surge line and at
/// choke, and the best efficiency.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Compressor {
    pub name: String,
    pub shaft: String,
    pub inlet: String,
    pub outlet: String,
    /// Impeller tip (exducer) diameter, m.
    pub wheel: f64,
    /// Inducer diameter, m.
    pub inducer: f64,
    /// Full blades (as many splitters between them sound at twice the rate).
    pub blades: u32,
    pub head: f64,
    #[serde(default = "default_shutoff")]
    pub shutoff: f64,
    pub surge_flow: f64,
    pub choke_flow: f64,
    pub efficiency: f64,
    /// Length of the flow path through it, for the inertia of the air in it (the L of
    /// Greitzer's surge model), m.
    #[serde(default = "default_compressor_duct")]
    pub duct_length: f64,
    /// Where it is in the part's frame, for its sound, m.
    #[serde(default)]
    pub at: [f64; 3],
}

fn default_shutoff() -> f64 {
    0.85
}
fn default_compressor_duct() -> f64 {
    0.25
}

/// A radial turbine between two volumes of an exhaust: from `inlet` (the manifold or
/// volute) to `outlet` (the downpipe's start).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Turbine {
    pub name: String,
    pub shaft: String,
    pub inlet: String,
    pub outlet: String,
    /// Wheel tip diameter, m.
    pub wheel: f64,
    /// Effective area of its nozzle (the housing's A/R and the wheel's throat together:
    /// its swallowing capacity), m².
    pub area: f64,
    /// Best total-to-static efficiency, at a blade speed ratio U/C_s of 0.7.
    pub efficiency: f64,
    /// Inertia of the shaft with both wheels, kg·m².
    pub inertia: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wastegate: Option<Wastegate>,
}

/// A wastegate: a poppet valve round the turbine, its diaphragm pushed by the boost (the
/// compressor's outlet over the air) against a spring.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Wastegate {
    pub diameter: f64,
    /// Boost at which it starts to open, and at which it is fully open, Pa.
    pub opens: f64,
    pub open: f64,
}

/// A volume in which the gas is taken as uniform: a plenum, an air box, a collector, a
/// silencer's chamber, a resonator's cavity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Volume {
    pub name: String,
    /// m³.
    pub volume: f64,
    #[serde(default = "default_wall")]
    pub wall_temperature: f64,
}

fn default_wall() -> f64 {
    320.0
}

/// A pipe from end `a` to end `b`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PipeSpec {
    pub name: String,
    pub length: f64,
    /// Diameter along the pipe from `a` (0) to `b` (1), m.
    pub diameter: Vec<(f64, f64)>,
    #[serde(default = "default_wall")]
    pub wall_temperature: f64,
    /// Relative roughness of the wall.
    #[serde(default = "default_roughness")]
    pub roughness: f64,
    /// Multiplier on the wall friction: a catalyst's brick, a filter's element, bends.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub friction: f64,
    /// Multiplier on the wall heat transfer: an intercooler's core, its fins and
    /// many small tubes, is a pipe with a great deal of it.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub heat: f64,
    pub a: End,
    pub b: End,
}

fn default_roughness() -> f64 {
    1e-4
}
fn one() -> f64 {
    1.0
}
fn is_one(v: &f64) -> bool {
    *v == 1.0
}

/// What a pipe's end opens into.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum End {
    Closed,
    Volume {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        restriction: Option<Restriction>,
    },
    /// Open to the air: a snorkel's mouth, a tailpipe. `at` is where, in the part's frame
    /// (m), for the sound.
    Ambient {
        #[serde(default)]
        at: [f64; 3],
        #[serde(default, skip_serializing_if = "Option::is_none")]
        restriction: Option<Restriction>,
    },
    /// Joined to another part's terminal, or to an engine port.
    Terminal(String),
    /// Joined straight to the one other pipe end of this network with the same joint name:
    /// a change of section (a silencer's chamber, a megaphone) or a bend between pipes.
    Join(String),
}

/// A restriction to the flow.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Restriction {
    /// A fixed orifice: discharge coefficient and diameter.
    Fixed { cd: f64, diameter: f64 },
    /// A blow-off (diverter) valve: a spring-loaded poppet between the charge pipe and
    /// the air (or the compressor's inlet), pushed open when the charge's pressure is
    /// `opens` Pa over that of the `reference` volume (the manifold behind the throttle),
    /// fully open `span` Pa above that.
    BlowOff {
        diameter: f64,
        reference: String,
        opens: f64,
        #[serde(default = "default_blow_off_span")]
        span: f64,
    },
    /// A butterfly throttle worked by the pedal.
    Throttle {
        bore: f64,
        #[serde(default)]
        shaft: f64,
        #[serde(default = "default_closed_angle")]
        closed_angle_deg: f64,
    },
}

fn default_blow_off_span() -> f64 {
    0.2e5
}

fn default_closed_angle() -> f64 {
    7.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Orifice {
    pub name: String,
    /// Names of two volumes, or of a volume and `"ambient"`.
    pub between: (String, String),
    pub restriction: Restriction,
    /// Where it vents, when to the air, in the part's frame, m (for its sound).
    #[serde(default, skip_serializing_if = "is_origin")]
    pub at: [f64; 3],
}

/// Open area of a butterfly throttle at a pedal opening 0..1, m²: the plate turns from
/// its closed angle to 90°. Heywood, *Internal Combustion Engine Fundamentals*, eq. 7.14,
/// with the shaft (diameter `shaft`); once the plate's edge clears the shaft's shadow,
/// the bore less the shaft's section.
pub fn throttle_area(bore: f64, shaft: f64, closed_angle_deg: f64, opening: f64) -> f64 {
    use std::f64::consts::{FRAC_PI_2, PI};
    let bore_area = PI * 0.25 * bore * bore;
    let psi0 = closed_angle_deg.to_radians();
    let psi = psi0 + opening.clamp(0.0, 1.0) * (FRAC_PI_2 - psi0);
    let a = (shaft / bore).clamp(0.0, 0.5);
    let (c, c0) = (psi.cos(), psi0.cos());
    let open = if c > a * c0 {
        bore_area * (1.0 - c / c0)
            + 0.5
                * bore
                * bore
                * (a / c * (c * c - a * a * c0 * c0).sqrt() + c / c0 * (a * c0 / c).asin()
                    - a * (1.0 - a * a).sqrt()
                    - a.asin())
    } else {
        bore_area - shaft * bore
    };
    // Leakage past a shut plate.
    open.max(0.0) + 0.002 * bore_area
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_is_bilinear() {
        let m = Map2 {
            rpm: vec![1000.0, 3000.0],
            load: vec![0.0, 1.0],
            values: vec![vec![10.0, 20.0], vec![30.0, 40.0]],
        };
        assert_eq!(m.at(2000.0, 0.5), 25.0);
        assert_eq!(m.at(500.0, 2.0), 20.0);
        assert!(m.is_valid());
    }

    #[test]
    fn throttle_opens_progressively() {
        let bore_area = std::f64::consts::PI * 0.25 * 0.06 * 0.06;
        let a0 = throttle_area(0.06, 0.0, 7.0, 0.0);
        let a1 = throttle_area(0.06, 0.0, 7.0, 1.0);
        assert!(a0 < 0.01 * bore_area);
        assert!((a1 - bore_area).abs() / bore_area < 0.01);
        // With a shaft: opening grows from nothing, and the shaft blocks some when wide open.
        let mut last = 0.0;
        for k in 0..=20 {
            let a = throttle_area(0.06, 0.008, 7.0, k as f64 / 20.0);
            assert!(a >= last - 1e-12, "{k}: {a} < {last}");
            last = a;
        }
        assert!(throttle_area(0.06, 0.008, 7.0, 0.15) > 0.03 * bore_area);
        assert!((last - (bore_area - 0.008 * 0.06) - 0.002 * bore_area).abs() < 0.02 * bore_area);
        assert!(throttle_area(0.06, 0.0, 7.0, 0.2) < 0.25 * bore_area);
    }
}
