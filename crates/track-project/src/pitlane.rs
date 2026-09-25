//! Laying out a pit lane: a road beside a stretch of the main road that leaves it,
//! runs parallel to it past the pit boxes, and joins it again.

use glam::DVec3;

use crate::Error;
use crate::curve::Sampled;
use crate::ops::{Curve, Op};
use crate::project::{Barrier, Key, Mark, PaintLine, Pit, Project, Range, Side};

/// What pit lane to lay.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// Where it leaves and rejoins the main road, as its spline parameters; on a closed
    /// road `from > to` runs across the start line.
    pub from: f64,
    pub to: f64,
    /// Which side of the main road it runs on.
    pub side: Side,
    /// Gap between the main road's edge and the lane's, m.
    pub gap: f64,
    /// The lane's width, m.
    pub width: f64,
    /// Pit boxes along the lane's straight part.
    pub boxes: usize,
    /// Speed limit, m/s.
    pub speed_limit: f64,
}

impl Plan {
    /// A lane along the stretch round the start line, as most circuits have it.
    pub fn around_start(project: &Project) -> Self {
        let period = project.road(&project.main_road).map_or(1.0, |r| r.period());
        let start = project.markers.start;
        Self {
            from: (start - 0.12 * period).rem_euclid(period),
            to: (start + 0.08 * period).rem_euclid(period),
            side: Side::Right,
            gap: 8.0,
            width: 10.0,
            boxes: 12,
            speed_limit: 80.0 / 3.6,
        }
    }
}

/// Spacing of the lane's nodes, m.
const SPACING: f64 = 25.0;
/// Longest the lane takes to move out from the track or back in, m.
const MERGE: f64 = 160.0;

