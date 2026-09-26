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
    /// The test lap, when one was driven.
    pub drive: Option<Drive>,
    /// The race-pace lap round the racing line, when the test lap got round.
    pub pace: Option<Drive>,
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
#[derive(Clone, Debug)]
pub struct Drive {
    /// How long a whole lap took, s, if the car got round.
    pub lap_time: Option<f64>,
    /// Fastest the car went, m/s.
    pub top_speed: f64,
    /// Distance covered along the centreline, m.
    pub distance: f64,
    /// Time with every wheel off the track, s.
    pub off_track: f64,
    /// Where the car left the ground (s, m), if it did.
    pub fell_off: Option<f64>,
    /// Where the car was, twenty times a second, to replay the lap.
    pub path: Vec<LapSample>,
}

/// The car at a moment of a test lap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LapSample {
    /// Seconds from the start.
    pub t: f64,
    pub pos: glam::DVec3,
    /// Which way it points: forward, level.
    pub heading: f64,
    /// m/s.
    pub speed: f64,
    /// Every wheel off the track.
    pub off: bool,
}

/// Whether a car at `p` has left the ground: more than 2 m above what is under it (the
/// ground mesh, or without one the road's surface carried on sideways), or nothing is.
fn airborne(track: &Track, p: glam::DVec3, s: f64, d: f64) -> bool {
    let under = match &track.ground {
        Some(g) => match g.raycast_down(p + glam::DVec3::Z * 2.0, 50.0) {
            Some(h) => h.point.z,
            None => return true,
        },
        None => track.pose_at(s, d).0.z,
    };
    (p.z - under).abs() > 2.0
}

/// Steps of the simulation between samples of the path.
const SAMPLE_EVERY: usize = 50;

/// Follows the centreline in a GT3 car at a steady `speed` for `seconds`.
pub fn drive(track: &Track, seconds: f64, speed: f64) -> Drive {
    let model = Arc::new(CarModel::gt3());
    let ratio = model.params.steering.ratio;
    let start = track.layout.start();
    let mut car = Car::new(model, track, start.s, 0.0, speed, 2);
    let mut hint = track.nearest_index(car.state.position);
    let mut s_prev = track.query(car.state.position, hint).s;
    let mut out = Drive {
        lap_time: None,
        top_speed: 0.0,
        distance: 0.0,
        off_track: 0.0,
        fell_off: None,
        path: Vec::new(),
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
        if !p.is_finite() || airborne(track, p, q.s, q.d) {
            out.fell_off = Some(q.s);
            break;
        }
        let off = car.telemetry.wheels.iter().all(|w| w.surface.off_track());
        if off {
            out.off_track += open_racing_sim::DT;
        }
        if k % SAMPLE_EVERY == 0 {
            let forward = car.state.orientation * glam::DVec3::X;
            out.path.push(LapSample {
                t: k as f64 * open_racing_sim::DT,
                pos: p,
                heading: forward.y.atan2(forward.x),
                speed: car.speed(),
                off,
            });
        }
    }
    out
}

/// Share of the racing line's speeds the race-pace lap is driven at: near a GT3's
/// limit, with a little in hand for a simple driver.
pub const PACE: f64 = 0.85;

