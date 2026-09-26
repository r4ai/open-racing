//! Parts: what a machine is built from, each designed on its own and kept in its own
//! file, `parts/<kind>/<name>.ron`.
//!
//! Every part has a physical side — shapes that stand in for its model and give its mass,
//! and mounts where other parts attach — and a design of its kind. Frames and bodies are
//! structure; engines, intakes and exhausts are simulated in detail (`open-racing-engine-
//! sim`); suspensions, wheels (with their tyres), brakes and the rest carry the game's own
//! parameters for now, until their workbenches simulate them too.
//!
//! Positions are in metres in the part's own frame: x forward, y left, z up.

use open_racing_engine_sim::{EngineSpec, Network};
use open_racing_sim::params::{
    AeroElement, BrakeParams, ClutchParams, CoolingParams, DifferentialParams, Drive,
    ElectronicsParams, GearboxParams, SteeringParams, ThrottleKind,
};
use open_racing_sim::suspension::{Actuation, Linkage};
use open_racing_sim::tire::TireParams;
use serde::{Deserialize, Serialize};

/// The kinds of part, each with its directory under `parts/`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Kind {
    Frame,
    Body,
    Aero,
    Engine,
    Intake,
    Exhaust,
    Transmission,
    Driveline,
    Suspension,
    Wheel,
    Brakes,
    Steering,
    Interior,
    FuelTank,
    Electronics,
}

impl Kind {
    pub const ALL: [Kind; 15] = [
        Kind::Frame,
        Kind::Body,
        Kind::Aero,
        Kind::Engine,
        Kind::Intake,
        Kind::Exhaust,
        Kind::Transmission,
        Kind::Driveline,
        Kind::Suspension,
        Kind::Wheel,
        Kind::Brakes,
        Kind::Steering,
        Kind::Interior,
        Kind::FuelTank,
        Kind::Electronics,
    ];

    /// Its directory under `parts/`, and the prefix of its part references.
    pub fn dir(self) -> &'static str {
        match self {
            Kind::Frame => "frame",
            Kind::Body => "body",
            Kind::Aero => "aero",
            Kind::Engine => "engine",
            Kind::Intake => "intake",
            Kind::Exhaust => "exhaust",
            Kind::Transmission => "transmission",
            Kind::Driveline => "driveline",
            Kind::Suspension => "suspension",
            Kind::Wheel => "wheel",
            Kind::Brakes => "brakes",
            Kind::Steering => "steering",
            Kind::Interior => "interior",
            Kind::FuelTank => "fuel_tank",
            Kind::Electronics => "electronics",
        }
    }

    pub fn from_dir(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.dir() == s)
    }

    /// How many of it a machine has: at most one, one per axle, or any number.
    pub fn count(self) -> Count {
        match self {
            Kind::Suspension | Kind::Wheel => Count::PerAxle,
            Kind::Intake | Kind::Exhaust | Kind::Aero | Kind::Body | Kind::FuelTank => Count::Any,
            _ => Count::One,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Count {
    One,
    PerAxle,
    Any,
}

/// A reference to a part: `kind/name`, e.g. `engine/v8_4l_flatplane`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PartRef {
    pub kind: Kind,
    pub name: String,
}

impl PartRef {
    pub fn parse(s: &str) -> Result<Self, String> {
        let (k, n) = s.split_once('/').ok_or_else(|| {
            format!("\"{s}\": a part is named kind/name, such as engine/i4_2l_na")
        })?;
        let kind = Kind::from_dir(k).ok_or_else(|| {
            format!(
                "\"{s}\": no kind \"{k}\" (one of {})",
                Kind::ALL.map(|k| k.dir()).join(", ")
            )
        })?;
        crate::check_name(n)?;
        Ok(Self {
            kind,
            name: n.to_string(),
        })
    }
}

impl std::fmt::Display for PartRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.kind.dir(), self.name)
    }
}

/// A part file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Part {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default)]
    pub physical: Physical,
    pub design: Design,
}

impl Part {
    pub fn kind(&self) -> Kind {
        self.design.kind()
    }
}

/// What a part is, by kind.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Design {
    Frame(Frame),
    Body(Body),
    Aero(Aero),
    Engine(EnginePart),
    Intake(IntakePart),
    Exhaust(ExhaustPart),
    Transmission(Transmission),
    Driveline(Driveline),
    Suspension(Suspension),
    Wheel(Wheel),
    Brakes(Brakes),
    Steering(Steering),
    Interior(Interior),
    FuelTank(FuelTank),
    Electronics(Electronics),
}

impl Design {
    pub fn kind(&self) -> Kind {
        match self {
            Design::Frame(_) => Kind::Frame,
            Design::Body(_) => Kind::Body,
            Design::Aero(_) => Kind::Aero,
            Design::Engine(_) => Kind::Engine,
            Design::Intake(_) => Kind::Intake,
            Design::Exhaust(_) => Kind::Exhaust,
            Design::Transmission(_) => Kind::Transmission,
            Design::Driveline(_) => Kind::Driveline,
            Design::Suspension(_) => Kind::Suspension,
            Design::Wheel(_) => Kind::Wheel,
            Design::Brakes(_) => Kind::Brakes,
            Design::Steering(_) => Kind::Steering,
            Design::Interior(_) => Kind::Interior,
            Design::FuelTank(_) => Kind::FuelTank,
            Design::Electronics(_) => Kind::Electronics,
        }
    }
}

/// Shapes, mass and mounts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Physical {
    #[serde(default)]
    pub shapes: Vec<Shape>,
    #[serde(default)]
    pub mounts: Vec<Mount>,
    /// The part's mass properties: from its shapes, or given.
    #[serde(default, skip_serializing_if = "MassSpec::is_shapes")]
    pub mass: MassSpec,
    /// A glTF model standing for the part in place of its shapes (later).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// A solid primitive: the part's stand-in look, its mass, and later its collision and
