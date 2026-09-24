//! Loads every car package in the content directory and drives it around the default
//! circuit. Packages are converted from user-supplied cars and are not part of the
//! repository, so this test is ignored by default; run it with
//! `cargo test -p open-racing-api --test content_cars -- --ignored`.

use std::sync::Arc;

use open_racing_api::{Car, Controls, Track, load_car_with_visual};

/// Follows the centreline at a steady speed and returns the distance driven.
fn drive(car: &mut Car, track: &Track, seconds: f64, speed: f64) -> f64 {
    let ratio = car.model.params.steering.ratio;
    let mut hint = track.nearest_index(car.state.position);
    let (mut s_prev, mut distance) = (track.query(car.state.position, hint).s, 0.0);
    for k in 0..(seconds / open_racing_sim::DT) as usize {
        let q = track.query(car.state.position, hint);
        hint = q.index;
        if k % 10 == 0 {
            distance += track.delta_s(s_prev, q.s);
            s_prev = q.s;
        }
        let target = track.sample_at(q.s + 12.0).pos;
        let local = car.state.orientation.inverse() * (target - car.state.position);
        let v = car.speed();
        let controls = Controls {
            steer_wheel_angle: local.y.atan2(local.x) * ratio * 1.5,
            throttle: ((speed - v) * 0.3).clamp(0.0, 1.0),
            brake: ((v - speed - 2.0) * 0.2).clamp(0.0, 1.0),
            ..Default::default()
        };
        car.step(track, &controls);
        assert!(car.state.position.is_finite(), "state blew up at step {k}");
    }
    distance
}

#[test]
#[ignore = "needs car packages in the content directory"]
fn content_cars_drive() {
    let names = open_racing_car::list();
    assert!(
        !names.is_empty(),
        "no car packages in {}",
        open_racing_car::cars_dir().display()
    );
    let track = Track::default_circuit();
    for name in names {
        let (model, visual) = load_car_with_visual(&name).unwrap_or_else(|e| panic!("{name}: {e}"));
        if let Some(v) = &visual {
            assert_eq!(v.mesh_parts.len(), v.visual.meshes.len());
        }
        let mut car = Car::new(Arc::new(model), &track, 0.0, 0.0, 20.0, 2);
        let distance = drive(&mut car, &track, 60.0, 20.0);
        eprintln!(
            "{name}: drove {distance:.0} m in 60 s, model: {}",
            visual.map_or("none".into(), |v| format!(
                "{} batches",
                v.visual.meshes.len()
            ))
        );
        assert!(distance > 1000.0, "{name}: only {distance:.0} m");
    }
}