/// Drives a GT3 round the racing line from the start line at `pace` of the speeds it
/// allows, for a lap: the lap time, where it went off or left the ground.
pub fn drive_line(track: &Track, line: &open_racing_sim::RacingLine, pace: f64) -> Drive {
    let model = Arc::new(CarModel::gt3());
    let ratio = model.params.steering.ratio;
    let start = track.layout.start();
    let speed_at = |s: f64| line.at(track, s).1 * pace;
    let mut car = Car::new(
        model,
        track,
        start.s,
        start.d,
        speed_at(start.s).min(20.0),
        2,
    );
    let mut hint = track.nearest_index(car.state.position);
    let mut s_prev = track.query(car.state.position, hint).s;
    let mut out = Drive {
        lap_time: None,
        top_speed: 0.0,
        distance: 0.0,
        off_track: 0.0,
        fell_off: None,
        path: Vec::new(),
    };
    let seconds = track.length / 15.0 + 30.0;
    let steps = (seconds / open_racing_sim::DT) as usize;
    let mut clutch = open_racing_sim::ClutchAssist::default();
    for k in 0..steps {
        let q = track.query(car.state.position, hint);
        hint = q.index;
        out.distance += track.delta_s(s_prev, q.s);
        s_prev = q.s;
        let t = k as f64 * open_racing_sim::DT;
        if out.distance >= track.length {
            out.lap_time = Some(t);
            break;
        }
        let v = car.speed();
        out.top_speed = out.top_speed.max(v);
        // Stanley's steering: the line's own curve, the car's heading against the
        // line's, and back towards it by how far off it the front axle is.
        let forward = car.state.orientation * glam::DVec3::X;
        let front = car.state.position + forward * 0.5 * WHEELBASE;
        let qf = track.query(front, hint);
        let on = |s: f64| track.pose_at(s, line.at(track, s).0).0;
        let (a, b, c) = (on(qf.s - 5.0), on(qf.s), on(qf.s + 5.0));
        let tangent = (c - a).truncate().normalize_or(glam::DVec2::X);
        let (u, w) = ((b - a).truncate(), (c - b).truncate());
        let curvature = 2.0 * u.perp_dot(w).atan2(u.dot(w)) / (u.length() + w.length()).max(1e-6);
        let heading = forward.truncate().normalize_or(glam::DVec2::X);
        let heading_error = heading.angle_to(tangent);
        let off_line = (front - b).truncate().perp_dot(tangent);
        let wheel = (WHEELBASE * curvature).atan()
            + heading_error
            + (STANLEY_GAIN * off_line / (v + 1.0)).atan();
        // Brakes for what is coming: the slowest the line allows in the next second.
        let target = (0..=10)
            .map(|i| speed_at(q.s + v * 0.1 * i as f64))
            .fold(f64::INFINITY, f64::min);
        let mut controls = Controls {
            steer_wheel_angle: wheel * ratio,
            throttle: ((target - v) * 0.5).clamp(0.0, 1.0),
            brake: ((v - target) * 0.4).clamp(0.0, 1.0),
            shift: open_racing_sim::AutoShift.shift(&car),
            ..Default::default()
        };
        clutch.apply(&car, &mut controls);
        car.step(track, &controls);

        let p = car.state.position;
        if !p.is_finite() || airborne(track, p, q.s, q.d) {
            out.fell_off = Some(q.s);
            break;
        }
        let off = car.telemetry.wheels.iter().all(|w| w.surface.off_track());
        if off {
            out.off_track += open_racing_sim::DT;
        }
        if k % SAMPLE_EVERY == 0 {
            let forward = car.state.orientation * glam::DVec3::X;
            out.path.push(LapSample {
                t,
                pos: p,
                heading: forward.y.atan2(forward.x),
                speed: v,
                off,
            });
        }
    }
    out
}

/// A GT3's wheelbase, m, for steering along a line.
const WHEELBASE: f64 = 2.7;
/// How hard the race-pace driver steers back towards the line, per m off it.
const STANLEY_GAIN: f64 = 2.5;

/// Minutes and seconds.
pub fn lap_time(t: f64) -> String {
    let m = (t / 60.0).floor();
    format!("{m:.0}:{:06.3}", t - 60.0 * m)
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
        let drove = r.errors.is_empty();
        r.drive = Some(d);
        // The track drives: now at race pace round the racing line, which finds crests
        // that launch a car, kerbs that throw it and corners it cannot make.
        if drove {
            let line = open_racing_sim::RacingLine::new(&track);
            let d = drive_line(&track, &line, PACE);
            let time = d.lap_time.map_or("no lap".into(), lap_time);
            r.lines.push(format!(
                "race-pace lap ({:.0} % of a GT3 on the racing line): {time}, top {:.0} km/h, {:.1} s off track",
                PACE * 100.0,
                d.top_speed * 3.6,
                d.off_track
            ));
            if let Some(s) = d.fell_off {
                r.lines.push(format!(
                    "note: at race pace the car left the ground at s = {s:.0} m: a crest or kerb too sharp?"
                ));
            } else if d.lap_time.is_none() {
                r.lines
                    .push("note: at race pace the car did not get round".into());
            }
            r.pace = Some(d);
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_race_pace_lap_gets_round_the_oval() {
        let p = crate::Project::new("pace");
        let mut cache = crate::Cache::default();
        let package = crate::bake(&p, std::path::Path::new("."), &mut cache).unwrap();
        let track = package.build_track().unwrap();
        let line = open_racing_sim::RacingLine::new(&track);
        let d = drive_line(&track, &line, PACE);
        println!(
            "lap {:?}, top {:.0} km/h, off {:.1} s, fell {:?}",
            d.lap_time.map(lap_time),
            d.top_speed * 3.6,
            d.off_track,
            d.fell_off
        );
        assert!(d.fell_off.is_none());
        assert!(d.lap_time.is_some(), "got round");
        assert!(d.off_track < 1.0, "{:.1} s off", d.off_track);
        assert!(
            d.top_speed > 40.0,
            "at speed: {:.0} km/h",
            d.top_speed * 3.6
        );
    }
}
