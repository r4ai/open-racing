//! Reinforcement-learning environment (application layer).
//!
//! `Env` wraps one car on one track and turns agent actions into physics steps,
//! observations, rewards and episode boundaries. `BatchEnv` runs many `Env`s in
//! parallel and writes results into contiguous, preallocated buffers.

mod lap;
pub mod obs;
pub mod reward;
mod rng;

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Arc;

use glam::DVec3;
use open_racing_sim::{
    AutoShift, BlipAssist, Car, CarModel, CarState, ClutchAssist, Controls, DT, GRAVITY, RubberMap,
    Shift, Telemetry, Track, TrackEvolution, Weather,
};
use rayon::prelude::*;

pub use lap::{LapTimer, MAX_SECTORS};
pub use obs::{AppliedInput, ObsLayout, ObsSpec};
pub use reward::{
    DefaultReward, DefaultTermination, Done, OFF_COURSE_WHEELS, RewardFn, StepInfo, TerminationFn,
};
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
    /// Restrict random starts to this longitudinal track interval, in metres.
    /// `None` samples the whole circuit. With [`Self::start_s_focus_fraction`]
    /// below one, other starts still sample the whole circuit.
    pub start_s_range: Option<(f64, f64)>,
    /// Share of random starts sampled from `start_s_range`, when present.
    pub start_s_focus_fraction: f64,
    /// Initial speed range in m/s.
    pub start_speed: (f64, f64),
    /// Cap the initial speed at what the corners within [`SAFE_START_DISTANCE`] allow at
    /// [`SAFE_START_LATERAL_G`], so no episode starts in a crash it cannot avoid.
    pub safe_start: bool,
    /// Initial lateral offset range in m.
    pub start_offset: (f64, f64),
    /// Let an automatic gear selector drive the sequential gearbox.
    pub auto_shift: bool,
    /// Anti-lock brakes, as GT3 cars have: the brake pressure backs off while a wheel
    /// locks. Without it the pedal locks the wheels well short of full travel.
    pub abs: bool,
    /// Limit throttle when the driven tyres spin beyond useful longitudinal slip.
    pub traction_control: bool,
    /// Maximum steering wheel speed in rad/s (a human arm / wheel base limit).
    pub max_steer_rate: f64,
    /// Track evolution: grip on the racing line at the start of an episode, relative
    /// to a fully rubbered-in line, drawn uniformly from this range (see
    /// `TrackCondition` for named levels). Off the line the asphalt is dirtier. `None`
    /// gives the whole asphalt the tyres' nominal grip.
    pub track_grip: Option<(f64, f64)>,
    /// Grip the racing line gains per lap the car drives, with `track_grip`. Off by
    /// default: one car adds little within an episode, and the laid rubber takes a
    /// grid of the whole track per environment.
    pub grip_gain_per_lap: f64,
    pub lookahead_points: usize,
    /// Distance from the car to the first lookahead point and between the first two, m.
    pub lookahead_spacing: f64,
    /// Each gap between lookahead points is this many times the one before, so a few
    /// points reach far ahead (braking from top speed) while the near ones stay dense.
    /// 1 spaces them evenly.
    pub lookahead_growth: f64,
    /// Append ground-truth tyre state to the observation.
    pub privileged_obs: bool,
    /// Append tread temperatures and pressures, as sims report them in telemetry.
    pub tyre_obs: bool,
    /// Append the track widths left and right of each lookahead point.
    pub edge_obs: bool,
    /// Append what changes over a stint, as sims report it in telemetry: tyre wear and
    /// carcass temperatures, brake disc temperatures and body damage.
    pub stint_obs: bool,
    /// Append the position around the lap (sine and cosine of its share).
    pub lap_position_obs: bool,
    /// The steering action spans the steering a car can use at its speed rather than
    /// the full lock: from the lock at walking pace to a few degrees at the wheels at
    /// top speed, so the same action resolution serves a hairpin and a fast sweeper.
    pub speed_scaled_steering: bool,
    /// Share of random starts on tyres as a stint leaves them: tread worn by up to
    /// `worn_start_max_wear`, treads and carcasses anywhere in their working range.
    pub worn_start_fraction: f64,
    pub worn_start_max_wear: f64,
    /// Share of resets from states recorded [`REPLAY_LEAD`] s before an earlier episode
    /// crashed (once any were recorded), so training dwells on the places it fails.
    pub replay_start_fraction: f64,
    /// Weather of each episode, drawn uniformly from these ranges: the air temperature,
    /// °C, how much warmer the road is than the air, K, and the wind speed at 10 m,
    /// m/s, blowing from any direction with gusts. The defaults keep the fixed
    /// standard conditions (25 °C air and road, 1.225 kg/m³, no wind).
    pub air_temperature: (f64, f64),
    pub road_heat: (f64, f64),
    pub wind_speed: (f64, f64),
    pub seed: u64,
}

