use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Parser, Subcommand};
use open_racing_api::{DefaultReward, EnvConfig, EnvSpec, Policy, RacingVecEnv, Surface, VecEnv};
use open_racing_train_burn::ppo::{self, PpoConfig, TrainContext};
use open_racing_train_burn::{BurnPolicy, Normalizer, PolicyMeta, TrainBackend};

#[derive(Parser)]
#[command(about = "Train and evaluate racing agents with PPO on Burn")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
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
        /// Start episodes no faster than the corners just ahead allow.
        #[arg(long)]
        safe_start: bool,
        /// Agent decisions per second.
        #[arg(long, default_value_t = EnvConfig::default().control_hz)]
        control_hz: f64,
        /// Fastest the steering wheel turns, rad/s.
        #[arg(long, default_value_t = EnvConfig::default().max_steer_rate)]
        max_steer_rate: f64,
        /// Hidden layer sizes of the actor and critic, e.g. 256,256.
        #[arg(long, value_delimiter = ',', default_values_t = PpoConfig::default().hidden)]
        hidden: Vec<usize>,
        /// Let the policy shift gears itself instead of the automatic gear selector.
        #[arg(long)]
        manual_shift: bool,
        #[arg(long, default_value = "runs/ppo")]
        out: PathBuf,
        /// Continue from a trained policy (weights and observation normaliser).
        #[arg(long)]
        init: Option<PathBuf>,
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
    },
}

fn main() {
    match Cli::parse().command {
        Command::Train {
            track,
            car,
            envs,
            iterations,
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
            seed,
            privileged,
            tyre_obs,
            edge_obs,
            safe_start,
            control_hz,
            max_steer_rate,
            hidden,
            manual_shift,
            out,
            init,
        } => {
            let config = EnvConfig {
                privileged_obs: privileged,
                tyre_obs,
                edge_obs,
                safe_start,
                control_hz,
                max_steer_rate,
                auto_shift: !manual_shift,
                seed,
                ..EnvConfig::default()
            };
            let mut spec =
                EnvSpec::from_names(&track, &car, config.clone()).unwrap_or_else(|e| panic!("{e}"));
            spec.reward = Arc::new(DefaultReward {
                termination_penalty: crash_penalty,
                grip_loss_weight: grip_loss_penalty,
                steer_change_weight: steer_change_penalty,
                off_track_weight: off_track_penalty,
                ..DefaultReward::default()
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
                privileged_obs: config.privileged_obs,
                tyre_obs: config.tyre_obs,
                edge_obs: config.edge_obs,
                max_steer_rate: config.max_steer_rate,
                auto_shift: config.auto_shift,
                track,
                car,
            };
            let cfg = PpoConfig {
                iterations,
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
        Command::Eval {
            model,
            seconds,
            envs,
            trace,
            safe_start,
        } => {
            let mut policy = BurnPolicy::load(&model)
                .unwrap_or_else(|e| panic!("loading {}: {e}", model.display()));
            flying_lap(&mut policy, seconds, trace.as_deref());
            if envs > 0 {
                robustness(&mut policy, seconds, envs, safe_start);
            }
        }
    }
}

fn eval_env(policy: &BurnPolicy, config: EnvConfig, envs: usize) -> (EnvSpec, RacingVecEnv) {
    let spec = EnvSpec::from_names(&policy.meta.track, &policy.meta.car, config)
        .unwrap_or_else(|e| panic!("{e}"));
    let env = spec.make_vec_env(envs);
    policy
        .check_compatible(env.observation_space())
        .unwrap_or_else(|e| panic!("{e}"));
    (spec, env)
}

/// One car from a standing start on the start line, as in a race.
fn flying_lap(policy: &mut BurnPolicy, seconds: f64, trace: Option<&Path>) {
    let config = EnvConfig {
        random_start: false,
        start_speed: (0.0, 0.0),
        start_offset: (0.0, 0.0),
        ..policy.meta.env_config()
    };
    let (spec, mut env) = eval_env(policy, config.clone(), 1);
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
    for _ in 0..(seconds * config.control_hz) as usize {
        policy.act(&obs, &mut actions);
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
            let q = spec.track.query(
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
                .filter(|w| w.surface == Surface::Grass)
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
                q.width_left,
                q.width_right,
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
}

/// Many cars from random points and speeds (the training distribution): how often and
/// where on the track the policy crashes.
fn robustness(policy: &mut BurnPolicy, seconds: f64, envs: usize, safe_start: bool) {
    const BIN: f64 = 100.0;
    let config = EnvConfig {
        safe_start,
        ..policy.meta.env_config()
    };
    let (spec, mut env) = eval_env(policy, config.clone(), envs);
    let track = &*spec.track;
    let mut obs = env.reset(1).to_vec();
    let mut actions = vec![0.0; envs * config.action_dim()];
    let mut crashes_at = vec![0usize; (track.length / BIN).ceil() as usize];
    let (mut crashes, mut distance, mut laps, mut best_lap) = (0usize, 0.0, 0u32, None::<f64>);
    for _ in 0..(seconds * config.control_hz) as usize {
        let before: Vec<_> = env.cars().map(|c| c.state.position).collect();
        policy.act(&obs, &mut actions);
        let r = env.step(&actions);
        obs.copy_from_slice(r.obs);
        for (e, pos) in before.iter().enumerate() {
            if r.terminated[e] != 0 {
                crashes += 1;
                crashes_at[(track.query(*pos, track.nearest_index(*pos)).s / BIN) as usize] += 1;
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
