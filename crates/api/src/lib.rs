//! Public API of open-racing (the "ports" of the onion architecture).
//!
//! Adapters — the Burn trainer, the Bevy app, future Python bindings — talk to the
//! simulation only through this crate. Everything crossing the boundary is plain
//! data: flat `f32` slices, `u8` flags and small spec structs, so any ML framework
//! or FFI layer can map it to its own tensor/array types without copies.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use open_racing_env::{
    ACTION_HIGH, ACTION_LOW, ACTION_NAMES, Actuator, AppliedInput, BatchEnv, EnvShared,
    MAX_ACTION_DIM,
};

pub use open_racing_env::{
    DefaultReward, DefaultTermination, Done, EnvConfig, EpisodeStats, LapTimer, MAX_SECTORS,
    RewardFn, StepInfo, TerminationFn,
};
pub use open_racing_sim::{
    Car, CarModel, Controls, RubberMap, Surface, Track, TrackCondition, TrackEvolution, parse_grip,
};

/// A box-shaped space with named dimensions.
#[derive(Clone, Debug, PartialEq)]
pub struct BoxSpace {
    pub names: Vec<String>,
    pub low: Vec<f32>,
    pub high: Vec<f32>,
}

impl BoxSpace {
    pub fn dim(&self) -> usize {
        self.names.len()
    }
}

/// Result of stepping a vectorised environment. Row `i` of every slice belongs to env `i`.
pub struct StepResult<'a> {
    /// `num_envs × obs_dim` observations after the step (already reset where done).
    pub obs: &'a [f32],
    /// `num_envs × obs_dim` last observation of episodes that ended; other rows are stale.
    pub final_obs: &'a [f32],
    pub rewards: &'a [f32],
    pub terminated: &'a [u8],
    pub truncated: &'a [u8],
}

/// A batch of independent environments stepped in lock-step (Gymnasium `VectorEnv` semantics
/// with automatic reset).
pub trait VecEnv: Send {
    fn num_envs(&self) -> usize;
    fn observation_space(&self) -> &BoxSpace;
    fn action_space(&self) -> &BoxSpace;
    /// Resets all envs and returns the `num_envs × obs_dim` observations.
    fn reset(&mut self, seed: u64) -> &[f32];
    /// Steps with `num_envs × action_dim` actions.
    fn step(&mut self, actions: &[f32]) -> StepResult<'_>;
    /// Episodes that ended during the last `step`: (env index, stats).
    fn finished_episodes(&self) -> &[(usize, EpisodeStats)];
}

/// Anything that maps observations to actions (a trained network, a scripted driver, …).
/// Batched: `obs` is `n × obs_dim`, `actions` is `n × action_dim`.
pub trait Policy {
    fn act(&mut self, obs: &[f32], actions: &mut [f32]);
}

#[derive(Debug)]
pub enum Error {
    Track(open_racing_sim::TrackError),
    Car(open_racing_sim::ParamsError),
    Package(open_racing_track::Error),
    CarPackage(open_racing_car::Error),
    NotFound(PathBuf),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Track(e) => e.fmt(f),
            Self::Car(e) => e.fmt(f),
            Self::Package(e) => e.fmt(f),
            Self::CarPackage(e) => e.fmt(f),
            Self::NotFound(p) => write!(f, "asset not found: {}", p.display()),
        }
    }
}

impl std::error::Error for Error {}

/// Directory containing `cars/` and `tracks/`. Override with `OPEN_RACING_ASSETS`.
pub fn assets_dir() -> PathBuf {
    std::env::var_os("OPEN_RACING_ASSETS")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets"))
}

fn resolve(kind: &str, name_or_path: &str) -> Result<PathBuf, Error> {
    let direct = PathBuf::from(name_or_path);
    let path = if direct.extension().is_some() {
        direct
    } else {
        assets_dir().join(kind).join(format!("{name_or_path}.ron"))
    };
    if path.exists() {
        Ok(path)
    } else {
        Err(Error::NotFound(path))
    }
}

/// Loads a track by name (`assets/tracks/<name>.ron` or the package
/// `<content>/tracks/<name>/`), or by path to either.
pub fn load_track(name_or_path: &str) -> Result<Track, Error> {
    Ok(load_track_and_model(name_or_path, false)?.0)
}

/// Like `load_track`, plus the 3D model for rendering when the track has one.
pub fn load_track_with_visual(
    name_or_path: &str,
) -> Result<(Track, Option<open_racing_track::Visual>), Error> {
    load_track_and_model(name_or_path, true)
}

