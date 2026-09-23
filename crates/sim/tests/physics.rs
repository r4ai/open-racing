//! Plausibility tests of the vehicle dynamics against GT3-class reference figures.

use std::sync::Arc;

use open_racing_sim::*;

fn circle(radius: f64) -> Track {
    let points = (0..32)
        .map(|i| {
            let a = i as f64 / 32.0 * std::f64::consts::TAU;
            TrackPoint { pos: (radius * a.cos(), radius * a.sin(), 0.0), width_left: 30.0, width_right: 30.0, bank: 0.0 }
        })
        .collect();
    Track::new(&TrackDef { name: "circle".into(), points, kerb_width: 1.0, kerb_height: 0.0, spacing: 1.0 }).unwrap()
}

fn gt3() -> Arc<CarModel> {
    Arc::new(CarModel::gt3())
}

fn shift_for(car: &Car) -> Shift {
    let dt = &car.state.drivetrain;
    if dt.rpm() > 8900.0 && dt.gear < 6 && dt.shift_timer == 0.0 { Shift::Up } else { Shift::None }
}

#[test]
fn settles_at_static_ride_height() {
    let track = circle(5000.0);
    let mut car = Car::new(gt3(), &track, 0.0, 0.0, 0.0, 0);
    let z0 = car.state.position.z;
    for _ in 0..3000 {
        car.step(&track, &Controls { brake: 0.3, ..Default::default() });
    }
    assert!(car.speed() < 0.01, "speed {}", car.speed());
    assert!((car.state.position.z - z0).abs() < 0.005, "z drift {}", car.state.position.z - z0);
    let total: f64 = car.telemetry.wheels.iter().map(|w| w.load).sum();
    let weight = car.model.params.mass * GRAVITY;
    assert!((total - weight).abs() / weight < 0.01, "loads {total} vs {weight}");
    let front = car.telemetry.wheels[FL].load + car.telemetry.wheels[FR].load;
    assert!((front / total - car.model.params.front_weight).abs() < 0.01);
}

#[test]
fn acceleration_and_top_speed() {
    let track = circle(5000.0);
    let mut car = Car::new(gt3(), &track, 0.0, 0.0, 0.0, 1);
    let mut t100 = None;
    for _ in 0..70_000 {
        let shift = shift_for(&car);
        car.step(&track, &Controls { throttle: 1.0, shift, ..Default::default() });
        if t100.is_none() && car.speed() >= 100.0 / 3.6 {
            t100 = Some(car.state.time);
        }
    }
    let t100 = t100.expect("reaches 100 km/h");
    let top = car.speed() * 3.6;
    // GT3: 0-100 in roughly 3-4 s (traction limited), top speed ~270-290 km/h.
    assert!((2.8..4.2).contains(&t100), "0-100 km/h in {t100:.2} s");
    assert!((255.0..300.0).contains(&top), "top speed {top:.1} km/h");
}

#[test]
fn braking_distance_from_100() {
    let track = circle(5000.0);
    let mut car = Car::new(gt3(), &track, 0.0, 0.0, 100.0 / 3.6, 3);
    let start = car.state.position;
    // Threshold braking without ABS: find the best pedal pressure.
    let best = (50..=100)
        .step_by(5)
        .map(|b| {
            car.reset(&track, 0.0, 0.0, 100.0 / 3.6, 3);
            while car.speed() > 0.2 && car.state.time < 10.0 {
                car.step(&track, &Controls { brake: b as f64 / 100.0, clutch: 1.0, ..Default::default() });
            }
            (car.state.position - start).length()
        })
        .fold(f64::INFINITY, f64::min);
    // Slick-shod GT3 cars stop from 100 km/h in roughly 25-35 m.
    assert!((22.0..36.0).contains(&best), "100-0 km/h in {best:.1} m");
}

/// Drives a constant-radius circle with a simple path/speed controller and returns the
/// highest lateral acceleration sustained for 3 s while staying on the line.
fn skidpad(radius: f64) -> f64 {
    let track = circle(radius);
    let mut car = Car::new(gt3(), &track, 0.0, 0.0, 10.0, 2);
    let mut target = 10.0_f64;
    let mut best = 0.0_f64;
    let mut stable_since = 0.0;
    let mut hint = 0;
    let ratio = car.model.params.steering.ratio;
    let wheelbase = car.model.params.wheelbase;
    while car.state.time < 120.0 && target < 80.0 {
        let q = track.query(car.state.position, hint);
        hint = q.index;
        let heading_err = {
            let fwd = car.state.orientation * glam::DVec3::X;
            q.tangent.truncate().perp_dot(fwd.truncate()).asin()
        };
        let feedforward = (wheelbase / radius).atan();
        let steer = (feedforward - 0.15 * q.d - 1.5 * heading_err - 0.5 * car.local_velocity().y / car.speed().max(1.0)) * ratio;
        let v = car.local_velocity().x;
        let throttle = (0.3 * (target - v) + 0.25).clamp(0.0, 1.0);
        let shift = shift_for(&car);
        car.step(&track, &Controls { steer_wheel_angle: steer, throttle, shift, ..Default::default() });

        let ay = v * v / radius;
        if q.d.abs() < 1.0 && (v - target).abs() < 0.5 {
            if car.state.time - stable_since > 3.0 {
                best = best.max(ay);
                target += 0.5;
                stable_since = car.state.time;
            }
        } else if q.d.abs() >= 1.0 {
            stable_since = car.state.time;
            if q.d.abs() > 8.0 {
                break;
            }
        }
    }
    best / GRAVITY
}

#[test]
fn skidpad_lateral_grip() {
    let g = skidpad(60.0);
    eprintln!("skidpad: {g:.3} g");
    // Mechanical grip of a GT3 car on slicks at low speed: ~1.3-1.7 g.
    assert!((1.25..1.8).contains(&g), "max lateral {g:.2} g");
}

#[test]
fn deterministic() {
    let track = Track::default_circuit();
    let run = || {
        let mut car = Car::new(gt3(), &track, 50.0, 1.0, 20.0, 2);
        for k in 0..20_000 {
            let steer = (k as f64 * 0.001).sin() * 0.8;
            car.step(&track, &Controls { steer_wheel_angle: steer, throttle: 0.6, ..Default::default() });
        }
        car.state
    };
    let (a, b) = (run(), run());
    assert_eq!(format!("{a:?}"), format!("{b:?}"));
}
