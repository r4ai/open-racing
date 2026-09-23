use std::path::PathBuf;

use clap::{Parser, Subcommand};
use open_racing_api::{EnvConfig, EnvSpec, Policy, VecEnv};
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
        #[arg(long, default_value_t = 0)]
        seed: u64,
        /// Include ground-truth tyre state in the observation.
        #[arg(long)]
        privileged: bool,
        #[arg(long, default_value = "runs/ppo")]
        out: PathBuf,
    },
    /// Run a trained policy and report lap times.
    Eval {
        #[arg(long, default_value = "runs/ppo")]
        model: PathBuf,
        #[arg(long, default_value_t = 180.0)]
        seconds: f64,
    },
}

fn main() {
    match Cli::parse().command {
        Command::Train { track, car, envs, iterations, rollout, lr, seed, privileged, out } => {
            let config = EnvConfig { privileged_obs: privileged, seed, ..EnvConfig::default() };
            let spec = EnvSpec::from_names(&track, &car, config.clone()).unwrap_or_else(|e| panic!("{e}"));
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
            let cfg = PpoConfig { iterations, rollout_len: rollout, learning_rate: lr, seed, out_dir: out, ..Default::default() };
            println!(
                "training on {} envs, obs dim {}, {} physics steps per action",
                envs,
                env.observation_space().dim(),
                config.substeps()
            );
            ppo::train::<TrainBackend>(&mut env, &cfg, TrainContext { meta }, &Default::default());
        }
        Command::Eval { model, seconds } => {
            let mut policy = BurnPolicy::load(&model).unwrap_or_else(|e| panic!("loading {}: {e}", model.display()));
            let config = EnvConfig {
                random_start: false,
                start_speed: (0.0, 0.0),
                start_offset: (0.0, 0.0),
                ..policy.meta.env_config()
            };
            let spec = EnvSpec::from_names(&policy.meta.track, &policy.meta.car, config.clone()).unwrap_or_else(|e| panic!("{e}"));
            let mut env = spec.make_vec_env(1);
            policy.check_compatible(env.observation_space()).unwrap_or_else(|e| panic!("{e}"));
            let mut obs = env.reset(0).to_vec();
            let mut actions = vec![0.0; 3];
            let steps = (seconds * config.control_hz) as usize;
            for _ in 0..steps {
                policy.act(&obs, &mut actions);
                let r = env.step(&actions);
                obs.copy_from_slice(r.obs);
                if r.terminated[0] != 0 {
                    let stats = env.finished_episodes()[0].1;
                    println!("episode terminated after {:.1}s, {:.0} m", stats.time, stats.progress);
                    return;
                }
            }
            let stats = env.episode_stats().next().copied().unwrap_or_default();
            println!(
                "{:.0} s: {:.0} m, {} laps, best lap {}",
                stats.time,
                stats.progress,
                stats.laps,
                stats.best_lap_time.map_or("-".into(), |l| format!("{l:.3}s"))
            );
        }
    }
}
