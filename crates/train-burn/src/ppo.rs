//! Proximal Policy Optimization (clipped objective, GAE, Gaussian policy).
//!
//! Rollouts run on the CPU through the `VecEnv` port; the network runs on the Burn
//! backend with one batched forward pass per environment step.

use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

use burn::grad_clipping::GradientClippingConfig;
use burn::module::AutodiffModule;
use burn::optim::{AdamConfig, GradientsParams, Optimizer};
use burn::prelude::*;
use burn::tensor::backend::AutodiffBackend;
use open_racing_api::VecEnv;

use crate::{Agent, Normalizer, PolicyMeta, load_agent, load_meta, save_policy, scale_action};

const LOG_2PI: f32 = 1.837_877_1;

#[derive(Clone, Debug)]
pub struct PpoConfig {
    pub iterations: usize,
    /// Stop after the current iteration once this many seconds have elapsed.
    pub max_duration_seconds: Option<f64>,
    /// Env steps per env per iteration.
    pub rollout_len: usize,
    pub epochs: usize,
    pub minibatch: usize,
    pub learning_rate: f64,
    pub gamma: f32,
    pub gae_lambda: f32,
    pub clip: f32,
    pub value_coef: f32,
    pub entropy_coef: f32,
    pub max_grad_norm: f32,
    pub hidden: Vec<usize>,
    pub init_log_std: f32,
    /// Where the cap on the exploration noise ends after the last iteration, falling
    /// linearly in log space from `init_log_std`; `None` keeps it at `init_log_std`.
    pub final_log_std: Option<f32>,
    pub seed: u64,
    pub out_dir: PathBuf,
    pub save_every: usize,
    /// Warm-start from the weights and observation normaliser saved in this directory.
    pub init: Option<PathBuf>,
}

impl Default for PpoConfig {
    fn default() -> Self {
        Self {
            iterations: 500,
            max_duration_seconds: None,
            rollout_len: 64,
            epochs: 5,
            minibatch: 4096,
            learning_rate: 3e-4,
            gamma: 0.99,
            gae_lambda: 0.95,
            clip: 0.2,
            value_coef: 0.5,
            entropy_coef: 0.0,
            max_grad_norm: 0.5,
            hidden: vec![256, 256],
            init_log_std: -0.5,
            final_log_std: None,
            seed: 0,
            out_dir: PathBuf::from("runs/ppo"),
            save_every: 10,
            init: None,
        }
    }
}

/// Small deterministic RNG for exploration noise and minibatch shuffling.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn uniform(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32 + 0.5) / (1u64 << 24) as f32
    }

    fn normal(&mut self) -> f32 {
        let (u1, u2) = (self.uniform(), self.uniform());
        (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()
    }

    fn shuffle(&mut self, v: &mut [usize]) {
        for i in (1..v.len()).rev() {
            let j = (self.next_u64() % (i as u64 + 1)) as usize;
            v.swap(i, j);
        }
    }
}

/// Rollout buffer, time-major: index = t * num_envs + env.
struct Rollout {
    obs: Vec<f32>,
    actions: Vec<f32>,
    logp: Vec<f32>,
    values: Vec<f32>,
    rewards: Vec<f32>,
    dones: Vec<f32>,
    advantages: Vec<f32>,
    returns: Vec<f32>,
}

impl Rollout {
    fn new(len: usize, n: usize, obs_dim: usize, act_dim: usize) -> Self {
        let size = len * n;
        Self {
            obs: vec![0.0; size * obs_dim],
            actions: vec![0.0; size * act_dim],
            logp: vec![0.0; size],
            values: vec![0.0; size],
            rewards: vec![0.0; size],
            dones: vec![0.0; size],
            advantages: vec![0.0; size],
            returns: vec![0.0; size],
        }
    }
}

fn to_tensor<B: Backend>(data: Vec<f32>, shape: [usize; 2], device: &B::Device) -> Tensor<B, 2> {
    Tensor::from_data(TensorData::new(data, shape), device)
}

fn to_vec<B: Backend, const D: usize>(t: Tensor<B, D>) -> Vec<f32> {
    t.into_data().into_vec::<f32>().expect("f32 tensor")
}

/// Statistics of episodes completed during one iteration.
#[derive(Default)]
struct EpisodeSummary {
    count: usize,
    return_sum: f64,
    progress_sum: f64,
    laps: u32,
    best_lap: Option<f64>,
}