impl Default for EnvConfig {
    fn default() -> Self {
        Self {
            control_hz: 50.0,
            random_start: true,
            start_s_range: None,
            start_s_focus_fraction: 1.0,
            start_speed: (0.0, 40.0),
            start_offset: (-2.0, 2.0),
            safe_start: false,
            auto_shift: true,
            abs: false,
            traction_control: false,
            max_steer_rate: 15.0,
            track_grip: None,
            grip_gain_per_lap: 0.0,
            lookahead_points: 20,
            lookahead_spacing: 10.0,
            lookahead_growth: 1.0,
            privileged_obs: false,
            tyre_obs: false,
            edge_obs: false,
            stint_obs: false,
            lap_position_obs: false,
            speed_scaled_steering: false,
            worn_start_fraction: 0.0,
            worn_start_max_wear: 0.0,
            replay_start_fraction: 0.0,
            air_temperature: STANDARD_WEATHER.0,
            road_heat: STANDARD_WEATHER.1,
            wind_speed: STANDARD_WEATHER.2,
            seed: 0,
        }
    }
}

/// The air temperature, road heat and wind ranges of the standard conditions.
const STANDARD_WEATHER: ((f64, f64), (f64, f64), (f64, f64)) =
    ((25.0, 25.0), (0.0, 0.0), (0.0, 0.0));
/// Range of the sea-level pressure of drawn weather, hPa.
const PRESSURE_RANGE: (f64, f64) = (995.0, 1030.0);

impl EnvConfig {
    /// Whether episodes run in [`Weather::STANDARD`] rather than drawn weather.
    pub fn standard_weather(&self) -> bool {
        (self.air_temperature, self.road_heat, self.wind_speed) == STANDARD_WEATHER
    }

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
            lookahead_growth: self.lookahead_growth,
            privileged: self.privileged_obs,
            tyres: self.tyre_obs,
            edges: self.edge_obs,
            stint: self.stint_obs,
            lap_position: self.lap_position_obs,
        }
    }
}

/// Shared, read-only pieces of the environment.
#[derive(Clone)]
pub struct EnvShared {
    pub config: EnvConfig,
    pub track: Arc<Track>,
    /// Racing line of `track`, when `config.track_grip` is set.
    pub rubber: Option<Arc<RubberMap>>,
    pub car: Arc<CarModel>,
    pub reward: Arc<dyn RewardFn>,
    pub termination: Arc<dyn TerminationFn>,
}

