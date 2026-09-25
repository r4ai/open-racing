//! Data-driven car description (`CarParams`, loaded from RON) and the derived,
//! simulation-ready `CarModel`. Tyres are separate RON files the car names per axle.

use std::path::{Path, PathBuf};

use glam::DVec3;
use serde::{Deserialize, Serialize};

use crate::GRAVITY;
use crate::brakes::BrakeModel;
use crate::engine::EngineModel;
use crate::suspension::{Actuation, Alignment, Kinematics, Linkage, Pose, Summary};
use crate::tire::{TireModel, TireParams};

/// How far the linkage reaches beyond the bump stops, m: the hard limits of the travel.
pub const OVERTRAVEL: f64 = 0.03;

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
    /// How the upright is guided: the left wheel's hardpoints (see [`crate::suspension`]).
    /// The steering axis, the Ackermann, bump steer, camber gain, the roll centre and the
    /// anti-dive / anti-squat follow from it.
    pub linkage: Linkage,
    /// How the wheel works the springs, dampers, anti-roll bar and heave spring; at the
    /// wheel when left out, so that their rates are wheel rates.
    #[serde(default)]
    pub actuation: Actuation,
    /// Spring rate, N/m of the actuation's compression.
    pub spring_rate: f64,
    /// Damping in compression / extension below `damper_knee`, N·s/m of the actuation.
    pub bump_damping: f64,
    pub rebound_damping: f64,
    /// Damping above `damper_knee` (the high-speed circuit); as below when left out.
    #[serde(default)]
    pub fast_bump_damping: Option<f64>,
    #[serde(default)]
    pub fast_rebound_damping: Option<f64>,
    /// Damper speed at which the high-speed circuit takes over, m/s.
    #[serde(default = "default_damper_knee")]
    pub damper_knee: f64,
    /// Anti-roll bar rate in N/m of the left-right difference of the actuations.
    pub anti_roll_rate: f64,
    /// Third (heave) spring working on both wheels' mean actuation; none when left out.
    #[serde(default)]
    pub heave: Option<HeaveParams>,
    /// Wheel travel from static ride height to the bump stops, m.
    pub bump_travel: f64,
    pub droop_travel: f64,
    /// Stiffness of the bump stops at the wheel in N/m.
    pub bump_stop_rate: f64,
    /// Static camber in radians, negative = top leaning inwards.
    pub static_camber: f64,
    /// Static toe in radians, positive = toe-in.
    #[serde(default)]
    pub static_toe: f64,
}

fn default_damper_knee() -> f64 {
    0.1
}

