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

pub mod acoustics;
pub mod analysis;
pub mod boundary;
pub mod build;
pub mod cam;
pub mod chem;
pub mod combustion;
pub mod crank;
pub mod dsp;
pub mod dyno;
pub mod gas;
pub mod model;
pub mod pipe;
pub mod realtime;
pub mod render;
pub mod samples;
pub mod spec;
pub mod table;
pub mod trace;

pub use acoustics::{Mic, SoundSettings};
pub use build::{Build, System};
pub use model::{Ambient, Controls, Load, Model, Quality};
pub use render::{Recording, Script};
pub use spec::{EngineSpec, Network};