impl EnvShared {
    /// Also estimates the racing line of `track` when `config.track_grip` is set.
    pub fn new(config: EnvConfig, track: Arc<Track>, car: Arc<CarModel>) -> Self {
        Self {
            rubber: config.track_grip.map(|_| Arc::new(RubberMap::new(&track))),
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
    /// Weather of this episode.
    pub weather: Weather,
    /// Rubber on this episode's track.
    pub evolution: TrackEvolution,
    rng: Rng,
    lap: LapTimer,
    actuator: Actuator,
    input: AppliedInput,
    info: StepInfo,
    pub stats: EpisodeStats,
    /// Stats of the previous episode (valid right after an automatic reset).
    pub last_episode: EpisodeStats,
    /// Recent states, one every [`SNAPSHOT_INTERVAL`] s, oldest overwritten first.
    history: [Option<Snapshot>; HISTORY_LEN],
    next_snapshot: f64,
    /// The state [`REPLAY_LEAD`] s before the last crash, until the batch collects it.
    crash_snapshot: Option<Snapshot>,
}

/// Everything that makes a car's situation, to restart an episode from.
#[derive(Clone, Copy, Debug)]
pub struct Snapshot {
    state: CarState,
    telemetry: Telemetry,
    actuator: Actuator,
    input: AppliedInput,
}

/// How often a car's state is recorded, and how far before a crash it is replayed
/// from, s.
pub const SNAPSHOT_INTERVAL: f64 = 0.5;
pub const REPLAY_LEAD: f64 = 3.0;
const HISTORY_LEN: usize = (REPLAY_LEAD / SNAPSHOT_INTERVAL) as usize + 2;
/// Crash states kept for [`EnvConfig::replay_start_fraction`].
const REPLAY_CAPACITY: usize = 4096;

impl Env {
    pub fn new(shared: &EnvShared, seed: u64) -> Self {
        let car = Car::new(shared.car.clone(), &shared.track, 0.0, 0.0, 0.0, 1);
        let mut env = Self {
            car,
            weather: Weather::STANDARD,
            evolution: TrackEvolution::UNIFORM,
            rng: Rng::new(seed),
            lap: LapTimer::default(),
            actuator: Actuator::default(),
            input: AppliedInput::default(),
            info: StepInfo::default(),
            stats: EpisodeStats::default(),
            last_episode: EpisodeStats::default(),
            history: [None; HISTORY_LEN],
            next_snapshot: 0.0,
            crash_snapshot: None,
        };
        env.reset(shared);
        env
    }

    pub fn reset(&mut self, shared: &EnvShared) {
        self.reset_from(shared, &[]);
    }

    /// Resets, restarting from one of `replay` with [`EnvConfig::replay_start_fraction`].
    pub fn reset_from(&mut self, shared: &EnvShared, replay: &[Snapshot]) {
        let cfg = &shared.config;
        let track = &*shared.track;
        if !cfg.standard_weather() {
            self.weather = self.draw_weather(cfg);
        }
        if !replay.is_empty()
            && cfg.replay_start_fraction > 0.0
            && self.rng.uniform(0.0, 1.0) < cfg.replay_start_fraction
        {
            let k = (self.rng.uniform(0.0, replay.len() as f64) as usize).min(replay.len() - 1);
            let snap = replay[k];
            self.car.state = CarState {
                time: 0.0,
                steps: 0,
                ..snap.state
            };
            self.car.telemetry = snap.telemetry;
            self.reset_episode(shared);
            self.actuator = snap.actuator;
            self.input = snap.input;
            return;
        }
        let s = if cfg.random_start {
            assert!((0.0..=1.0).contains(&cfg.start_s_focus_fraction));
            let (start, end) = match cfg.start_s_range {
                Some(range) if self.rng.uniform(0.0, 1.0) < cfg.start_s_focus_fraction => range,
                _ => (0.0, track.length),
            };
            assert!(start >= 0.0 && start < end && end <= track.length);
            self.rng.uniform(start, end)
        } else {
            0.0
        };
        let d = self.rng.uniform(cfg.start_offset.0, cfg.start_offset.1);
        let top = if cfg.safe_start {
            cfg.start_speed.1.min(cornering_speed(track, s))
        } else {
            cfg.start_speed.1
        };
        let speed = self.rng.uniform(cfg.start_speed.0.min(top), top);
        let gear = gear_for_speed(&shared.car, speed);
        self.car.reset_in(track, &self.weather, s, d, speed, gear);
        if cfg.worn_start_fraction > 0.0 && self.rng.uniform(0.0, 1.0) < cfg.worn_start_fraction {
            self.age_tyres(cfg.worn_start_max_wear);
        }
        self.reset_episode(shared);
    }

    fn draw_weather(&mut self, cfg: &EnvConfig) -> Weather {
        let mut draw = |(lo, hi): (f64, f64)| {
            if hi > lo {
                self.rng.uniform(lo, hi)
            } else {
                lo
            }
        };
        let air = draw(cfg.air_temperature);
        let road = air + draw(cfg.road_heat);
        let wind = draw(cfg.wind_speed);
        let pressure = draw(PRESSURE_RANGE);
        let heading = draw((0.0, std::f64::consts::TAU));
        let seed = self.rng.uniform(0.0, 1.0).to_bits();
        Weather::steady(air, road, pressure, wind, heading, seed)
    }

    /// Tyres as a stint leaves them: worn alike (within ±30 %) and at a common heat,
    /// each tread zone a little off its carcass.
    fn age_tyres(&mut self, max_wear: f64) {
        let wear = self.rng.uniform(0.0, max_wear);
        let heat = self.rng.uniform(65.0, 100.0);
        for w in &mut self.car.state.wheels {
            let t = &mut w.tire;
            t.wear = (wear * self.rng.uniform(0.7, 1.3)).min(1.0);
            t.core_temperature = heat + self.rng.uniform(-5.0, 5.0);
            for zone in &mut t.tread_temperature {
                *zone = t.core_temperature + self.rng.uniform(-10.0, 15.0);
            }
        }
    }

    /// Starts timing, rewards and the track's rubber afresh for the car where it is.
    fn reset_episode(&mut self, shared: &EnvShared) {
        let cfg = &shared.config;
        let track = &*shared.track;
        if let (Some((lo, hi)), Some(map)) = (cfg.track_grip, &shared.rubber) {
            // Only draw when randomised, so fixed-grip runs keep their start sequence.
            let grip = if hi > lo {
                self.rng.uniform(lo, hi)
            } else {
                lo
            };
            if self.evolution.map().is_some() {
                self.evolution.reset(grip);
            } else {
                self.evolution = TrackEvolution::new(map.clone(), grip, cfg.grip_gain_per_lap);
            }
        }
        self.lap = LapTimer::new(track, self.car.state.position);
        self.actuator = Actuator::default();
        self.input = AppliedInput::default();
        self.info = StepInfo::default();
        self.last_episode = self.stats;
        self.stats = EpisodeStats::default();
        self.history = [None; HISTORY_LEN];
        self.next_snapshot = 0.0;
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            state: self.car.state,
            telemetry: self.car.telemetry,
            actuator: self.actuator,
            input: self.input,
        }
    }

    /// Records the state every [`SNAPSHOT_INTERVAL`] s.
    fn record(&mut self) {
        let time = self.car.state.time;
        if time >= self.next_snapshot {
            let slot = (time / SNAPSHOT_INTERVAL) as usize % HISTORY_LEN;
            self.history[slot] = Some(self.snapshot());
            self.next_snapshot = time + SNAPSHOT_INTERVAL;
        }
    }

    /// The latest recorded state at least [`REPLAY_LEAD`] s before now.
    fn state_before_crash(&self) -> Option<Snapshot> {
        let now = self.car.state.time;
        self.history
            .iter()
            .flatten()
            .filter(|h| h.state.time <= now - REPLAY_LEAD)
            .max_by(|a, b| a.state.time.total_cmp(&b.state.time))
            .copied()
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
        if action.iter().any(|a| !a.is_finite()) {
            return self.terminate_invalid(shared);
        }
        let cfg = &shared.config;
        let track = &*shared.track;
        let prev_steer = self.input.steer;
        let wear_before: f64 = self.car.state.wheels.iter().map(|w| w.tire.wear).sum();
        let substeps = cfg.substeps();
        self.actuator.decide(cfg, action);
        let mut barrier_impact: f64 = 0.0;
        for _ in 0..substeps {
            let controls = self.actuator.controls(cfg, &self.car, action);
            self.weather.step(DT);
            self.car
                .step_in(track, &mut self.evolution, &self.weather, &controls);
            let st = &self.car.state;
            if self.car.telemetry.invalid
                || !st.position.is_finite()
                || !st.velocity.is_finite()
                || !st.orientation.is_finite()
                || !st.angular_velocity.is_finite()
                || st
                    .wheels
                    .iter()
                    .any(|w| !w.travel.is_finite() || !w.spin.is_finite())
            {
                return self.terminate_invalid(shared);
            }
            barrier_impact = barrier_impact.max(self.car.telemetry.barrier_impact);
        }
        self.input = self.actuator.applied(&self.car, action);
        let wear = self
            .car
            .state
            .wheels
            .iter()
            .map(|w| w.tire.wear)
            .sum::<f64>()
            - wear_before;

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
            .sample
            .tangent
            .truncate()
            .normalize()
            .dot(forward.truncate().normalize());
        let half_width = if q.d >= 0.0 {
            q.sample.width_left
        } else {
            q.sample.width_right
        };
        let wheels_off = self
            .car
            .telemetry
            .wheels
            .iter()
            .filter(|w| w.surface.off_track())
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
            wear,
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
            invalid: !(st.position.is_finite()
                && st.velocity.is_finite()
                && speed.is_finite()
                && progress.is_finite()
                && total_progress.is_finite()
                && (q.d / half_width).is_finite()
                && grip_loss.is_finite()
                && barrier_impact.is_finite()
                && heading_cos.is_finite()
                && st.time.is_finite()),
        };
        if info.invalid {
            return self.terminate_invalid(shared);
        }
        self.info = info;

        let done = shared.termination.done(&info);
        if done == Some(Done::Terminated) {
            self.crash_snapshot = self.state_before_crash();
        } else if done.is_none() && cfg.replay_start_fraction > 0.0 {
            self.record();
        }
        let reward = shared.reward.reward(&info, done);
        self.stats.time = info.time;
        self.stats.progress = info.total_progress;
        self.stats.return_ += reward;
        (reward, done)
    }