/// A third spring and damper working on the mean of an axle's two actuations: it
/// stiffens the axle in heave (under downforce) without stiffening it in roll.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HeaveParams {
    /// N/m of the mean compression beyond `gap`.
    pub rate: f64,
    /// Mean compression from static before the spring (its packers) starts to act, m.
    #[serde(default)]
    pub gap: f64,
    /// N·s/m of the mean compression's rate.
    #[serde(default)]
    pub damping: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SteeringParams {
    /// Steering wheel angle / road wheel angle near the centre: it sets the rack's
    /// travel per turn of the steering wheel.
    pub ratio: f64,
    /// Steering wheel lock in radians (each direction).
    pub lock: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BrakeParams {
    /// Total brake torque at full pedal in N·m (all four wheels), with the pads at the
    /// peak of their friction.
    pub max_torque: f64,
    /// Fraction of torque on the front axle.
    pub front_bias: f64,
    /// Friction of the pads against the disc temperature (°C), ascending, relative to
    /// its peak: pads bite less cold and fade when overheated. Road pads when left out.
    #[serde(default = "default_pad_friction")]
    pub pad_friction: Vec<(f64, f64)>,
    /// Mass of one front and one rear disc, kg: its heat capacity. Sized for the brake
    /// torque when left out.
    #[serde(default)]
    pub disc_mass: Option<(f64, f64)>,
    /// Heat one front and one rear disc sheds to the air through its vanes and duct at
    /// 50 m/s, W per K above the air. In proportion to the disc's mass when left out.
    #[serde(default)]
    pub disc_cooling: Option<(f64, f64)>,
    /// Boiling point of the brake fluid, °C: above it, vapour in the calipers takes the
    /// pressure the pedal builds.
    #[serde(default = "default_fluid_boiling_point")]
    pub fluid_boiling_point: f64,
    /// Temperature of the discs after a reset, °C; the calipers and the rims have warmed
    /// part of the way from the air towards it.
    #[serde(default = "default_brake_start_temperature")]
    pub start_temperature: f64,
}

fn default_pad_friction() -> Vec<(f64, f64)> {
    vec![
        (0.0, 0.85),
        (100.0, 0.95),
        (250.0, 1.0),
        (400.0, 0.97),
        (550.0, 0.8),
        (700.0, 0.55),
    ]
}
fn default_fluid_boiling_point() -> f64 {
    260.0
}
fn default_brake_start_temperature() -> f64 {
    150.0
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
    /// Gain of the idle control: it opens the throttle by 1 / `idle_band_rpm` for each rpm
    /// below `idle_rpm`, up to `idle_authority`, rpm.
    #[serde(default = "default_idle_band_rpm")]
    pub idle_band_rpm: f64,
    /// Swept volume, l. Estimated from the torque curve when left out.
    #[serde(default)]
    pub displacement: Option<f64>,
    /// Volume of the intake manifold behind the throttle, l. 1.5 × the displacement
    /// when left out.
    #[serde(default)]
    pub manifold_volume: Option<f64>,
    /// How the pedal works the throttle.
    #[serde(default)]
    pub throttle: ThrottleKind,
    /// Turbocharger, if any. The torque curve is then the engine's without boost (the
    /// manifold at atmospheric pressure); the boost adds to it.
    #[serde(default)]
    pub turbo: Option<TurboParams>,
    /// Radiator, oil cooler and thermostat.
    #[serde(default)]
    pub cooling: CoolingParams,
    /// Where the engine sits: the hot air from its bay flows past the tyres of the axle
    /// nearest to it.
    #[serde(default)]
    pub position: EnginePosition,
    /// Engine speed above which the valvetrain wears, rpm: the valves float and the
    /// springs and bearings are overloaded. 7 % above the limiter when left out.
    #[serde(default)]
    pub over_rev_rpm: Option<f64>,
}

/// How the engine sheds its heat.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CoolingParams {
    /// Heat the radiator sheds with the thermostat open and air flowing at 50 m/s, W per K
    /// of coolant above the air. Sized for the engine's power when left out.
    pub radiator: Option<f64>,
    /// Likewise for the oil cooler, W/K.
    pub oil_cooler: Option<f64>,
    /// Coolant temperature at which the thermostat starts to open, °C.
    pub thermostat: f64,
    /// Boiling point of the coolant at the pressure the cap holds, °C.
    pub boiling_point: f64,
    /// Whether a fan draws air through the radiator when the car is too slow to; racing
    /// cars often have none and overheat standing still.
    pub fan: bool,
}

impl Default for CoolingParams {
    fn default() -> Self {
        Self {
            radiator: None,
            oil_cooler: None,
            thermostat: 85.0,
            boiling_point: 125.0,
            fan: true,
        }
    }
}

/// Where the engine sits in the car.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnginePosition {
    /// Ahead of the cabin: its bay's air flows out past the front tyres.
    #[default]
    Front,
    /// Behind the cabin, ahead of the rear axle, or behind it: its bay's air flows out
    /// past the rear tyres.
    Mid,
    Rear,
}

impl EnginePosition {
    /// Whether the engine sits nearer the front axle.
    pub fn front(self) -> bool {
        self == Self::Front
    }
}

/// How the pedal works the throttle plate.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum ThrottleKind {
    /// A control unit opens the plate by a motor so that the torque follows the pedal
    /// evenly.
    #[default]
    DriveByWire,
    /// The pedal turns the plate directly: a butterfly uncovers most of the air a low
    /// engine speed needs early in its travel.
    Cable,
}

