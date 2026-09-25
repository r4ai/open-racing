//! Track projects: the editable source of a circuit, and its baking into the track
//! package the simulator reads.
//!
//! A project describes a track the way it is built rather than as meshes: roads laid
//! along splines with their width, banking and crown, strips beside them (kerbs,
//! run-off, gravel, grass) limited to stretches of the road, painted lines, barriers,
//! and markers for the start line, sectors, grid and pit lane. [`bake::bake`] builds
//! the meshes the physics and the renderer need from it; the open-racing-editor app
//! edits it.
//!
//! The crate does not depend on any renderer, so projects can be baked and checked from
//! tests and scripts.

pub mod bake;
pub mod builtin;
pub mod curve;
pub mod project;
pub mod road;
pub mod terrain;
pub mod validate;

use std::path::PathBuf;

pub use bake::{Scene, Textures, bake};
pub use project::*;

#[derive(Debug)]
pub enum Error {
    Io(PathBuf, std::io::Error),
    Parse(PathBuf, Box<ron::error::SpannedError>),
    Invalid(String),
    Package(open_racing_track::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(p, e) => write!(f, "{}: {e}", p.display()),
            Self::Parse(p, e) => write!(f, "{}: {e}", p.display()),
            Self::Invalid(msg) => f.write_str(msg),
            Self::Package(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

impl From<open_racing_track::Error> for Error {
    fn from(e: open_racing_track::Error) -> Self {
        Self::Package(e)
    }
}

/// Where projects are kept by default: `<content>/track-src/`.
pub fn projects_dir() -> PathBuf {
    open_racing_track::content_dir().join("track-src")
}

#[cfg(test)]
mod tests {
    use glam::DVec3;
    use open_racing_sim::Surface;

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("open-racing-project-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn project_round_trips() {
        let dir = temp_dir("round-trip");
        let p = Project::new("oval");
        p.save(&dir).unwrap();
        assert_eq!(Project::load(&dir).unwrap(), p);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn baked_package_drives() {
        let project = Project::new("oval");
        let dir = temp_dir("bake");
        let package = bake(&project, &dir, &mut Textures::default()).unwrap();
        package.save(&dir).unwrap();
        let package = open_racing_track::TrackPackage::load(&dir, true).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let track = package.build_track().unwrap();

        // Surfaces across the start straight: asphalt, then the grass (no kerbs on the
        // straight), and a wall 20 m beyond the edge.
        let s = 100.0;
        let at = |d: f64| {
            let smp = track.sample_at(s);
            track.query(smp.pos + smp.lateral * d + DVec3::Z * 0.5, 0)
        };
        assert_eq!(at(0.0).surface, Surface::Asphalt);
        assert_eq!(at(5.5).surface, Surface::Asphalt);
        assert_eq!(at(-8.0).surface, Surface::Grass);
        assert_eq!(
            at(40.0).surface,
            Surface::Grass,
            "terrain beyond the strips"
        );
        let smp = track.sample_at(s);
        assert!(
            track
                .wall_contact(smp.pos + smp.lateral * 26.0 + DVec3::Z * 0.5, 0.3)
                .is_some()
        );

        // Race layout.
        assert_eq!(track.layout.sectors.len(), 2);
        assert_eq!(track.layout.grid.len(), 12);
        let pole = track.layout.start();
        assert!(pole.s > track.length - 20.0 && pole.d > 0.0);

        let report = validate::check(&package, true);
        assert!(report.ok(), "{report}");
    }

    #[test]
    fn kerbs_sit_in_the_corners() {
        let project = Project::new("oval");
        let package = bake(&project, Path::new("."), &mut Textures::default()).unwrap();
        let track = package.build_track().unwrap();
        // The middle of the first corner: node 3, on the outside (right) and inside.
        let main = curve::Sampled::new(&project.roads[0], 2.0);
        let p = bake::frame_at_u(&main, 3.0).pos;
        let c = track.locate(p, track.nearest_index(p));
        let smp = track.sample_at(c.s);
        let at = |d: f64| {
            track
                .query(smp.pos + smp.lateral * d + DVec3::Z * 0.5, 0)
                .surface
        };
        assert_eq!(at(6.6), Surface::Kerb);
        assert_eq!(at(-6.6), Surface::Kerb);
    }

    use std::path::Path;
}
