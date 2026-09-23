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
pub mod car;
pub mod controls;
pub mod drivetrain;
pub mod params;
pub mod tire;
pub mod track;

pub use assist::AutoShift;
pub use car::{Car, CarState, Telemetry, WheelState, WheelTelemetry};
pub use controls::{Controls, Shift};
pub use params::{CarModel, CarParams, ParamsError};
pub use track::{Surface, Track, TrackDef, TrackError, TrackPoint, TrackQuery};

/// Fixed physics time step in seconds (1 kHz).
pub const DT: f64 = 1.0 / 1000.0;

/// Standard gravity in m/s².
pub const GRAVITY: f64 = 9.80665;

/// Air density at sea level, 15 °C, in kg/m³.
pub const AIR_DENSITY: f64 = 1.225;

/// Wheel index order used everywhere: front-left, front-right, rear-left, rear-right.
pub const FL: usize = 0;
pub const FR: usize = 1;
pub const RL: usize = 2;
pub const RR: usize = 3;
