//! A summary of a project with the numbers its file leaves implicit: road lengths,
//! where each node lies along its road, radii, grades, where the markers end up, and
//! warnings. For people in the editor and for agents reading `trackctl info --json`.

use glam::DVec3;
use serde::Serialize;

use crate::bake::Scene;
use crate::curve::Sampled;
use crate::project::{NodeHandles, Project, Range, Side};

#[derive(Serialize)]
pub struct Summary {
    pub name: String,
    pub main_road: String,
    /// Plan-view extent of the roads: [[min x, min y], [max x, max y]], m.
    pub bounds: [[f64; 2]; 2],
    pub roads: Vec<RoadSummary>,
    pub splines: Vec<SplineSummary>,
    pub markers: MarkerSummary,
    pub surfaces: Vec<String>,
    pub materials: Vec<String>,
    /// Problems: an invalid project, or geometry that will not drive well.
    pub warnings: Vec<String>,
}

#[derive(Serialize)]
pub struct RoadSummary {
    pub name: String,
    pub closed: bool,
    pub length: f64,
    pub nodes: Vec<NodeSummary>,
    pub width_left_keys: Vec<crate::project::Key>,
    pub width_right_keys: Vec<crate::project::Key>,
    pub bank_keys: Vec<crate::project::Key>,
    /// [min, max] of each side's width, m.
    pub width_left: [f64; 2],
    pub width_right: [f64; 2],
    /// [min, max] height of the centre, m.
    pub elevation: [f64; 2],
    /// Steepest grade, %, and where.
    pub max_grade: Place,
    /// Tightest horizontal radius, m, and where.
    pub min_radius: Place,
    pub strips_left: Vec<StripSummary>,
    pub strips_right: Vec<StripSummary>,
    pub lines: Vec<String>,
    pub barriers: Vec<String>,
}

/// A kerb, wall or fence on its own line.
#[derive(Serialize)]
pub struct SplineSummary {
    pub name: String,
    /// "Band" or "Wall".
    pub shape: &'static str,
    pub material: String,
    pub length: f64,
    /// Node positions.
    pub nodes: Vec<[f64; 3]>,
    /// Control nodes with their handle modes and offsets.
    pub control_nodes: Vec<NodeSummary>,
}

#[derive(Serialize)]
pub struct NodeSummary {
    pub index: usize,
    pub pos: [f64; 3],
    /// Distance along the road, m.
    pub s: f64,
    pub handles: NodeHandles,
}

/// A value at a place on a road.
#[derive(Serialize)]
pub struct Place {
    pub value: f64,
    pub u: f64,
    pub s: f64,
}

#[derive(Serialize)]
pub struct StripSummary {
    pub name: String,
    pub width: f64,
    pub surface: String,
    /// Stretches as [from s, to s] in m; empty when everywhere.
    pub stretches: Vec<[f64; 2]>,
}

#[derive(Serialize)]
pub struct MarkerSummary {
    pub start: Marker,
    pub sectors: Vec<Marker>,
    pub grid_slots: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pit: Option<PitSummary>,
}

#[derive(Serialize)]
pub struct Marker {
    pub u: f64,
    pub s: f64,
    pub pos: [f64; 3],
}

#[derive(Serialize)]
pub struct PitSummary {
    pub road: String,
    pub boxes: Vec<Marker>,
}

fn marker(sampled: &Sampled, u: f64) -> Marker {
    let s = sampled.s_at(u);
    Marker {
        u,
        s,
        pos: sampled.frame_at(s).pos.to_array(),
    }
}

fn stretches(sampled: &Sampled, ranges: &[Range]) -> Vec<[f64; 2]> {
    ranges
        .iter()
        .map(|r| [sampled.s_at(r.from), sampled.s_at(r.to)])
        .collect()
}

