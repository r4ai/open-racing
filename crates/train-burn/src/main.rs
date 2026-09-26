use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Parser, Subcommand};
use open_racing_api::{
    DefaultReward, DefaultTermination, EnvConfig, EnvSpec, Policy, RacingVecEnv, VecEnv,
    parse_grip_range,
};
use open_racing_train_burn::imitation::{self, ImitationConfig};
use open_racing_train_burn::ppo::{self, PpoConfig, TrainContext};
use open_racing_train_burn::reference::ReferenceConfig;
use open_racing_train_burn::{BurnPolicy, Normalizer, PolicyMeta, TrainBackend};

mod longrun;
mod suite;

#[derive(Parser)]
#[command(about = "Train and evaluate racing agents with PPO on Burn")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
// Clap keeps the train options in one variant for a flat command-line interface.
#[allow(clippy::large_enum_variant)]
enum Command {
    /// Bootstrap a Watkins Glen Formula policy from the tested reference controller.
    Imitate {
        #[arg(long, default_value_t = 256)]
        envs: usize,
        #[arg(long, default_value_t = 524288)]
        samples: usize,
        #[arg(long, default_value_t = 10)]
        epochs: usize,
        #[arg(long, default_value_t = 8192)]
        minibatch: usize,
        #[arg(long, default_value_t = 3e-4)]
        lr: f64,
        #[arg(long, default_value_t = 101)]
        seed: u64,
        /// Bootstrap from an earlier imitation checkpoint.
        #[arg(long)]
        init: Option<PathBuf>,
        /// Share of rollout cars driven by the student while the reference labels states.
        #[arg(long, default_value_t = 0.0)]
        student_fraction: f64,
        /// Loss weight for throttle and brake relative to the baseline of 1.
        #[arg(long, default_value_t = 1.0)]
        pedal_weight: f32,
        /// First metre of a focused random-start interval.
        #[arg(long, requires = "start_s_max")]
        start_s_min: Option<f64>,
        /// Last metre of a focused random-start interval.
        #[arg(long, requires = "start_s_min")]
        start_s_max: Option<f64>,
        /// Share of random starts in the focused interval; the rest span the whole track.
        #[arg(long, default_value_t = 1.0)]
        start_s_focus_fraction: f64,
        #[arg(long, default_value = "runs/watkins-formula-imitation")]
        out: PathBuf,
    },
    /// Train a new policy.
    Train {
        #[arg(long, default_value = "lakeside")]
        track: String,
        #[arg(long, default_value = "gt3")]
        car: String,
        #[arg(long, default_value_t = 256)]
        envs: usize,
        #[arg(long, default_value_t = 500)]
        iterations: usize,
        /// Wall-clock training limit; finishes the current iteration and saves a checkpoint.
        #[arg(long)]
        duration_seconds: Option<f64>,
        #[arg(long, default_value_t = 64)]
        rollout: usize,
        /// Samples per gradient step; larger batches keep a GPU busier.
        #[arg(long, default_value_t = PpoConfig::default().minibatch)]
        minibatch: usize,
        /// Passes over each rollout.
        #[arg(long, default_value_t = PpoConfig::default().epochs)]
        epochs: usize,
        #[arg(long, default_value_t = 3e-4)]
        lr: f64,
        /// Discount per agent step; the planning horizon is about 1 / (1 − gamma) steps.
        #[arg(long, default_value_t = 0.99)]
        gamma: f32,
        /// Weight of the entropy bonus that keeps the policy exploring.
        #[arg(long, default_value_t = 0.0)]
        entropy: f32,
        /// Exploration noise (standard deviation, in normalised action units) that the cap
        /// falls to by the last iteration; the cap starts at the initial noise.
        #[arg(long)]
        final_std: Option<f32>,
        /// Exploration noise at the start: the initial noise of a new policy and the cap
        /// the schedule starts from (set it to where a warm-started policy left off).
        #[arg(long, default_value_t = PpoConfig::default().init_log_std.exp())]
        init_std: f32,
        /// Reward lost when an episode ends in a crash (100 m of progress earns 10).
        #[arg(long, default_value_t = DefaultReward::default().termination_penalty)]
        crash_penalty: f64,
        /// Reward lost per step per unit of tyre grip lost to heat, pressure and wear
        /// (summed over the four tyres).
        #[arg(long, default_value_t = DefaultReward::default().grip_loss_weight)]
        grip_loss_penalty: f64,
        /// Reward lost per unit of change in the normalised steering input (−1..1) per step.
        #[arg(long, default_value_t = DefaultReward::default().steer_change_weight)]
        steer_change_penalty: f64,
        /// Reward lost per step for every wheel off the track.
        #[arg(long, default_value_t = DefaultReward::default().off_track_weight)]
        off_track_penalty: f64,
        /// Dense penalty for driving close to either track edge.
        #[arg(long, default_value_t = DefaultReward::default().edge_weight)]
        edge_penalty: f64,
        #[arg(long, default_value_t = 0)]
        seed: u64,
        /// Include ground-truth tyre state in the observation.
        #[arg(long)]
        privileged: bool,
        /// Include tread temperatures and pressures in the observation.
        #[arg(long)]
        tyre_obs: bool,
        /// Include the track widths at the lookahead points in the observation.
        #[arg(long)]
        edge_obs: bool,
        /// Include tyre wear, carcass and brake temperatures and damage in the observation.
        #[arg(long)]
        stint_obs: bool,
        /// Include the position around the lap in the observation.
        #[arg(long)]
        lap_position_obs: bool,
        /// Scale the steering action to the steering usable at the car's speed.
        #[arg(long)]
        speed_scaled_steering: bool,
        /// Number of lookahead points along the track.
        #[arg(long, default_value_t = EnvConfig::default().lookahead_points)]
        lookahead_points: usize,
        /// Distance to the first lookahead point, m.
        #[arg(long, default_value_t = EnvConfig::default().lookahead_spacing)]
        lookahead_spacing: f64,
        /// Each gap between lookahead points is this many times the one before.
        #[arg(long, default_value_t = EnvConfig::default().lookahead_growth)]
        lookahead_growth: f64,
        /// Share of random starts on tyres as a stint leaves them (worn and hot).
        #[arg(long, default_value_t = 0.0)]
        worn_start_fraction: f64,
        /// Most tread worn at those starts (1 = worn out).
        #[arg(long, default_value_t = 0.3)]
        worn_start_max_wear: f64,
        /// Share of resets from states shortly before earlier crashes.
        #[arg(long, default_value_t = 0.0)]
        replay_start_fraction: f64,
        /// Air temperature of each episode, °C: a value or a range such as 10..38.
        #[arg(long, value_parser = parse_range, default_value = "25")]
        air_temperature: (f64, f64),
        /// How much warmer the road is than the air, K: a value or a range.
        #[arg(long, value_parser = parse_range, default_value = "0")]
        road_heat: (f64, f64),
        /// Wind speed at 10 m, m/s, from any direction: a value or a range.
        #[arg(long, value_parser = parse_range, default_value = "0")]
        wind: (f64, f64),
        /// Share of random starts in a spin (turned up to half a turn, yawing).
        #[arg(long, default_value_t = 0.0)]
        spin_start_fraction: f64,
        /// Let the recovery aid get the car going again after a spin.
        #[arg(long)]
        recovery_assist: bool,
        /// Longest time with all four wheels off the track before an episode ends, s.
        #[arg(long, default_value_t = DefaultTermination::default().max_off_track_time)]
        max_off_track_seconds: f64,
        /// Longest time facing the wrong way while moving before an episode ends, s.
        #[arg(long, default_value_t = DefaultTermination::default().max_wrong_way_time)]
        max_wrong_way_seconds: f64,
        /// Longest time standing still before an episode ends, s.
        #[arg(long, default_value_t = DefaultTermination::default().max_stuck_time)]
        max_stuck_seconds: f64,
        /// Also keep the policy every this many minutes in `<out>/checkpoints/`, for
        /// `select` to choose from.
        #[arg(long)]
        checkpoint_minutes: Option<f64>,
        /// Reward lost per unit of tread worn (summed over the four tyres).
        #[arg(long, default_value_t = DefaultReward::default().wear_weight)]
        wear_penalty: f64,
        /// Start episodes no faster than the corners just ahead allow.
        #[arg(long)]
        safe_start: bool,
        /// Highest random starting speed in m/s.
        #[arg(long, default_value_t = EnvConfig::default().start_speed.1)]
        start_speed_max: f64,
        /// Absolute limit of the random lateral starting offset in metres.
        #[arg(long, default_value_t = EnvConfig::default().start_offset.1)]
        start_offset_meters: f64,
        /// Episode time limit in simulated seconds.
        #[arg(long, default_value_t = DefaultTermination::default().max_time)]
        max_episode_seconds: f64,
        /// Agent decisions per second.
        #[arg(long, default_value_t = EnvConfig::default().control_hz)]
        control_hz: f64,
        /// Fastest the steering wheel turns, rad/s.
        #[arg(long, default_value_t = EnvConfig::default().max_steer_rate)]
        max_steer_rate: f64,
        /// Hidden layer sizes of the actor and critic, e.g. 256,256.
        #[arg(long, value_delimiter = ',', default_values_t = PpoConfig::default().hidden)]
        hidden: Vec<usize>,
        /// Anti-lock brakes.
        #[arg(long)]
        abs: bool,
        /// Reduce throttle when driven tyres spin excessively.
        #[arg(long)]
        traction_control: bool,
        /// Let the policy shift gears itself instead of the automatic gear selector.
        #[arg(long)]
        manual_shift: bool,
        /// Track evolution: grip on the racing line at the start of each episode, as a
        /// level (dusty, green, fast, optimum, or e.g. 0.97) or a range to randomise over
        /// (e.g. green..optimum). Rubber lies on the racing line, dust off it. Without it
        /// the whole asphalt has the tyres' nominal grip.
        #[arg(long, value_parser = parse_grip_range)]
        track_grip: Option<(f64, f64)>,
        /// Grip the racing line gains per lap the car drives, with --track-grip.
        #[arg(long, default_value_t = EnvConfig::default().grip_gain_per_lap)]
        grip_gain: f64,
        #[arg(long, default_value = "runs/ppo")]
        out: PathBuf,
        /// Continue from a trained policy (weights and observation normaliser).
        #[arg(long)]
        init: Option<PathBuf>,
    },
    /// Drive a stint from a standing start: several cars, the first as the policy
    /// drives, the others with small perturbations; lap times, tyres and crashes.
    Longrun {
        #[arg(long, default_value = "runs/ppo")]
        model: PathBuf,
        #[arg(long, default_value_t = 10)]
        laps: u32,
        #[arg(long, default_value_t = 16)]
        cars: usize,
        /// Standard deviation of the noise added to every car's actions but the first.
        #[arg(long, default_value_t = 0.02)]
        noise: f32,
        /// Air temperature of each episode, °C: a value or a range such as 10..38.
        #[arg(long, value_parser = parse_range, default_value = "25")]
        air_temperature: (f64, f64),
        /// How much warmer the road is than the air, K: a value or a range.
        #[arg(long, value_parser = parse_range, default_value = "0")]
        road_heat: (f64, f64),
        /// Wind speed at 10 m, m/s, from any direction: a value or a range.
        #[arg(long, value_parser = parse_range, default_value = "0")]
        wind: (f64, f64),
        /// Racing-line grip (see `train --track-grip`): rubber on the racing line, dust
        /// off it, and dirt dragged onto the road. Defaults to what the policy trained on.
        #[arg(long, value_parser = parse_grip_range)]
        track_grip: Option<(f64, f64)>,
        /// Grip the racing line gains per lap, with --track-grip.
        #[arg(long, default_value_t = open_racing_api::TrackEvolution::DEFAULT_GAIN_PER_LAP)]
        grip_gain: f64,
        /// Write one car's stint step by step to this CSV file.
        #[arg(long)]
        trace: Option<PathBuf>,
        /// The car whose stint `--trace` writes (0 drives without noise).
        #[arg(long, default_value_t = 0)]
        trace_car: usize,
        /// Drive the evaluation suite (see `select`) instead of one stint.
        #[arg(long)]
        suite: bool,
        /// Start every car in a spin somewhere on the track (as the suite's recovery).
        #[arg(long)]
        spin: bool,
        /// Switch the recovery aid on, whatever the policy trained with.
        #[arg(long)]
        recovery_assist: bool,
    },
    /// Drive every checkpoint of a run in the evaluation suite and copy the best to
    /// `<run>/best`. The suite: 10-lap stints from a standing start in the training
    /// conditions and on drawn tracks in drawn weather; 3 laps with half the cars in the
    /// app's weather model from the grid (a hot afternoon, a cold morning on a dusty
    /// track, an overcast day on a green one); then a lap after a spin.
    Select {
        /// A training run's output directory.
        #[arg(long)]
        run: PathBuf,
        #[arg(long, default_value_t = 10)]
        laps: u32,
        #[arg(long, default_value_t = 16)]
        cars: usize,
    },
    /// Run a trained policy and report lap times.
    Eval {
        #[arg(long, default_value = "runs/ppo")]
        model: PathBuf,
        #[arg(long, default_value_t = 180.0)]
        seconds: f64,
        /// Cars started at random points for the robustness run (0 skips it).
        #[arg(long, default_value_t = 64)]
        envs: usize,
        /// Write the start-line run step by step to this CSV file.
        #[arg(long)]
        trace: Option<PathBuf>,
        /// Start the robustness cars no faster than the corners just ahead allow.
        #[arg(long)]
        safe_start: bool,
        /// Episode time limit in simulated seconds.
        #[arg(long, default_value_t = DefaultTermination::default().max_time)]
        max_episode_seconds: f64,
        /// Racing-line grip level or range to evaluate on (see `train --track-grip`);
        /// defaults to what the policy was trained with.
        #[arg(long, value_parser = parse_grip_range)]
        track_grip: Option<(f64, f64)>,
        /// Use a tested trajectory and tyre-slip controller as a safety supervisor.
        #[arg(long)]
        reference_assist: bool,
        /// Steering share from the model when it agrees closely with the reference.
        #[arg(long, default_value_t = 0.0)]
        reference_blend: f32,
        /// Limit learned throttle and brake actions around Watkins Glen's rough section.
        /// The model still controls steering everywhere.
        #[arg(long)]
        hazard_speed_governor: bool,
        /// Use reference steering inside the rough section and learned
        /// steering everywhere else.
        #[arg(long, requires = "hazard_speed_governor")]
        hazard_steer_fallback: bool,
    },
}

