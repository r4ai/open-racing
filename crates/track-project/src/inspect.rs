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
    /// Its corners, numbered from the start line on the main road.
    pub corners: Vec<CornerSummary>,
}

/// A corner: which way, where (m along the road), how tight and how far it turns.
#[derive(Serialize)]
pub struct CornerSummary {
    pub number: usize,
    pub dir: Side,
    pub entry: f64,
    pub apex: f64,
    pub exit: f64,
    pub radius: f64,
    /// Degrees.
    pub angle: f64,
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

/// A problem found in a project, and where it is when it has a place.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Issue {
    pub text: String,
    /// The road it is on, and how far along it, m.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub road: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub s: Option<f64>,
}

impl Issue {
    fn at(road: &str, s: f64, text: String) -> Self {
        Self {
            text,
            road: Some(road.to_string()),
            s: Some(s),
        }
    }

    fn general(text: String) -> Self {
        Self {
            text,
            road: None,
            s: None,
        }
    }
}

/// Steepest bank that still drives sensibly, radians (about 17°).
const MAX_BANK: f64 = 0.3;

/// What will not drive well, or not at all: an invalid project, corners too tight,
/// slopes and banks too steep, roads crossing on the level, markers off their road.
pub fn issues(project: &Project, scene: &Scene) -> Vec<Issue> {
    let mut out = Vec::new();
    if let Err(e) = project.validate() {
        out.push(Issue::general(e.to_string()));
    }
    for (road, b) in project.roads.iter().zip(&scene.roads) {
        let smp = &b.sampled;
        let (grade, radius) = extremes(smp);
        if radius.value < 10.0 {
            out.push(Issue::at(
                &road.name,
                radius.s,
                format!(
                    "road \"{}\": radius {:.1} m at s = {:.0} m (u = {:.2}) is too tight to drive",
                    road.name, radius.value, radius.s, radius.u
                ),
            ));
        }
        if grade.value > 20.0 {
            out.push(Issue::at(
                &road.name,
                grade.s,
                format!(
                    "road \"{}\": {:.0} % grade at s = {:.0} m (u = {:.2})",
                    road.name, grade.value, grade.s, grade.u
                ),
            ));
        }
        if let Some(k) = road.bank.keys.iter().find(|k| k.value.abs() > MAX_BANK) {
            out.push(Issue::at(
                &road.name,
                smp.s_at(k.u),
                format!(
                    "road \"{}\": {:.0}° of bank at u = {:.2} is steeper than circuits have",
                    road.name,
                    k.value.to_degrees(),
                    k.u
                ),
            ));
        }
        for (s, other) in crossings(smp, smp) {
            out.push(Issue::at(
                &road.name,
                s,
                format!(
                    "road \"{}\" crosses itself at s = {s:.0} m and {other:.0} m at the same level",
                    road.name
                ),
            ));
        }
    }
    for (i, (a, ba)) in project.roads.iter().zip(&scene.roads).enumerate() {
        for (b, bb) in project.roads.iter().zip(&scene.roads).skip(i + 1) {
            let (sa, sb) = (&ba.sampled, &bb.sampled);
            let crossed = crossings(sa, sb);
            for &(s, other) in &crossed {
                // Where an open road starts or ends on another is a junction.
                if near_end(sa, s) || near_end(sb, other) {
                    continue;
                }
                out.push(Issue::at(
                    &a.name,
                    s,
                    format!(
                        "road \"{}\" crosses \"{}\" at the same level at s = {s:.0} m ({other:.0} m along it); raise one over the other, or join them at a node",
                        a.name, b.name
                    ),
                ));
            }
            for (from, to) in overlaps(sa, sb, &crossed) {
                out.push(Issue::at(
                    &a.name,
                    from,
                    format!(
                        "road \"{}\" runs into \"{}\" from s = {from:.0} m to {to:.0} m: their surfaces overlap",
                        a.name, b.name
                    ),
                ));
            }
        }
    }
    let m = &project.markers;
    if let Some(main) = project.road(&project.main_road) {
        let period = main.period();
        let off = |u: f64| !(0.0..=period).contains(&u);
        if off(m.start) {
            out.push(Issue::general(format!(
                "the start line at u = {:.2} is beyond the main road (0 to {period})",
                m.start
            )));
        }
        for (i, &u) in m.sectors.iter().enumerate() {
            if off(u) {
                out.push(Issue::general(format!(
                    "sector {} at u = {u:.2} is beyond the main road (0 to {period})",
                    i + 2
                )));
            }
        }
    }
    if let Some(pit) = &m.pit
        && let Some(road) = project.road(&pit.road)
    {
        let period = road.period();
        if pit.boxes.iter().any(|&u| !(0.0..=period).contains(&u)) {
            out.push(Issue::general(format!(
                "a pit box is beyond the pit lane \"{}\" (u 0 to {period})",
                pit.road
            )));
        }
    }
    out
}

