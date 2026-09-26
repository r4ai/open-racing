//! Machines built the way real cars are: from parts designed on their own — a frame,
//! an engine with its intake and exhaust, a transmission, suspensions, wheels and tyres,
//! aero, a body and an interior — assembled into a machine with its setup, and baked into
//! a car package the game drives.
//!
//! A library is a directory of plain-text files (`parts/<kind>/<name>.ron`,
//! `machines/<name>.ron`) that refer to each other by name. Every change is an [`ops::Op`],
//! the same for the editor, the command line (`open-racing-machinectl`) and agents,
//! applied all or nothing and checked.

pub mod assembly;
pub mod bake;
pub mod chart;
pub mod engines;
pub mod inspect;
pub mod library;
pub mod machine;
pub mod mass;
pub mod mesh;
pub mod ops;
pub mod part;
pub mod samples;
pub mod sound;
pub mod validate;

use std::path::PathBuf;

pub use library::Library;
pub use machine::Machine;
pub use part::{Kind, Part, PartRef};

/// Where the working library is: `<content>/machine-src/`.
pub fn library_dir() -> PathBuf {
    open_racing_track::content_dir().join("machine-src")
}

/// Names of parts, machines and instances must work as file names and in references.
pub fn check_name(n: &str) -> Result<(), String> {
    let ok = !n.is_empty()
        && n.len() <= 64
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ' '))
        && !n.starts_with(['.', ' '])
        && !n.ends_with([' ', '.']);
    if ok {
        Ok(())
    } else {
        Err(format!(
            "\"{n}\" is not a usable name: letters, digits, spaces, _, - and . (not first or last), up to 64"
        ))
    }
}