#[derive(Clone, Copy)]
struct EvalSupervision {
    reference_assist: bool,
    reference_blend: f32,
    hazard_speed_governor: bool,
    hazard_steer_fallback: bool,
}

fn main() {
    match Cli::parse().command {
        Command::Imitate {
            envs,
            samples,
            epochs,
            minibatch,
            lr,
            seed,
            init,
            student_fraction,
            pedal_weight,
            start_s_min,
            start_s_max,
            start_s_focus_fraction,
            out,
        } => {
            let start_s_range = start_s_min.zip(start_s_max);
            let config = EnvConfig {
                control_hz: 25.0,
                max_steer_rate: 4.0,
                safe_start: true,
                start_speed: (0.0, 12.0),
                start_offset: (-0.5, 0.5),
                start_s_range,
                start_s_focus_fraction,
                traction_control: true,
                privileged_obs: true,
                tyre_obs: true,
                edge_obs: true,
                track_grip: Some((0.94, 1.0)),
                grip_gain_per_lap: 0.01,
                seed,
                ..EnvConfig::default()
            };
            let spec = EnvSpec::from_names("watkins_glen", "formula", config.clone())
                .unwrap_or_else(|e| panic!("{e}"));
            let mut env = spec.make_vec_env(envs);
            let space = env.action_space().clone();
            let meta = PolicyMeta {
                obs_names: env.observation_space().names.clone(),
                act_low: space.low,
                act_high: space.high,
                hidden: vec![],
                normalizer: Normalizer::new(env.observation_space().dim()),
                control_hz: config.control_hz,
                lookahead_points: config.lookahead_points,
                lookahead_spacing: config.lookahead_spacing,
                lookahead_growth: config.lookahead_growth,
                privileged_obs: config.privileged_obs,
                tyre_obs: config.tyre_obs,
                edge_obs: config.edge_obs,
                stint_obs: config.stint_obs,
                lap_position_obs: config.lap_position_obs,
                speed_scaled_steering: config.speed_scaled_steering,
                recovery_assist: config.recovery_assist,
                abs: config.abs,
                traction_control: config.traction_control,
                max_steer_rate: config.max_steer_rate,
                auto_shift: config.auto_shift,
                track_grip: config.track_grip,
                grip_gain_per_lap: Some(config.grip_gain_per_lap),
                track: "watkins_glen".into(),
                car: "formula".into(),
            };
            imitation::train::<TrainBackend>(
                &mut env,
                &spec.track,
                &ImitationConfig {
                    samples,
                    epochs,
                    minibatch,
                    learning_rate: lr,
                    hidden: vec![512, 512],
                    seed,
                    out_dir: out,
                    reference: ReferenceConfig::default(),
                    init,
                    student_fraction,
                    pedal_weight,
                },
                meta,
                &Default::default(),
            );
        }
        Command::Train {
            track,
            car,
            envs,
            iterations,
            duration_seconds,
            rollout,
            minibatch,
            epochs,
            lr,
            gamma,
            entropy,
            final_std,
            init_std,
            crash_penalty,
            grip_loss_penalty,
            steer_change_penalty,
            off_track_penalty,
            edge_penalty,
            seed,
            privileged,
            tyre_obs,
            edge_obs,
            stint_obs,
            lap_position_obs,
            speed_scaled_steering,
            lookahead_points,
            lookahead_spacing,
            lookahead_growth,
            worn_start_fraction,
            worn_start_max_wear,
            replay_start_fraction,
            wear_penalty,
            air_temperature,
            road_heat,
            wind,
            spin_start_fraction,
            recovery_assist,
            max_off_track_seconds,
            max_wrong_way_seconds,
            max_stuck_seconds,
            checkpoint_minutes,
            safe_start,
            start_speed_max,
            start_offset_meters,
            max_episode_seconds,
            abs,
            traction_control,
            control_hz,
            max_steer_rate,
            hidden,
            manual_shift,
            track_grip,
            grip_gain,
            out,
            init,
        } => {
            assert!(duration_seconds.is_none_or(|seconds| seconds.is_finite() && seconds > 0.0));
            assert!(max_episode_seconds.is_finite() && max_episode_seconds > 0.0);
            assert!(start_speed_max.is_finite() && start_speed_max > 0.0);
            assert!(start_offset_meters.is_finite() && start_offset_meters >= 0.0);
            let config = EnvConfig {
                privileged_obs: privileged,
                tyre_obs,
                edge_obs,
                stint_obs,
                lap_position_obs,
                speed_scaled_steering,
                lookahead_points,
                lookahead_spacing,
                lookahead_growth,
                worn_start_fraction,
                worn_start_max_wear,
                replay_start_fraction,
                air_temperature,
                road_heat,
                wind_speed: wind,
                spin_start_fraction,
                recovery_assist,
                safe_start,
                start_speed: (0.0, start_speed_max),
                start_offset: (-start_offset_meters, start_offset_meters),
                abs,
                traction_control,
                control_hz,
                max_steer_rate,
                auto_shift: !manual_shift,
                track_grip,
                grip_gain_per_lap: grip_gain,
                seed,
                ..EnvConfig::default()
            };
            let mut spec =
                EnvSpec::from_names(&track, &car, config.clone()).unwrap_or_else(|e| panic!("{e}"));
            spec.reward = Arc::new(DefaultReward {
                termination_penalty: crash_penalty,
                grip_loss_weight: grip_loss_penalty,
                wear_weight: wear_penalty,
                steer_change_weight: steer_change_penalty,
                off_track_weight: off_track_penalty,
                edge_weight: edge_penalty,
                ..DefaultReward::default()
            });
            spec.termination = Arc::new(DefaultTermination {
                max_time: max_episode_seconds,
                max_off_track_time: max_off_track_seconds,
                max_wrong_way_time: max_wrong_way_seconds,
                max_stuck_time: max_stuck_seconds,
                ..DefaultTermination::default()
            });
            let mut env = spec.make_vec_env(envs);
            let space = env.action_space().clone();
            let meta = PolicyMeta {
                obs_names: env.observation_space().names.clone(),
                act_low: space.low,
                act_high: space.high,
                hidden: vec![],
                normalizer: Normalizer::new(0),
                control_hz: config.control_hz,
                lookahead_points: config.lookahead_points,
                lookahead_spacing: config.lookahead_spacing,
                lookahead_growth: config.lookahead_growth,
                privileged_obs: config.privileged_obs,
                tyre_obs: config.tyre_obs,
                edge_obs: config.edge_obs,
                stint_obs: config.stint_obs,
                lap_position_obs: config.lap_position_obs,
                speed_scaled_steering: config.speed_scaled_steering,
                recovery_assist: config.recovery_assist,
                abs: config.abs,
                traction_control: config.traction_control,
                max_steer_rate: config.max_steer_rate,
                auto_shift: config.auto_shift,
                track_grip: config.track_grip,
                grip_gain_per_lap: Some(config.grip_gain_per_lap),
                track,
                car,
            };
            let cfg = PpoConfig {
                iterations,
                max_duration_seconds: duration_seconds,
                rollout_len: rollout,
                minibatch,
                epochs,
                learning_rate: lr,
                gamma,
                entropy_coef: entropy,
                final_log_std: final_std.map(f32::ln),
                init_log_std: init_std.ln(),
                hidden,
                seed,
                out_dir: out,
                init,
                checkpoint_every_seconds: checkpoint_minutes.map(|m| m * 60.0),
                ..Default::default()
            };
            println!(
                "training on {} envs, obs dim {}, {} physics steps per action",
                envs,
                env.observation_space().dim(),
                config.substeps()
            );
            ppo::train::<TrainBackend>(&mut env, &cfg, TrainContext { meta }, &Default::default());
        }
        Command::Longrun {
            model,
            laps,
            cars,
            noise,
            air_temperature,
            road_heat,
            wind,
            track_grip,
            grip_gain,
            trace,
            trace_car,
            suite,
            spin,
            recovery_assist,
        } => {
            let mut policy = BurnPolicy::load(&model)
                .unwrap_or_else(|e| panic!("loading {}: {e}", model.display()));
            policy.meta.recovery_assist |= recovery_assist;
            if suite {
                println!("per condition: finished/cars, steps per lap with a wheel off, mean time");
                suite::print_header();
                let results = suite::evaluate(&mut policy, laps, cars);
                suite::print_row(&model.display().to_string(), &results);
                return;
            }
            let conditions = open_racing_api::EnvConfig {
                air_temperature,
                road_heat,
                wind_speed: wind,
                track_grip: track_grip.or(policy.meta.track_grip),
                grip_gain_per_lap: grip_gain,
                ..Default::default()
            };
            let setup = longrun::Setup {
                noise,
                trace: trace.map(|path| (path, trace_car)),
                verbose: true,
                ..longrun::Setup::standing_start(&policy, laps, cars, &conditions)
            };
            let setup = if spin {
                suite::after_spin(setup)
            } else {
                setup
            };
            longrun::run(&mut policy, &setup);
        }
        Command::Select { run, laps, cars } => suite::select(&run, laps, cars),
        Command::Eval {
            model,
            seconds,
            envs,
            trace,
            safe_start,
            max_episode_seconds,
            track_grip,
            reference_assist,
            reference_blend,
            hazard_speed_governor,
            hazard_steer_fallback,
        } => {
            assert!((0.0..=1.0).contains(&reference_blend));
            let supervision = EvalSupervision {
                reference_assist,
                reference_blend,
                hazard_speed_governor,
                hazard_steer_fallback,
            };
            let mut policy = BurnPolicy::load(&model)
                .unwrap_or_else(|e| panic!("loading {}: {e}", model.display()));
            if track_grip.is_some() {
                policy.meta.track_grip = track_grip;
            }
            flying_lap(
                &mut policy,
                seconds,
                max_episode_seconds,
                trace.as_deref(),
                supervision,
            );
            if envs > 0 {
                robustness(
                    &mut policy,
                    seconds,
                    envs,
                    safe_start,
                    max_episode_seconds,
                    supervision,
                );
            }
        }
    }
}

