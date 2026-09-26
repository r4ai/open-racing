//! How long each step of building a project takes, as the editor rebuilds it on every
//! edit: `cargo run -p open-racing-track-project --example build_time -- <project>`.

use std::path::Path;
use std::time::Instant;

use open_racing_track_project::{BuildCache, Project, bake, inspect, overlap, road, terrain};

fn main() {
    let dir = std::env::args().nth(1).expect("a project directory");
    let p = Project::load(Path::new(&dir)).expect("the project loads");
    let time =
        |what: &str, t: Instant| println!("{what}: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
    // The first round warms up; the second is what an edit costs.
    let mut roads = Vec::new();
    for round in 0..2 {
        println!("round {}", round + 1);
        let t = Instant::now();
        roads = (0..p.roads.len()).map(|i| road::build(&p, i)).collect();
        time("  roads, one at a time", t);
        let t = Instant::now();
        overlap::resolve(&mut roads);
        time("  overlaps", t);
        let t = Instant::now();
        let _ = terrain::build(&p, &roads, None);
        time("  terrain", t);
        let t = Instant::now();
        let scene = bake::build(&p);
        time("  whole build", t);
        let t = Instant::now();
        let surfaces: Vec<_> = p.surfaces.iter().map(|s| s.props).collect();
        let _ = scene.ground.build(&surfaces);
        time("  physics ground", t);
        let t = Instant::now();
        let _ = inspect::issues(&p, &scene);
        time("  checks", t);
    }
    // An edit that leaves the roads and terrain alone (a kerb, a prop, a marker).
    let mut cache = BuildCache::default();
    let _ = bake::build_with(&p, &mut cache);
    let t = Instant::now();
    let _ = bake::build_with(&p, &mut cache);
    time("whole build, roads and terrain unchanged", t);
    // A brush stroke: the terrain's grid before sculpting is kept, the rest built again.
    let mut stroked = p.clone();
    let middle = (roads[0].sampled.frames[0].pos.truncate()
        + roads[0].sampled.frames[roads[0].sampled.frames.len() / 2]
            .pos
            .truncate())
        * 0.5;
    stroked
        .terrain
        .sculpt
        .push(open_racing_track_project::Stroke {
            brush: open_racing_track_project::Brush::Raise,
            radius: 30.0,
            strength: 2.0,
            points: vec![middle, middle + glam::DVec2::X * 60.0],
            fill: false,
            hardness: open_racing_track_project::project::HARDNESS,
        });
    let t = Instant::now();
    let _ = bake::build_with(&stroked, &mut cache);
    time("whole build after a brush stroke", t);
}
