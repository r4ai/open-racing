//! open-racing's car package format.
//!
//! A package is a directory holding one car:
//! - `car.ron`: the physics, in the format of `assets/cars/*.ron` (`CarParams`). It names
//!   its tyres by path relative to itself.
//! - `front_tire.ron`, `rear_tire.ron`: the tyres (`TireParams`).
//! - `visual.ron` (optional): format version, and which part of the car each mesh of the
//!   3D model belongs to, i.e. how it moves.
//! - `visual.bin` (with `visual.ron`): meshes, materials and textures, in the encoding of
//!   track packages' render data. Only the app reads it.
//!
//! Simulation-only processes read `car.ron` and the tyres, a few kB, and never the model.
//! The runtime reads only this format. Cars made in other formats are converted into
//! packages ahead of time by separate converter crates.

use std::path::{Path, PathBuf};

use open_racing_sim::CarParams;
use open_racing_sim::params::tire_path;
use open_racing_sim::tire::TireParams;
use open_racing_track::Visual;
use serde::{Deserialize, Serialize};

/// Version of `visual.ron` and of how its parts are laid out.
pub const FORMAT_VERSION: u32 = 1;
const CAR: &str = "car.ron";
const FRONT_TIRE: &str = "front_tire.ron";
const REAR_TIRE: &str = "rear_tire.ron";
const MANIFEST: &str = "visual.ron";
const VISUAL: &str = "visual.bin";

#[derive(Debug)]
pub enum Error {
    Io(PathBuf, std::io::Error),
    Ron(PathBuf, String),
    Format(String),
    Visual(open_racing_track::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(p, e) => write!(f, "{}: {e}", p.display()),
            Self::Ron(p, e) => write!(f, "{}: {e}", p.display()),
            Self::Format(msg) => f.write_str(msg),
            Self::Visual(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

/// Where packages are looked up by name: `<content>/cars/<name>/`.
pub fn cars_dir() -> PathBuf {
    open_racing_track::content_dir().join("cars")
}

/// Whether `dir` holds a package.
pub fn is_package(dir: &Path) -> bool {
    dir.join(CAR).is_file()
}

/// The physics file of the package in `dir`, which `CarModel::load` reads with its tyres.
pub fn physics_path(dir: &Path) -> PathBuf {
    dir.join(CAR)
}

/// Names of the packages in `cars_dir()`, sorted.
pub fn list() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(cars_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| is_package(&e.path()))
        .filter_map(|e| e.file_name().to_str().map(String::from))
        .collect();
    names.sort();
    names
}

/// How the meshes of a part move with the simulated car. Positions are in m along the
/// body axes: x forward, y left, z up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Part {
    /// Fixed to the body. Relative to the centre of gravity at static ride height.
    Body,
    /// A wheel: rim and tyre, which spin about the axle (`CarVisual::wheel_axles`, else
    /// y), steer and follow the suspension. Relative to the wheel centre. The index is the simulation's wheel order:
    /// front-left, front-right, rear-left, rear-right.
    Wheel(u8),
    /// What steers and follows the suspension with a wheel without spinning, such as
    /// uprights and brake callipers. Relative to the wheel centre.
    Hub(u8),
    /// The steering wheel, turning about `SteeringWheel::axis`. Relative to its pivot.
    SteeringWheel,
}

/// The steering wheel's pivot and axis.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SteeringWheel {
    /// Centre of the rim, relative to the centre of gravity.
    pub pivot: [f32; 3],
    /// Unit vector along the column, pointing to the driver. The steering input, the
    /// steering wheel angle, turns the wheel by that angle about this axis, so that a
    /// positive (left) input turns it anticlockwise as the driver sees it.
    pub axis: [f32; 3],
}

