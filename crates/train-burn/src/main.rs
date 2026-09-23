use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Parser, Subcommand};
use open_racing_api::{DefaultReward, EnvConfig, EnvSpec, Policy, RacingVecEnv, VecEnv};
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
        #[arg(long, default_value_t = 3e-4)]
        lr: f64,
        /// Discount per agent step; the planning horizon is about 1 / (1 − gamma) steps.
        #[arg(long, default_value_t = 0.99)]
        gamma: f32,
        /// Weight of the entropy bonus that keeps the policy exploring.
        #[arg(long, default_value_t = 0.0)]
        entropy: f32,
        /// Reward lost when an episode ends in a crash (100 m of progress earns 10).
        #[arg(long, default_value_t = DefaultReward::default().termination_penalty)]
        crash_penalty: f64,
        #[arg(long, default_value_t = 0)]
        seed: u64,
        /// Include ground-truth tyre state in the observation.
        #[arg(long)]
        privileged: bool,
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
    },
}

fn main() {
    match Cli::parse().command {
        Command::Train { track, car, envs, iterations, rollout, lr, gamma, entropy, crash_penalty, seed, privileged, manual_shift, out, init } => {
            let config = EnvConfig { privileged_obs: privileged, auto_shift: !manual_shift, seed, ..EnvConfig::default() };
            let mut spec = EnvSpec::from_names(&track, &car, config.clone()).unwrap_or_else(|e| panic!("{e}"));
            spec.reward = Arc::new(DefaultReward { termination_penalty: crash_penalty, ..DefaultReward::default() });
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
                max_steer_rate: config.max_steer_rate,
                auto_shift: config.auto_shift,
                track,
                car,
            };
            let cfg = PpoConfig { iterations, rollout_len: rollout, learning_rate: lr, gamma, entropy_coef: entropy, seed, out_dir: out, init, ..Default::default() };
            println!(
                "training on {} envs, obs dim {}, {} physics steps per action",
                envs,
                env.observation_space().dim(),
                config.substeps()
            );
            ppo::train::<TrainBackend>(&mut env, &cfg, TrainContext { meta }, &Default::default());
        }
        Command::Eval { model, seconds, envs, trace } => {
            let mut policy = BurnPolicy::load(&model).unwrap_or_else(|e| panic!("loading {}: {e}", model.display()));
            flying_lap(&mut policy, seconds, trace.as_deref());
            if envs > 0 {
                robustness(&mut policy, seconds, envs);
            }
        }
    }
}

fn eval_env(policy: &BurnPolicy, config: EnvConfig, envs: usize) -> (EnvSpec, RacingVecEnv) {
    let spec = EnvSpec::from_names(&policy.meta.track, &policy.meta.car, config).unwrap_or_else(|e| panic!("{e}"));
    let env = spec.make_vec_env(envs);
    policy.check_compatible(env.observation_space()).unwrap_or_else(|e| panic!("{e}"));
    (spec, env)
}

/// One car from a standing start on the start line, as in a race.
fn flying_lap(policy: &mut BurnPolicy, seconds: f64, trace: Option<&Path>) {
    let config = EnvConfig { random_start: false, start_speed: (0.0, 0.0), start_offset: (0.0, 0.0), ..policy.meta.env_config() };
    let (spec, mut env) = eval_env(policy, config.clone(), 1);
    let mut trace = trace.map(|path| {
        let mut file = std::io::BufWriter::new(std::fs::File::create(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())));
        writeln!(file, "time,s,offset,speed,steer,throttle,brake,gear,curvature").unwrap();
        file
    });
    let mut obs = env.reset(0).to_vec();
    let mut actions = vec![0.0; config.action_dim()];
    let mut stats = Default::default();
    let mut ending = "";
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
        if let Some(file) = &mut trace {
            let car = env.cars().next().expect("one car");
            let q = spec.track.query(car.state.position, spec.track.nearest_index(car.state.position));
            writeln!(
                file,
                "{:.2},{:.1},{:.2},{:.2},{:.3},{:.3},{:.3},{},{:.5}",
                stats.time,
                q.s,
                q.d,
                car.speed(),
                actions[0],
                actions[1],
                actions[2],
                car.state.drivetrain.gear,
                spec.track.samples[q.index].curvature
            )
            .unwrap();
        }
    }
    println!(
        "start line: {:.1} s{ending}, {:.0} m, {} laps, best lap {}",
        stats.time,
        stats.progress,
        stats.laps,
        stats.best_lap_time.map_or("-".into(), |l| format!("{l:.3}s"))
    );
}

/// Many cars from random points and speeds (the training distribution): how often and
/// where on the track the policy crashes.
fn robustness(policy: &mut BurnPolicy, seconds: f64, envs: usize) {
    const BIN: f64 = 100.0;
    let config = policy.meta.env_config();
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
            best_lap = stats.best_lap_time.map_or(best_lap, |l| Some(best_lap.map_or(l, |b: f64| b.min(l))));
        }
    }
    for stats in env.episode_stats() {
        distance += stats.progress;
        laps += stats.laps;
        best_lap = stats.best_lap_time.map_or(best_lap, |l| Some(best_lap.map_or(l, |b: f64| b.min(l))));
    }
    println!(
        "random starts: {envs} cars × {seconds:.0} s, {:.0} km, {laps} laps, best lap {}, {crashes} crashes ({:.2} per lap)",
        distance / 1000.0,
        best_lap.map_or("-".into(), |l| format!("{l:.3}s")),
        crashes as f64 * track.length / distance.max(1.0),
    );
    let mut hot: Vec<_> = crashes_at.iter().enumerate().filter(|(_, n)| **n > 0).collect();
    hot.sort_by(|a, b| b.1.cmp(a.1));
    for (bin, n) in hot.iter().take(5) {
        println!("  crashes at {:>4.0}–{:<4.0} m: {n}", *bin as f64 * BIN, (*bin + 1) as f64 * BIN);
    }
}