/// A road's steepest grade and tightest radius.
fn extremes(smp: &Sampled) -> (Place, Place) {
    let frames = &smp.frames;
    let n = frames.len();
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
    (grade, radius)
}

pub fn summarize(project: &Project, scene: &Scene) -> Summary {
    let warnings = issues(project, scene).into_iter().map(|i| i.text).collect();
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
            let (grade, radius) = extremes(smp);
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
                corners: {
                    let start = if road.name == project.main_road {
                        smp.s_at(project.markers.start)
                    } else {
                        0.0
                    };
                    crate::corners::find(smp, start)
                        .into_iter()
                        .map(|c| CornerSummary {
                            number: c.number,
                            dir: c.dir,
                            entry: c.entry,
                            apex: c.apex,
                            exit: c.exit,
                            radius: c.radius,
                            angle: c.angle.to_degrees(),
                        })
                        .collect()
                },
            }
        })
        .collect();

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

/// How near an open road's end, m, it may meet another road: a junction.
const JOIN: f64 = 15.0;

/// Whether `s` is near an end of an open road.
fn near_end(smp: &Sampled, s: f64) -> bool {
    !smp.closed && (s < JOIN || s > smp.length - JOIN)
}

/// Stretches of `a` (from, to, m along it) whose surface overlaps `b`'s at about the
/// same level, apart from where they cross and where one road starts or ends on the
/// other (a junction, a pit lane leaving the track).
fn overlaps(a: &Sampled, b: &Sampled, crossed: &[(f64, f64)]) -> Vec<(f64, f64)> {
    if a.frames.is_empty() || b.frames.is_empty() {
        return vec![];
    }
    let step = 2;
    // Runs of overlapping frames, and whether each touches a road's end.
    let mut runs: Vec<(f64, f64, bool)> = Vec::new();
    let mut open: Option<(f64, f64, bool)> = None;
    for k in (0..a.frames.len()).step_by(step) {
        let f = &a.frames[k];
        let g = &b.frames[b.nearest(f.pos)];
        let d = (f.pos - g.pos).truncate().dot(g.lateral.truncate());
        // The facing halves of both roads.
        let (wa, wb) = if d >= 0.0 {
            (f.width_right.min(f.width_left), g.width_left)
        } else {
            (f.width_left.min(f.width_right), g.width_right)
        };
        let beside = (f.pos - g.pos).truncate().length();
        let over = d.abs() < wa + wb - 0.5
            && beside < wa + wb + 2.0
            && (f.pos.z - g.pos.z).abs() < 3.0
            && !crossed.iter().any(|&(s, _)| (s - f.s).abs() < 40.0);
        if over {
            let end = near_end(a, f.s) || near_end(b, g.s);
            open = Some(match open {
                Some((from, _, e)) => (from, f.s, e || end),
                None => (f.s, f.s, end),
            });
        } else if let Some(r) = open.take() {
            runs.push(r);
        }
    }
    runs.extend(open);
    runs.into_iter()
        .filter(|&(from, to, end)| !end && to - from >= 2.0)
        .map(|(from, to, _)| (from, to))
        .collect()
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
    fn roads_crossing_or_overlapping_each_other_are_found_but_junctions_are_not() {
        let warnings = |p: &Project| summarize(p, &crate::bake::build(p)).warnings;
        // A pit lane leaves and rejoins the track: no warning.
        let mut p = Project::new("t");
        let plan = crate::pitlane::Plan::around_start(&p);
        let ops = crate::pitlane::ops(&p, "pit", &plan).unwrap();
        apply_all(&mut p, &ops).unwrap();
        assert!(warnings(&p).is_empty(), "{:?}", warnings(&p));
        // A road straight across the oval's bottom straight, at its level.
        let add = |p: &mut Project, name: &str, nodes: Vec<DVec3>| {
            apply_all(
                p,
                &[Op::AddRoad {
                    name: name.into(),
                    closed: false,
                    nodes,
                    like: None,
                }],
            )
            .unwrap();
        };
        let mut q = p.clone();
        add(
            &mut q,
            "across",
            vec![DVec3::new(150.0, -80.0, 0.0), DVec3::new(150.0, 80.0, 0.0)],
        );
        let w = warnings(&q);
        assert!(w.iter().any(|w| w.contains("crosses \"across\"")), "{w:?}");
        // Over it on a bridge: fine.
        q.roads
            .last_mut()
            .unwrap()
            .nodes
            .iter_mut()
            .for_each(|n| n.pos.z = 8.0);
        let w = warnings(&q);
        assert!(!w.iter().any(|w| w.contains("across")), "{w:?}");
        // A road along the top straight, half on it.
        let mut q = p.clone();
        add(
            &mut q,
            "beside",
            vec![
                DVec3::new(-60.0, 265.0, 0.0),
                DVec3::new(20.0, 264.0, 0.0),
                DVec3::new(100.0, 266.0, 0.0),
            ],
        );
        let w = warnings(&q);
        assert!(
            w.iter().any(|w| w.contains("runs into \"beside\"")),
            "{w:?}"
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
