//! A real circuit made the way the editor and `open-racing-trackctl` make one: a road
//! laid along a noisy GPS lap (`fixtures/circuit.gpx`, 5.7 km), a pit lane beside its
//! start, kerbs, gravel and tyre walls round every corner. It must drive (no warnings)
//! and bake, and what is laid round the corners must stay with them as the road
//! changes. Also prints how long each step takes on a track this long
//! (`cargo test -p open-racing-track-project --test real_circuit -- --nocapture`).

use std::time::Instant;

use glam::DVec3;
use open_racing_track_project::corners::{self, Kit};
use open_racing_track_project::ops::{Op, apply_all};
use open_racing_track_project::project::CornerPart;
use open_racing_track_project::{Cache, Project, bake, centreline, inspect, pitlane};

fn timed<T>(what: &str, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let v = f();
    println!("{what}: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
    v
}

fn circuit() -> Project {
    let src = include_str!("fixtures/circuit.gpx");
    let line = centreline::read("circuit.gpx", src).unwrap();
    assert!(line.closed && line.origin.is_some());
    let mut p = Project::new("gps");
    let mut ops = centreline::road_ops(&p, &line, "gp", 1.0);
    ops.push(Op::SetMainRoad { road: "gp".into() });
    ops.push(Op::RemoveRoad {
        road: "circuit".into(),
    });
    // The main road changes first, then the oval can go.
    let (first, last) = ops.split_at(ops.len() - 1);
    apply_all(&mut p, first).unwrap();
    apply_all(&mut p, last).unwrap();
    p
}

#[test]
fn a_gps_lap_becomes_a_circuit_with_a_pit_lane_and_kerbed_corners() {
    let mut p = timed("lay the road", circuit);
    let (smp, found) = timed("find corners", || corners::of_road(&p, 0));
    assert!(
        (5400.0..6000.0).contains(&smp.length),
        "lap {:.0} m",
        smp.length
    );
    println!(
        "{:.0} m, {} nodes, {} corners",
        smp.length,
        p.roads[0].nodes.len(),
        found.len()
    );
    assert!((8..=20).contains(&found.len()), "{} corners", found.len());
    assert!(found.iter().all(|c| c.radius > 15.0), "{found:?}");

    let plan = pitlane::Plan::around_start(&p);
    let ops = pitlane::ops(&p, "pit", &plan).unwrap();
    apply_all(&mut p, &ops).unwrap();

    let kit = Kit {
        outside: Some(("gravel".into(), 12.0)),
        wall: Some(("tyre wall".into(), 25.0)),
        ..Kit::kerbs(&p, None, 1.5)
    };
    let ops: Vec<Op> = found
        .iter()
        .flat_map(|c| corners::kit_ops(&p, "gp", &smp, &found, c, &kit))
        .collect();
    timed("kerb every corner", || apply_all(&mut p, &ops)).unwrap();
    let road = p.road("gp").unwrap();
    let laid = road
        .left
        .iter()
        .chain(&road.right)
        .filter(|s| s.corner.is_some())
        .count();
    assert_eq!(laid, 4 * found.len());

    let scene = timed("build", || bake::build(&p));
    let issues = timed("check", || inspect::issues(&p, &scene));
    assert!(issues.is_empty(), "{issues:#?}");
    let dir = std::env::temp_dir().join(format!("open-racing-gps-{}", std::process::id()));
    let package = timed("bake", || bake::bake(&p, &dir, &mut Cache::default())).unwrap();
    assert!(package.visual.is_some());
    let _ = std::fs::remove_dir_all(&dir);

    // Moving a corner's node outwards, as dragging one does: every corner keeps its
    // kit, refitted.
    let c = &found[found.len() / 2];
    let f = smp.frame_at(c.apex);
    let nodes = &p.road("gp").unwrap().nodes;
    let n = (0..nodes.len())
        .min_by(|&a, &b| {
            let d = |i: usize| nodes[i].pos.distance(f.pos);
            d(a).total_cmp(&d(b))
        })
        .unwrap();
    let outwards = -f.lateral * c.dir.sign();
    let pos = nodes[n].pos + DVec3::new(outwards.x, outwards.y, 0.0) * 6.0;
    let op = [Op::MoveNode {
        line: "gp".into(),
        index: n,
        pos,
    }];
    timed("move a node (with refitting)", || apply_all(&mut p, &op)).unwrap();
    let (smp, after) = corners::of_road(&p, 0);
    assert_eq!(after.len(), found.len());
    let road = p.road("gp").unwrap();
    for c in &after {
        let has = Kit::of(road, &smp, &after, c);
        assert_eq!(has, kit, "T{}", c.number);
    }
    // Every apex kerb round its apex, on the inside.
    for s in road.left.iter().chain(&road.right) {
        let Some(a) = s.corner.filter(|a| a.part == CornerPart::Apex) else {
            continue;
        };
        let c = corners::owner(&smp, &after, &a).expect("its corner");
        let (from, mut to) = (smp.s_at(s.ranges[0].from), smp.s_at(s.ranges[0].to));
        if to < from {
            to += smp.length;
        }
        let apex = if c.apex < from {
            c.apex + smp.length
        } else {
            c.apex
        };
        assert!(
            from < apex && apex < to,
            "{}: {from}..{to}, apex {apex}",
            s.name
        );
        let inside = if c.dir == open_racing_track_project::Side::Left {
            &road.left
        } else {
            &road.right
        };
        assert!(inside.iter().any(|t| t.name == s.name), "{} inside", s.name);
    }
    let mut sampled = 0.0;
    timed("sample the road 10 times", || {
        for _ in 0..10 {
            sampled += open_racing_track_project::curve::Sampled::new(road, road.resolution).length;
        }
    });
    assert!(sampled > 0.0);
}