/// The 3D model of a car and how its parts move.
#[derive(Clone, Debug, PartialEq)]
pub struct CarVisual {
    /// Meshes in the frames of their parts (see `Part`).
    pub visual: Visual,
    /// The part of each of `visual.meshes`.
    pub mesh_parts: Vec<Part>,
    pub steering_wheel: Option<SteeringWheel>,
    /// The driver's eye point relative to the centre of gravity, for the cockpit view.
    pub driver_eye: Option<[f32; 3]>,
    /// Each wheel's axle as modelled, a unit vector pointing left: models may tilt the
    /// wheels by their camber and toe, and a wheel spun about y would then wobble.
    pub wheel_axles: Option<[[f32; 3]; 4]>,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    format: u32,
    mesh_parts: Vec<Part>,
    steering_wheel: Option<SteeringWheel>,
    driver_eye: Option<[f32; 3]>,
    #[serde(default)]
    wheel_axles: Option<[[f32; 3]; 4]>,
}

impl CarVisual {
    fn validate(&self) -> Result<(), Error> {
        self.visual.validate().map_err(Error::Visual)?;
        if self.mesh_parts.len() != self.visual.meshes.len() {
            return Err(Error::Format(format!(
                "visual: {} parts for {} meshes",
                self.mesh_parts.len(),
                self.visual.meshes.len()
            )));
        }
        let bad_wheel = |p: &Part| matches!(p, Part::Wheel(i) | Part::Hub(i) if *i > 3);
        if self.mesh_parts.iter().any(bad_wheel) {
            return Err(Error::Format("visual: wheel index out of range".into()));
        }
        if self.mesh_parts.contains(&Part::SteeringWheel) && self.steering_wheel.is_none() {
            return Err(Error::Format(
                "visual: steering wheel meshes without a pivot".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct CarPackage {
    /// The car's tyre fields are replaced by the package's tyre files when saved.
    pub params: CarParams,
    pub front_tire: TireParams,
    pub rear_tire: TireParams,
    pub visual: Option<CarVisual>,
}

fn read(path: &Path) -> Result<Vec<u8>, Error> {
    std::fs::read(path).map_err(|e| Error::Io(path.to_path_buf(), e))
}

fn write(path: &Path, data: &[u8]) -> Result<(), Error> {
    std::fs::write(path, data).map_err(|e| Error::Io(path.to_path_buf(), e))
}

fn read_ron<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, Error> {
    let src = String::from_utf8_lossy(&read(path)?).into_owned();
    ron::from_str(&src).map_err(|e| Error::Ron(path.to_path_buf(), e.to_string()))
}

fn write_ron<T: Serialize>(path: &Path, value: &T) -> Result<(), Error> {
    let src = ron::ser::to_string_pretty(value, ron::ser::PrettyConfig::default())
        .map_err(|e| Error::Ron(path.to_path_buf(), e.to_string()))?;
    write(path, src.as_bytes())
}

impl CarPackage {
    /// Reads a package; `visual` also reads the 3D model when there is one.
    pub fn load(dir: &Path, visual: bool) -> Result<Self, Error> {
        let params: CarParams = read_ron(&dir.join(CAR))?;
        let front_tire = read_ron(&tire_path(dir, &params.front.tire))?;
        let rear_tire = read_ron(&tire_path(dir, &params.rear.tire))?;
        let visual = if visual {
            Self::load_visual(dir)?
        } else {
            None
        };
        Ok(Self {
            params,
            front_tire,
            rear_tire,
            visual,
        })
    }

    /// Reads only the 3D model of the package in `dir`, if it has one.
    pub fn load_visual(dir: &Path) -> Result<Option<CarVisual>, Error> {
        let path = dir.join(MANIFEST);
        if !path.is_file() {
            return Ok(None);
        }
        let manifest: Manifest = read_ron(&path)?;
        if manifest.format != FORMAT_VERSION {
            return Err(Error::Format(format!(
                "{}: format version {}, expected {FORMAT_VERSION}; convert the car again",
                path.display(),
                manifest.format
            )));
        }
        let visual = CarVisual {
            visual: Visual::decode(&read(&dir.join(VISUAL))?).map_err(Error::Visual)?,
            mesh_parts: manifest.mesh_parts,
            steering_wheel: manifest.steering_wheel,
            driver_eye: manifest.driver_eye,
            wheel_axles: manifest.wheel_axles,
        };
        visual.validate()?;
        Ok(Some(visual))
    }

    /// Writes the package into `dir`, creating it if needed.
    pub fn save(&self, dir: &Path) -> Result<(), Error> {
        if let Some(v) = &self.visual {
            v.validate()?;
        }
        std::fs::create_dir_all(dir).map_err(|e| Error::Io(dir.to_path_buf(), e))?;
        write_ron(&dir.join(FRONT_TIRE), &self.front_tire)?;
        write_ron(&dir.join(REAR_TIRE), &self.rear_tire)?;
        let (manifest, visual) = (dir.join(MANIFEST), dir.join(VISUAL));
        match &self.visual {
            Some(v) => {
                write(&visual, &v.visual.encode())?;
                write_ron(
                    &manifest,
                    &Manifest {
                        format: FORMAT_VERSION,
                        mesh_parts: v.mesh_parts.clone(),
                        steering_wheel: v.steering_wheel,
                        driver_eye: v.driver_eye,
                        wheel_axles: v.wheel_axles,
                    },
                )?;
            }
            None => {
                for path in [manifest, visual] {
                    if path.exists() {
                        std::fs::remove_file(&path).map_err(|e| Error::Io(path, e))?;
                    }
                }
            }
        }
        let mut params = self.params.clone();
        params.front.tire = FRONT_TIRE.into();
        params.rear.tire = REAR_TIRE.into();
        // Last, so a directory with a car file is always a complete package.
        write_ron(&dir.join(CAR), &params)
    }
}

#[cfg(test)]
mod tests {
    use open_racing_sim::CarModel;
    use open_racing_track::{Material, VisualBuilder};

    use super::*;

    fn package() -> CarPackage {
        let gt3 = CarModel::gt3();
        let mut v = VisualBuilder::new();
        let tri = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let mut mesh_parts = Vec::new();
        for part in [Part::Body, Part::Wheel(2), Part::SteeringWheel] {
            let m = v.add_material(Material::default());
            v.add_mesh(
                m,
                true,
                &tri,
                &[[0.0, 0.0, 1.0]; 3],
                &[[0.0; 2]; 3],
                &[0, 1, 2],
            );
            mesh_parts.push(part);
        }
        CarPackage {
            params: gt3.params.clone(),
            front_tire: gt3.front_tire.p.clone(),
            rear_tire: gt3.rear_tire.p.clone(),
            visual: Some(CarVisual {
                visual: v.build(),
                mesh_parts,
                steering_wheel: Some(SteeringWheel {
                    pivot: [0.5, 0.3, 0.2],
                    axis: [-1.0, 0.0, 0.0],
                }),
                driver_eye: Some([-0.2, 0.3, 0.5]),
                wheel_axles: Some([[0.0, 0.998, 0.061]; 4]),
            }),
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("open-racing-car-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn round_trips_through_files() {
        let dir = temp_dir("round-trip");
        let pkg = package();
        pkg.save(&dir).unwrap();
        assert!(is_package(&dir));

        // The physics file loads on its own, with the tyres it names.
        let model = CarModel::load(physics_path(&dir)).unwrap();
        assert_eq!(model.params.mass, pkg.params.mass);
        assert_eq!(model.rear_tire.p.name, pkg.rear_tire.name);

        let physics = CarPackage::load(&dir, false).unwrap();
        assert!(physics.visual.is_none());
        let full = CarPackage::load(&dir, true).unwrap();
        assert_eq!(full.visual, pkg.visual);

        // Saving without a model removes the old one.
        CarPackage {
            visual: None,
            ..pkg
        }
        .save(&dir)
        .unwrap();
        assert!(CarPackage::load_visual(&dir).unwrap().is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_inconsistent_models() {
        let dir = temp_dir("broken");
        let mut pkg = package();
        pkg.visual.as_mut().unwrap().mesh_parts.pop();
        assert!(pkg.save(&dir).is_err());
        let mut pkg = package();
        pkg.visual.as_mut().unwrap().mesh_parts[1] = Part::Hub(4);
        assert!(pkg.save(&dir).is_err());
        let mut pkg = package();
        pkg.visual.as_mut().unwrap().steering_wheel = None;
        assert!(pkg.save(&dir).is_err());
    }
}
