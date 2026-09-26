//! open-racing's track package format.
//!
//! A package is a directory holding one track:
//! - `track.ron`: format version, centreline, surface table, race layout and the sky
//!   and light the track is shown in. Small and readable.
//! - `ground.bin`: drivable surfaces and walls for the physics.
//! - `visual.bin` (optional): meshes, materials and textures for rendering.
//!
//! Physics and rendering data live in separate files so that simulation-only processes
//! (training, evaluation) never read the bulk of a package. All geometry is in the
//! simulation's world frame (m, Z up), stored as `f32`.
//!
//! The runtime reads only this format. Tracks made in other formats are converted into
//! packages ahead of time by separate converter crates.

mod bin;
pub mod ground;
#[cfg(feature = "encode")]
pub mod texture;
pub mod visual;

use std::path::{Path, PathBuf};

use open_racing_sim::{Layout, Sky, SurfaceProps, Track, TrackDef, TrackError};
use serde::{Deserialize, Serialize};

pub use ground::{Ground, Patch, PatchKind};
pub use visual::{
    AlphaMode, BARE, Detail, DetailLayer, DetailMask, DetailNormal, FAR_AWAY, IMPOSTOR_FRAMES,
    Instance, Instances, Level, Material, Mesh, Shape, Texture, Varies, Visual, VisualBuilder,
};

/// Version of the package layout and of `track.ron`.
pub const FORMAT_VERSION: u32 = 2;
const MANIFEST: &str = "track.ron";
const GROUND: &str = "ground.bin";
const VISUAL: &str = "visual.bin";

#[derive(Debug)]
pub enum Error {
    Io(PathBuf, std::io::Error),
    Manifest(PathBuf, Box<ron::error::SpannedError>),
    Format(String),
    Track(TrackError),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(p, e) => write!(f, "{}: {e}", p.display()),
            Self::Manifest(p, e) => write!(f, "{}: {e}", p.display()),
            Self::Format(msg) => f.write_str(msg),
            Self::Track(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

/// Directory for user-supplied content, which is never part of the repository.
/// Override with `OPEN_RACING_CONTENT`.
pub fn content_dir() -> PathBuf {
    std::env::var_os("OPEN_RACING_CONTENT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            // The repository's root, two folders up from this crate, without the `..`s
            // that would show in every path built on it.
            let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
            let root = crate_dir.ancestors().nth(2).unwrap_or(crate_dir);
            root.join("content")
        })
}

/// Where packages are looked up by name: `<content>/tracks/<name>/`.
pub fn tracks_dir() -> PathBuf {
    content_dir().join("tracks")
}

/// Whether `dir` holds a package.
pub fn is_package(dir: &Path) -> bool {
    dir.join(MANIFEST).is_file()
}

/// Names of the packages in `tracks_dir()`, sorted.
pub fn list() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(tracks_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| is_package(&e.path()))
        .filter_map(|e| e.file_name().to_str().map(String::from))
        .collect();
    names.sort();
    names
}

/// The sky and the light a track is shown in: the time of day and the season, the
/// weather, and how the picture is exposed. The app starts in them unless its weather
/// settings say otherwise.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Environment {
    pub sky: Sky,
    /// Whether the sky changes by itself as time goes by.
    pub changing: bool,
    /// Time of day, h (local solar time).
    pub hour: f64,
    /// Month, 1 to 12.
    pub month: u32,
    /// Latitude of the place, degrees north: where the sun runs.
    pub latitude: Option<f64>,
    /// Added to the air temperature the month and the sky give, K.
    pub temperature: f64,
    /// Brightens (above 0) or darkens the picture from what the daylight calls for, EV.
    pub exposure: f64,
    /// How thick the haze is against what the weather gives: 0 none, 1 as it gives.
    pub haze: f64,
    /// The season plants show (leaves turned, fallen or fresh), when not the month's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub season: Option<Season>,
}

/// A season, as plants show it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Season {
    /// Fresh leaves.
    Spring,
    Summer,
    /// Leaves turned yellow, orange and red.
    Autumn,
    /// Broadleaf trees bare, grass straw.
    Winter,
}

impl Season {
    pub const ALL: [Season; 4] = [
        Season::Spring,
        Season::Summer,
        Season::Autumn,
        Season::Winter,
    ];
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            sky: Sky::Fair,
            changing: true,
            hour: 14.0,
            month: 6,
            latitude: None,
            temperature: 0.0,
            exposure: 0.0,
            haze: 1.0,
            season: None,
        }
    }
}

impl Environment {
    pub fn is_valid(&self) -> bool {
        (0.0..24.0).contains(&self.hour)
            && (1..=12).contains(&self.month)
            && self.latitude.is_none_or(|l| (-89.0..=89.0).contains(&l))
            && (-30.0..=30.0).contains(&self.temperature)
            && (-5.0..=5.0).contains(&self.exposure)
            && (0.0..=10.0).contains(&self.haze)
    }

