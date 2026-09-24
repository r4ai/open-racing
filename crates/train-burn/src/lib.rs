//! Burn adapter: actor-critic network, PPO trainer and a `Policy` implementation for
//! running trained agents anywhere the `open_racing_api::Policy` port is accepted.

pub mod ppo;

use std::path::Path;

use burn::module::{Module, Param};
use burn::nn::{Linear, LinearConfig};
use burn::prelude::*;
use burn::record::{BinFileRecorder, FullPrecisionSettings};
use open_racing_api::{BoxSpace, EnvConfig, Policy};
use serde::{Deserialize, Serialize};

#[cfg(feature = "wgpu")]
pub type InferenceBackend = burn::backend::Wgpu;
#[cfg(all(feature = "flex", not(feature = "wgpu")))]
pub type InferenceBackend = burn::backend::Flex;
#[cfg(all(feature = "ndarray", not(any(feature = "wgpu", feature = "flex"))))]
pub type InferenceBackend = burn::backend::NdArray;
pub type TrainBackend = burn::backend::Autodiff<InferenceBackend>;

/// Multi-layer perceptron with tanh activations between layers.
#[derive(Module, Debug)]
pub struct Mlp<B: Backend> {
    layers: Vec<Linear<B>>,
}

impl<B: Backend> Mlp<B> {
    pub fn new(sizes: &[usize], device: &B::Device) -> Self {
        Self {
            layers: sizes
                .windows(2)
                .map(|w| LinearConfig::new(w[0], w[1]).init(device))
                .collect(),
        }
    }

    pub fn forward(&self, mut x: Tensor<B, 2>) -> Tensor<B, 2> {
        let last = self.layers.len() - 1;
        for (i, layer) in self.layers.iter().enumerate() {
            x = layer.forward(x);
            if i < last {
                x = x.tanh();
            }
        }
        x
    }
}

/// Gaussian actor and value critic with separate trunks.
#[derive(Module, Debug)]
pub struct Agent<B: Backend> {
    pub actor: Mlp<B>,
    pub critic: Mlp<B>,
    pub log_std: Param<Tensor<B, 1>>,
}

impl<B: Backend> Agent<B> {
    pub fn new(
        obs_dim: usize,
        act_dim: usize,
        hidden: &[usize],
        init_log_std: f32,
        device: &B::Device,
    ) -> Self {
        let sizes = |out: usize| [&[obs_dim][..], hidden, &[out]].concat();
        Self {
            actor: Mlp::new(&sizes(act_dim), device),
            critic: Mlp::new(&sizes(1), device),
            log_std: Param::from_tensor(Tensor::full([act_dim], init_log_std, device)),
        }
    }

    /// Returns (action mean `[n, act]`, value `[n, 1]`).
    pub fn forward(&self, obs: Tensor<B, 2>) -> (Tensor<B, 2>, Tensor<B, 2>) {
        (self.actor.forward(obs.clone()), self.critic.forward(obs))
    }
}

/// Running mean / variance of observations (Welford / Chan parallel update).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Normalizer {
    pub mean: Vec<f64>,
    pub var: Vec<f64>,
    pub count: f64,
}

impl Normalizer {
    const CLIP: f64 = 10.0;

    pub fn new(dim: usize) -> Self {
        Self {
            mean: vec![0.0; dim],
            var: vec![1.0; dim],
            count: 1e-4,
        }
    }

    /// Updates statistics with a batch of rows.
    pub fn update(&mut self, rows: &[f32]) {
        let dim = self.mean.len();
        let n = (rows.len() / dim) as f64;
        if n == 0.0 {
            return;
        }
        for j in 0..dim {
            let col = rows.iter().skip(j).step_by(dim).map(|&x| x as f64);
            let batch_mean = col.clone().sum::<f64>() / n;
            let batch_var = col.map(|x| (x - batch_mean).powi(2)).sum::<f64>() / n;
            let total = self.count + n;
            let delta = batch_mean - self.mean[j];
            self.mean[j] += delta * n / total;
            let m2 =
                self.var[j] * self.count + batch_var * n + delta * delta * self.count * n / total;
            self.var[j] = m2 / total;
        }
        self.count += n;
    }

    pub fn normalize(&self, rows: &[f32], out: &mut [f32]) {
        let dim = self.mean.len();
        for (i, (x, o)) in rows.iter().zip(out.iter_mut()).enumerate() {
            let j = i % dim;
            let z = (*x as f64 - self.mean[j]) / (self.var[j] + 1e-8).sqrt();
            *o = z.clamp(-Self::CLIP, Self::CLIP) as f32;
        }
    }
}

