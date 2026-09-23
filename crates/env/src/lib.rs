//! Reinforcement-learning environment (application layer).
//!
//! `Env` wraps one car on one track and turns agent actions into physics steps,
//! observations, rewards and episode boundaries. `BatchEnv` runs many `Env`s in
//! parallel and writes results into contiguous, preallocated buffers.

mod lap;
pub mod obs;
pub mod reward;
mod rng;

use std::sync::Arc;

use glam::DVec3;
use open_racing_sim::{AutoShift, Car, CarModel, Controls, DT, Shift, Surface, Track};
use rayon::prelude::*;

pub use lap::LapTimer;
pub use obs::{AppliedInput, ObsLayout, ObsSpec};
pub use reward::{DefaultReward, DefaultTermination, Done, RewardFn, StepInfo, TerminationFn};
pub use rng::Rng;

/// Action dimensions: steering (−1..1 of full lock), throttle (0..1), brake (0..1) and,
/// without `auto_shift`, a gear request (> 0.5 up, < −0.5 down).
pub const ACTION_NAMES: [&str; MAX_ACTION_DIM] = ["steer", "throttle", "brake", "shift"];
pub const ACTION_LOW: [f32; MAX_ACTION_DIM] = [-1.0, 0.0, 0.0, -1.0];
pub const ACTION_HIGH: [f32; MAX_ACTION_DIM] = [1.0, 1.0, 1.0, 1.0];
pub const MAX_ACTION_DIM: usize = 4;
const SHIFT_THRESHOLD: f32 = 0.5;

#[derive(Clone, Debug)]
pub struct EnvConfig {
    /// Agent decisions per second; each decision spans `1000 / control_hz` physics steps.
    pub control_hz: f64,
    /// Start at a random point of the track (otherwise at the start line).
    pub random_start: bool,
    /// Initial speed range in m/s.
    pub start_speed: (f64, f64),
    /// Initial lateral offset range in m.
    pub start_offset: (f64, f64),
    /// Let an automatic gear selector drive the sequential gearbox.
    pub auto_shift: bool,
    /// Maximum steering wheel speed in rad/s (a human arm / wheel base limit).
    pub max_steer_rate: f64,
    pub lookahead_points: usize,
    pub lookahead_spacing: f64,
    /// Append ground-truth tyre state to the observation.
    pub privileged_obs: bool,
    /// Append tread temperatures and pressures, as sims report them in telemetry.
    pub tyre_obs: bool,
    /// Append the track widths left and right of each lookahead point.
    pub edge_obs: bool,
    pub seed: u64,
}

impl Default for EnvConfig {
    fn default() -> Self {
        Self {
            control_hz: 50.0,
            random_start: true,
            start_speed: (0.0, 40.0),
            start_offset: (-2.0, 2.0),
            auto_shift: true,
            max_steer_rate: 15.0,
            lookahead_points: 20,
            lookahead_spacing: 10.0,
            privileged_obs: false,
            tyre_obs: false,
            edge_obs: false,
            seed: 0,
        }
    }
}

impl EnvConfig {
    pub fn substeps(&self) -> usize {
        ((1.0 / DT) / self.control_hz).round().max(1.0) as usize
    }

    /// Number of action dimensions: the gear request only exists without `auto_shift`.
    pub fn action_dim(&self) -> usize {
        if self.auto_shift {
            MAX_ACTION_DIM - 1
        } else {
            MAX_ACTION_DIM
        }
    }

    pub fn obs_layout(&self) -> ObsLayout {
        ObsLayout {
            lookahead_points: self.lookahead_points,
            lookahead_spacing: self.lookahead_spacing,
            privileged: self.privileged_obs,
            tyres: self.tyre_obs,
            edges: self.edge_obs,
        }
    }
}

/// Shared, read-only pieces of the environment.
#[derive(Clone)]
pub struct EnvShared {
    pub config: EnvConfig,
    pub track: Arc<Track>,
    pub car: Arc<CarModel>,
    pub reward: Arc<dyn RewardFn>,
    pub termination: Arc<dyn TerminationFn>,
}

impl EnvShared {
    pub fn new(config: EnvConfig, track: Arc<Track>, car: Arc<CarModel>) -> Self {
        Self {
            config,
            track,
            car,
            reward: Arc::new(DefaultReward::default()),
            termination: Arc::new(DefaultTermination::default()),
        }
    }
}

