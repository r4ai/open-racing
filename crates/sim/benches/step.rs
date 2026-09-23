use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use open_racing_sim::{Car, CarModel, Controls, Track};

fn step(c: &mut Criterion) {
    let track = Track::default_circuit();
    let mut car = Car::new(Arc::new(CarModel::gt3()), &track, 10.0, 0.0, 30.0, 3);
    let controls = Controls {
        throttle: 0.5,
        steer_wheel_angle: 0.05,
        ..Default::default()
    };
    c.bench_function("car_step", |b| {
        b.iter(|| {
            car.step(&track, std::hint::black_box(&controls));
            if car.state.time > 20.0 {
                car.reset(&track, 10.0, 0.0, 30.0, 3);
            }
        })
    });
}

criterion_group!(benches, step);
criterion_main!(benches);