/// Everything besides the weights needed to rebuild and run a trained policy.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PolicyMeta {
    pub obs_names: Vec<String>,
    pub act_low: Vec<f32>,
    pub act_high: Vec<f32>,
    pub hidden: Vec<usize>,
    pub normalizer: Normalizer,
    pub control_hz: f64,
    pub lookahead_points: usize,
    pub lookahead_spacing: f64,
    pub privileged_obs: bool,
    /// Tread temperatures and pressures in the observation (absent in older policies).
    #[serde(default)]
    pub tyre_obs: bool,
    /// Track widths at the lookahead points in the observation (absent in older policies).
    #[serde(default)]
    pub edge_obs: bool,
    /// Anti-lock brakes (absent in older policies).
    #[serde(default)]
    pub abs: bool,
    pub max_steer_rate: f64,
    pub auto_shift: bool,
    pub track: String,
    pub car: String,
}

impl PolicyMeta {
    /// Environment configuration matching what the policy was trained with.
    pub fn env_config(&self) -> EnvConfig {
        EnvConfig {
            control_hz: self.control_hz,
            lookahead_points: self.lookahead_points,
            lookahead_spacing: self.lookahead_spacing,
            privileged_obs: self.privileged_obs,
            tyre_obs: self.tyre_obs,
            edge_obs: self.edge_obs,
            abs: self.abs,
            max_steer_rate: self.max_steer_rate,
            auto_shift: self.auto_shift,
            ..EnvConfig::default()
        }
    }
}

/// Maps a raw network output (unbounded) to the env's action box: tanh-free clip to
/// [−1, 1], then affine to [low, high].
pub fn scale_action(raw: &[f32], low: &[f32], high: &[f32], out: &mut [f32]) {
    for (i, (r, o)) in raw.iter().zip(out.iter_mut()).enumerate() {
        let j = i % low.len();
        let u = r.clamp(-1.0, 1.0);
        *o = low[j] + 0.5 * (u + 1.0) * (high[j] - low[j]);
    }
}

fn recorder() -> BinFileRecorder<FullPrecisionSettings> {
    BinFileRecorder::new()
}

pub fn save_policy<B: Backend>(
    dir: &Path,
    agent: &Agent<B>,
    meta: &PolicyMeta,
) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    agent
        .clone()
        .save_file(dir.join("policy"), &recorder())
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let text = ron::ser::to_string_pretty(meta, ron::ser::PrettyConfig::default())
        .map_err(std::io::Error::other)?;
    std::fs::write(dir.join("meta.ron"), text)
}

pub fn load_meta(dir: &Path) -> std::io::Result<PolicyMeta> {
    ron::from_str(&std::fs::read_to_string(dir.join("meta.ron"))?).map_err(std::io::Error::other)
}

/// Rebuilds the network described by `meta` and loads the weights saved in `dir`.
pub fn load_agent<B: Backend>(
    dir: &Path,
    meta: &PolicyMeta,
    device: &B::Device,
) -> std::io::Result<Agent<B>> {
    Agent::new(
        meta.obs_names.len(),
        meta.act_low.len(),
        &meta.hidden,
        0.0,
        device,
    )
    .load_file(dir.join("policy"), &recorder(), device)
    .map_err(|e| std::io::Error::other(e.to_string()))
}

/// A trained policy running deterministically (distribution mean) for inference.
pub struct BurnPolicy {
    agent: Agent<InferenceBackend>,
    pub meta: PolicyMeta,
    device: burn::tensor::Device<InferenceBackend>,
    norm_buf: Vec<f32>,
}

impl BurnPolicy {
    pub fn load(dir: &Path) -> std::io::Result<Self> {
        let meta = load_meta(dir)?;
        let device = Default::default();
        let agent = load_agent(dir, &meta, &device)?;
        Ok(Self {
            agent,
            meta,
            device,
            norm_buf: Vec::new(),
        })
    }

    /// Fails when the policy was trained on a different observation layout.
    pub fn check_compatible(&self, obs_space: &BoxSpace) -> Result<(), String> {
        if obs_space.names == self.meta.obs_names {
            Ok(())
        } else {
            Err(format!(
                "observation layout mismatch: policy has {} dims, env has {}",
                self.meta.obs_names.len(),
                obs_space.dim()
            ))
        }
    }
}

impl Policy for BurnPolicy {
    fn act(&mut self, obs: &[f32], actions: &mut [f32]) {
        let dim = self.meta.obs_names.len();
        let n = obs.len() / dim;
        self.norm_buf.resize(obs.len(), 0.0);
        self.meta.normalizer.normalize(obs, &mut self.norm_buf);
        let x = Tensor::<InferenceBackend, 2>::from_data(
            TensorData::new(self.norm_buf.clone(), [n, dim]),
            &self.device,
        );
        let mean = self.agent.actor.forward(x);
        let raw = mean.into_data().into_vec::<f32>().expect("f32 output");
        scale_action(&raw, &self.meta.act_low, &self.meta.act_high, actions);
    }
}