/// An exhaust-driven turbocharger with a wastegate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TurboParams {
    /// Boost at which the wastegate holds the pressure ahead of the throttle, bar (gauge).
    pub max_boost: f64,
    /// Engine speed at which the exhaust at full throttle spins the turbo up to
    /// `max_boost` with the wastegate shut, rpm: the turbine's size.
    pub reference_rpm: f64,
    /// Time the shaft takes to spool up at the reference flow, s: its inertia.
    #[serde(default = "default_spool_time")]
    pub spool_time: f64,
    /// How steeply the boost the exhaust can give grows with the air flow: the steady
    /// compressor work goes with the flow raised to this.
    #[serde(default = "default_flow_exponent")]
    pub flow_exponent: f64,
    /// Boost range over which the wastegate goes from shut to fully open, bar.
    #[serde(default = "default_wastegate_band")]
    pub wastegate_band: f64,
    /// Whether a blow-off valve vents the boost when the throttle shuts, so the shaft
    /// keeps spinning.
    #[serde(default = "default_blow_off_valve")]
    pub blow_off_valve: bool,
}

fn default_spool_time() -> f64 {
    0.4
}
fn default_flow_exponent() -> f64 {
    2.0
}
fn default_wastegate_band() -> f64 {
    0.1
}
fn default_blow_off_valve() -> bool {
    true
}

fn default_idle_authority() -> f64 {
    0.35
}
fn default_idle_band_rpm() -> f64 {
    300.0
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClutchParams {
    /// Maximum transmissible torque in N·m, fully engaged and not slipping.
    pub max_torque: f64,
    /// Pedal travel (0 = released, 1 = floored) beyond which the clutch transmits nothing.
    /// Between it and the released pedal the capacity grows with the travel from it raised
    /// to `engagement_exponent`.
    #[serde(default = "default_bite_point")]
    pub bite_point: f64,
    /// Shape of the engagement past the bite point: 1 = linear in the pedal travel, above 1
    /// = a gentle bite that firms up as the pedal comes back.
    #[serde(default = "default_engagement_exponent")]
    pub engagement_exponent: f64,
    /// Friction while slipping relative to friction while stuck.
    #[serde(default = "default_kinetic_ratio")]
    pub kinetic_ratio: f64,
}

fn default_bite_point() -> f64 {
    0.7
}
fn default_engagement_exponent() -> f64 {
    2.0
}
fn default_kinetic_ratio() -> f64 {
    0.8
}

impl ClutchParams {
    /// Fraction of the capacity transmitted at `pedal`.
    pub fn engagement(&self, pedal: f64) -> f64 {
        ((self.bite_point - pedal) / self.bite_point)
            .clamp(0.0, 1.0)
            .powf(self.engagement_exponent)
    }

    /// Pedal travel that transmits `engagement` of the capacity (inverse of
    /// [`Self::engagement`]).
    pub fn pedal_for(&self, engagement: f64) -> f64 {
        self.bite_point
            * (1.0
                - engagement
                    .clamp(0.0, 1.0)
                    .powf(1.0 / self.engagement_exponent))
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
    /// Oil drag on the input shaft while no gear holds it, N·m.
    #[serde(default = "default_input_drag")]
    pub input_drag: f64,
    pub kind: GearboxKind,
}

fn default_input_inertia() -> f64 {
    0.02
}
fn default_input_drag() -> f64 {
    1.0
}

/// How gears are selected and engaged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum GearboxKind {
    /// Manual gearbox with an H-pattern lever and synchromesh: a synchroniser cone matches
    /// the input shaft to the selected gear before its dog teeth can mesh, which it can
    /// only do quickly with the clutch open.
    HPattern {
        /// Torque a synchroniser cone exerts on the input shaft, N·m.
        synchro_torque: f64,
        /// Time the driver's hand takes from one gate to the next with sequential
        /// up / down requests, s.
        lever_time: f64,
        /// Speed difference across a synchroniser at which the dogs mesh, rad/s at the input
        /// shaft.
        #[serde(default = "default_sync_window")]
        sync_window: f64,
        /// How long a synchroniser fights before its gear grinds, s.
        #[serde(default = "default_grind_time")]
        grind_time: f64,
        /// Whether reverse has a synchroniser. Without one, reverse goes in only once the
        /// input shaft turns with the wheels, as it does with the clutch down at rest after
        /// its drag has stopped it.
        #[serde(default = "default_reverse_synchro")]
        reverse_synchro: bool,
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
        /// How the control unit slips, closes and changes down.
        #[serde(default)]
        control: DualClutchControl,
    },
}

fn default_sync_window() -> f64 {
    3.0
}
fn default_grind_time() -> f64 {
    0.3
}
fn default_reverse_synchro() -> bool {
    true
}

impl GearboxKind {
    /// An H-pattern gearbox with the default synchroniser tolerances and a synchronised
    /// reverse.
    pub fn h_pattern(synchro_torque: f64, lever_time: f64) -> Self {
        Self::HPattern {
            synchro_torque,
            lever_time,
            sync_window: default_sync_window(),
            grind_time: default_grind_time(),
            reverse_synchro: default_reverse_synchro(),
        }
    }
}

/// How a dual-clutch control unit works the clutch of the gear carrying the drive. Speeds
/// are relative to the engine's `idle_rpm`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DualClutchControl {
    /// Engine speed above idle at which the clutch starts to bite with the throttle closed;
    /// the throttle raises it towards `launch_rpm`, rpm.
    pub bite_rpm: f64,
    /// Engine speed range above the bite speed over which the clutch goes from creeping to
    /// its full capacity, rpm.
    pub engage_band_rpm: f64,
    /// Slip between the engine and the gear below which the clutch closes fully, rpm.
    pub lock_slip_rpm: f64,
    /// Speed above idle the gear must turn the engine at before the clutch closes fully,
    /// rpm.
    pub lock_rpm: f64,
    /// Speed above idle below which the gear turns the engine when the control unit drops
    /// a gear, rpm.
    pub downshift_rpm: f64,
}