/// The exploration noise, capped at `cap`. Actions are clipped to the action box, so
/// noise beyond it costs nothing and an entropy bonus would inflate it without bound (the
/// policy then learns to dither between the limits); the cap lets the bonus keep
/// exploration from collapsing without letting it grow.
fn capped_log_std<B: Backend>(agent: &Agent<B>, cap: f32) -> Tensor<B, 1> {
    agent.log_std.val().clamp_max(cap)
}

/// Cap on the log exploration noise in `iteration` (1-based). Lowering it over training
/// brings the noisy policy that collected the data close to the deterministic one that
/// is run afterwards: with clipped actions the two differ more the larger the noise.
fn log_std_cap(cfg: &PpoConfig, done: f32) -> f32 {
    let end = cfg.final_log_std.unwrap_or(cfg.init_log_std);
    cfg.init_log_std + (end - cfg.init_log_std) * done
}

pub struct TrainContext {
    pub meta: PolicyMeta,
}

/// Trains a policy on `env` and writes checkpoints to `cfg.out_dir`.
pub fn train<B: AutodiffBackend>(
    env: &mut dyn VecEnv,
    cfg: &PpoConfig,
    ctx: TrainContext,
    device: &B::Device,
) {
    let n = env.num_envs();
    let obs_dim = env.observation_space().dim();
    let act_space = env.action_space().clone();
    let act_dim = act_space.dim();
    let mut meta = ctx.meta;
    B::seed(device, cfg.seed);
    let mut agent: Agent<B> = match &cfg.init {
        Some(dir) => {
            let init = load_meta(dir).unwrap_or_else(|e| panic!("loading {}: {e}", dir.display()));
            assert_eq!(
                init.obs_names,
                meta.obs_names,
                "observation layout of {} differs from the env",
                dir.display()
            );
            meta.hidden = init.hidden;
            meta.normalizer = init.normalizer;
            load_agent(dir, &meta, device)
                .unwrap_or_else(|e| panic!("loading {}: {e}", dir.display()))
        }
        None => {
            meta.hidden = cfg.hidden.clone();
            meta.normalizer = Normalizer::new(obs_dim);
            Agent::new(obs_dim, act_dim, &cfg.hidden, cfg.init_log_std, device)
        }
    };
    let mut optim = AdamConfig::new()
        .with_epsilon(1e-5)
        .with_grad_clipping(Some(GradientClippingConfig::Norm(cfg.max_grad_norm)))
        .init::<B, Agent<B>>();
    let mut rng = Rng(cfg.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);

    std::fs::create_dir_all(&cfg.out_dir).expect("create output dir");
    let mut log = std::fs::File::create(cfg.out_dir.join("log.csv")).expect("create log");
    writeln!(log, "iteration,env_steps,episodes,mean_return,mean_progress,laps,best_lap,policy_loss,value_loss,approx_kl,std,sps").unwrap();

    let mut buf = Rollout::new(cfg.rollout_len, n, obs_dim, act_dim);
    let mut raw_obs = env.reset(cfg.seed).to_vec();
    let mut norm_obs = vec![0.0; n * obs_dim];
    let mut env_actions = vec![0.0; n * act_dim];
    let mut best_lap_overall: Option<f64> = None;
    let mut total_steps = 0usize;
    let training_started = Instant::now();

    for iteration in 1..=cfg.iterations {
        let started = Instant::now();
        let progress = cfg.max_duration_seconds.map_or_else(
            || (iteration - 1) as f64 / cfg.iterations.max(1) as f64,
            |seconds| (training_started.elapsed().as_secs_f64() / seconds).min(1.0),
        );
        // Linear decay to zero lets the policy settle instead of jittering around the optimum.
        let learning_rate = cfg.learning_rate * (1.0 - progress);
        let policy = agent.valid();
        let cap = log_std_cap(cfg, progress as f32);
        let std: Vec<f32> = to_vec(capped_log_std(&policy, cap).exp());
        let mut episodes = EpisodeSummary::default();

        // ---- Collect rollout -------------------------------------------------------
        for t in 0..cfg.rollout_len {
            meta.normalizer.update(&raw_obs);
            meta.normalizer.normalize(&raw_obs, &mut norm_obs);
            let (mean, value) = policy.forward(to_tensor(norm_obs.clone(), [n, obs_dim], device));
            // One read-back per step: each row is the action mean followed by the value.
            let out = to_vec(Tensor::cat(vec![mean, value], 1));

            let row = t * n;
            buf.obs[row * obs_dim..(row + n) * obs_dim].copy_from_slice(&norm_obs);
            let actions = &mut buf.actions[row * act_dim..(row + n) * act_dim];
            for e in 0..n {
                let out = &out[e * (act_dim + 1)..(e + 1) * (act_dim + 1)];
                buf.values[row + e] = out[act_dim];
                let mut logp = 0.0;
                for a in 0..act_dim {
                    let noise = rng.normal();
                    actions[e * act_dim + a] = out[a] + std[a] * noise;
                    logp += -0.5 * noise * noise - std[a].ln() - 0.5 * LOG_2PI;
                }
                buf.logp[row + e] = logp;
            }
            scale_action(actions, &act_space.low, &act_space.high, &mut env_actions);

            let result = env.step(&env_actions);
            let rewards = &mut buf.rewards[row..row + n];
            rewards.copy_from_slice(result.rewards);
            // Bootstrap time-limit truncations with the value of the final observation.
            let truncated: Vec<usize> = (0..n).filter(|&e| result.truncated[e] != 0).collect();
            if !truncated.is_empty() {
                let mut rows = Vec::with_capacity(truncated.len() * obs_dim);
                for &e in &truncated {
                    rows.extend_from_slice(&result.final_obs[e * obs_dim..(e + 1) * obs_dim]);
                }
                let mut normed = vec![0.0; rows.len()];
                meta.normalizer.normalize(&rows, &mut normed);
                let v = to_vec(policy.critic.forward(to_tensor(
                    normed,
                    [truncated.len(), obs_dim],
                    device,
                )));
                for (k, &e) in truncated.iter().enumerate() {
                    rewards[e] += cfg.gamma * v[k];
                }
            }
            for e in 0..n {
                buf.dones[row + e] =
                    (result.terminated[e] != 0 || result.truncated[e] != 0) as u8 as f32;
            }
            raw_obs.copy_from_slice(result.obs);
            for (_, stats) in env.finished_episodes() {
                episodes.count += 1;
                episodes.return_sum += stats.return_;
                episodes.progress_sum += stats.progress;
                episodes.laps += stats.laps;
                if let Some(lap) = stats.best_lap_time {
                    episodes.best_lap = Some(episodes.best_lap.map_or(lap, |b: f64| b.min(lap)));
                }
            }
        }
        total_steps += cfg.rollout_len * n;
        let rollout_time = started.elapsed().as_secs_f64();

        // ---- Advantages (GAE) ----------------------------------------------------------
        meta.normalizer.normalize(&raw_obs, &mut norm_obs);
        let last_value = to_vec(policy.critic.forward(to_tensor(
            norm_obs.clone(),
            [n, obs_dim],
            device,
        )));
        let mut gae = vec![0.0f32; n];
        for t in (0..cfg.rollout_len).rev() {
            for e in 0..n {
                let i = t * n + e;
                let next_value = if t + 1 == cfg.rollout_len {
                    last_value[e]
                } else {
                    buf.values[i + n]
                };
                let not_done = 1.0 - buf.dones[i];
                let delta = buf.rewards[i] + cfg.gamma * next_value * not_done - buf.values[i];
                gae[e] = delta + cfg.gamma * cfg.gae_lambda * not_done * gae[e];
                buf.advantages[i] = gae[e];
                buf.returns[i] = gae[e] + buf.values[i];
            }
        }

        // ---- Optimise ------------------------------------------------------------------
        // The rollout goes to the device once; minibatches are gathered there, and the
        // loss statistics are read back once per iteration, so the device never waits
        // for the host between updates.
        let size = cfg.rollout_len * n;
        let mb = cfg.minibatch.min(size);
        let all_obs = to_tensor::<B>(buf.obs.clone(), [size, obs_dim], device);
        let all_act = to_tensor::<B>(buf.actions.clone(), [size, act_dim], device);
        let all_logp = to_tensor::<B>(buf.logp.clone(), [size, 1], device);
        let all_adv = to_tensor::<B>(buf.advantages.clone(), [size, 1], device);
        let all_ret = to_tensor::<B>(buf.returns.clone(), [size, 1], device);
        let mut indices: Vec<usize> = (0..size).collect();
        let mut stats = Tensor::<B::InnerBackend, 1>::zeros([3], device);
        let mut updates = 0usize;
        for _ in 0..cfg.epochs {
            rng.shuffle(&mut indices);
            for chunk in indices.chunks(mb) {
                let m = chunk.len();
                let idx = Tensor::<B, 1, Int>::from_data(
                    TensorData::new(chunk.iter().map(|&i| i as i64).collect(), [m]),
                    device,
                );
                let obs = all_obs.clone().select(0, idx.clone());
                let act = all_act.clone().select(0, idx.clone());
                let old_logp = all_logp.clone().select(0, idx.clone());
                let ret = all_ret.clone().select(0, idx.clone());
                let adv = all_adv.clone().select(0, idx);
                let adv_mean = adv.clone().mean();
                let adv_std = (adv.clone() - adv_mean.clone().unsqueeze())
                    .powf_scalar(2.0)
                    .mean()
                    .sqrt()
                    .add_scalar(1e-8);
                let adv = (adv - adv_mean.unsqueeze()) / adv_std.unsqueeze();

                let (mean, value) = agent.forward(obs);
                let log_std = capped_log_std(&agent, cap)
                    .unsqueeze_dim::<2>(0)
                    .expand([m, act_dim]);
                let z = (act - mean) / log_std.clone().exp();
                let logp = (z.powf_scalar(2.0).mul_scalar(-0.5) - log_std.clone() - 0.5 * LOG_2PI)
                    .sum_dim(1);
                let log_ratio = logp - old_logp;
                let ratio = log_ratio.clone().exp();
                let surr1 = ratio.clone() * adv.clone();
                let surr2 = ratio.clamp(1.0 - cfg.clip, 1.0 + cfg.clip) * adv;
                let policy_loss = -surr1.min_pair(surr2).mean();
                let value_loss = (value - ret).powf_scalar(2.0).mean().mul_scalar(0.5);
                let entropy = (capped_log_std(&agent, cap) + 0.5 * (1.0 + LOG_2PI)).sum();
                let loss = policy_loss.clone() + value_loss.clone().mul_scalar(cfg.value_coef)
                    - entropy.mul_scalar(cfg.entropy_coef);

                stats = stats
                    + Tensor::cat(
                        vec![
                            policy_loss.clone().inner(),
                            value_loss.clone().inner(),
                            (-log_ratio).mean().inner(),
                        ],
                        0,
                    );
                updates += 1;

                let grads = GradientsParams::from_grads(loss.backward(), &agent);
                agent = optim.step(learning_rate, agent, grads);
            }
        }

        // ---- Report --------------------------------------------------------------------
        let sps = (cfg.rollout_len * n) as f64 / started.elapsed().as_secs_f64();
        let u = updates.max(1) as f32;
        let [pl_sum, vl_sum, kl_sum]: [f32; 3] = to_vec(stats).try_into().expect("three stats");
        let ep = episodes.count.max(1) as f64;
        if let Some(lap) = episodes.best_lap {
            best_lap_overall = Some(best_lap_overall.map_or(lap, |b| b.min(lap)));
        }
        let mean_std = std.iter().sum::<f32>() / std.len() as f32;
        writeln!(
            log,
            "{iteration},{total_steps},{},{:.3},{:.1},{},{},{:.4},{:.4},{:.5},{:.3},{:.0}",
            episodes.count,
            episodes.return_sum / ep,
            episodes.progress_sum / ep,
            episodes.laps,
            episodes
                .best_lap
                .map_or(String::new(), |l| format!("{l:.3}")),
            pl_sum / u,
            vl_sum / u,
            kl_sum / u,
            mean_std,
            sps,
        )
        .unwrap();
        println!(
            "it {iteration:4} | steps {total_steps:>10} | eps {:4} | return {:8.2} | progress {:7.1} m | laps {:3} | best lap {} | kl {:.4} | std {:.3} | {:.0} sps (rollout {:.0}%)",
            episodes.count,
            episodes.return_sum / ep,
            episodes.progress_sum / ep,
            episodes.laps,
            best_lap_overall.map_or("-".into(), |l| format!("{l:.2}s")),
            kl_sum / u,
            mean_std,
            sps,
            100.0 * rollout_time / started.elapsed().as_secs_f64(),
        );

        let reached_duration = cfg
            .max_duration_seconds
            .is_some_and(|seconds| training_started.elapsed().as_secs_f64() >= seconds);
        if iteration % cfg.save_every == 0 || iteration == cfg.iterations || reached_duration {
            save_policy(&cfg.out_dir, &agent.valid(), &meta).expect("save policy");
        }
        if reached_duration {
            break;
        }
    }
}
