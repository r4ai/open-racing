//! Vehicle dynamics core for open-racing.
//!
//! This crate is the innermost layer: it knows nothing about rendering, input
//! devices or machine learning. Everything here is deterministic and
//! `Car::step` performs no heap allocation.
//!
//! Coordinate conventions (ISO 8855):
//! - world: X/Y horizontal, Z up
//! - vehicle body: x forward, y left, z up, origin at the centre of gravity

pub mod assist;
pub mod brakes;
pub mod car;
pub mod controls;
pub mod drivetrain;
pub mod engine;
pub mod engine_thermal;
pub mod evolution;
pub mod ground;
pub mod params;
pub mod suspension;
pub mod tire;
pub mod track;
pub mod weather;

pub use assist::{AutoShift, BlipAssist, ClutchAssist};
pub use brakes::BrakeState;
pub use car::{
    AeroTelemetry, Car, CarState, MAX_AERO_TELEMETRY, Realism, Telemetry, WheelState,
    WheelTelemetry,
};
pub use controls::{Controls, Shift};
pub use drivetrain::ShiftPhase;
pub use engine::{EngineModel, EngineState};
pub use engine_thermal::{EngineHeat, EngineWear};
pub use evolution::{RubberMap, TrackCondition, TrackEvolution, parse_grip};
pub use ground::{GroundHit, GroundMesh, GroundMeshBuilder, SurfaceProps};
pub use params::{
    AeroElement, AntiStall, CarModel, CarParams, CoolingParams, Drive, DualClutchControl,
    ElectronicsParams, EnginePosition, GearboxKind, ParamsError, ThrottleKind, TurboParams,
};
pub use tire::TireCondition;
pub use track::{
    Coat, GridSlot, Layout, PitLane, Pose, Surface, Track, TrackCoords, TrackDef, TrackError,
    TrackPoint, TrackQuery,
};
pub use weather::{Air, Sky, Weather, WeatherSettings};

/// Fixed physics time step in seconds (1 kHz).
pub const DT: f64 = 1.0 / 1000.0;

/// Physics steps between exchanges of heat among the brakes' and the engine's parts and
/// the air: their temperatures change over seconds, so 100 Hz is plenty.
pub const THERMAL_STEPS: u64 = 10;

/// Standard gravity in m/s².
pub const GRAVITY: f64 = 9.80665;

/// Air density at sea level, 15 °C, in kg/m³.
pub const AIR_DENSITY: f64 = 1.225;

/// Air and road temperature in °C.
pub const AMBIENT_TEMPERATURE: f64 = 25.0;

/// 0 °C in K.
pub const KELVIN: f64 = 273.15;

/// Air speed at which the coolers and the brakes' cooling are rated, m/s, in air of
/// [`AIR_DENSITY`].
pub const RATED_AIRSPEED: f64 = 50.0;

/// Air flowing over a part of the car.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Airflow {
    /// Speed of the air past the car, m/s: its forward airspeed, the wind included.
    pub speed: f64,
    /// °C.
    pub temperature: f64,
    /// kg/m³.
    pub density: f64,
}

impl Airflow {
    /// Still standard air.
    pub const STILL: Self = Self {
        speed: 0.0,
        temperature: AMBIENT_TEMPERATURE,
        density: AIR_DENSITY,
    };

    /// Turbulent forced convection of air at `speed` m/s of this density, relative to
    /// that at the rated airspeed in standard air: it goes with the Reynolds number
    /// (∝ ρ·v) raised to 0.8, so thin air, high or hot, cools less.
    #[inline]
    pub fn convection(&self, speed: f64) -> f64 {
        (self.density / AIR_DENSITY * speed.max(0.0) / RATED_AIRSPEED).powf(0.8)
    }

    /// Mass flow of this air at `speed` relative to that at the rated airspeed in
    /// standard air.
    #[inline]
    pub fn mass_flow(&self, speed: f64) -> f64 {
        self.density / AIR_DENSITY * speed.max(0.0) / RATED_AIRSPEED
    }
}

/// Wheel index order used everywhere: front-left, front-right, rear-left, rear-right.
pub const FL: usize = 0;
pub const FR: usize = 1;
pub const RL: usize = 2;
pub const RR: usize = 3;
