//! Drives a lap on every track package in the content directory. Packages are converted
//! from user-supplied tracks and are not part of the repository, so this test is ignored
//! by default; run it with
//! `cargo test -p open-racing-api --test content_tracks -- --ignored`.

use std::sync::Arc;

use open_racing_api::{Car, CarModel, Controls, Track, load_track};

/// Follows the centreline at a steady speed and returns (distance, seconds off track).
fn drive(track: &Track, seconds: f64, speed: f64) -> (f64, f64) {
    let model = Arc::new(CarModel::gt3());
    let ratio = model.params.steering.ratio;
    let mut car = Car::new(model, track, 0.0, 0.0, speed, 2);
    let mut hint = track.nearest_index(car.state.position);
    let (mut s_prev, mut distance, mut off) = (track.query(car.state.position, hint).s, 0.0, 0.0);
    let steps = (seconds / open_racing_sim::DT) as usize;
    for k in 0..steps {
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

        let p = car.state.position;
        assert!(p.is_finite(), "state blew up at step {k}");
        let ground = track.pose_at(q.s, q.d).0;
        assert!(
            (p.z - ground.z).abs() < 2.0,
            "car left the surface at s = {:.0}: z {:.2} vs {:.2}",
            q.s,
            p.z,
            ground.z
        );
        if car.telemetry.wheels.iter().all(|w| w.surface.off_track()) {
            off += open_racing_sim::DT;
        }
    }
    (distance, off)
}

#[test]
#[ignore = "needs track packages in the content directory"]
fn laps_on_content_tracks() {
    let names = open_racing_track::list();
    assert!(
        !names.is_empty(),
        "no track packages in {}",
        open_racing_track::tracks_dir().display()
    );
    for name in names {
        let track = load_track(&name).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(track.ground.is_some());
        let (distance, off) = drive(&track, track.length / 20.0 + 10.0, 20.0);
        eprintln!(
            "{name}: {:.0} m lap, drove {distance:.0} m, {off:.1} s off track",
            track.length
        );
        assert!(
            distance > track.length,
            "{name}: did not complete a lap: {distance:.0} m of {:.0} m",
            track.length
        );
        assert!(off < 1.0, "{name}: {off:.1} s off track");
    }
}