/// The operations that add pit lane road `name` as `plan` says and make it the pit
/// lane, with its boxes.
pub fn ops(project: &Project, name: &str, plan: &Plan) -> Result<Vec<Op>, Error> {
    let main = project
        .road(&project.main_road)
        .ok_or_else(|| Error::Invalid("no main road".into()))?;
    let smp = Sampled::new(main, 2.0);
    let (a, mut b) = (smp.s_at(plan.from), smp.s_at(plan.to));
    if b <= a {
        b += smp.length;
    }
    let length = b - a;
    if length < 250.0 || length > 0.9 * smp.length {
        return Err(Error::Invalid(format!(
            "a pit lane of {length:.0} m: make it 250 m or longer, and shorter than the lap"
        )));
    }
    let merge = MERGE.min(length / 3.0);
    let half = 0.5 * plan.width;
    let count = (length / SPACING).ceil().max(4.0) as usize;
    let nodes: Vec<DVec3> = (0..=count)
        .map(|k| {
            let s = a + length * k as f64 / count as f64;
            let f = smp.frame_at(s);
            let edge = match plan.side {
                Side::Left => f.width_left,
                Side::Right => f.width_right,
            };
            // From just inside the track's edge out to beside it, eased both ways.
            let t = ((s - a).min(b - s) / merge).clamp(0.0, 1.0);
            let ease = t * t * (3.0 - 2.0 * t);
            let inside = (edge - half).max(0.0);
            let out = inside + (edge + plan.gap + half - inside) * ease;
            let left = DVec3::Z.cross(f.tangent).normalize_or(DVec3::Y);
            f.pos + left * (plan.side.sign() * out)
        })
        .collect();
    let lane_period = count as f64;
    // Boxes along the straight part, clear of where it merges.
    let (first, last) = (
        merge / length * lane_period,
        (1.0 - merge / length) * lane_period,
    );
    let boxes = (0..plan.boxes)
        .map(|i| first + (last - first) * (i as f64 + 0.5) / plan.boxes.max(1) as f64)
        .collect();
    let width = |curve| Op::SetProfile {
        road: name.to_string(),
        curve,
        keys: vec![Key::new(0.0, half)],
    };
    // Walls on that side that would cross the lane stop short of it, and a pit wall
    // runs between the track and the lane where they are apart.
    let u = |s: f64| smp.u_at(s.rem_euclid(smp.length));
    let reach = plan.gap + plan.width + 2.0;
    let mut walls: Vec<Op> = main
        .barriers
        .iter()
        .filter(|b| b.side == plan.side && b.ranges.is_empty() && b.offset < reach)
        .map(|b| Op::PutBarrier {
            road: main.name.clone(),
            barrier: Barrier {
                ranges: vec![Range {
                    from: plan.to,
                    to: plan.from,
                }],
                ..b.clone()
            },
        })
        .collect();
    let wall_name = (1..)
        .map(|k| {
            if k == 1 {
                "pit wall".to_string()
            } else {
                format!("pit wall {k}")
            }
        })
        .find(|n| main.barriers.iter().all(|b| &b.name != n))
        .expect("some name is free");
    walls.push(Op::PutBarrier {
        road: main.name.clone(),
        barrier: Barrier {
            name: wall_name,
            side: plan.side,
            offset: 0.5 * plan.gap,
            height: 1.0,
            thickness: 0.5,
            material: wall_material(project),
            ranges: vec![Range {
                from: u(a + merge),
                to: u(b - merge),
            }],
            model: None,
            style: project
                .wall_style("concrete wall")
                .map(|w| w.name.clone())
                .filter(|_| wall_material(project) == "concrete"),
            corner: None,
        },
    });
    // White lines along both edges, and across the lane where the speed limit starts
    // and ends: where it has moved out beside the track and before it moves back.
    let paint = paint_material(project);
    let edge = |name: &str, offset: f64| Op::PutLine {
        road: name.to_string(),
        line: PaintLine {
            name: format!("{} edge", if offset > 0.0 { "left" } else { "right" }),
            offset,
            width: 0.15,
            material: paint.clone(),
            ranges: vec![],
            dash: None,
        },
    };
    let limit = |mark: &str, at: f64| Op::PutMark {
        road: name.to_string(),
        mark: Mark {
            name: mark.to_string(),
            at,
            length: 0.4,
            from: -half,
            to: half,
            material: paint.clone(),
        },
    };
    let lines = vec![
        edge(name, half - 0.25),
        edge(name, -(half - 0.25)),
        limit("speed limit", first),
        limit("speed limit end", last),
    ];
    Ok(vec![
        Op::AddRoad {
            name: name.to_string(),
            closed: false,
            nodes,
            like: None,
        },
        width(Curve::Width),
        Op::SetPit {
            pit: Some(Pit {
                road: name.to_string(),
                speed_limit: plan.speed_limit,
                boxes,
                // The garages are on the far side from the track.
                box_side: plan.side,
                box_offset: (half - 2.0).max(0.0),
            }),
        },
    ]
    .into_iter()
    .chain(lines)
    .chain(walls)
    .collect())
}

/// White paint, or else the project's first material.
fn paint_material(project: &Project) -> String {
    project
        .material_index("paint")
        .or((!project.materials.is_empty()).then_some(0))
        .map_or("paint".into(), |i| project.materials[i].name.clone())
}

