//! Data-driven car description (`CarParams`, loaded from RON) and the derived,
//! simulation-ready `CarModel`. Tyres are separate RON files the car names per axle.

use std::path::{Path, PathBuf};

use glam::DVec3;
use serde::{Deserialize, Serialize};

use crate::GRAVITY;
use crate::tire::{TireModel, TireParams};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AxleParams {
    /// Tyre fitted to this axle: a name in `assets/tires/` (the `tires` directory next to
    /// the car's directory), or a `.ron` path relative to the car file.
    pub tire: String,
    /// Cold inflation pressure set at the ambient temperature, bar (gauge).
    pub pressure: f64,
    /// Mass of one wheel assembly (wheel, tyre, upright, brake, half of the links) in kg.
    pub unsprung_mass: f64,
    /// Rotational inertia of one wheel about its spin axis in kg·m².
    pub wheel_inertia: f64,
    /// Wheel rate of the spring in N/m.
    pub spring_rate: f64,
    /// Damping in compression / extension in N·s/m (wheel rate).
    pub bump_damping: f64,
    pub rebound_damping: f64,
    /// Anti-roll bar rate in N/m of left-right travel difference.
    pub anti_roll_rate: f64,
    /// Travel available from static ride height, in m.
    pub bump_travel: f64,
    pub droop_travel: f64,
    /// Stiffness of the progressive bump stop in N/m.
    pub bump_stop_rate: f64,
    /// Static camber in radians, negative = top leaning inwards.
    pub static_camber: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SteeringParams {
    /// Steering wheel angle / road wheel angle.
    pub ratio: f64,
    /// Steering wheel lock in radians (each direction).
    pub lock: f64,
    /// 0 = parallel steer, 1 = full Ackermann.
    pub ackermann: f64,
    /// Caster angle in radians: the steering axis leans back at the top.
    #[serde(default = "default_caster")]
    pub caster: f64,
    /// Kingpin inclination in radians: the steering axis leans inwards at the top.
    #[serde(default = "default_kingpin_inclination")]
    pub kingpin_inclination: f64,
    /// Mechanical trail in m: how far the contact patch trails the point where the
    /// steering axis meets the ground.
    #[serde(default = "default_trail")]
    pub trail: f64,
    /// Scrub radius in m: how far the contact patch lies outboard of that point.
    #[serde(default = "default_scrub_radius")]
    pub scrub_radius: f64,
}

fn default_caster() -> f64 {
    0.12
}
fn default_kingpin_inclination() -> f64 {
    0.15
}
fn default_trail() -> f64 {
    0.025
}
fn default_scrub_radius() -> f64 {
    0.015
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BrakeParams {
    /// Total brake torque at full pedal in N·m (all four wheels).
    pub max_torque: f64,
    /// Fraction of torque on the front axle.
    pub front_bias: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EngineParams {
    /// Full-throttle torque curve: (rpm, N·m), ascending rpm.
    pub torque_curve: Vec<(f64, f64)>,
    /// Closed-throttle friction / pumping torque: (rpm, N·m positive), ascending rpm.
    pub drag_curve: Vec<(f64, f64)>,
    /// Crank + flywheel inertia in kg·m².
    pub inertia: f64,
    pub idle_rpm: f64,
    /// Rev limiter cuts fuel above this.
    pub limiter_rpm: f64,
    /// Engine stops below this unless the clutch is open.
    pub stall_rpm: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClutchParams {
    /// Maximum transmissible torque in N·m.
    pub max_torque: f64,
    /// Anti-stall: clutch capacity fades to zero between `stall_rpm` and this rpm.
    pub anti_stall_rpm: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GearboxParams {
    /// Forward ratios, 1st first.
    pub ratios: Vec<f64>,
    pub reverse: f64,
    pub final_drive: f64,
    /// Time with no drive during a gear change, in s.
    pub shift_time: f64,
    /// Mechanical efficiency of gearbox + differential.
    pub efficiency: f64,
}

/// A limited-slip differential: it splits its input torque and moves up to its locking
/// torque from the faster output to the slower one. An open differential has no locking
/// torque.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DifferentialParams {
    /// Locking torque with no input torque, N·m.
    pub preload: f64,
    /// Locking torque per N·m of input torque on power / on overrun.
    pub power_ramp: f64,
    pub coast_ramp: f64,
}

/// Which wheels the engine drives.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Drive {
    /// The rear wheels, through `CarParams::differential`.
    #[default]
    Rear,
    /// The front wheels, through `CarParams::differential`.
    Front,
    /// All four wheels. A centre differential splits the torque between the axles; the
    /// rear axle's differential is `CarParams::differential`.
    All {
        /// Share of the torque sent to the front axle, 0..1.
        front_share: f64,
        centre_differential: DifferentialParams,
        front_differential: DifferentialParams,
    },
}

impl Drive {
    /// Whether the engine drives the front (`true`) or rear axle.
    pub fn drives(&self, front: bool) -> bool {
        match self {
            Self::Rear => !front,
            Self::Front => front,
            Self::All { .. } => true,
        }
    }

    /// Share of the torque sent to the front axle.
    pub fn front_share(&self) -> f64 {
        match self {
            Self::Rear => 0.0,
            Self::Front => 1.0,
            Self::All { front_share, .. } => *front_share,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AeroParams {
    /// Drag coefficient × frontal area, m².
    pub drag_area: f64,
    /// Downforce coefficient × area at the front / rear axle, m².
    pub downforce_area_front: f64,
    pub downforce_area_rear: f64,
    /// Height of the centre of pressure above the CG, m.
    pub drag_height: f64,
}

/// Complete description of a car, deserialised from `assets/cars/*.ron`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CarParams {
    pub name: String,
    /// Total mass including driver and fuel, kg.
    pub mass: f64,
    /// Principal moments of inertia about roll, pitch, yaw axes (body x, y, z), kg·m².
    pub inertia: [f64; 3],
    /// CG height above the ground at static ride height, m.
    pub cg_height: f64,
    /// Fraction of static weight on the front axle.
    pub front_weight: f64,
    pub wheelbase: f64,
    pub track_front: f64,
    pub track_rear: f64,
    pub front: AxleParams,
    pub rear: AxleParams,
    pub steering: SteeringParams,
    pub brakes: BrakeParams,
    pub engine: EngineParams,
    pub clutch: ClutchParams,
    pub gearbox: GearboxParams,
    /// Driven wheels; rear-wheel drive when left out.
    #[serde(default)]
    pub drive: Drive,
    /// Differential of the driven axle; for all-wheel drive, of the rear axle.
    pub differential: DifferentialParams,
    pub aero: AeroParams,
}

#[derive(Debug)]
pub enum ParamsError {
    Io(std::io::Error),
    Parse(ron::error::SpannedError),
    Invalid(&'static str),
}

impl std::fmt::Display for ParamsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "car params: {e}"),
            Self::Parse(e) => write!(f, "car params: {e}"),
            Self::Invalid(msg) => write!(f, "car params: {msg}"),
        }
    }
}

impl std::error::Error for ParamsError {}

impl TireParams {
    pub fn from_ron(src: &str) -> Result<Self, ParamsError> {
        ron::from_str(src).map_err(ParamsError::Parse)
    }

    pub fn load(path: impl AsRef<std::path::Path>) -> Result<Self, ParamsError> {
        Self::from_ron(&std::fs::read_to_string(path).map_err(ParamsError::Io)?)
    }
}

impl CarParams {
    pub fn from_ron(src: &str) -> Result<Self, ParamsError> {
        ron::from_str(src).map_err(ParamsError::Parse)
    }

    pub fn load(path: impl AsRef<std::path::Path>) -> Result<Self, ParamsError> {
        Self::from_ron(&std::fs::read_to_string(path).map_err(ParamsError::Io)?)
    }

    fn validate(&self) -> Result<(), ParamsError> {
        let check = |ok: bool, msg| {
            if ok {
                Ok(())
            } else {
                Err(ParamsError::Invalid(msg))
            }
        };
        check(self.mass > 0.0, "mass must be positive")?;
        check(
            self.inertia.iter().all(|&i| i > 0.0),
            "inertia must be positive",
        )?;
        check(
            (0.0..=1.0).contains(&self.front_weight),
            "front_weight must be in 0..1",
        )?;
        check(
            !self.gearbox.ratios.is_empty(),
            "gearbox needs at least one ratio",
        )?;
        check(
            self.engine.torque_curve.len() >= 2,
            "torque_curve needs two points",
        )?;
        check(
            self.engine.drag_curve.len() >= 2,
            "drag_curve needs two points",
        )?;
        check(
            self.engine.stall_rpm < self.clutch.anti_stall_rpm,
            "anti_stall_rpm must exceed stall_rpm",
        )?;
        check(
            self.front.pressure > 0.0 && self.rear.pressure > 0.0,
            "tyre pressure must be positive",
        )?;
        check(
            (0.0..=1.0).contains(&self.drive.front_share()),
            "drive: front_share must be in 0..1",
        )?;
        let sprung = self.mass - 2.0 * (self.front.unsprung_mass + self.rear.unsprung_mass);
        check(sprung > 0.0, "unsprung mass exceeds total mass")
    }
}

/// Per-corner constants derived from `CarParams`.
#[derive(Clone, Debug)]
pub struct CornerModel {
    /// Suspension top mount in body coordinates (the strut runs along body −z from here).
    pub hardpoint: DVec3,
    /// +1 for left wheels, −1 for right wheels.
    pub side: f64,
    pub front: bool,
    pub driven: bool,
    /// Static extension of the wheel below the hardpoint.
    pub static_extension: f64,
    /// Extension at which the spring is unloaded.
    pub spring_free_extension: f64,
    pub min_extension: f64,
    pub max_extension: f64,
}

/// Simulation-ready car: validated params plus derived constants.
#[derive(Clone, Debug)]
pub struct CarModel {
    pub params: CarParams,
    pub sprung_mass: f64,
    pub corners: [CornerModel; 4],
    pub front_tire: TireModel,
    pub rear_tire: TireModel,
}

impl CarModel {
    /// Builds the model of `params` fitted with the given front and rear tyres.
    pub fn new(
        params: CarParams,
        front_tire: TireParams,
        rear_tire: TireParams,
    ) -> Result<Self, ParamsError> {
        params.validate()?;
        let p = &params;
        let unsprung_total = 2.0 * (p.front.unsprung_mass + p.rear.unsprung_mass);
        let sprung_mass = p.mass - unsprung_total;

        let front_x = p.wheelbase * (1.0 - p.front_weight);
        let rear_x = -p.wheelbase * p.front_weight;

        let corner = |front: bool, side: f64| {
            let axle = if front { &p.front } else { &p.rear };
            let tire = if front { &front_tire } else { &rear_tire };
            let axle_weight = if front {
                p.front_weight
            } else {
                1.0 - p.front_weight
            };
            // Load carried by the spring at this corner in static equilibrium.
            let corner_sprung = 0.5 * p.mass * axle_weight - axle.unsprung_mass;
            let spring_load = corner_sprung * GRAVITY;
            let tire_load = spring_load + axle.unsprung_mass * GRAVITY;
            let tire_deflection = tire_load / tire.vertical_stiffness;
            // Hardpoints at CG height ⇒ extension = CG height − wheel-centre height.
            let static_extension = p.cg_height - (tire.radius - tire_deflection);
            let track = if front { p.track_front } else { p.track_rear };
            CornerModel {
                hardpoint: DVec3::new(
                    if front { front_x } else { rear_x },
                    side * 0.5 * track,
                    0.0,
                ),
                side,
                front,
                driven: p.drive.drives(front),
                static_extension,
                spring_free_extension: static_extension + spring_load / axle.spring_rate,
                min_extension: static_extension - axle.bump_travel,
                max_extension: static_extension + axle.droop_travel,
            }
        };

        let corners = [
            corner(true, 1.0),
            corner(true, -1.0),
            corner(false, 1.0),
            corner(false, -1.0),
        ];
        Ok(Self {
            sprung_mass,
            corners,
            front_tire: TireModel::new(front_tire),
            rear_tire: TireModel::new(rear_tire),
            params,
        })
    }

    /// Parses a car and fits the tyres it names, read by `tire`.
    pub fn from_ron(
        src: &str,
        tire: impl Fn(&str) -> Result<TireParams, ParamsError>,
    ) -> Result<Self, ParamsError> {
        let params = CarParams::from_ron(src)?;
        let (front, rear) = (tire(&params.front.tire)?, tire(&params.rear.tire)?);
        Self::new(params, front, rear)
    }

    /// Loads a car file and the tyres it names (see [`AxleParams::tire`]).
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ParamsError> {
        let path = path.as_ref();
        let dir = path.parent().unwrap_or(Path::new("."));
        Self::from_ron(
            &std::fs::read_to_string(path).map_err(ParamsError::Io)?,
            |name| TireParams::load(tire_path(dir, name)),
        )
    }

    /// The bundled GT3-class car on GT3-class slicks.
    pub fn gt3() -> Self {
        Self::from_ron(include_str!("../../../assets/cars/gt3.ron"), |name| {
            TireParams::from_ron(match name {
                "velloni_zeta_gt_front" => {
                    include_str!("../../../assets/tires/velloni_zeta_gt_front.ron")
                }
                "velloni_zeta_gt_rear" => {
                    include_str!("../../../assets/tires/velloni_zeta_gt_rear.ron")
                }
                _ => {
                    return Err(ParamsError::Invalid(
                        "bundled gt3.ron names a tyre that is not bundled",
                    ));
                }
            })
        })
        .expect("bundled gt3.ron is valid")
    }

    #[inline]
    pub fn axle(&self, wheel: usize) -> &AxleParams {
        if wheel < 2 {
            &self.params.front
        } else {
            &self.params.rear
        }
    }

    #[inline]
    pub fn tire(&self, wheel: usize) -> &TireModel {
        if wheel < 2 {
            &self.front_tire
        } else {
            &self.rear_tire
        }
    }
}

/// Path of the tyre `name` for a car file in `car_dir`.
pub fn tire_path(car_dir: &Path, name: &str) -> PathBuf {
    if Path::new(name).extension().is_some() {
        car_dir.join(name)
    } else {
        car_dir.join("../tires").join(format!("{name}.ron"))
    }
}

/// Piecewise-linear lookup in an ascending (x, y) table, clamped at both ends.
#[inline]
pub(crate) fn lookup(table: &[(f64, f64)], x: f64) -> f64 {
    let first = table[0];
    if x <= first.0 {
        return first.1;
    }
    for w in table.windows(2) {
        let (a, b) = (w[0], w[1]);
        if x <= b.0 {
            return a.1 + (b.1 - a.1) * (x - a.0) / (b.0 - a.0);
        }
    }
    table[table.len() - 1].1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_reads_the_tyres_the_car_names() {
        let car =
            CarModel::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/cars/gt3.ron"))
                .unwrap();
        let bundled = CarModel::gt3();
        assert_eq!(car.front_tire.p.name, bundled.front_tire.p.name);
        assert_eq!(car.rear_tire.p.name, bundled.rear_tire.p.name);
        assert_ne!(car.front_tire.p.name, car.rear_tire.p.name);
    }
}
