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
    /// Most throttle the idle control opens to hold `idle_rpm`, 0..1.
    #[serde(default = "default_idle_authority")]
    pub idle_authority: f64,
}

fn default_idle_authority() -> f64 {
    0.35
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClutchParams {
    /// Maximum transmissible torque in N·m, fully engaged and not slipping.
    pub max_torque: f64,
    /// Pedal travel (0 = released, 1 = floored) beyond which the clutch transmits nothing.
    /// Between it and the released pedal the capacity grows with the square of the travel.
    #[serde(default = "default_bite_point")]
    pub bite_point: f64,
    /// Friction while slipping relative to friction while stuck.
    #[serde(default = "default_kinetic_ratio")]
    pub kinetic_ratio: f64,
}

fn default_bite_point() -> f64 {
    0.7
}
fn default_kinetic_ratio() -> f64 {
    0.8
}

impl ClutchParams {
    /// Fraction of the capacity transmitted at `pedal`.
    pub fn engagement(&self, pedal: f64) -> f64 {
        ((self.bite_point - pedal) / self.bite_point)
            .clamp(0.0, 1.0)
            .powi(2)
    }

    /// Pedal travel that transmits `engagement` of the capacity (inverse of
    /// [`Self::engagement`]).
    pub fn pedal_for(&self, engagement: f64) -> f64 {
        self.bite_point * (1.0 - engagement.clamp(0.0, 1.0).sqrt())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GearboxParams {
    /// Forward ratios, 1st first.
    pub ratios: Vec<f64>,
    pub reverse: f64,
    pub final_drive: f64,
    /// Mechanical efficiency of gearbox + differential.
    pub efficiency: f64,
    /// Clutch disc, input shaft and the gears turning with it, kg·m² at the input shaft.
    #[serde(default = "default_input_inertia")]
    pub input_inertia: f64,
    pub kind: GearboxKind,
}

fn default_input_inertia() -> f64 {
    0.02
}

/// How gears are selected and engaged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum GearboxKind {
    /// Manual gearbox with an H-pattern lever and synchromesh: a synchroniser cone matches
    /// the input shaft to the selected gear before its dog teeth can mesh, which it can
    /// only do quickly with the clutch open. Reverse has no synchroniser.
    HPattern {
        /// Torque a synchroniser cone exerts on the input shaft, N·m.
        synchro_torque: f64,
        /// Time the driver's hand takes from one gate to the next with sequential
        /// up / down requests, s.
        lever_time: f64,
    },
    /// Sequential dog box actuated by paddles: a dog ring leaves its gear once the torque
    /// through it is low enough, a barrel turns to the next gear, and the dogs mesh at
    /// whatever speed difference the clutch then absorbs.
    Sequential {
        /// Time the barrel takes from one gear to the next, s.
        shift_time: f64,
        /// Most torque through the dogs, at the input shaft, at which they let go, N·m.
        dog_release_torque: f64,
    },
    /// Dual-clutch gearbox: odd and even gears sit on two input shafts with a clutch each,
    /// the next gear is preselected, and a shift hands the torque from one clutch to the
    /// other. Its control unit works the clutches; the car has no clutch pedal.
    DualClutch {
        /// Time over which the torque passes from one clutch to the other, s.
        shift_time: f64,
        /// Torque the clutch transmits at rest with neither pedal pressed, N·m.
        creep_torque: f64,
        /// Engine speed the control unit holds while slipping the clutch at full throttle
        /// to pull away, rpm.
        launch_rpm: f64,
    },
}

/// Engine and gearbox control electronics a car is fitted with.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ElectronicsParams {
    /// Opens the clutch as the engine slows towards stalling (not on dual-clutch
    /// gearboxes, whose control unit always does).
    pub anti_stall: Option<AntiStall>,
    /// Blips the throttle to match revs on downshifts.
    pub auto_blip: bool,
    /// Cuts the ignition to unload the dogs on sequential upshifts.
    pub ignition_cut: bool,
    /// Refuses a downshift that would spin the engine faster than this, rpm.
    pub downshift_protection_rpm: Option<f64>,
}

/// Anti-stall: while the driveline drags the engine down, the clutch opens over the
/// [`AntiStall::BAND`] above `rpm` and is fully open below it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AntiStall {
    /// Engine speed below which the clutch is fully open, rpm.
    pub rpm: f64,
}

impl AntiStall {
    /// Engine speed range over which the clutch opens, rpm.
    pub const BAND: f64 = 200.0;
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
    /// Engine and gearbox control electronics; none when left out.
    #[serde(default)]
    pub electronics: ElectronicsParams,
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
            self.engine.stall_rpm < self.engine.idle_rpm,
            "idle_rpm must exceed stall_rpm",
        )?;
        check(
            (0.0..=1.0).contains(&self.engine.idle_authority),
            "idle_authority must be in 0..1",
        )?;
        check(
            self.clutch.max_torque > 0.0,
            "clutch max_torque must be positive",
        )?;
        check(
            self.clutch.bite_point > 0.0 && self.clutch.bite_point <= 1.0,
            "clutch bite_point must be in (0, 1]",
        )?;
        check(
            self.clutch.kinetic_ratio > 0.0 && self.clutch.kinetic_ratio <= 1.0,
            "clutch kinetic_ratio must be in (0, 1]",
        )?;
        check(
            self.gearbox.input_inertia > 0.0,
            "gearbox input_inertia must be positive",
        )?;
        check(
            match self.gearbox.kind {
                GearboxKind::HPattern {
                    synchro_torque,
                    lever_time,
                } => synchro_torque > 0.0 && lever_time >= 0.0,
                GearboxKind::Sequential {
                    shift_time,
                    dog_release_torque,
                } => shift_time >= 0.0 && dog_release_torque > 0.0,
                GearboxKind::DualClutch {
                    shift_time,
                    creep_torque,
                    launch_rpm,
                } => {
                    shift_time > 0.0
                        && creep_torque >= 0.0
                        && launch_rpm > self.engine.idle_rpm
                        && launch_rpm < self.engine.limiter_rpm
                }
            },
            "gearbox kind: invalid timing, torque or launch_rpm",
        )?;
        check(
            self.electronics
                .anti_stall
                .as_ref()
                .is_none_or(|a| a.rpm > self.engine.stall_rpm),
            "anti_stall rpm must exceed stall_rpm",
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