/// A value, or a range written `low..high`.
fn parse_range(s: &str) -> Result<(f64, f64), String> {
    let number = |x: &str| x.trim().parse::<f64>().map_err(|e| format!("{x}: {e}"));
    let (lo, hi) = match s.split_once("..") {
        Some((lo, hi)) => (number(lo)?, number(hi)?),
        None => (number(s)?, number(s)?),
    };
    if lo <= hi {
        Ok((lo, hi))
    } else {
        Err(format!("{s}: the low end is above the high end"))
    }
}

fn eval_env(
    policy: &BurnPolicy,
    config: EnvConfig,
    envs: usize,
    max_episode_seconds: f64,
) -> (EnvSpec, RacingVecEnv) {
    let mut spec = EnvSpec::from_names(&policy.meta.track, &policy.meta.car, config)
        .unwrap_or_else(|e| panic!("{e}"));
    spec.termination = Arc::new(DefaultTermination {
        max_time: max_episode_seconds,
        ..DefaultTermination::default()
    });
    let env = spec.make_vec_env(envs);
    policy
        .check_compatible(env.observation_space())
        .unwrap_or_else(|e| panic!("{e}"));
    (spec, env)
}

/// One car from a standing start on the start line, as in a race.
fn flying_lap(
    policy: &mut BurnPolicy,
    seconds: f64,
    max_episode_seconds: f64,
    trace: Option<&Path>,
    supervision: EvalSupervision,
) {
    let config = EnvConfig {
        random_start: false,
        start_speed: (0.0, 0.0),
        start_offset: (0.0, 0.0),
        ..policy.meta.env_config()
    };
    let (spec, mut env) = eval_env(policy, config.clone(), 1, max_episode_seconds);
    let mut trace = trace.map(|path| {
        let mut file = std::io::BufWriter::new(
            std::fs::File::create(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
        );
        writeln!(
            file,
            "time,s,offset,speed,steer,throttle,brake,gear,curvature,sideslip_deg,tread_fl,tread_fr,tread_rl,tread_rr,grip_fl,grip_fr,grip_rl,grip_rr,wheels_off,width_left,width_right"
        )
        .unwrap();
        file
    });
    let mut obs = env.reset(0).to_vec();
    let mut actions = vec![0.0; config.action_dim()];
    let mut stats = Default::default();
    let mut ending = "";
    let mut laps: Vec<f64> = Vec::new();
    let mut hint = 0;
    let mut model_steer_steps = 0_usize;
    let mut governed_steps = 0_usize;
    let mut fallback_steps = 0_usize;
    let mut total_steps = 0_usize;
    for _ in 0..(seconds * config.control_hz) as usize {
        policy.act(&obs, &mut actions);
        if supervision.reference_assist {
            let car = env.cars().next().expect("one car");
            let reference = ReferenceConfig::default().action(car, &spec.track, &mut hint);
            model_steer_steps +=
                supervise_action(&mut actions, reference, supervision.reference_blend) as usize;
            total_steps += 1;
        }
        if supervision.hazard_speed_governor {
            let car = env.cars().next().expect("one car");
            let (governed, fallback) = govern_rough_section_speed(
                &mut actions,
                car,
                &spec.track,
                &mut hint,
                supervision.hazard_steer_fallback,
            );
            governed_steps += governed as usize;
            fallback_steps += fallback as usize;
        }
        let r = env.step(&actions);
        obs.copy_from_slice(r.obs);
        let (terminated, truncated) = (r.terminated[0] != 0, r.truncated[0] != 0);
        if terminated || truncated {
            stats = env.finished_episodes()[0].1;
            ending = if terminated { " (crashed)" } else { "" };
            break;
        }
        stats = env.episode_stats().next().copied().unwrap_or_default();
        if stats.laps as usize > laps.len() {
            laps.extend(stats.last_lap_time);
        }
        if let Some(file) = &mut trace {
            let car = env.cars().next().expect("one car");
            let q = spec.track.locate(
                car.state.position,
                spec.track.nearest_index(car.state.position),
            );
            // Body sideslip, and per tyre the load-weighted tread temperature and the
            // grip left by temperature, pressure and wear.
            let v = car.state.orientation.inverse() * car.state.velocity;
            let sideslip = if v.x > 1.0 {
                v.y.atan2(v.x).to_degrees()
            } else {
                0.0
            };
            let tyres = (0..4).map(|i| {
                let (w, t) = (&car.state.wheels[i].tire, &car.telemetry.wheels[i]);
                let model = car.model.tire(i);
                (
                    w.surface_temperature(&t.tread_load),
                    model.condition_grip(w, &t.tread_load, t.pressure),
                )
            });
            let (temps, grips): (Vec<_>, Vec<_>) = tyres.unzip();
            let wheels_off = car
                .telemetry
                .wheels
                .iter()
                .filter(|w| w.surface.off_track())
                .count();
            writeln!(
                file,
                "{:.2},{:.1},{:.2},{:.2},{:.3},{:.3},{:.3},{},{:.5},{:.1},{:.0},{:.0},{:.0},{:.0},{:.3},{:.3},{:.3},{:.3},{},{:.2},{:.2}",
                stats.time,
                q.s,
                q.d,
                car.speed(),
                actions[0],
                actions[1],
                actions[2],
                car.state.drivetrain.gear,
                spec.track.samples[q.index].curvature,
                sideslip,
                temps[0],
                temps[1],
                temps[2],
                temps[3],
                grips[0],
                grips[1],
                grips[2],
                grips[3],
                wheels_off,
                q.sample.width_left,
                q.sample.width_right,
            )
            .unwrap();
        }
    }
    let laps: Vec<String> = laps.iter().map(|l| format!("{l:.3}")).collect();
    println!(
        "start line: {:.1} s{ending}, {:.0} m, {} laps, best lap {}, laps [{}]",
        stats.time,
        stats.progress,
        stats.laps,
        stats
            .best_lap_time
            .map_or("-".into(), |l| format!("{l:.3}s")),
        laps.join(", ")
    );
    if supervision.reference_assist {
        println!(
            "reference assist: model steering blended on {model_steer_steps}/{total_steps} steps"
        );
    }
    if supervision.hazard_speed_governor {
        println!("hazard speed governor: applied on {governed_steps} steps");
    }
    if supervision.hazard_steer_fallback {
        println!("hazard steering fallback: applied on {fallback_steps} steps");
    }
}

/// A bounded steering residual from the model; speed and traction remain supervised.
fn supervise_action(action: &mut [f32], reference: [f32; 3], blend: f32) -> bool {
    let close = (action[0] - reference[0]).abs() <= 0.1;
    action[0] = if close && blend > 0.0 {
        (1.0 - blend) * reference[0] + blend * action[0]
    } else {
        reference[0]
    };
    action[1] = reference[1];
    action[2] = reference[2];
    close && blend > 0.0
}

/// Keep the learned steering policy while bounding its speed in the measured
/// rough section, including the approach needed for braking.
fn govern_rough_section_speed(
    action: &mut [f32],
    car: &open_racing_api::Car,
    track: &open_racing_api::Track,
    hint: &mut usize,
    steer_fallback: bool,
) -> (bool, bool) {
    if car.state.time == 0.0 {
        *hint = track.nearest_index(car.state.position);
    }
    let q = track.locate(car.state.position, *hint);
    *hint = q.index;
    if !(3750.0..4600.0).contains(&q.s) {
        return (false, false);
    }
    let reference = ReferenceConfig::default().action(car, track, hint);
    action[1] = action[1].min(reference[1]);
    action[2] = action[2].max(reference[2]);
    let fallback = steer_fallback;
    if fallback {
        action[0] = reference[0];
    }
    (true, fallback)
}

/// Many cars from random points and speeds (the training distribution): how often and
/// where on the track the policy crashes.
fn robustness(
    policy: &mut BurnPolicy,
    seconds: f64,
    envs: usize,
    safe_start: bool,
    max_episode_seconds: f64,
    supervision: EvalSupervision,
) {
    const BIN: f64 = 100.0;
    let config = EnvConfig {
        safe_start,
        ..policy.meta.env_config()
    };
    let (spec, mut env) = eval_env(policy, config.clone(), envs, max_episode_seconds);
    let track = &*spec.track;
    let mut obs = env.reset(1).to_vec();
    let mut actions = vec![0.0; envs * config.action_dim()];
    let mut hints = vec![0_usize; envs];
    let mut crashes_at = vec![0usize; (track.length / BIN).ceil() as usize];
    let (mut crashes, mut distance, mut laps, mut best_lap) = (0usize, 0.0, 0u32, None::<f64>);
    let mut governed_steps = 0_usize;
    let mut fallback_steps = 0_usize;
    for _ in 0..(seconds * config.control_hz) as usize {
        let before: Vec<_> = env.cars().map(|c| c.state.position).collect();
        policy.act(&obs, &mut actions);
        if supervision.reference_assist {
            for (e, car) in env.cars().enumerate() {
                let reference = ReferenceConfig::default().action(car, track, &mut hints[e]);
                supervise_action(
                    &mut actions[e * 3..e * 3 + 3],
                    reference,
                    supervision.reference_blend,
                );
            }
        }
        if supervision.hazard_speed_governor {
            for (e, car) in env.cars().enumerate() {
                let (governed, fallback) = govern_rough_section_speed(
                    &mut actions[e * 3..e * 3 + 3],
                    car,
                    track,
                    &mut hints[e],
                    supervision.hazard_steer_fallback,
                );
                governed_steps += governed as usize;
                fallback_steps += fallback as usize;
            }
        }
        let r = env.step(&actions);
        obs.copy_from_slice(r.obs);
        for (e, pos) in before.iter().enumerate() {
            if r.terminated[e] != 0 {
                crashes += 1;
                crashes_at[(track.locate(*pos, track.nearest_index(*pos)).s / BIN) as usize] += 1;
            }
        }
        for (_, stats) in env.finished_episodes() {
            distance += stats.progress;
            laps += stats.laps;
            best_lap = stats
                .best_lap_time
                .map_or(best_lap, |l| Some(best_lap.map_or(l, |b: f64| b.min(l))));
        }
    }
    for stats in env.episode_stats() {
        distance += stats.progress;
        laps += stats.laps;
        best_lap = stats
            .best_lap_time
            .map_or(best_lap, |l| Some(best_lap.map_or(l, |b: f64| b.min(l))));
    }
    println!(
        "random starts: {envs} cars × {seconds:.0} s, {:.0} km, {laps} laps, best lap {}, {crashes} crashes ({:.2} per lap)",
        distance / 1000.0,
        best_lap.map_or("-".into(), |l| format!("{l:.3}s")),
        crashes as f64 * track.length / distance.max(1.0),
    );
    if supervision.hazard_speed_governor {
        println!("hazard speed governor: applied on {governed_steps} car-steps");
    }
    if supervision.hazard_steer_fallback {
        println!("hazard steering fallback: applied on {fallback_steps} car-steps");
    }
    let mut hot: Vec<_> = crashes_at
        .iter()
        .enumerate()
        .filter(|(_, n)| **n > 0)
        .collect();
    hot.sort_by(|a, b| b.1.cmp(a.1));
    for (bin, n) in hot.iter().take(5) {
        println!(
            "  crashes at {:>4.0}–{:<4.0} m: {n}",
            *bin as f64 * BIN,
            (*bin + 1) as f64 * BIN
        );
    }
}