/// Per-episode statistics exposed to the caller.
#[derive(Clone, Copy, Debug, Default)]
pub struct EpisodeStats {
    pub time: f64,
    pub progress: f64,
    pub return_: f64,
    pub laps: u32,
    pub last_lap_time: Option<f64>,
    pub best_lap_time: Option<f64>,
}

pub struct Env {
    pub car: Car,
    rng: Rng,
    lap: LapTimer,
    actuator: Actuator,
    input: AppliedInput,
    info: StepInfo,
    pub stats: EpisodeStats,
    /// Stats of the previous episode (valid right after an automatic reset).
    pub last_episode: EpisodeStats,
}

impl Env {
    pub fn new(shared: &EnvShared, seed: u64) -> Self {
        let car = Car::new(shared.car.clone(), &shared.track, 0.0, 0.0, 0.0, 1);
        let mut env = Self {
            car,
            rng: Rng::new(seed),
            lap: LapTimer::default(),
            actuator: Actuator::default(),
            input: AppliedInput::default(),
            info: StepInfo::default(),
            stats: EpisodeStats::default(),
            last_episode: EpisodeStats::default(),
        };
        env.reset(shared);
        env
    }

    pub fn reset(&mut self, shared: &EnvShared) {
        let cfg = &shared.config;
        let track = &*shared.track;
        let s = if cfg.random_start {
            self.rng.uniform(0.0, track.length)
        } else {
            0.0
        };
        let d = self.rng.uniform(cfg.start_offset.0, cfg.start_offset.1);
        let speed = self.rng.uniform(cfg.start_speed.0, cfg.start_speed.1);
        let gear = gear_for_speed(&shared.car, speed);
        self.car.reset(track, s, d, speed, gear);
        self.lap = LapTimer::new(track, self.car.state.position);
        self.actuator = Actuator::default();
        self.input = AppliedInput::default();
        self.info = StepInfo::default();
        self.last_episode = self.stats;
        self.stats = EpisodeStats::default();
    }

    pub fn observe(&self, shared: &EnvShared, out: &mut [f32]) {
        obs::encode(
            &shared.config.obs_layout(),
            &self.car,
            &shared.track,
            self.lap.hint(),
            &self.input,
            out,
        );
    }

    /// Applies one agent action. Returns the reward and whether the episode ended.
    pub fn step(&mut self, shared: &EnvShared, action: &[f32]) -> (f64, Option<Done>) {
        let cfg = &shared.config;
        let track = &*shared.track;
        let prev_steer = self.input.steer;
        let substeps = cfg.substeps();
        self.actuator.decide(cfg, action);
        let mut barrier_impact: f64 = 0.0;
        for _ in 0..substeps {
            let controls = self.actuator.controls(cfg, &self.car, action);
            self.car.step(track, &controls);
            barrier_impact = barrier_impact.max(self.car.telemetry.barrier_impact);
        }
        self.input = self.actuator.applied(&self.car, action);

        let dt = substeps as f64 * DT;
        let st = &self.car.state;
        let (q, progress) = self.lap.update(track, st.position, st.time);
        let total_progress = self.lap.progress;
        self.stats.laps = self.lap.laps;
        self.stats.last_lap_time = self.lap.last_lap;
        self.stats.best_lap_time = self.lap.best_lap;

        let speed = self.car.speed();
        let forward = st.orientation * DVec3::X;
        let heading_cos = q
            .tangent
            .truncate()
            .normalize()
            .dot(forward.truncate().normalize());
        let half_width = if q.d >= 0.0 {
            q.width_left
        } else {
            q.width_right
        };
        let wheels_off = self
            .car
            .telemetry
            .wheels
            .iter()
            .filter(|w| w.surface == Surface::Grass)
            .count();
        let grip_loss = (0..4)
            .map(|i| {
                let t = &self.car.telemetry.wheels[i];
                1.0 - self.car.model.tire(i).condition_grip(
                    &st.wheels[i].tire,
                    &t.tread_load,
                    t.pressure,
                )
            })
            .sum();
        let prev = self.info;
        let info = StepInfo {
            progress,
            total_progress,
            speed,
            offset: q.d / half_width,
            wheels_off,
            grip_loss,
            barrier_impact,
            heading_cos,
            steer_change: self.input.steer - prev_steer,
            time: st.time,
            off_track_time: if wheels_off == 4 {
                prev.off_track_time + dt
            } else {
                0.0
            },
            stuck_time: if speed < 1.0 && st.time > 3.0 {
                prev.stuck_time + dt
            } else {
                0.0
            },
            wrong_way_time: if heading_cos < -0.3 && speed > 3.0 {
                prev.wrong_way_time + dt
            } else {
                0.0
            },
            invalid: !(st.position.is_finite() && st.velocity.is_finite()),
        };
        self.info = info;

        let done = shared.termination.done(&info);
        let reward = shared.reward.reward(&info, done);
        self.stats.time = st.time;
        self.stats.progress = info.total_progress;
        self.stats.return_ += reward;
        (reward, done)
    }
}

