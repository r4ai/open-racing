//! GPU behavioural cloning from a tested reference controller.

use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

use burn::grad_clipping::GradientClippingConfig;
use burn::module::AutodiffModule;
use burn::optim::{AdamConfig, GradientsParams, Optimizer};
use burn::prelude::*;
use burn::tensor::backend::AutodiffBackend;
use open_racing_api::{Policy, RacingVecEnv, VecEnv};
use open_racing_sim::Track;
use rayon::prelude::*;

use crate::reference::ReferenceConfig;
use crate::{Agent, BurnPolicy, PolicyMeta, load_agent, load_meta, save_policy};

pub struct ImitationConfig {
    pub samples: usize,
    pub epochs: usize,
    pub minibatch: usize,
    pub learning_rate: f64,
    pub hidden: Vec<usize>,
    pub seed: u64,
    pub out_dir: PathBuf,
    pub reference: ReferenceConfig,
    pub init: Option<PathBuf>,
    pub student_fraction: f64,
    pub pedal_weight: f32,
}

pub fn train<B: AutodiffBackend>(
    env: &mut RacingVecEnv,
    track: &Track,
    cfg: &ImitationConfig,
    mut meta: PolicyMeta,
    device: &B::Device,
) {
    let n = env.num_envs();
    let obs_dim = env.observation_space().dim();
    let act_dim = env.action_space().dim();
    assert_eq!(act_dim, 3, "reference controller uses automatic shifting");
    assert!(n > 0 && cfg.samples >= n && cfg.epochs > 0 && cfg.minibatch > 0);
    assert!((0.0..=1.0).contains(&cfg.student_fraction));
    assert!(cfg.pedal_weight.is_finite() && cfg.pedal_weight > 0.0);
    B::seed(device, cfg.seed);
    let mut student = cfg
        .init
        .as_ref()
        .map(|dir| BurnPolicy::load(dir).expect("load student"));
    if let Some(student) = &student {
        assert_eq!(student.meta.obs_names, meta.obs_names);
    }
    let size = cfg.samples / n * n;
    let mut observations = Vec::with_capacity(size * obs_dim);
    let mut targets = Vec::with_capacity(size * act_dim);
    let mut actions = vec![0.0_f32; n * act_dim];
    let mut student_actions = vec![0.0_f32; n * act_dim];
    let mut hints = vec![0_usize; n];
    let mut obs = env.reset(cfg.seed).to_vec();
    let started = Instant::now();
    let mut finished = 0_usize;
    while observations.len() < size * obs_dim {
        observations.extend_from_slice(&obs);
        let cars: Vec<_> = env.cars().collect();
        actions
            .par_chunks_mut(3)
            .zip(hints.par_iter_mut())
            .zip(cars.par_iter())
            .for_each(|((out, hint), car)| {
                out.copy_from_slice(&cfg.reference.action(car, track, hint));
            });
        for action in actions.as_chunks::<3>().0 {
            targets.extend_from_slice(&[action[0], 2.0 * action[1] - 1.0, 2.0 * action[2] - 1.0]);
        }
        if let Some(policy) = &mut student {
            policy.act(&obs, &mut student_actions);
            for e in 0..n {
                if (e as f64) < cfg.student_fraction * n as f64 {
                    actions[e * 3..e * 3 + 3].copy_from_slice(&student_actions[e * 3..e * 3 + 3]);
                }
            }
        }
        let result = env.step(&actions);
        obs.copy_from_slice(result.obs);
        finished += env.finished_episodes().len();
    }
    println!(
        "reference data: {size} samples, {finished} ended episodes, {:.0} samples/s",
        size as f64 / started.elapsed().as_secs_f64()
    );

    if let Some(dir) = &cfg.init {
        let previous = load_meta(dir).expect("load initial metadata");
        meta.hidden = previous.hidden;
        meta.normalizer = previous.normalizer;
    } else {
        meta.hidden = cfg.hidden.clone();
    }
    // A warm-started actor was trained in the checkpoint's input coordinate
    // system. Updating its normalizer without transforming the first layer
    // can make an unchanged policy fail immediately at the start line.
    if cfg.init.is_none() {
        meta.normalizer.update(&observations);
    }
    let mut normalized = vec![0.0_f32; observations.len()];
    meta.normalizer.normalize(&observations, &mut normalized);
    drop(observations);
    let all_obs = Tensor::<B, 2>::from_data(TensorData::new(normalized, [size, obs_dim]), device);
    let all_targets = Tensor::<B, 2>::from_data(TensorData::new(targets, [size, 3]), device);
    let weights = Tensor::<B, 2>::from_data(
        TensorData::new(vec![8.0, cfg.pedal_weight, cfg.pedal_weight], [1, 3]),
        device,
    );
    let mut agent = match &cfg.init {
        Some(dir) => load_agent(dir, &meta, device).expect("load initial agent"),
        None => Agent::<B>::new(obs_dim, 3, &cfg.hidden, 0.12_f32.ln(), device),
    };
    let mut optimizer = AdamConfig::new()
        .with_epsilon(1e-5)
        .with_grad_clipping(Some(GradientClippingConfig::Norm(1.0)))
        .init::<B, Agent<B>>();
    std::fs::create_dir_all(&cfg.out_dir).expect("create output directory");
    let mut log = std::fs::File::create(cfg.out_dir.join("imitation.csv")).expect("create log");
    writeln!(log, "epoch,weighted_mse,seconds").unwrap();
    let mut order: Vec<usize> = (0..size).collect();
    let mut rng = cfg.seed | 1;
    for epoch in 1..=cfg.epochs {
        let epoch_started = Instant::now();
        for i in (1..size).rev() {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            order.swap(i, (rng % (i as u64 + 1)) as usize);
        }
        let mut loss_sum = Tensor::<B::InnerBackend, 1>::zeros([1], device);
        for chunk in order.chunks(cfg.minibatch) {
            let indices = Tensor::<B, 1, Int>::from_data(
                TensorData::new(chunk.iter().map(|&i| i as i64).collect(), [chunk.len()]),
                device,
            );
            let input = all_obs.clone().select(0, indices.clone());
            let target = all_targets.clone().select(0, indices);
            let prediction = agent.actor.forward(input);
            let loss = ((prediction - target).powf_scalar(2.0) * weights.clone()).mean();
            loss_sum = loss_sum + loss.clone().inner();
            let grads = GradientsParams::from_grads(loss.backward(), &agent);
            agent = optimizer.step(cfg.learning_rate, agent, grads);
        }
        let mse =
            loss_sum.into_scalar().elem::<f32>() as f64 / order.chunks(cfg.minibatch).len() as f64;
        writeln!(
            log,
            "{epoch},{mse:.6},{:.1}",
            epoch_started.elapsed().as_secs_f64()
        )
        .unwrap();
        log.flush().unwrap();
        println!("imitation epoch {epoch}: weighted mse {mse:.6}");
        if epoch % 2 == 0 || epoch == cfg.epochs {
            let checkpoint = cfg.out_dir.join(format!("epoch-{epoch:02}"));
            save_policy(&checkpoint, &agent.valid(), &meta).expect("save checkpoint");
            save_policy(&cfg.out_dir, &agent.valid(), &meta).expect("save policy");
        }
    }
}