/// The operations that paint the start/finish line across the main road and a line
/// at the front of each grid slot, in place of those painted before.
pub fn start_and_grid(project: &Project) -> Vec<Op> {
    let Some(main) = project.road(&project.main_road) else {
        return vec![];
    };
    let smp = Sampled::new(main, main.resolution);
    let m = &project.markers;
    let start = smp.s_at(m.start);
    let f = smp.frame_at(start);
    let paint = paint_material(project);
    let road = main.name.clone();
    let mut ops: Vec<Op> = main
        .marks
        .iter()
        .filter(|k| k.name == "start line" || k.name.starts_with("grid "))
        .map(|k| Op::RemoveMark {
            road: road.clone(),
            name: k.name.clone(),
        })
        .collect();
    ops.push(Op::PutMark {
        road: road.clone(),
        mark: Mark {
            name: "start line".into(),
            at: m.start,
            length: 0.8,
            from: -f.width_right,
            to: f.width_left,
            material: paint.clone(),
        },
    });
    // Each slot's line a car's half length ahead of where it stands.
    for i in 0..m.grid.count {
        let side = if i % 2 == 0 { 1.0 } else { -1.0 };
        let s = start - (m.grid.behind + i as f64 * m.grid.spacing) + 2.5;
        let d = m.grid.pole.sign() * side * m.grid.stagger;
        let at = if smp.closed {
            smp.u_at(s.rem_euclid(smp.length))
        } else {
            smp.u_at(s.max(0.0))
        };
        ops.push(Op::PutMark {
            road: road.clone(),
            mark: Mark {
                name: format!("grid {}", i + 1),
                at,
                length: 0.2,
                from: d - 1.2,
                to: d + 1.2,
                material: paint.clone(),
            },
        });
    }
    ops
}

/// Concrete for the pit wall, or else the project's first material.
fn wall_material(project: &Project) -> String {
    project
        .material_index("concrete")
        .or((!project.materials.is_empty()).then_some(0))
        .map_or("concrete".into(), |i| project.materials[i].name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::apply_all;

    #[test]
    fn lays_a_lane_that_leaves_and_rejoins_the_track() {
        let mut p = Project::new("pits");
        // Far enough out that the lane would cross the wall 20 m from the edge.
        let plan = Plan {
            gap: 12.0,
            ..Plan::around_start(&p)
        };
        let ops = ops(&p, "pit", &plan).unwrap();
        apply_all(&mut p, &ops).unwrap();
        let lane = p.road("pit").unwrap();
        assert_eq!(p.markers.pit.as_ref().unwrap().boxes.len(), 12);
        // Its ends on the track, its middle beside it.
        let main = Sampled::new(p.road("circuit").unwrap(), 2.0);
        let off = |pos: DVec3| {
            let f = main.frames[main.nearest(pos)];
            (pos - f.pos).truncate().length()
        };
        let n = lane.nodes.len();
        assert!(off(lane.nodes[0].pos) < 6.0);
        assert!(off(lane.nodes[n - 1].pos) < 6.0);
        assert!(off(lane.nodes[n / 2].pos) > 6.0 + 12.0 + 4.0);
        // The wall on that side stops short of the lane, and a pit wall stands between.
        let barriers = &p.road("circuit").unwrap().barriers;
        let right = barriers
            .iter()
            .find(|b| b.side == Side::Right && b.name == "wall");
        assert_eq!(right.unwrap().ranges.len(), 1);
        assert!(barriers.iter().any(|b| b.name == "pit wall"));
        // Too short a stretch is refused.
        let short = Plan {
            to: plan.from + 0.2,
            ..plan
        };
        assert!(super::ops(&p, "pit2", &short).is_err());
    }
}

#[cfg(test)]
mod paint_tests {
    use super::*;
    use crate::ops::apply_all;

    #[test]
    fn paints_the_lane_s_lines_and_the_start_and_grid_once() {
        let mut p = Project::new("paint");
        let ops = ops(&p, "pit", &Plan::around_start(&p)).unwrap();
        apply_all(&mut p, &ops).unwrap();
        let lane = p.road("pit").unwrap();
        assert_eq!(lane.lines.len(), 2);
        assert_eq!(lane.marks.len(), 2);
        for _ in 0..2 {
            let ops = start_and_grid(&p);
            apply_all(&mut p, &ops).unwrap();
        }
        let main = p.road("circuit").unwrap();
        assert_eq!(main.marks.len(), 1 + p.markers.grid.count);
        // Built as meshes over the road.
        let b = crate::road::build(&p, 0);
        let lines = b
            .visual
            .iter()
            .filter(|v| v.layer == crate::road::Layer::Line)
            .count();
        assert!(lines >= main.marks.len());
    }
}
