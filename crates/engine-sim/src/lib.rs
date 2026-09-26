//! open-racing's engine simulator.
//!
//! A crank-angle resolved model of a four-stroke engine in the manner of GT-Power, Ricardo
//! WAVE and Blair's *Design and Simulation of Four-Stroke Engines*: the intake and exhaust
//! are one-dimensional gas dynamics (pipes) joined by volumes, and each cylinder is a
//! single-zone thermodynamic system with its valves, combustion and heat loss, on a crank
//! whose layout (inline, V, flat, any throws) sets when each cylinder fires. The pressure
//! waves in the pipes tune the breathing, and what leaves the tailpipes and enters the
//! intake mouths is also the engine's sound.
//!
//! Units are SI; engine speeds are in rpm and crank angles in degrees where named so.

pub mod boundary;
pub mod build;
pub mod cam;
pub mod combustion;
pub mod crank;
pub mod dyno;
pub mod gas;
pub mod model;
pub mod pipe;
pub mod samples;
pub mod spec;
pub mod table;

pub use build::{Build, System};
pub use model::{Ambient, Controls, Load, Model, Quality};
pub use spec::{EngineSpec, Network};