/// Turns a normalised agent action into physical `Controls`, one physics step at a
/// time: steering wheel speed is limited like a human's, gears are chosen
/// automatically when enabled.
#[derive(Clone, Copy, Debug, Default)]
pub struct Actuator {
    steer: f64,
    /// Gear request of the current decision, not yet sent to the gearbox.
    shift: Shift,
}

impl Actuator {
    /// Call once per agent decision, before its physics steps: the gear request of
    /// `action` is sent on the next physics step only, like one press of a paddle.
    pub fn decide(&mut self, cfg: &EnvConfig, action: &[f32]) {
        self.shift = match action.get(MAX_ACTION_DIM - 1) {
            Some(&a) if !cfg.auto_shift && a > SHIFT_THRESHOLD => Shift::Up,
            Some(&a) if !cfg.auto_shift && a < -SHIFT_THRESHOLD => Shift::Down,
            _ => Shift::None,
        };
    }

    pub fn controls(&mut self, cfg: &EnvConfig, car: &Car, action: &[f32]) -> Controls {
        let lock = car.model.params.steering.lock;
        let target = f64::from(action[0]).clamp(-1.0, 1.0) * lock;
        let max_delta = cfg.max_steer_rate * DT;
        self.steer += (target - self.steer).clamp(-max_delta, max_delta);
        Controls {
            steer_wheel_angle: self.steer,
            throttle: f64::from(action[1]).clamp(0.0, 1.0),
            brake: f64::from(action[2]).clamp(0.0, 1.0),
            clutch: 0.0,
            shift: if cfg.auto_shift {
                AutoShift.shift(car)
            } else {
                self.take_shift(car)
            },
        }
    }

    /// The pending gear request; never shifts below first gear (into neutral or reverse),
    /// and, like a GT3 gearbox controller, refuses a downshift that would over-rev the engine.
    fn take_shift(&mut self, car: &Car) -> Shift {
        let dt = &car.state.drivetrain;
        match std::mem::take(&mut self.shift) {
            Shift::Down if dt.gear <= 1 => Shift::None,
            Shift::Down => {
                let p = &car.model.params;
                let g = &p.gearbox.ratios;
                let idx = (dt.gear - 1) as usize;
                if dt.rpm() * g[idx - 1] / g[idx] > p.engine.limiter_rpm {
                    Shift::None
                } else {
                    Shift::Down
                }
            }
            shift => shift,
        }
    }

    /// The input actually applied, as seen by the observation.
    pub fn applied(&self, car: &Car, action: &[f32]) -> AppliedInput {
        AppliedInput {
            steer: self.steer / car.model.params.steering.lock,
            throttle: f64::from(action[1]).clamp(0.0, 1.0),
            brake: f64::from(action[2]).clamp(0.0, 1.0),
        }
    }
}

/// Highest gear that keeps the engine above ~55% of the limiter at `speed`.
fn gear_for_speed(car: &CarModel, speed: f64) -> i32 {
    let p = &car.params;
    let wheel = speed / car.rear_tire.p.radius;
    let rpm = |g: usize| {
        wheel * p.gearbox.ratios[g] * p.gearbox.final_drive * 60.0 / std::f64::consts::TAU
    };
    let target = 0.55 * p.engine.limiter_rpm;
    (0..p.gearbox.ratios.len())
        .rev()
        .find(|&g| rpm(g) >= target)
        .map_or(1, |g| g as i32 + 1)
}