/// Tries a package directory given by path, then `assets/tracks/<name>.ron` (or a file
/// path), then the package `<content>/tracks/<name>/`.
fn load_track_and_model(
    name_or_path: &str,
    visual: bool,
) -> Result<(Track, Option<open_racing_track::Visual>), Error> {
    let direct = Path::new(name_or_path);
    let named = open_racing_track::tracks_dir().join(name_or_path);
    let dir = match resolve("tracks", name_or_path) {
        _ if open_racing_track::is_package(direct) => direct.to_path_buf(),
        Ok(ron) => return Ok((Track::load(ron).map_err(Error::Track)?, None)),
        Err(_) if direct.extension().is_none() && open_racing_track::is_package(&named) => named,
        Err(not_found) => return Err(not_found),
    };
    let mut package =
        open_racing_track::TrackPackage::load(&dir, visual).map_err(Error::Package)?;
    Ok((
        package.build_track().map_err(Error::Package)?,
        package.visual.take(),
    ))
}

/// The sky and light a track package gives, if it is a package that gives them.
pub fn track_environment(name_or_path: &str) -> Option<open_racing_track::Environment> {
    let direct = Path::new(name_or_path);
    let named = open_racing_track::tracks_dir().join(name_or_path);
    let dir = if open_racing_track::is_package(direct) {
        direct
    } else {
        named.as_path()
    };
    open_racing_track::environment(dir)
}

/// Loads a car by name (`assets/cars/<name>.ron` or the package `<content>/cars/<name>/`),
/// or by path to either.
pub fn load_car(name_or_path: &str) -> Result<CarModel, Error> {
    let file = match car_package(name_or_path) {
        Some(dir) => open_racing_car::physics_path(&dir),
        None => resolve("cars", name_or_path)?,
    };
    CarModel::load(file).map_err(Error::Car)
}

/// Like `load_car`, plus the 3D model for rendering when the car has one.
pub fn load_car_with_visual(
    name_or_path: &str,
) -> Result<(CarModel, Option<open_racing_car::CarVisual>), Error> {
    let visual = match car_package(name_or_path) {
        Some(dir) => open_racing_car::CarPackage::load_visual(&dir).map_err(Error::CarPackage)?,
        None => None,
    };
    Ok((load_car(name_or_path)?, visual))
}

/// The package directory a car name or path refers to: a package given by path, or,
/// unless `assets/cars/<name>.ron` exists, the package `<content>/cars/<name>/`.
fn car_package(name_or_path: &str) -> Option<PathBuf> {
    let direct = Path::new(name_or_path);
    if open_racing_car::is_package(direct) {
        return Some(direct.to_path_buf());
    }
    let named = open_racing_car::cars_dir().join(name_or_path);
    (resolve("cars", name_or_path).is_err()
        && direct.extension().is_none()
        && open_racing_car::is_package(&named))
    .then_some(named)
}

/// Names of the cars in the assets directory, then of the packages in the content directory.
pub fn list_cars() -> Vec<String> {
    let mut names = ron_names("cars");
    names.extend(open_racing_car::list());
    names
}

fn ron_names(kind: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(assets_dir().join(kind))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension()? == "ron").then(|| p.file_stem()?.to_str().map(String::from))?
        })
        .collect();
    names.sort();
    names
}

/// Names of the tracks in the assets directory, then of the packages in the content directory.
pub fn list_tracks() -> Vec<String> {
    let mut names = ron_names("tracks");
    names.extend(open_racing_track::list());
    names
}

/// Parses a racing-line grip level or range for `EnvConfig::track_grip`: `green`,
/// `0.97`, `97%`, or a range such as `green..optimum`.
pub fn parse_grip_range(s: &str) -> Result<(f64, f64), String> {
    let (lo, hi) = match s.split_once("..") {
        Some((lo, hi)) => (parse_grip(lo)?, parse_grip(hi)?),
        None => {
            let g = parse_grip(s)?;
            (g, g)
        }
    };
    Ok((lo.min(hi), lo.max(hi)))
}

/// Everything needed to build environments.
#[derive(Clone)]
pub struct EnvSpec {
    pub track: Arc<Track>,
    pub car: Arc<CarModel>,
    pub config: EnvConfig,
    pub reward: Arc<dyn RewardFn>,
    pub termination: Arc<dyn TerminationFn>,
}

impl EnvSpec {
    pub fn new(track: Track, car: CarModel, config: EnvConfig) -> Self {
        Self {
            track: Arc::new(track),
            car: Arc::new(car),
            config,
            reward: Arc::new(DefaultReward::default()),
            termination: Arc::new(DefaultTermination::default()),
        }
    }

    /// Loads track and car by name (see `load_track` and `load_car`).
    pub fn from_names(track: &str, car: &str, config: EnvConfig) -> Result<Self, Error> {
        Ok(Self::new(load_track(track)?, load_car(car)?, config))
    }