    fn terminate_invalid(&mut self, shared: &EnvShared) -> (f64, Option<Done>) {
        self.info = StepInfo {
            invalid: true,
            time: self.stats.time,
            total_progress: self.stats.progress,
            ..StepInfo::default()
        };
        let done = Some(Done::Terminated);
        let reward = shared.reward.reward(&self.info, done);
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
    /// Share of the brake pedal the anti-lock system currently takes away.
    abs_release: f64,
    /// Gear request of the current decision, not yet sent to the gearbox.
    shift: Shift,
    /// Works the clutch pedal: the agent drives with its two feet on throttle and brake.
    clutch: ClutchAssist,
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
        let range = if cfg.speed_scaled_steering {
            steer_range(car)
        } else {
            1.0
        };
        let target = f64::from(action[0]).clamp(-1.0, 1.0) * range * lock;
        let max_delta = cfg.max_steer_rate * DT;
        self.steer += (target - self.steer).clamp(-max_delta, max_delta);
        let mut controls = Controls {
            steer_wheel_angle: self.steer,
            throttle: f64::from(action[1]).clamp(0.0, 1.0),
            brake: self.brake(cfg, car, f64::from(action[2]).clamp(0.0, 1.0)),
            clutch: 0.0,
            shift: if cfg.auto_shift {
                AutoShift.shift(car)
            } else {
                self.take_shift(car)
            },
            selector: None,
        };
        if cfg.traction_control {
            let spin = (0..4)
                .filter(|&i| car.model.corners[i].driven)
                .map(|i| car.state.wheels[i].kappa)
                .fold(0.0_f64, f64::max);
            controls.throttle = controls
                .throttle
                .min((1.0 - 8.0 * (spin - 0.1)).clamp(0.0, 1.0));
        }
        self.clutch.apply(car, &mut controls);
        BlipAssist.apply(car, &mut controls);
        controls
    }