/// Many environments stepped in parallel with results in flat buffers.
pub struct BatchEnv {
    pub shared: EnvShared,
    envs: Vec<Env>,
    obs_dim: usize,
    obs: Vec<f32>,
    final_obs: Vec<f32>,
    rewards: Vec<f32>,
    terminated: Vec<u8>,
    truncated: Vec<u8>,
    /// Stats of episodes that finished during the last `step`.
    finished: Vec<(usize, EpisodeStats)>,
}

/// Borrowed view of the result of a batch step. All slices are indexed by env.
pub struct BatchStep<'a> {
    /// Observations after the step (after automatic reset for finished envs).
    pub obs: &'a [f32],
    /// Observation at the end of each finished episode (unchanged rows otherwise).
    pub final_obs: &'a [f32],
    pub rewards: &'a [f32],
    pub terminated: &'a [u8],
    pub truncated: &'a [u8],
}

impl BatchEnv {
    pub fn new(shared: EnvShared, num_envs: usize) -> Self {
        let obs_dim = shared.config.obs_layout().spec().dim();
        let envs = (0..num_envs)
            .map(|i| {
                Env::new(
                    &shared,
                    shared
                        .config
                        .seed
                        .wrapping_mul(0x1000_0001)
                        .wrapping_add(i as u64),
                )
            })
            .collect();
        let mut batch = Self {
            envs,
            obs_dim,
            obs: vec![0.0; num_envs * obs_dim],
            final_obs: vec![0.0; num_envs * obs_dim],
            rewards: vec![0.0; num_envs],
            terminated: vec![0; num_envs],
            truncated: vec![0; num_envs],
            finished: Vec::with_capacity(num_envs),
            shared,
        };
        batch.write_all_obs();
        batch
    }

    pub fn num_envs(&self) -> usize {
        self.envs.len()
    }

    pub fn obs_dim(&self) -> usize {
        self.obs_dim
    }

    pub fn envs(&self) -> &[Env] {
        &self.envs
    }

    pub fn obs(&self) -> &[f32] {
        &self.obs
    }

    /// Episodes that ended in the last `step`: (env index, stats).
    pub fn finished(&self) -> &[(usize, EpisodeStats)] {
        &self.finished
    }

    /// Re-seeds and resets every environment.
    pub fn reset(&mut self, seed: u64) {
        let shared = &self.shared;
        for (i, env) in self.envs.iter_mut().enumerate() {
            env.rng = Rng::new(seed.wrapping_mul(0x1000_0001).wrapping_add(i as u64));
            env.reset(shared);
        }
        self.write_all_obs();
    }

    fn write_all_obs(&mut self) {
        let shared = &self.shared;
        self.envs
            .par_iter()
            .zip(self.obs.par_chunks_mut(self.obs_dim))
            .for_each(|(env, out)| env.observe(shared, out));
    }

    /// Steps every env with its row of `actions` (`num_envs × config.action_dim()`).
    /// Finished episodes are reset automatically.
    pub fn step(&mut self, actions: &[f32]) -> BatchStep<'_> {
        let shared = &self.shared;
        let action_dim = shared.config.action_dim();
        assert_eq!(
            actions.len(),
            self.envs.len() * action_dim,
            "actions must be num_envs × {action_dim}"
        );
        let d = self.obs_dim;
        (
            self.envs.par_iter_mut(),
            actions.par_chunks(action_dim),
            self.obs.par_chunks_mut(d),
            self.final_obs.par_chunks_mut(d),
            self.rewards.par_iter_mut(),
            self.terminated.par_iter_mut(),
            self.truncated.par_iter_mut(),
        )
            .into_par_iter()
            .with_min_len(4)
            .for_each(|(env, action, obs, final_obs, reward, term, trunc)| {
                let (r, done) = env.step(shared, action);
                *reward = r as f32;
                *term = (done == Some(Done::Terminated)) as u8;
                *trunc = (done == Some(Done::Truncated)) as u8;
                if done.is_some() {
                    env.observe(shared, final_obs);
                    env.reset(shared);
                }
                env.observe(shared, obs);
            });

        self.finished.clear();
        for (i, env) in self.envs.iter().enumerate() {
            if self.terminated[i] != 0 || self.truncated[i] != 0 {
                self.finished.push((i, env.last_episode));
            }
        }
        BatchStep {
            obs: &self.obs,
            final_obs: &self.final_obs,
            rewards: &self.rewards,
            terminated: &self.terminated,
            truncated: &self.truncated,
        }
    }
}