    fn shared(&self) -> EnvShared {
        EnvShared {
            reward: self.reward.clone(),
            termination: self.termination.clone(),
            ..EnvShared::new(self.config.clone(), self.track.clone(), self.car.clone())
        }
    }

    pub fn observation_space(&self) -> BoxSpace {
        let names = self.config.obs_layout().spec().names;
        let n = names.len();
        BoxSpace {
            names,
            low: vec![f32::NEG_INFINITY; n],
            high: vec![f32::INFINITY; n],
        }
    }

    pub fn action_space(&self) -> BoxSpace {
        let dim = self.config.action_dim();
        BoxSpace {
            names: ACTION_NAMES[..dim].iter().map(|n| n.to_string()).collect(),
            low: ACTION_LOW[..dim].to_vec(),
            high: ACTION_HIGH[..dim].to_vec(),
        }
    }

    /// Builds a vectorised environment with `num_envs` parallel copies.
    pub fn make_vec_env(&self, num_envs: usize) -> RacingVecEnv {
        RacingVecEnv {
            obs_space: self.observation_space(),
            act_space: self.action_space(),
            inner: BatchEnv::new(self.shared(), num_envs),
        }
    }

    /// Builds a driver that lets a `Policy` drive a car simulated elsewhere (e.g. the app).
    pub fn agent_driver(&self) -> AgentDriver {
        AgentDriver {
            config: self.config.clone(),
            obs: vec![0.0; self.observation_space().dim()],
            action: [0.0; MAX_ACTION_DIM],
            actuator: Actuator::default(),
            input: AppliedInput::default(),
            hint: 0,
            countdown: 0,
        }
    }
}

/// The racing environment behind the `VecEnv` port.
pub struct RacingVecEnv {
    inner: BatchEnv,
    obs_space: BoxSpace,
    act_space: BoxSpace,
}

impl RacingVecEnv {
    /// Read access to the underlying cars (for visualisation or debugging).
    pub fn cars(&self) -> impl Iterator<Item = &Car> {
        self.inner.envs().iter().map(|e| &e.car)
    }

    pub fn episode_stats(&self) -> impl Iterator<Item = &EpisodeStats> {
        self.inner.envs().iter().map(|e| &e.stats)
    }
}

impl VecEnv for RacingVecEnv {
    fn num_envs(&self) -> usize {
        self.inner.num_envs()
    }

    fn observation_space(&self) -> &BoxSpace {
        &self.obs_space
    }

    fn action_space(&self) -> &BoxSpace {
        &self.act_space
    }

    fn reset(&mut self, seed: u64) -> &[f32] {
        self.inner.reset(seed);
        self.inner.obs()
    }

    fn step(&mut self, actions: &[f32]) -> StepResult<'_> {
        let r = self.inner.step(actions);
        StepResult {
            obs: r.obs,
            final_obs: r.final_obs,
            rewards: r.rewards,
            terminated: r.terminated,
            truncated: r.truncated,
        }
    }

    fn finished_episodes(&self) -> &[(usize, EpisodeStats)] {
        self.inner.finished()
    }
}

/// Drives a single externally simulated car with a `Policy`, reproducing exactly the
/// observation, action timing and actuation used in training.
pub struct AgentDriver {
    config: EnvConfig,
    obs: Vec<f32>,
    action: [f32; MAX_ACTION_DIM],
    actuator: Actuator,
    input: AppliedInput,
    hint: usize,
    countdown: usize,
}

impl AgentDriver {
    /// Call once per physics step, before `Car::step`.
    pub fn controls(&mut self, policy: &mut dyn Policy, car: &Car, track: &Track) -> Controls {
        if self.countdown == 0 {
            self.hint = track.locate(car.state.position, self.hint).index;
            open_racing_env::obs::encode(
                &self.config.obs_layout(),
                car,
                track,
                self.hint,
                &self.input,
                &mut self.obs,
            );
            let action = &mut self.action[..self.config.action_dim()];
            policy.act(&self.obs, action);
            self.actuator.decide(&self.config, action);
            self.countdown = self.config.substeps();
        }
        self.countdown -= 1;
        let action = &self.action[..self.config.action_dim()];
        let c = self.actuator.controls(&self.config, car, action);
        self.input = self.actuator.applied(car, action);
        c
    }

    /// Forget internal state (call after teleporting / resetting the car).
    pub fn reset(&mut self, car: &Car, track: &Track) {
        self.actuator = Actuator::default();
        self.input = AppliedInput::default();
        self.hint = track.nearest_index(car.state.position);
        self.countdown = 0;
    }
}