impl Default for DualClutchControl {
    fn default() -> Self {
        Self {
            bite_rpm: 150.0,
            engage_band_rpm: 500.0,
            lock_slip_rpm: 100.0,
            lock_rpm: 100.0,
            downshift_rpm: 0.0,
        }
    }
}

/// Engine and gearbox control electronics a car is fitted with.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ElectronicsParams {
    /// Opens the clutch as the engine slows towards stalling (not on dual-clutch
    /// gearboxes, whose control unit always does).
    pub anti_stall: Option<AntiStall>,
    /// Blips the throttle to match revs on downshifts, a dual clutch's included.
    pub auto_blip: bool,
    /// How far below the incoming gear's speed the engine has to be for the blip to open
    /// the throttle fully; closer, it opens in proportion, rpm.
    pub blip_band_rpm: f64,
    /// Cuts the ignition to unload the dogs on sequential upshifts.
    pub ignition_cut: bool,
    /// Refuses a downshift that would spin the engine faster than this, rpm.
    pub downshift_protection_rpm: Option<f64>,
}

impl Default for ElectronicsParams {
    fn default() -> Self {
        Self {
            anti_stall: None,
            auto_blip: false,
            blip_band_rpm: 500.0,
            ignition_cut: false,
            downshift_protection_rpm: None,
        }
    }
}

/// Anti-stall: while the driveline drags the engine down, the clutch opens over
/// `band_rpm` above `rpm` and is fully open below it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AntiStall {
    /// Engine speed below which the clutch is fully open, rpm.
    pub rpm: f64,
    /// Engine speed range over which the clutch opens, rpm.
    #[serde(default = "default_anti_stall_band_rpm")]
    pub band_rpm: f64,
}

fn default_anti_stall_band_rpm() -> f64 {
    200.0
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

/// The car's aerodynamics: the body, wings, splitter and floor, each an element whose
/// forces follow the car's attitude.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AeroParams {
    /// Ride height of the floor at the front / rear axle at rest, m: where the elements'
    /// ride-height maps are read at static ride height.
    pub ride_height: [f64; 2],
    pub elements: Vec<AeroElement>,
}

/// Damage zones of the body, in the order of [`crate::CarState::damage`].
pub const DAMAGE_ZONES: usize = 4;

