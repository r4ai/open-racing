use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use open_racing_sim::{
    Car, CarModel, Controls, DT, RubberMap, Sky, Track, TrackEvolution, Weather, WeatherSettings,
};

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

/// A step in live weather: the weather advances with the car and the car feels its air,
/// wind and road temperature.
fn step_in_weather(c: &mut Criterion) {
    let track = Track::default_circuit();
    let mut car = Car::new(Arc::new(CarModel::gt3()), &track, 10.0, 0.0, 30.0, 3);
    let mut evolution = TrackEvolution::UNIFORM;
    let mut weather = Weather::new(
        &track,
        None,
        WeatherSettings {
            sky: Sky::PartlyCloudy,
            ..Default::default()
        },
    );
    let controls = Controls {
        throttle: 0.5,
        steer_wheel_angle: 0.05,
        ..Default::default()
    };
    c.bench_function("car_step_in_weather", |b| {
        b.iter(|| {
            weather.step(DT);
            car.step_in(
                &track,
                &mut evolution,
                &weather,
                std::hint::black_box(&controls),
            );
            if car.state.time > 20.0 {
                car.reset(&track, 10.0, 0.0, 30.0, 3);
            }
        })
    });
}

/// The playable path: weather plus per-cell dust, rubber, debris and contact heat.
fn step_in_evolving_weather(c: &mut Criterion) {
    let track = Track::default_circuit();
    let mut car = Car::new(Arc::new(CarModel::gt3()), &track, 10.0, 0.0, 30.0, 3);
    let mut evolution = TrackEvolution::new(
        Arc::new(RubberMap::new(&track)),
        0.94,
        TrackEvolution::DEFAULT_GAIN_PER_LAP,
    );
    let mut weather = Weather::new(
        &track,
        None,
        WeatherSettings {
            sky: Sky::PartlyCloudy,
            ..Default::default()
        },
    );
    let controls = Controls {
        throttle: 0.5,
        steer_wheel_angle: 0.05,
        ..Default::default()
    };
    c.bench_function("car_step_in_evolving_weather", |b| {
        b.iter(|| {
            weather.step(DT);
            car.step_in(
                &track,
                &mut evolution,
                &weather,
                std::hint::black_box(&controls),
            );
            if car.state.time > 20.0 {
                car.reset(&track, 10.0, 0.0, 30.0, 3);
                evolution.reset(0.94);
            }
        })
    });
}

criterion_group!(benches, step, step_in_weather, step_in_evolving_weather);
criterion_main!(benches);
