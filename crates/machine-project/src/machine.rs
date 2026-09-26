//! Machines: which parts a car is built from, where they sit, how the engine's gas
//! passages join, and the car's setup. `machines/<name>.ron`.
//!
//! The machine's frame is the game's body axes (x forward, y left, z up), with its origin
//! on the ground under the front axle's centre.

use open_racing_sim::params::{DifferentialParams, HeaveParams};
use serde::{Deserialize, Serialize};

/// Current format of machine files.
pub const FORMAT: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Machine {
    #[serde(default = "format")]
    pub format: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// The parts, each under an instance name.
    pub parts: Vec<Placed>,
    /// Joined gas terminals, `instance:terminal`, e.g. `("exhaust:exhaust.1",
    /// "engine:exhaust.1")`. Engine ports not listed join the one intake or exhaust
    /// terminal of the same name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gas: Vec<(String, String)>,
    pub setup: Setup,
    #[serde(default)]
    pub driver: Driver,
}

fn format() -> u32 {
    FORMAT
}

/// A part in a machine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placed {
    /// Instance name, unique in the machine.
    pub name: String,
    /// `kind/name`.
    pub part: String,
    /// Fixed to a mount of another placed part; else placed in the machine's frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attach: Option<Attach>,
    /// Offset from where it attaches (or from the machine's origin), m.
    #[serde(default, skip_serializing_if = "is_zero3")]
    pub at: [f64; 3],
    /// Rotation about x, y, z, degrees.
    #[serde(default, skip_serializing_if = "is_zero3")]
    pub rotation_deg: [f64; 3],
    /// For suspensions and wheels: which axle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axle: Option<Axle>,
}

fn is_zero3(v: &[f64; 3]) -> bool {
    *v == [0.0; 3]
}

/// Where a part attaches: the parent's mount it sits on, and which of its own mounts
/// meets it (its origin when left out).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Attach {
    pub to: String,
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Axle {
    Front,
    Rear,
}

/// The adjustable settings, as a race engineer changes them between runs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Setup {
    pub front: AxleSetup,
    pub rear: AxleSetup,
    /// Share of the brake torque at the front.
    pub brake_bias: f64,
    /// Angle of attack of named aero elements, rad.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wings: Vec<(String, f64)>,
    /// Gear ratios and the final drive, if not the gearbox's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gearing: Option<Gearing>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub differential: Option<DifferentialParams>,
    /// Fuel in the tank, l.
    #[serde(default)]
    pub fuel: f64,
    /// Ballast: position (m) and mass (kg).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ballast: Vec<([f64; 3], f64)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AxleSetup {
    /// Cold tyre pressure, bar (gauge).
    pub pressure: f64,
    /// N/m of the actuation's compression.
    pub spring_rate: f64,
    pub bump_damping: f64,
    pub rebound_damping: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_bump_damping: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_rebound_damping: Option<f64>,
    #[serde(default = "damper_knee")]
    pub damper_knee: f64,
    pub anti_roll_rate: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heave: Option<HeaveParams>,
    /// rad, negative: top in.
    pub camber: f64,
    /// rad, positive: toe-in.
    #[serde(default)]
    pub toe: f64,
    /// Floor height at the axle at rest, m.
    pub ride_height: f64,
}

fn damper_knee() -> f64 {
    0.1
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Gearing {
    pub ratios: Vec<f64>,
    pub final_drive: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Driver {
    /// kg, with the helmet and suit.
    pub mass: f64,
}

impl Default for Driver {
    fn default() -> Self {
        Self { mass: 80.0 }
    }
}

impl Machine {
    pub fn placed(&self, name: &str) -> Option<&Placed> {
        self.parts.iter().find(|p| p.name == name)
    }
}