    /// Day of the year in the middle of its month.
    pub fn day_of_year(&self) -> u32 {
        let m = self.month.clamp(1, 12) as usize;
        [15, 46, 74, 105, 135, 166, 196, 227, 258, 288, 319, 349][m - 1]
    }

    /// The time of year plants show, as months into a northern year (0 the start of
    /// January, 12 its end): the middle of the month, half a year on south of the
    /// equator (at `latitude` unless the environment gives its own), or the season's
    /// when it gives one.
    pub fn plant_month(&self, latitude: f64) -> f64 {
        match self.season {
            Some(Season::Spring) => 4.5,
            Some(Season::Summer) => 7.0,
            Some(Season::Autumn) => 10.1,
            Some(Season::Winter) => 1.0,
            None => {
                let south = self.latitude.unwrap_or(latitude) < 0.0;
                let m = self.month.clamp(1, 12) as f64 - 0.5;
                if south { (m + 6.0) % 12.0 } else { m }
            }
        }
    }
}

/// Reads the sky and light of the package in `dir`, if it gives them.
pub fn environment(dir: &Path) -> Option<Environment> {
    let src = std::fs::read_to_string(dir.join(MANIFEST)).ok()?;
    ron::from_str::<Manifest>(&src).ok()?.environment
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    format: u32,
    /// Centreline for track coordinates (progress, observations, lap timing). The
    /// start/finish line is at its first point.
    centreline: TrackDef,
    /// Surface types; `PatchKind::Ground` indexes into this.
    surfaces: Vec<SurfaceProps>,
    #[serde(default)]
    layout: Layout,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    environment: Option<Environment>,
}

#[derive(Clone, Debug)]
pub struct TrackPackage {
    pub centreline: TrackDef,
    pub surfaces: Vec<SurfaceProps>,
    pub layout: Layout,
    pub environment: Option<Environment>,
    pub ground: Ground,
    pub visual: Option<Visual>,
}

fn read(path: &Path) -> Result<Vec<u8>, Error> {
    std::fs::read(path).map_err(|e| Error::Io(path.to_path_buf(), e))
}

fn write(path: &Path, data: &[u8]) -> Result<(), Error> {
    std::fs::write(path, data).map_err(|e| Error::Io(path.to_path_buf(), e))
}

impl TrackPackage {
    /// Reads a package; `visual` also reads the render data when there is any.
    pub fn load(dir: &Path, visual: bool) -> Result<Self, Error> {
        let path = dir.join(MANIFEST);
        let src = String::from_utf8_lossy(&read(&path)?).into_owned();
        let manifest: Manifest =
            ron::from_str(&src).map_err(|e| Error::Manifest(path.clone(), Box::new(e)))?;
        if manifest.format != FORMAT_VERSION {
            return Err(Error::Format(format!(
                "{}: format version {}, expected {FORMAT_VERSION}; convert the track again",
                path.display(),
                manifest.format
            )));
        }
        let ground = Ground::decode(&read(&dir.join(GROUND))?)?;
        ground.validate(manifest.surfaces.len())?;
        let visual_path = dir.join(VISUAL);
        let visual = if visual && visual_path.is_file() {
            let v = Visual::decode(&read(&visual_path)?)?;
            v.validate()?;
            Some(v)
        } else {
            None
        };
        Ok(Self {
            centreline: manifest.centreline,
            surfaces: manifest.surfaces,
            layout: manifest.layout,
            environment: manifest.environment,
            ground,
            visual,
        })
    }

    /// Writes the package into `dir`, creating it if needed.
    pub fn save(&self, dir: &Path) -> Result<(), Error> {
        self.ground.validate(self.surfaces.len())?;
        if let Some(v) = &self.visual {
            v.validate()?;
        }
        std::fs::create_dir_all(dir).map_err(|e| Error::Io(dir.to_path_buf(), e))?;
        let manifest = Manifest {
            format: FORMAT_VERSION,
            centreline: self.centreline.clone(),
            surfaces: self.surfaces.clone(),
            layout: self.layout.clone(),
            environment: self.environment,
        };
        let ron = ron::ser::to_string_pretty(&manifest, ron::ser::PrettyConfig::default())
            .expect("manifest serialises");
        write(&dir.join(GROUND), &self.ground.encode())?;
        let visual_path = dir.join(VISUAL);
        match &self.visual {
            Some(v) => write(&visual_path, &v.encode())?,
            None if visual_path.exists() => {
                std::fs::remove_file(&visual_path).map_err(|e| Error::Io(visual_path, e))?
            }
            None => {}
        }
        // Last, so a directory with a manifest is always a complete package.
        write(&dir.join(MANIFEST), ron.as_bytes())
    }

    /// The simulation's track: the centreline with the tyres riding on the ground.
    pub fn build_track(&self) -> Result<Track, Error> {
        if !self.ground.is_drivable() {
            return Err(Error::Format(format!(
                "{}: no drivable surface",
                self.centreline.name
            )));
        }
        Ok(Track::new(&self.centreline)
            .map_err(Error::Track)?
            .with_ground(self.ground.build(&self.surfaces))
            .with_layout(self.layout.clone()))
    }
}

