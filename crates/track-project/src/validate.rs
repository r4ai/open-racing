//! Checks that a track package drives: the centreline runs over the road meshes, the
//! road's edges are track, and a car following the centreline completes a lap.

use std::sync::Arc;

use open_racing_sim::{Car, CarModel, Controls, Surface, Track};
use open_racing_track::TrackPackage;

/// Findings about a package, as lines of text.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub lines: Vec<String>,
    /// Problems that make the track unfit to drive.
    pub errors: Vec<String>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for l in &self.lines {
            writeln!(f, "{l}")?;
        }
        for e in &self.errors {
            writeln!(f, "error: {e}")?;
        }
        Ok(())
    }
}

/// Result of driving round a track.
#[derive(Clone, Copy, Debug)]
pub struct Drive {
    /// Distance covered along the centreline, m.
    pub distance: f64,
    /// Time with every wheel off the track, s.
    pub off_track: f64,
    /// Where the car left the ground (s, m), if it did.
    pub fell_off: Option<f64>,
}

/// Follows the centreline in a GT3 car at a steady `speed` for `seconds`.
pub fn drive(track: &Track, seconds: f64, speed: f64) -> Drive {
    let model = Arc::new(CarModel::gt3());
    let ratio = model.params.steering.ratio;
    let start = track.layout.start();
    let mut car = Car::new(model, track, start.s, 0.0, speed, 2);
    let mut hint = track.nearest_index(car.state.position);
    let mut s_prev = track.query(car.state.position, hint).s;
    let mut out = Drive {
        distance: 0.0,
        off_track: 0.0,
        fell_off: None,
    };
    let steps = (seconds / open_racing_sim::DT) as usize;
    for k in 0..steps {
        let q = track.query(car.state.position, hint);
        hint = q.index;
        if k % 10 == 0 {
            out.distance += track.delta_s(s_prev, q.s);
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
        let ground = track.pose_at(q.s, q.d).0;
        if !p.is_finite() || (p.z - ground.z).abs() > 2.0 {
            out.fell_off = Some(q.s);
            break;
        }
        if car.telemetry.wheels.iter().all(|w| w.surface.off_track()) {
            out.off_track += open_racing_sim::DT;
        }
    }
    out
}

/// Checks a package; `lap` also drives a lap, which takes a few seconds.
pub fn check(package: &TrackPackage, lap: bool) -> Report {
    let mut r = Report::default();
    let track = match package.build_track() {
        Ok(t) => t,
        Err(e) => {
            r.errors.push(e.to_string());
            return r;
        }
    };
    let ground = track.ground.as_ref().expect("packages have ground");
    r.lines.push(format!(
        "length {:.0} m, {} sectors, {} grid slots{}",
        track.length,
        track.layout.sectors.len() + 1,
        track.layout.grid.len(),
        track
            .layout
            .pit
            .as_ref()
            .map_or(String::new(), |p| format!(", {} pit boxes", p.boxes.len()))
    ));

    let (mut n, mut covered, mut edges_ok, mut dz) = (0, 0, 0, 0.0f64);
    for smp in track.samples.iter().step_by(5) {
        n += 1;
        if let Some(h) = ground.raycast_down(smp.pos, 3.0) {
            covered += 1;
            dz = dz.max((h.point.z - smp.pos.z).abs());
        }
        // Just inside each edge the surface counts as track.
        let on_track = |d: f64| {
            let p = smp.pos + smp.lateral * d + smp.normal * 0.5;
            ground
                .raycast_down(p, 3.0)
                .is_some_and(|h| !h.surface.kind.off_track())
        };
        if on_track(smp.width_left - 0.3) && on_track(0.3 - smp.width_right) {
            edges_ok += 1;
        }
    }
    let share = |k: usize| 100.0 * k as f64 / n.max(1) as f64;
    r.lines.push(format!(
        "centreline over the road {:.0} %, {:.2} m from it at most; edges on track {:.0} %",
        share(covered),
        dz,
        share(edges_ok)
    ));
    if covered < n {
        r.errors
            .push("the centreline leaves the road meshes somewhere".into());
    }
    let start = track.layout.start();
    let q = track.query(track.pose_at(start.s, start.d).0, 0);
    if q.surface != Surface::Asphalt {
        r.errors
            .push(format!("the pole slot is on {:?}, not asphalt", q.surface));
    }

    if lap {
        // Slow enough for the tightest hairpins.
        let seconds = track.length / 15.0 + 20.0;
        let d = drive(&track, seconds, 15.0);
        r.lines.push(format!(
            "test lap at 54 km/h: {:.0} m covered, {:.1} s off track",
            d.distance, d.off_track
        ));
        if let Some(s) = d.fell_off {
            r.errors
                .push(format!("the car left the ground at s = {s:.0} m"));
        } else if d.distance < track.length {
            r.errors.push("the test car did not complete a lap".into());
        }
        if d.off_track > 1.0 {
            r.errors.push(format!(
                "the test car was off track for {:.1} s",
                d.off_track
            ));
        }
    }
    r
}