pub fn summarize(project: &Project, scene: &Scene) -> Summary {
    let mut warnings = Vec::new();
    if let Err(e) = project.validate() {
        warnings.push(e.to_string());
    }
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    let roads: Vec<RoadSummary> = project
        .roads
        .iter()
        .zip(&scene.roads)
        .map(|(road, b)| {
            let smp = &b.sampled;
            let frames = &smp.frames;
            let n = frames.len();
            for f in frames {
                lo = [lo[0].min(f.pos.x), lo[1].min(f.pos.y)];
                hi = [hi[0].max(f.pos.x), hi[1].max(f.pos.y)];
            }
            let range = |v: &dyn Fn(usize) -> f64| {
                (0..n).fold([f64::INFINITY, f64::NEG_INFINITY], |[a, b], k| {
                    [a.min(v(k)), b.max(v(k))]
                })
            };
            let pairs = if smp.closed { n } else { n.saturating_sub(1) };
            let (mut grade, mut radius) = (
                Place {
                    value: 0.0,
                    u: 0.0,
                    s: 0.0,
                },
                Place {
                    value: f64::INFINITY,
                    u: 0.0,
                    s: 0.0,
                },
            );
            for k in 0..pairs {
                let (a, b) = (&frames[k], &frames[(k + 1) % n]);
                let run = (b.pos - a.pos).truncate().length();
                if run < 1e-6 {
                    continue;
                }
                let g = 100.0 * (b.pos.z - a.pos.z).abs() / run;
                if g > grade.value {
                    grade = Place {
                        value: g,
                        u: a.u,
                        s: a.s,
                    };
                }
                let turn = a
                    .tangent
                    .truncate()
                    .normalize()
                    .angle_to(b.tangent.truncate().normalize());
                if turn.abs() > 1e-6 && run / turn.abs() < radius.value {
                    radius = Place {
                        value: run / turn.abs(),
                        u: a.u,
                        s: a.s,
                    };
                }
            }
            let strips = |side: Side| {
                road.strips(side)
                    .iter()
                    .map(|s| StripSummary {
                        name: s.name.clone(),
                        width: s.width,
                        surface: s.surface.clone(),
                        stretches: stretches(smp, &s.ranges),
                    })
                    .collect()
            };
            RoadSummary {
                name: road.name.clone(),
                closed: road.closed,
                length: smp.length,
                nodes: road
                    .nodes
                    .iter()
                    .enumerate()
                    .map(|(i, nd)| NodeSummary {
                        index: i,
                        pos: nd.pos.to_array(),
                        s: smp.s_at(i as f64),
                        handles: nd.handles,
                    })
                    .collect(),
                width_left_keys: road.width_left.keys.clone(),
                width_right_keys: road.width_right.keys.clone(),
                bank_keys: road.bank.keys.clone(),
                width_left: range(&|k| frames[k].width_left),
                width_right: range(&|k| frames[k].width_right),
                elevation: range(&|k| frames[k].pos.z),
                max_grade: grade,
                min_radius: radius,
                strips_left: strips(Side::Left),
                strips_right: strips(Side::Right),
                lines: road.lines.iter().map(|l| l.name.clone()).collect(),
                barriers: road.barriers.iter().map(|b| b.name.clone()).collect(),
            }
        })
        .collect();

    for r in &roads {
        if r.min_radius.value < 10.0 {
            warnings.push(format!(
                "road \"{}\": radius {:.1} m at s = {:.0} m (u = {:.2}) is too tight to drive",
                r.name, r.min_radius.value, r.min_radius.s, r.min_radius.u
            ));
        }
        if r.max_grade.value > 20.0 {
            warnings.push(format!(
                "road \"{}\": {:.0} % grade at s = {:.0} m (u = {:.2})",
                r.name, r.max_grade.value, r.max_grade.s, r.max_grade.u
            ));
        }
    }
    for (road, b) in project.roads.iter().zip(&scene.roads) {
        for (s, other) in crossings(&b.sampled, &b.sampled) {
            warnings.push(format!(
                "road \"{}\" crosses itself at s = {s:.0} m and {other:.0} m at the same level",
                road.name
            ));
        }
    }

    let main = project
        .road_index(&project.main_road)
        .map(|i| &scene.roads[i].sampled);
    let empty = || Marker {
        u: 0.0,
        s: 0.0,
        pos: [0.0; 3],
    };
    let m = &project.markers;
    let markers = MarkerSummary {
        start: main.map_or_else(empty, |smp| marker(smp, m.start)),
        sectors: main.map_or_else(Vec::new, |smp| {
            m.sectors.iter().map(|&u| marker(smp, u)).collect()
        }),
        grid_slots: m.grid.count,
        pit: m.pit.as_ref().and_then(|p| {
            let i = project.road_index(&p.road)?;
            let smp = &scene.roads[i].sampled;
            Some(PitSummary {
                road: p.road.clone(),
                boxes: p.boxes.iter().map(|&u| marker(smp, u)).collect(),
            })
        }),
    };
    let splines = project
        .splines
        .iter()
        .zip(&scene.splines)
        .map(|(sp, b)| SplineSummary {
            name: sp.name.clone(),
            shape: match sp.shape {
                crate::project::Shape::Band { .. } => "Band",
                crate::project::Shape::Wall { .. } => "Wall",
            },
            material: sp.material().to_string(),
            length: b.sampled.length,
            nodes: sp.nodes.iter().map(|n| n.pos.to_array()).collect(),
            control_nodes: sp
                .nodes
                .iter()
                .enumerate()
                .map(|(i, nd)| NodeSummary {
                    index: i,
                    pos: nd.pos.to_array(),
                    s: b.sampled.s_at(i as f64),
                    handles: nd.handles,
                })
                .collect(),
        })
        .collect();
    Summary {
        name: project.name.clone(),
        main_road: project.main_road.clone(),
        bounds: [lo, hi],
        roads,
        splines,
        markers,
        surfaces: project.surfaces.iter().map(|s| s.name.clone()).collect(),
        materials: project.materials.iter().map(|m| m.name.clone()).collect(),
        warnings,
    }
}

