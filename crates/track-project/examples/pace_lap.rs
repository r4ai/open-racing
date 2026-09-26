//! A race-pace lap round a project's racing line, second by second: where the car is,
//! how fast against the line's speed, and when it is off the track or in the air.
//! `cargo run -p open-racing-track-project --example pace_lap -- <project>`.

use std::path::Path;

use open_racing_track_project::validate::{PACE, drive_line, lap_time};
use open_racing_track_project::{Cache, Project, bake};

fn main() {
    let dir = std::env::args().nth(1).expect("a project directory");
    let dir = Path::new(&dir);
    let p = Project::load(dir).expect("the project loads");
    let package = bake(&p, dir, &mut Cache::default()).expect("it bakes");
    let track = package.build_track().expect("it builds");
    let line = open_racing_sim::RacingLine::new(&track);
    let d = drive_line(&track, &line, PACE);
    let mut last = -1.0;
    for sample in &d.path {
        if sample.t - last < 1.0 {
            continue;
        }
        last = sample.t;
        let q = track.query(sample.pos, 0);
        let (offset, speed) = line.at(&track, q.s);
        println!(
            "{:5.1} s  s {:6.0} m  d {:6.1} m (line {:5.1})  {:4.0} km/h (line {:4.0}){}",
            sample.t,
            q.s,
            q.d,
            offset,
            sample.speed * 3.6,
            speed * PACE * 3.6,
            if sample.off { "  OFF" } else { "" }
        );
    }
    println!(
        "lap {}, top {:.0} km/h, {:.1} s off track{}",
        d.lap_time.map_or("none".into(), lap_time),
        d.top_speed * 3.6,
        d.off_track,
        d.fell_off.map_or(String::new(), |s| format!(
            ", left the ground at s = {s:.0} m"
        ))
    );
}