/// One aerodynamic element: a wing, the floor or the body.
///
/// Its downforce is `q · area · lift(angle of attack) · height_lift(ride height)` and its
/// drag likewise, where the angle of attack is its setup `angle` plus the car's pitch
/// (nose down positive) and the ride height is the floor's height under it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AeroElement {
    pub name: String,
    /// Centre of pressure relative to the CG at static ride height: forward, up, m.
    pub position: [f64; 2],
    /// Reference area, m².
    pub area: f64,
    /// Angle of attack at rest, rad.
    #[serde(default)]
    pub angle: f64,
    /// Downforce and drag coefficients against the angle of attack (rad), ascending.
    pub lift: Vec<(f64, f64)>,
    pub drag: Vec<(f64, f64)>,
    /// Multipliers of the downforce and drag coefficients against the ride height at
    /// the element (m), ascending; none when left out.
    #[serde(default)]
    pub height_lift: Vec<(f64, f64)>,
    #[serde(default)]
    pub height_drag: Vec<(f64, f64)>,
    /// Relative change of the downforce per radian of sideslip.
    #[serde(default)]
    pub yaw_lift: f64,
    /// Share of the downforce / drag lost per m/s of impact damage in each zone (front,
    /// rear, left, right).
    #[serde(default)]
    pub damage_lift: [f64; DAMAGE_ZONES],
    #[serde(default)]
    pub damage_drag: [f64; DAMAGE_ZONES],
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
            self.engine.idle_band_rpm > 0.0,
            "idle_band_rpm must be positive",
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
            self.clutch.engagement_exponent > 0.0,
            "clutch engagement_exponent must be positive",
        )?;
        check(
            self.gearbox.input_inertia > 0.0,
            "gearbox input_inertia must be positive",
        )?;
        check(
            self.gearbox.input_drag >= 0.0,
            "gearbox input_drag must not be negative",
        )?;
        check(
            match self.gearbox.kind {
                GearboxKind::HPattern {
                    synchro_torque,
                    lever_time,
                    sync_window,
                    grind_time,
                    ..
                } => {
                    synchro_torque > 0.0
                        && lever_time >= 0.0
                        && sync_window > 0.0
                        && grind_time >= 0.0
                }
                GearboxKind::Sequential {
                    shift_time,
                    dog_release_torque,
                } => shift_time >= 0.0 && dog_release_torque > 0.0,
                GearboxKind::DualClutch {
                    shift_time,
                    creep_torque,
                    launch_rpm,
                    ref control,
                } => {
                    shift_time > 0.0
                        && creep_torque >= 0.0
                        && launch_rpm > self.engine.idle_rpm
                        && launch_rpm < self.engine.limiter_rpm
                        && control.bite_rpm >= 0.0
                        && control.engage_band_rpm > 0.0
                        && control.lock_slip_rpm > 0.0
                        && control.lock_rpm >= 0.0
                        && control.downshift_rpm >= 0.0
                }
            },
            "gearbox kind: invalid timing, torque, window or control speed",
        )?;
        check(
            self.electronics
                .anti_stall
                .as_ref()
                .is_none_or(|a| a.rpm > self.engine.stall_rpm && a.band_rpm > 0.0),
            "anti_stall rpm must exceed stall_rpm, and its band_rpm be positive",
        )?;
        check(
            self.electronics.blip_band_rpm > 0.0,
            "blip_band_rpm must be positive",
        )?;
        check(
            self.front.pressure > 0.0 && self.rear.pressure > 0.0,
            "tyre pressure must be positive",
        )?;
        check(
            self.steering.ratio > 0.0 && self.steering.lock > 0.0,
            "steering ratio and lock must be positive",
        )?;
        check(
            [&self.front, &self.rear].iter().all(|a| {
                a.spring_rate > 0.0
                    && a.bump_damping >= 0.0
                    && a.rebound_damping >= 0.0
                    && a.fast_bump_damping.is_none_or(|d| d >= 0.0)
                    && a.fast_rebound_damping.is_none_or(|d| d >= 0.0)
                    && a.damper_knee > 0.0
                    && a.anti_roll_rate >= 0.0
                    && a.bump_travel > 0.0
                    && a.droop_travel > 0.0
                    && a.bump_stop_rate >= 0.0
                    && a.unsprung_mass > 0.0
                    && a.heave
                        .as_ref()
                        .is_none_or(|h| h.rate >= 0.0 && h.damping >= 0.0)
            }),
            "suspension: spring rate, travels and unsprung mass must be positive, damping, \
             bars, bump stops and heave springs not negative",
        )?;
        check(
            (0.0..=1.0).contains(&self.drive.front_share()),
            "drive: front_share must be in 0..1",
        )?;
        check(
            self.engine.displacement.is_none_or(|v| v > 0.0)
                && self.engine.manifold_volume.is_none_or(|v| v > 0.0),
            "engine displacement and manifold_volume must be positive",
        )?;
        check(
            self.engine.turbo.as_ref().is_none_or(|t| {
                t.max_boost > 0.0
                    && t.reference_rpm > 0.0
                    && t.spool_time > 0.0
                    && t.flow_exponent > 0.0
                    && t.wastegate_band > 0.0
            }),
            "turbo: max_boost, reference_rpm, spool_time, flow_exponent and wastegate_band \
             must be positive",
        )?;
        let ascending = |t: &[(f64, f64)]| t.windows(2).all(|w| w[0].0 < w[1].0);
        let b = &self.brakes;
        let positive = |pair: Option<(f64, f64)>| pair.is_none_or(|(f, r)| f > 0.0 && r > 0.0);
        check(
            b.max_torque >= 0.0
                && (0.0..=1.0).contains(&b.front_bias)
                && !b.pad_friction.is_empty()
                && ascending(&b.pad_friction)
                && b.pad_friction.iter().all(|p| p.1 > 0.0)
                && positive(b.disc_mass)
                && positive(b.disc_cooling),
            "brakes: front_bias must be in 0..1, pad_friction ascending with positive \
             friction, disc_mass and disc_cooling positive",
        )?;
        let c = &self.engine.cooling;
        check(
            c.radiator.is_none_or(|v| v > 0.0)
                && c.oil_cooler.is_none_or(|v| v > 0.0)
                && c.boiling_point > c.thermostat
                && self
                    .engine
                    .over_rev_rpm
                    .is_none_or(|r| r > self.engine.idle_rpm),
            "engine cooling: radiator and oil_cooler must be positive, boiling_point above \
             thermostat, over_rev_rpm above idle",
        )?;
        check(
            self.aero.elements.iter().all(|e| {
                e.area >= 0.0
                    && !e.lift.is_empty()
                    && !e.drag.is_empty()
                    && ascending(&e.lift)
                    && ascending(&e.drag)
                    && ascending(&e.height_lift)
                    && ascending(&e.height_drag)
            }),
            "aero elements need a non-negative area and ascending, non-empty lift and drag \
             tables",
        )?;
        let sprung = self.mass - 2.0 * (self.front.unsprung_mass + self.rear.unsprung_mass);
        check(sprung > 0.0, "unsprung mass exceeds total mass")
    }
}