/// Places where a road crosses another (or itself, away from its own neighbourhood)
/// within 4 m of height: (s on `a`, s on `b`), one per crossing.
fn crossings(a: &Sampled, b: &Sampled) -> Vec<(f64, f64)> {
    let same = std::ptr::eq(a, b);
    let step = 5;
    let seg = |smp: &Sampled, k: usize| {
        let n = smp.frames.len();
        let next = if smp.closed {
            (k + step) % n
        } else {
            (k + step).min(n - 1)
        };
        (smp.frames[k].pos, smp.frames[next].pos)
    };
    let mut out: Vec<(f64, f64)> = Vec::new();
    for i in (0..a.frames.len()).step_by(step) {
        let (p0, p1) = seg(a, i);
        for j in (0..b.frames.len()).step_by(step) {
            if same {
                let gap = (a.frames[i].s - b.frames[j].s).abs();
                let gap = if a.closed {
                    gap.min(a.length - gap)
                } else {
                    gap
                };
                if gap < 30.0 || j <= i {
                    continue;
                }
            }
            let (q0, q1) = seg(b, j);
            if let Some(t) = intersect(p0, p1, q0, q1) {
                let (za, zb) = (p0.lerp(p1, t.0).z, q0.lerp(q1, t.1).z);
                let s = a.frames[i].s;
                if (za - zb).abs() < 4.0 && out.iter().all(|&(x, _)| (x - s).abs() > 30.0) {
                    out.push((s, b.frames[j].s));
                }
            }
        }
    }
    out
}

/// Parameters where two segments cross in the plane.
fn intersect(p0: DVec3, p1: DVec3, q0: DVec3, q1: DVec3) -> Option<(f64, f64)> {
    let (r, s) = ((p1 - p0).truncate(), (q1 - q0).truncate());
    let den = r.perp_dot(s);
    if den.abs() < 1e-12 {
        return None;
    }
    let d = (q0 - p0).truncate();
    let (t, u) = (d.perp_dot(s) / den, d.perp_dot(r) / den);
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then_some((t, u))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{Op, apply_all};
    use crate::project::NodeHandles;

    #[test]
    fn summarizes_and_warns_about_crossings() {
        let mut p = Project::new("t");
        let s = summarize(&p, &crate::bake::build(&p));
        assert!(s.warnings.is_empty(), "{:?}", s.warnings);
        let r = &s.roads[0];
        assert!(r.length > 1000.0 && r.min_radius.value > 30.0);
        assert_eq!(s.markers.sectors.len(), 2);
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["roads"][0]["nodes"][0]["handles"], "Auto");
        assert!(json["roads"][0]["width_left_keys"].is_array());

        // A figure of eight on the level.
        apply_all(
            &mut p,
            &[Op::SetNodes {
                line: "circuit".into(),
                nodes: [(0., 0.), (200., 200.), (400., 0.), (200., -200.)]
                    .iter()
                    .chain(&[(0., 0.), (-200., 200.), (-400., 0.), (-200., -200.)])
                    .map(|&(x, y)| DVec3::new(x, y, 0.0))
                    .collect(),
            }],
        )
        .unwrap();
        // Two nodes at the same place make the crossing; move one aside.
        p.roads[0].nodes[4].pos = DVec3::new(0.0, 0.0, 0.0);
        p.roads[0].nodes[0].pos = DVec3::new(10.0, -10.0, 0.0);
        let s = summarize(&p, &crate::bake::build(&p));
        assert!(
            s.warnings.iter().any(|w| w.contains("crosses itself")),
            "{:?}",
            s.warnings
        );
    }

    #[test]
    fn json_summary_contains_editable_curve_controls() {
        let mut p = Project::new("controls");
        p.roads[0].nodes[0].handles = NodeHandles::Free {
            incoming: DVec3::new(-2.0, 1.0, 0.0),
            outgoing: DVec3::new(3.0, 0.0, 0.0),
        };
        p.roads[0].width_left.keys[0].slope_out = 0.2;
        let json = serde_json::to_value(summarize(&p, &crate::bake::build(&p))).unwrap();
        assert_eq!(
            json["roads"][0]["nodes"][0]["handles"]["Free"]["outgoing"][0],
            3.0
        );
        assert_eq!(json["roads"][0]["width_left_keys"][0]["slope_out"], 0.2);
    }
}