    /// Brake pressure for `pedal`: with [`EnvConfig::abs`], released while a wheel slips
    /// past [`ABS_SLIP`] and reapplied once it grips again.
    fn brake(&mut self, cfg: &EnvConfig, car: &Car, pedal: f64) -> f64 {
        if !cfg.abs || pedal == 0.0 || car.speed() < ABS_MIN_SPEED {
            self.abs_release = 0.0;
            return pedal;
        }
        let locking = car
            .telemetry
            .wheels
            .iter()
            .any(|w| w.slip_ratio < -ABS_SLIP);
        self.abs_release = if locking {
            (self.abs_release + ABS_RELEASE_RATE * DT).min(1.0)
        } else {
            (self.abs_release - ABS_APPLY_RATE * DT).max(0.0)
        };
        pedal * (1.0 - self.abs_release)
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

/// Share of the steering lock the steering action spans with
/// [`EnvConfig::speed_scaled_steering`]: the road wheel angle that turns the car at
/// [`STEER_RANGE_ACCEL`] (kinematically, wheelbase × acceleration / speed²) plus
/// [`STEER_RANGE_SLIP`] for the tyres' slip angles and for catching slides.
pub fn steer_range(car: &Car) -> f64 {
    let p = &car.model.params;
    let v = car.speed().max(1.0);
    let road = p.wheelbase * STEER_RANGE_ACCEL / (v * v) + STEER_RANGE_SLIP;
    (road * p.steering.ratio / p.steering.lock).clamp(STEER_RANGE_MIN, 1.0)
}
/// Lateral acceleration, m/s², and road wheel angle, rad, of [`steer_range`], and the
/// least share of the lock it spans.
pub const STEER_RANGE_ACCEL: f64 = 25.0;
pub const STEER_RANGE_SLIP: f64 = 0.07;
const STEER_RANGE_MIN: f64 = 0.2;

/// Braking slip ratio beyond which the anti-lock system releases the brakes; the tyres
/// peak around 0.1.
pub const ABS_SLIP: f64 = 0.12;
/// Rates at which the anti-lock system releases and reapplies the brakes, share of the
/// pedal per second, and the speed below which it stays out of the way, m/s.
const ABS_RELEASE_RATE: f64 = 20.0;
const ABS_APPLY_RATE: f64 = 10.0;
const ABS_MIN_SPEED: f64 = 3.0;

/// Look-ahead distance and lateral acceleration of [`EnvConfig::safe_start`].
pub const SAFE_START_DISTANCE: f64 = 60.0;
pub const SAFE_START_LATERAL_G: f64 = 1.3;

/// Speed at which the corners within [`SAFE_START_DISTANCE`] after `s` can be taken at
/// [`SAFE_START_LATERAL_G`], m/s.
fn cornering_speed(track: &Track, s: f64) -> f64 {
    (0..=(SAFE_START_DISTANCE / 5.0) as usize)
        .map(|k| track.sample_at(s + 5.0 * k as f64).curvature.abs())
        .fold(f64::INFINITY, |v, c| {
            v.min((SAFE_START_LATERAL_G * GRAVITY / c.max(1e-6)).sqrt())
        })
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
    /// States shortly before crashes, to restart from; the oldest are overwritten.
    replay: Vec<Snapshot>,
    replay_next: usize,
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
            replay: Vec::new(),
            replay_next: 0,
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
            .par_iter_mut()
            .zip(self.obs.par_chunks_mut(self.obs_dim))
            .for_each(|(env, out)| observe_finite(env, shared, out));
    }

    /// Steps every env with its row of `actions` (`num_envs × config.action_dim()`).
    /// Finished episodes are reset automatically.
    pub fn step(&mut self, actions: &[f32]) -> BatchStep<'_> {
        let shared = &self.shared;
        let replay = &self.replay[..];
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
                let stats_before = env.stats;
                // A rare non-finite tyre force can still reach an internal float clamp.
                // Isolate that numerical failure to one car; unrelated panics propagate.
                let (mut r, mut done) = match catch_unwind(AssertUnwindSafe(|| {
                    env.step(shared, action)
                })) {
                    Ok(result) => result,
                    Err(payload) => {
                        let message = payload
                            .downcast_ref::<String>()
                            .map(String::as_str)
                            .or_else(|| payload.downcast_ref::<&str>().copied());
                        if message.is_some_and(|s| s.starts_with("min > max, or either was NaN")) {
                            env.terminate_invalid(shared)
                        } else {
                            resume_unwind(payload);
                        }
                    }
                };
                if !r.is_finite() {
                    env.stats = stats_before;
                    (r, done) = env.terminate_invalid(shared);
                }
                if done.is_some() {
                    if env.info.invalid {
                        final_obs.fill(0.0);
                    } else {
                        env.observe(shared, final_obs);
                        if final_obs.iter().any(|x| !x.is_finite()) {
                            env.stats = stats_before;
                            (r, done) = env.terminate_invalid(shared);
                            final_obs.fill(0.0);
                        }
                    }
                    env.reset_from(shared, replay);
                    observe_finite(env, shared, obs);
                } else {
                    env.observe(shared, obs);
                    if obs.iter().any(|x| !x.is_finite()) {
                        env.stats = stats_before;
                        (r, done) = env.terminate_invalid(shared);
                        final_obs.fill(0.0);
                        env.reset(shared);
                        observe_finite(env, shared, obs);
                    }
                }
                *reward = r as f32;
                *term = (done == Some(Done::Terminated)) as u8;
                *trunc = (done == Some(Done::Truncated)) as u8;
            });

        self.finished.clear();
        for (i, env) in self.envs.iter_mut().enumerate() {
            if self.terminated[i] != 0 || self.truncated[i] != 0 {
                self.finished.push((i, env.last_episode));
            }
            if let Some(snap) = env.crash_snapshot.take() {
                if self.replay.len() < REPLAY_CAPACITY {
                    self.replay.push(snap);
                } else {
                    self.replay[self.replay_next] = snap;
                }
                self.replay_next = (self.replay_next + 1) % REPLAY_CAPACITY;
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

fn observe_finite(env: &mut Env, shared: &EnvShared, out: &mut [f32]) {
    let last_episode = env.last_episode;
    env.observe(shared, out);
    for _ in 0..8 {
        if out.iter().all(|x| x.is_finite()) {
            env.last_episode = last_episode;
            return;
        }
        env.reset(shared);
        env.observe(shared, out);
    }
    env.last_episode = last_episode;
    out.fill(0.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_starts_stay_within_requested_track_interval() {
        let track = Arc::new(Track::default_circuit());
        let shared = EnvShared::new(
            EnvConfig {
                start_s_range: Some((100.0, 200.0)),
                start_speed: (0.0, 0.0),
                start_offset: (0.0, 0.0),
                ..EnvConfig::default()
            },
            track.clone(),
            Arc::new(CarModel::gt3()),
        );
        let mut env = Env::new(&shared, 42);
        for _ in 0..100 {
            env.reset(&shared);
            let q = track.locate(
                env.car.state.position,
                track.nearest_index(env.car.state.position),
            );
            assert!((99.0..=201.0).contains(&q.s), "spawn s={}", q.s);
        }
    }

    #[test]
    fn steering_range_narrows_with_speed() {
        let track = Track::default_circuit();
        let model = Arc::new(CarModel::gt3());
        let range = |speed| steer_range(&Car::new(model.clone(), &track, 0.0, 0.0, speed, 1));
        assert_eq!(range(3.0), 1.0);
        assert!(range(20.0) > range(40.0) && range(40.0) > range(70.0));
        // A few degrees at the road wheels at top speed.
        let p = &model.params;
        let road = range(75.0) * p.steering.lock / p.steering.ratio;
        assert!((0.05..0.12).contains(&road), "{road}");
    }

    #[test]
    fn worn_starts_age_the_tyres() {
        let shared = EnvShared::new(
            EnvConfig {
                worn_start_fraction: 1.0,
                worn_start_max_wear: 0.4,
                ..EnvConfig::default()
            },
            Arc::new(Track::default_circuit()),
            Arc::new(CarModel::gt3()),
        );
        let mut env = Env::new(&shared, 3);
        let mut worn = 0;
        for _ in 0..20 {
            env.reset(&shared);
            for w in &env.car.state.wheels {
                assert!(w.tire.wear <= 0.4 * 1.3 + 1e-9);
                assert!((60.0..=120.0).contains(&w.tire.core_temperature));
            }
            worn += env.car.state.wheels.iter().any(|w| w.tire.wear > 0.0) as usize;
        }
        assert!(worn > 15);
    }

    #[test]
    fn crashes_are_replayed_from_seconds_before() {
        let shared = EnvShared::new(
            EnvConfig {
                random_start: false,
                start_speed: (10.0, 10.0),
                start_offset: (0.0, 0.0),
                replay_start_fraction: 1.0,
                ..EnvConfig::default()
            },
            Arc::new(Track::default_circuit()),
            Arc::new(CarModel::gt3()),
        );
        let mut batch = BatchEnv::new(shared, 1);
        // Steering slightly left leaves the road within seconds.
        let mut crash_time = None;
        for _ in 0..2000 {
            let r = batch.step(&[0.1, 0.5, 0.0]);
            if r.terminated[0] != 0 {
                crash_time = Some(batch.finished()[0].1.time);
                break;
            }
        }
        let crash_time = crash_time.expect("the car crashes");
        assert!(crash_time > REPLAY_LEAD, "crashed after {crash_time} s");
        assert_eq!(batch.replay.len(), 1);
        // The next reset restarts from the recorded state.
        let (shared, replay) = (&batch.shared, &batch.replay[..]);
        batch.envs[0].reset_from(shared, replay);
        let restarted = batch.envs[0].car.state;
        assert_eq!(restarted.time, 0.0);
        assert_eq!(restarted.position, batch.replay[0].state.position);
        assert!(batch.replay[0].state.time <= crash_time - REPLAY_LEAD);
        assert!(batch.replay[0].state.time > crash_time - REPLAY_LEAD - 2.0 * SNAPSHOT_INTERVAL);
    }

    #[test]
    fn episodes_draw_their_weather_from_the_ranges() {
        let shared = EnvShared::new(
            EnvConfig {
                air_temperature: (10.0, 35.0),
                road_heat: (0.0, 20.0),
                wind_speed: (0.0, 8.0),
                ..EnvConfig::default()
            },
            Arc::new(Track::default_circuit()),
            Arc::new(CarModel::gt3()),
        );
        let mut env = Env::new(&shared, 5);
        let mut airs = Vec::new();
        for _ in 0..20 {
            env.reset(&shared);
            let p = env.car.state.position;
            let air = env.weather.air_at(p).temperature;
            let road = env.weather.road_temperature(0.0, 0.0);
            assert!((10.0..=35.0).contains(&air) && (air..=air + 20.0).contains(&road));
            assert!(env.weather.wind().0 <= 8.0);
            // The tyres are inflated in the air they start in.
            assert_eq!(env.car.state.wheels[0].tire.inflated_at, air);
            airs.push(air);
        }
        assert!(airs.iter().any(|&a| a < 20.0) && airs.iter().any(|&a| a > 25.0));
        assert!(EnvConfig::default().standard_weather());
    }

    #[test]
    fn growing_lookahead_reaches_further() {
        let layout = ObsLayout {
            lookahead_growth: 1.1,
            lookahead_spacing: 6.0,
            lookahead_points: 24,
            ..EnvConfig::default().obs_layout()
        };
        assert!((layout.lookahead_distance(1) - 6.0).abs() < 1e-9);
        assert!((layout.lookahead_distance(2) - 12.6).abs() < 1e-9);
        assert!(layout.lookahead_distance(24) > 500.0);
        assert_eq!(
            EnvConfig::default().obs_layout().lookahead_distance(3),
            30.0
        );
    }

    #[test]
    fn zero_focus_fraction_keeps_whole_track_starts() {
        let track = Arc::new(Track::default_circuit());
        let shared = EnvShared::new(
            EnvConfig {
                start_s_range: Some((100.0, 200.0)),
                start_s_focus_fraction: 0.0,
                start_speed: (0.0, 0.0),
                start_offset: (0.0, 0.0),
                ..EnvConfig::default()
            },
            track.clone(),
            Arc::new(CarModel::gt3()),
        );
        let mut env = Env::new(&shared, 42);
        let mut outside = false;
        for _ in 0..100 {
            env.reset(&shared);
            let q = track.locate(
                env.car.state.position,
                track.nearest_index(env.car.state.position),
            );
            outside |= !(99.0..=201.0).contains(&q.s);
        }
        assert!(outside);
    }
}