/// crush zones.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub name: String,
    pub kind: ShapeKind,
    /// Centre, m.
    #[serde(default)]
    pub at: [f64; 3],
    /// Rotation about x, then y, then z, degrees.
    #[serde(default, skip_serializing_if = "is_zero3")]
    pub rotation_deg: [f64; 3],
    /// kg (uniform density).
    #[serde(default)]
    pub mass: f64,
    /// sRGB, 0..1.
    #[serde(default = "grey")]
    pub colour: [f32; 3],
    /// What moves it in the game: the body, or a wheel.
    #[serde(default, skip_serializing_if = "Role::is_body")]
    pub role: Role,
}

fn is_zero3(v: &[f64; 3]) -> bool {
    *v == [0.0; 3]
}

fn grey() -> [f32; 3] {
    [0.6, 0.6, 0.62]
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ShapeKind {
    Box {
        size: [f64; 3],
    },
    Cylinder {
        radius: f64,
        length: f64,
        axis: Axis,
    },
    Sphere {
        radius: f64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    X,
    Y,
    Z,
}

/// How a shape moves with the simulated car.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    /// Sprung: fixed to the body.
    #[default]
    Body,
    /// Spins with the wheel (rim, tyre, disc): unsprung.
    Wheel,
    /// Follows the wheel without spinning (upright, caliper): unsprung.
    Hub,
    /// Half of it moves with the wheel (links, driveshafts): half unsprung.
    Link,
    /// The steering wheel, turning about the interior's steering axis.
    SteeringWheel,
}

impl Role {
    fn is_body(&self) -> bool {
        *self == Role::Body
    }

    /// Share of the mass that moves with the wheel.
    pub fn unsprung(self) -> f64 {
        match self {
            Role::Wheel | Role::Hub => 1.0,
            Role::Link => 0.5,
            _ => 0.0,
        }
    }
}

/// A named attachment point and its axes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mount {
    pub name: String,
    #[serde(default)]
    pub at: [f64; 3],
    #[serde(default, skip_serializing_if = "is_zero3")]
    pub rotation_deg: [f64; 3],
    /// How what attaches here is held: for damage, later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub joint: Option<Joint>,
}

/// The joint at a mount: stiff, and strong up to a point.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Joint {
    /// N/m and N·m/rad.
    pub stiffness: [f64; 2],
    /// Force and moment at which it gives, N and N·m.
    pub strength: [f64; 2],
}

/// Mass properties.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum MassSpec {
    /// The sum of the shapes'.
    #[default]
    Shapes,
    /// Given: kg, centre of mass (m) and principal moments about it (kg·m²).
    Given {
        mass: f64,
        centre: [f64; 3],
        inertia: [f64; 3],
    },
}

impl MassSpec {
    fn is_shapes(&self) -> bool {
        *self == MassSpec::Shapes
    }
}

/// The structure everything mounts to. Its mounts name where the other parts go.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Frame {
    /// Torsional stiffness, N·m/rad (for the chassis's flex and damage, later).
    #[serde(default)]
    pub torsional_stiffness: f64,
}

/// Bodywork.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Body {}

/// Aerodynamic elements. Their `position` is (forward, up) in the part's frame here; the
/// bake refers them to the car's centre of gravity.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Aero {
    pub elements: Vec<AeroElement>,
}

/// An engine. Its intake and exhaust are parts of their own; `bench` names the ones it is
/// developed with when it runs on its own.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnginePart {
    pub spec: EngineSpec,
    #[serde(default)]
    pub cooling: CoolingParams,
    #[serde(default)]
    pub bench: Bench,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Bench {
    /// `intake/<name>` and `exhaust/<name>` fitted on the bench.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intake: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exhaust: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IntakePart {
    pub network: Network,
    /// How the pedal works its throttles, for the game.
    #[serde(default)]
    pub throttle: ThrottleKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExhaustPart {
    pub network: Network,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transmission {
    pub clutch: ClutchParams,
    pub gearbox: GearboxParams,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Driveline {
    #[serde(default)]
    pub drive: Drive,
    pub differential: DifferentialParams,
}

/// One axle's suspension. Hardpoints are the left wheel's, relative to its wheel centre
/// at static ride height (the right mirrors them), as the game takes them; the part's
/// origin is the axle's centre at wheel-centre height.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Suspension {
    /// Distance between the wheel centres, m.
    pub track: f64,
    pub linkage: Linkage,
    #[serde(default)]
    pub actuation: Actuation,
    pub bump_travel: f64,
    pub droop_travel: f64,
    pub bump_stop_rate: f64,
}

/// A wheel and its tyre; a machine fits one per axle and mirrors it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Wheel {
    pub tire: TireParams,
    /// About the spin axis, kg·m² (rim, tyre and disc).
    pub inertia: f64,
}

/// Brakes; the balance is set up per machine.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Brakes {
    pub brakes: BrakeParams,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Steering {
    pub steering: SteeringParams,
}

/// The cockpit: where the driver sits and looks, and the steering wheel.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Interior {
    /// The driver's hip point, m.
    pub seat: [f64; 3],
    /// Between the driver's eyes, m.
    pub eye: [f64; 3],
    /// Centre of the steering wheel's rim, and the column's direction towards the driver.
    pub steering_wheel: [f64; 3],
    pub steering_axis: [f64; 3],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FuelTank {
    /// l.
    pub capacity: f64,
    /// kg/l.
    #[serde(default = "fuel_density")]
    pub density: f64,
}

fn fuel_density() -> f64 {
    0.745
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Electronics {
    pub electronics: ElectronicsParams,
}