#[cfg(test)]
mod tests {
    use open_racing_sim::{Surface, TrackPoint};

    use super::*;

    fn package() -> TrackPackage {
        let n = 64;
        let points = (0..n)
            .map(|i| {
                let a = std::f64::consts::TAU * i as f64 / n as f64;
                TrackPoint {
                    pos: (100.0 * a.cos(), 100.0 * a.sin(), 0.0),
                    width_left: 6.0,
                    width_right: 6.0,
                    bank: 0.0,
                }
            })
            .collect();
        let centreline = TrackDef {
            name: "ring".into(),
            points,
            kerb_width: 1.0,
            kerb_height: 0.0,
            runoff_width: 30.0,
            spacing: 1.0,
        };
        let mut ground = Ground::default();
        let quad = [
            [-200.0, -200.0, 0.0],
            [200.0, -200.0, 0.0],
            [200.0, 200.0, 0.0],
            [-200.0, 200.0, 0.0],
        ];
        ground.add(
            PatchKind::Ground(0),
            &quad,
            &[[0.0, 0.0, 1.0]; 4],
            &[0, 1, 2, 0, 2, 3],
        );
        ground.add(
            PatchKind::Wall,
            &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            &[],
            &[0, 1, 2],
        );
        let mut v = VisualBuilder::new();
        let t = v.add_texture(Texture {
            data: b"DDS data".to_vec(),
        });
        let mask = v.add_texture(Texture {
            data: b"DDS mask".to_vec(),
        });
        let normal = v.add_texture(Texture {
            data: b"DDS normal".to_vec(),
        });
        let detail = Detail {
            mask: DetailMask::Texture(mask),
            layers: [
                None,
                Some(DetailLayer {
                    texture: t,
                    scale: 20.0,
                }),
                None,
                None,
            ],
            multiplier: 2.0,
            world_uv: true,
            normal: None,
        };
        let m = v.add_material(Material {
            base_color: [0.5, 0.5, 0.5, 1.0],
            base_color_texture: Some(t),
            roughness: 0.6,
            reflectance: 0.4,
            reflection: 0.2,
            surface_texture: Some(mask),
            normal_texture: Some(normal),
            alpha_mode: AlphaMode::Mask(0.4),
            double_sided: true,
            detail: Some(detail.clone()),
            varies: None,
            impostor: true,
        });
        v.add_material(Material {
            detail: Some(Detail {
                mask: DetailMask::BaseAlpha,
                world_uv: false,
                normal: Some(DetailNormal {
                    texture: normal,
                    scale: 40.0,
                    strength: 2.0,
                }),
                ..detail
            }),
            ..Default::default()
        });
        v.add_mesh(
            m,
            false,
            &quad,
            &[[0.0, 0.0, 1.0]; 4],
            &[[0.0, 0.0]; 4],
            &[0, 1, 2, 0, 2, 3],
        );
        TrackPackage {
            centreline,
            surfaces: vec![SurfaceProps::of(Surface::Asphalt)],
            layout: Layout {
                sectors: vec![200.0, 400.0],
                grid: vec![open_racing_sim::GridSlot { s: 620.0, d: 1.5 }],
                pit: None,
            },
            environment: None,
            ground,
            visual: Some(v.build()),
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("open-racing-track-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn round_trips_through_files() {
        let dir = temp_dir("round-trip");
        let pkg = package();
        pkg.save(&dir).unwrap();
        assert!(is_package(&dir));

        let physics = TrackPackage::load(&dir, false).unwrap();
        assert!(physics.visual.is_none());
        assert_eq!(physics.ground, pkg.ground);
        assert_eq!(physics.surfaces, pkg.surfaces);
        assert_eq!(physics.layout, pkg.layout);
        assert_eq!(physics.centreline.points.len(), 64);

        let full = TrackPackage::load(&dir, true).unwrap();
        assert_eq!(full.visual, pkg.visual);

        let track = full.build_track().unwrap();
        let q = track.query(glam::DVec3::new(100.0, 0.0, 0.5), 0);
        assert!(q.surface_point.z.abs() < 1e-9);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_broken_packages() {
        let dir = temp_dir("broken");
        let mut pkg = package();
        pkg.ground.patches[0].indices.push(99);
        assert!(pkg.save(&dir).is_err());
        let mut pkg = package();
        pkg.visual.as_mut().unwrap().materials[0].normal_texture = Some(99);
        assert!(pkg.save(&dir).is_err());

        let pkg = package();
        pkg.save(&dir).unwrap();
        let ground = dir.join(GROUND);
        let mut bytes = std::fs::read(&ground).unwrap();
        bytes.truncate(bytes.len() - 5);
        std::fs::write(&ground, &bytes).unwrap();
        assert!(TrackPackage::load(&dir, false).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
