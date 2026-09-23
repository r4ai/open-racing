use std::sync::Arc;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use open_racing_env::{ACTION_DIM, BatchEnv, EnvConfig, EnvShared};
use open_racing_sim::{CarModel, Track};

fn batch(c: &mut Criterion) {
    let shared = EnvShared::new(EnvConfig::default(), Arc::new(Track::default_circuit()), Arc::new(CarModel::gt3()));
    let num_envs = 256;
    let mut env = BatchEnv::new(shared, num_envs);
    let actions: Vec<f32> = (0..num_envs).flat_map(|i| [((i as f32) * 0.01).sin() * 0.1, 0.6, 0.0]).collect();
    assert_eq!(actions.len(), num_envs * ACTION_DIM);
    let mut group = c.benchmark_group("batch_env");
    // One agent step = 20 physics steps.
    group.throughput(Throughput::Elements((num_envs * 20) as u64));
    group.bench_function("step_256_envs", |b| b.iter(|| {
        env.step(&actions);
    }));
    group.finish();
}

criterion_group!(benches, batch);
criterion_main!(benches);