/// Per-corner constants derived from `CarParams`.
#[derive(Clone, Debug)]
pub struct CornerModel {
    /// Wheel centre at static ride height in body coordinates.
    pub origin: DVec3,
    /// +1 for left wheels, −1 for right wheels.
    pub side: f64,
    pub front: bool,
    pub driven: bool,
    /// Travel (bump positive) at which the droop and bump stops start, m.
    pub droop_stop: f64,
    pub bump_stop: f64,
    /// Force of the coil-over at static ride height, N: it holds the corner's weight.
    pub preload: f64,
    /// Tyre deflection under the static load, m.
    pub static_deflection: f64,
}

/// Simulation-ready car: validated params plus derived constants.
#[derive(Clone, Debug)]
pub struct CarModel {
    pub params: CarParams,
    pub sprung_mass: f64,
    pub corners: [CornerModel; 4],
    /// The front and rear linkages solved over their travel (and the front's rack).
    pub front_kinematics: Kinematics,
    pub rear_kinematics: Kinematics,
    pub front_tire: TireModel,
    pub rear_tire: TireModel,
    pub engine: EngineModel,
    pub brakes: BrakeModel,
}

impl CarModel {
    /// Builds the model of `params` fitted with the given front and rear tyres.
    pub fn new(
        params: CarParams,
        front_tire: TireParams,
        rear_tire: TireParams,
    ) -> Result<Self, ParamsError> {
        params.validate()?;
        for tire in [&front_tire, &rear_tire] {
            let curve = &tire.thermal.grip_curve;
            if curve.is_empty()
                || curve.iter().any(|g| g.1 <= 0.0)
                || curve.windows(2).any(|w| w[0].0 >= w[1].0)
                || tire.thermal.wear_window <= 0.0
            {
                return Err(ParamsError::Invalid(
                    "tyre grip_curve must be ascending with positive grip, wear_window positive",
                ));
            }
        }
        let p = &params;
        let unsprung_total = 2.0 * (p.front.unsprung_mass + p.rear.unsprung_mass);
        let sprung_mass = p.mass - unsprung_total;

        let front_x = p.wheelbase * (1.0 - p.front_weight);
        let rear_x = -p.wheelbase * p.front_weight;

        // Per axle: the tyre's load and deflection at rest, and the linkage solved.
        let axle = |front: bool| -> Result<(f64, f64, Kinematics), ParamsError> {
            let axle = if front { &p.front } else { &p.rear };
            let tire = if front { &front_tire } else { &rear_tire };
            let axle_weight = if front {
                p.front_weight
            } else {
                1.0 - p.front_weight
            };
            let tire_load = 0.5 * p.mass * axle_weight * GRAVITY;
            let deflection = tire_load / tire.vertical_stiffness;
            let track = if front { p.track_front } else { p.track_rear };
            let kinematics = Kinematics::new(
                axle.linkage.clone(),
                axle.actuation.clone(),
                Alignment {
                    camber: axle.static_camber,
                    toe: axle.static_toe,
                },
                -(axle.droop_travel + OVERTRAVEL),
                axle.bump_travel + OVERTRAVEL,
                front.then_some((p.steering.ratio, p.steering.lock)),
                tire.radius - deflection,
                0.5 * track,
            )?;
            Ok((tire_load, deflection, kinematics))
        };
        let (front_load, front_deflection, front_kinematics) = axle(true)?;
        let (rear_load, rear_deflection, rear_kinematics) = axle(false)?;

        let mut corners = Vec::with_capacity(4);
        for (front, side) in [(true, 1.0), (true, -1.0), (false, 1.0), (false, -1.0)] {
            let (axle, tire, kinematics, load, deflection) = if front {
                (
                    &p.front,
                    &front_tire,
                    &front_kinematics,
                    front_load,
                    front_deflection,
                )
            } else {
                (
                    &p.rear,
                    &rear_tire,
                    &rear_kinematics,
                    rear_load,
                    rear_deflection,
                )
            };
            // At rest the tyre's load, which does work as the contact patch rises with
            // the travel, and the upright's weight are held by the coil-over.
            let pose = kinematics.pose(0.0, 0.0);
            let patch = -DVec3::Z * (tire.radius - deflection);
            let patch_rise = (pose.centre_travel + pose.spin_travel.cross(patch)).z;
            let held = load * patch_rise - axle.unsprung_mass * GRAVITY * pose.centre_travel.z;
            if pose.motion_ratio < 0.05 {
                return Err(ParamsError::Invalid(
                    "actuation: the coil-over must compress as the wheel rises",
                ));
            }
            let track = if front { p.track_front } else { p.track_rear };
            corners.push(CornerModel {
                origin: DVec3::new(
                    if front { front_x } else { rear_x },
                    side * 0.5 * track,
                    tire.radius - deflection - p.cg_height,
                ),
                side,
                front,
                driven: p.drive.drives(front),
                droop_stop: -axle.droop_travel,
                bump_stop: axle.bump_travel,
                preload: held / pose.motion_ratio,
                static_deflection: deflection,
            });
        }
        let corners: [CornerModel; 4] = corners.try_into().expect("four corners");
        Ok(Self {
            sprung_mass,
            corners,
            front_kinematics,
            rear_kinematics,
            engine: EngineModel::new(&p.engine),
            brakes: BrakeModel::new(&p.brakes),
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
    pub fn kinematics(&self, wheel: usize) -> &Kinematics {
        if wheel < 2 {
            &self.front_kinematics
        } else {
            &self.rear_kinematics
        }
    }

    /// The upright's pose of `wheel` at `travel` (bump positive) and `rack` (left), m,
    /// relative to its static wheel centre (`CornerModel::origin`) in body axes.
    #[inline]
    pub fn pose(&self, wheel: usize, travel: f64, rack: f64) -> Pose {
        let k = self.kinematics(wheel);
        if self.corners[wheel].side > 0.0 {
            k.pose(travel, rack)
        } else {
            k.pose(travel, -rack).mirrored()
        }
    }

    /// Lines between the joints of `wheel`'s linkage and actuation at `travel` and
    /// `rack`, in body coordinates, appended to `out`: for drawing.
    pub fn linkage_segments(
        &self,
        wheel: usize,
        travel: f64,
        rack: f64,
        out: &mut Vec<(DVec3, DVec3)>,
    ) {
        let c = &self.corners[wheel];
        let from = out.len();
        self.kinematics(wheel).segments(travel, c.side * rack, out);
        for (a, b) in &mut out[from..] {
            for p in [a, b] {
                *p = c.origin + DVec3::new(p.x, c.side * p.y, p.z);
            }
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

/// What an axle's suspension does at static ride height, for setting it up.
#[derive(Clone, Copy, Debug, Default)]
pub struct AxleFigures {
    pub kinematics: Summary,
    /// Share of the pitch its linkage takes off the springs under braking (anti-dive at
    /// the front, anti-lift at the rear) and under drive (anti-squat at the rear,
    /// anti-lift at the front), from the brake balance and the drive's split.
    pub anti_brake: f64,
    pub anti_drive: f64,
    /// Rates at the wheel, N/m: the spring's, the anti-roll bar's (per metre of left-right
    /// difference) and the heave spring's (per wheel, both rising).
    pub spring_rate: f64,
    pub anti_roll_rate: f64,
    pub heave_rate: f64,
    /// Natural frequency of the body's corner on the spring and the tyre, Hz.
    pub ride_frequency: f64,
    /// How far the inner wheel turns more than the outer at 10° of mean lock, as a share
    /// of what full Ackermann would give (negative: anti-Ackermann). Front only.
    pub ackermann: f64,
}

impl CarModel {
    /// The figures of the front or rear suspension.
    pub fn axle_figures(&self, front: bool) -> AxleFigures {
        let p = &self.params;
        let wheel = if front { 0 } else { 2 };
        let (axle, k, tire) = (self.axle(wheel), self.kinematics(wheel), self.tire(wheel));
        let s = k.summary;
        let pose = k.pose(0.0, 0.0);
        let mr2 = pose.motion_ratio * pose.motion_ratio;
        let lever = p.wheelbase / p.cg_height;
        let bias = p.brakes.front_bias;
        let drive = p.drive.front_share();
        let (anti_brake, anti_drive) = if front {
            (bias * s.patch_pitch * lever, drive * s.centre_pitch * lever)
        } else {
            (
                (1.0 - bias) * -s.patch_pitch * lever,
                (1.0 - drive) * -s.centre_pitch * lever,
            )
        };
        let spring = axle.spring_rate * mr2;
        let tyre = tire.p.vertical_stiffness;
        let weight = if front {
            p.front_weight
        } else {
            1.0 - p.front_weight
        };
        let corner = 0.5 * p.mass * weight - axle.unsprung_mass;
        let ride = spring * tyre / (spring + tyre);
        let ackermann = if front {
            // Rack for 10° of mean lock to the left: the left wheel is the inner one.
            let lock = 10f64.to_radians();
            let rack = lock * p.steering.ratio * k.rack_gain;
            let (inner, outer) = (
                self.pose(0, 0.0, rack).steer() - self.pose(0, 0.0, 0.0).steer(),
                self.pose(1, 0.0, rack).steer() - self.pose(1, 0.0, 0.0).steer(),
            );
            let ideal = (1.0 / (1.0 / inner.tan() + p.track_front / p.wheelbase)).atan();
            (inner - outer) / (inner - ideal)
        } else {
            0.0
        };
        AxleFigures {
            kinematics: s,
            anti_brake,
            anti_drive,
            spring_rate: spring,
            anti_roll_rate: axle.anti_roll_rate * mr2,
            heave_rate: axle.heave.as_ref().map_or(0.0, |h| 0.5 * h.rate * mr2),
            ride_frequency: (ride / corner).sqrt() / std::f64::consts::TAU,
            ackermann,
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
    fn the_engagement_exponent_shapes_the_clutch_and_its_inverse_follows() {
        let clutch = |engagement_exponent| ClutchParams {
            max_torque: 500.0,
            bite_point: 0.7,
            engagement_exponent,
            kinetic_ratio: 0.8,
        };
        // Halfway from the bite point to the released pedal.
        for (exponent, half) in [(1.0, 0.5), (2.0, 0.25), (3.0, 0.125)] {
            let c = clutch(exponent);
            assert!((c.engagement(0.35) - half).abs() < 1e-12, "{exponent}");
            for e in [0.0, 0.1, 0.5, 0.9, 1.0] {
                assert!(
                    (c.engagement(c.pedal_for(e)) - e).abs() < 1e-12,
                    "{exponent} {e}"
                );
            }
        }
    }

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
