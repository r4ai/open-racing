//! Edits to a project as data. Every change the editor makes, and every change a script
//! or an AI agent asks for (`open-racing-trackctl apply`), is one of these, so the same
//! checks apply whoever edits. Things are addressed by name; `Put…` operations add or
//! replace by name.
//!
//! A list of operations is applied atomically: if one fails, or the result is not a
//! valid project, the project is left as it was.

use glam::DVec3;
use serde::{Deserialize, Serialize};

use crate::Error;
use crate::project::{
    Barrier, Grid, HandleMode, Key, MaterialDef, NamedSurface, Node, NodeHandles, PaintLine, Pit,
    Project, Prop, Reference, Road, Side, Spline, StationCurve, Strip, Terrain,
};

/// Which profile along a road.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Curve {
    WidthLeft,
    WidthRight,
    /// Both widths.
    Width,
    /// Banking, radians.
    Bank,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Op {
    /// Renames the project.
    SetName {
        name: String,
    },

    /// Adds a road through `nodes`. It takes its cross-section (widths, banking, crown,
    /// surfaces, and the strips, lines and barriers that run its whole length) from the
    /// road `like`, or else a plain 12 m asphalt road.
    AddRoad {
        name: String,
        closed: bool,
        nodes: Vec<DVec3>,
        #[serde(default)]
        like: Option<String>,
    },
    RemoveRoad {
        road: String,
    },
    /// Renames a road and the references to it.
    RenameRoad {
        road: String,
        to: String,
    },
    /// Sets a road's properties; those left out stay as they are.
    SetRoad {
        road: String,
        #[serde(default)]
        closed: Option<bool>,
        #[serde(default)]
        crown: Option<f64>,
        #[serde(default)]
        surface: Option<String>,
        #[serde(default)]
        material: Option<String>,
        #[serde(default)]
        resolution: Option<f64>,
    },
    /// Makes a closed road the circuit.
    SetMainRoad {
        road: String,
    },

    /// Adds a node to a road or spline (`line` names either) before node `before`, or
    /// at the end.
    AddNode {
        line: String,
        pos: DVec3,
        #[serde(default)]
        before: Option<usize>,
    },
    MoveNode {
        line: String,
        index: usize,
        pos: DVec3,
    },
    /// Sets both handle offsets and their relation. Auto ignores the offsets.
    SetNodeHandles {
        line: String,
        index: usize,
        mode: HandleMode,
        incoming: DVec3,
        outgoing: DVec3,
    },
    RemoveNode {
        line: String,
        index: usize,
    },
    /// Replaces all of a road's or spline's nodes (automatic handles). A road's
    /// profiles and stretches keep their spline parameters.
    SetNodes {
        line: String,
        nodes: Vec<DVec3>,
    },

    /// Replaces a profile's keys.
    SetProfile {
        road: String,
        curve: Curve,
        keys: Vec<Key>,
    },
    /// Sets a profile's value at `u`, adding a key there.
    SetKey {
        road: String,
        curve: Curve,
        u: f64,
        value: f64,
    },
    /// Sets the value change per unit `u` before and after a profile key.
    SetKeyTangents {
        road: String,
        curve: Curve,
        u: f64,
        slope_in: f64,
        slope_out: f64,
    },

    /// Adds a strip to a side, or replaces the one of the same name. New strips go at
    /// `at` from the road outwards, or outermost.
    PutStrip {
        road: String,
        side: Side,
        strip: Strip,
        #[serde(default)]
        at: Option<usize>,
    },
    RemoveStrip {
        road: String,
        side: Side,
        name: String,
    },
    PutLine {
        road: String,
        line: PaintLine,
    },
    RemoveLine {
        road: String,
        name: String,
    },
    PutBarrier {
        road: String,
        barrier: Barrier,
    },
    RemoveBarrier {
        road: String,
        name: String,
    },

    /// Adds a kerb, wall or fence along its own spline, or replaces the one of the same
    /// name.
    PutSpline {
        spline: Spline,
    },
    RemoveSpline {
        name: String,
    },
    /// Renames a spline, keeping its place in the list.
    RenameSpline {
        name: String,
        to: String,
    },

    /// Places a 3D model, or replaces the prop of the same name.
    PutProp {
        prop: Prop,
    },
    RemoveProp {
        name: String,
    },
    /// Renames a prop, keeping its place in the list.
    RenameProp {
        name: String,
        to: String,
    },
    /// Moves, turns or resizes a prop; what is left out stays as it is.
    MoveProp {
        name: String,
        #[serde(default)]
        pos: Option<DVec3>,
        #[serde(default)]
        yaw: Option<f64>,
        #[serde(default)]
        scale: Option<f64>,
    },

    /// Sets the race markers; those left out stay as they are.
    SetMarkers {
        #[serde(default)]
        start: Option<f64>,
        #[serde(default)]
        sectors: Option<Vec<f64>>,
        #[serde(default)]
        grid: Option<Grid>,
    },
    /// Sets or removes the pit lane.
    SetPit {
        pit: Option<Pit>,
    },
    SetTerrain {
        terrain: Terrain,
    },
    /// Sets or removes the reference image the editor shows to trace over.
    SetReference {
        reference: Option<Reference>,
    },

    PutSurface {
        surface: NamedSurface,
    },
    RemoveSurface {
        name: String,
    },
    PutMaterial {
        material: MaterialDef,
    },
    RemoveMaterial {
        name: String,
    },
}

fn missing(what: &str, name: &str) -> Error {
    Error::Invalid(format!("no {what} named \"{name}\""))
}

fn road_mut<'a>(p: &'a mut Project, name: &str) -> Result<&'a mut Road, Error> {
    p.roads
        .iter_mut()
        .find(|r| r.name == name)
        .ok_or_else(|| missing("road", name))
}

/// A road or a spline, for editing its nodes.
enum Line<'a> {
    Road(&'a mut Road),
    Spline(&'a mut Spline),
}

impl Line<'_> {
    fn nodes(&mut self) -> &mut Vec<Node> {
        match self {
            Line::Road(r) => &mut r.nodes,
            Line::Spline(s) => &mut s.nodes,
        }
    }

    fn node_index(&mut self, name: &str, index: usize) -> Result<usize, Error> {
        let n = self.nodes().len();
        if index < n {
            Ok(index)
        } else {
            Err(Error::Invalid(format!(
                "\"{name}\" has no node {index} (it has {n})"
            )))
        }
    }

    fn insert(&mut self, index: usize, node: Node) {
        match self {
            Line::Road(r) => r.insert_node(index, node),
            Line::Spline(s) => s.nodes.insert(index, node),
        }
    }

    fn remove(&mut self, index: usize) {
        match self {
            Line::Road(r) => r.remove_node(index),
            Line::Spline(s) => {
                s.nodes.remove(index);
            }
        }
    }
}

fn line_mut<'a>(p: &'a mut Project, name: &str) -> Result<Line<'a>, Error> {
    if let Some(r) = p.roads.iter_mut().find(|r| r.name == name) {
        return Ok(Line::Road(r));
    }
    p.splines
        .iter_mut()
        .find(|s| s.name == name)
        .map(Line::Spline)
        .ok_or_else(|| missing("road or spline", name))
}

/// Adds `item` to `list`, replacing the one with the same name.
fn put<T>(list: &mut Vec<T>, item: T, name: impl Fn(&T) -> &str, at: Option<usize>) {
    match list.iter().position(|x| name(x) == name(&item)) {
        Some(i) => list[i] = item,
        None => {
            let at = at.unwrap_or(list.len()).min(list.len());
            list.insert(at, item);
        }
    }
}

fn remove<T>(
    list: &mut Vec<T>,
    what: &str,
    target: &str,
    name: impl Fn(&T) -> &str,
) -> Result<(), Error> {
    let i = list
        .iter()
        .position(|x| name(x) == target)
        .ok_or_else(|| missing(what, target))?;
    list.remove(i);
    Ok(())
}

/// A plain 12 m asphalt road without strips, for roads not made like another.
fn plain_road(project: &Project, name: String, closed: bool, nodes: Vec<Node>) -> Road {
    let asphalt = project
        .surface_of(open_racing_sim::Surface::Asphalt)
        .or(project.surfaces.first().map(|s| s.name.as_str()))
        .unwrap_or("asphalt")
        .to_string();
    let material = project
        .material_index("asphalt")
        .or((!project.materials.is_empty()).then_some(0))
        .map_or("asphalt".to_string(), |i| project.materials[i].name.clone());
    Road {
        name,
        closed,
        nodes,
        width_left: StationCurve::constant(6.0),
        width_right: StationCurve::constant(6.0),
        bank: StationCurve::constant(0.0),
        crown: 0.05,
        surface: asphalt,
        material,
        left: vec![],
        right: vec![],
        lines: vec![],
        barriers: vec![],
        resolution: 2.0,
    }
}

impl Op {
    /// Applies the operation, without checking the whole project afterwards.
    pub fn apply(&self, p: &mut Project) -> Result<(), Error> {
        match self.clone() {
            Op::SetName { name } => p.name = name,
            Op::AddRoad {
                name,
                closed,
                nodes,
                like,
            } => {
                if p.road(&name).is_some() {
                    return Err(Error::Invalid(format!("a road named \"{name}\" exists")));
                }
                let nodes: Vec<Node> = nodes.into_iter().map(Node::new).collect();
                let road = match like {
                    Some(like) => {
                        let mut r = p.road(&like).ok_or_else(|| missing("road", &like))?.clone();
                        // Profiles become constant at their first value; strips, lines
                        // and barriers limited to stretches (a corner's kerbs, a gravel
                        // trap) belong to that road's corners and stay behind.
                        for c in [&mut r.width_left, &mut r.width_right, &mut r.bank] {
                            let v = c.keys.first().map_or(0.0, |k| k.value);
                            *c = StationCurve::constant(v);
                        }
                        r.left.retain(|s| s.ranges.is_empty());
                        r.right.retain(|s| s.ranges.is_empty());
                        r.lines.retain(|l| l.ranges.is_empty());
                        r.barriers.retain(|b| b.ranges.is_empty());
                        Road {
                            name,
                            closed,
                            nodes,
                            ..r
                        }
                    }
                    None => plain_road(p, name, closed, nodes),
                };
                p.roads.push(road);
            }
            Op::RemoveRoad { road } => {
                if road == p.main_road {
                    return Err(Error::Invalid("the main road cannot be removed".into()));
                }
                remove(&mut p.roads, "road", &road, |r| &r.name)?;
                if p.markers.pit.as_ref().is_some_and(|pit| pit.road == road) {
                    p.markers.pit = None;
                }
            }
            Op::RenameRoad { road, to } => {
                if p.road(&to).is_some() {
                    return Err(Error::Invalid(format!("a road named \"{to}\" exists")));
                }
                road_mut(p, &road)?.name = to.clone();
                if p.main_road == road {
                    p.main_road = to.clone();
                }
                if let Some(pit) = &mut p.markers.pit
                    && pit.road == road
                {
                    pit.road = to;
                }
            }
            Op::SetRoad {
                road,
                closed,
                crown,
                surface,
                material,
                resolution,
            } => {
                let r = road_mut(p, &road)?;
                if let Some(v) = closed {
                    r.closed = v;
                }
                if let Some(v) = crown {
                    r.crown = v;
                }
                if let Some(v) = surface {
                    r.surface = v;
                }
                if let Some(v) = material {
                    r.material = v;
                }
                if let Some(v) = resolution {
                    r.resolution = v;
                }
            }
            Op::SetMainRoad { road } => {
                let period = p
                    .road(&road)
                    .ok_or_else(|| missing("road", &road))?
                    .period();
                if p.main_road != road {
                    // The markers' places were on the old road: the start goes to the
                    // new one's first node and the sectors split it evenly.
                    let m = &mut p.markers;
                    let k = m.sectors.len();
                    m.start = 0.0;
                    m.sectors = (1..=k)
                        .map(|i| period * i as f64 / (k + 1) as f64)
                        .collect();
                }
                p.main_road = road;
            }
            Op::AddNode { line, pos, before } => {
                let mut l = line_mut(p, &line)?;
                let node = Node::new(pos);
                match before {
                    Some(i) => {
                        if i > l.nodes().len() {
                            l.node_index(&line, i)?;
                        }
                        l.insert(i, node)
                    }
                    None => {
                        // Through `insert`, so that a closed road's keys and ranges on its
                        // closing segment move onto the new one.
                        let end = l.nodes().len();
                        l.insert(end, node)
                    }
                }
            }
            Op::MoveNode { line, index, pos } => {
                let mut l = line_mut(p, &line)?;
                let i = l.node_index(&line, index)?;
                l.nodes()[i].pos = pos;
            }
            Op::SetNodeHandles {
                line,
                index,
                mode,
                incoming,
                outgoing,
            } => {
                let mut l = line_mut(p, &line)?;
                let i = l.node_index(&line, index)?;
                let node = &mut l.nodes()[i];
                node.handles = NodeHandles::from_offsets(mode, incoming, outgoing);
            }
            Op::RemoveNode { line, index } => {
                let mut l = line_mut(p, &line)?;
                let i = l.node_index(&line, index)?;
                l.remove(i);
            }
            Op::SetNodes { line, nodes } => {
                *line_mut(p, &line)?.nodes() = nodes.into_iter().map(Node::new).collect();
            }
            Op::PutSpline { spline } => put(&mut p.splines, spline, |s| &s.name, None),
            Op::RemoveSpline { name } => remove(&mut p.splines, "spline", &name, |s| &s.name)?,
            Op::RenameSpline { name, to } => {
                if p.line(&to).is_some() {
                    return Err(Error::Invalid(format!(
                        "a road or spline named \"{to}\" exists"
                    )));
                }
                p.splines
                    .iter_mut()
                    .find(|s| s.name == name)
                    .ok_or_else(|| missing("spline", &name))?
                    .name = to;
            }
            Op::PutProp { prop } => put(&mut p.props, prop, |x| &x.name, None),
            Op::RemoveProp { name } => remove(&mut p.props, "prop", &name, |x| &x.name)?,
            Op::RenameProp { name, to } => {
                if p.props.iter().any(|x| x.name == to) {
                    return Err(Error::Invalid(format!("a prop named \"{to}\" exists")));
                }
                p.props
                    .iter_mut()
                    .find(|x| x.name == name)
                    .ok_or_else(|| missing("prop", &name))?
                    .name = to;
            }
            Op::MoveProp {
                name,
                pos,
                yaw,
                scale,
            } => {
                let prop = p
                    .props
                    .iter_mut()
                    .find(|x| x.name == name)
                    .ok_or_else(|| missing("prop", &name))?;
                if let Some(v) = pos {
                    prop.pos = v;
                }
                if let Some(v) = yaw {
                    prop.yaw = v;
                }
                if let Some(v) = scale {
                    prop.scale = v;
                }
            }
            Op::SetProfile { road, curve, keys } => {
                let r = road_mut(p, &road)?;
                let mut keys = keys;
                keys.sort_by(|a, b| a.u.total_cmp(&b.u));
                if keys.is_empty() {
                    return Err(Error::Invalid("a profile needs at least one key".into()));
                }
                for c in curves(r, curve) {
                    c.keys = keys.clone();
                }
            }
            Op::SetKey {
                road,
                curve,
                u,
                value,
            } => {
                let r = road_mut(p, &road)?;
                for c in curves(r, curve) {
                    c.set(u, value);
                }
            }
            Op::SetKeyTangents {
                road,
                curve,
                u,
                slope_in,
                slope_out,
            } => {
                let r = road_mut(p, &road)?;
                for c in curves(r, curve) {
                    let key = c
                        .keys
                        .iter_mut()
                        .find(|k| (k.u - u).abs() < 1e-6)
                        .ok_or_else(|| Error::Invalid(format!("no profile key at u = {u}")))?;
                    key.slope_in = slope_in;
                    key.slope_out = slope_out;
                }
            }
            Op::PutStrip {
                road,
                side,
                strip,
                at,
            } => {
                let r = road_mut(p, &road)?;
                let list = match side {
                    Side::Left => &mut r.left,
                    Side::Right => &mut r.right,
                };
                put(list, strip, |s| &s.name, at);
            }
            Op::RemoveStrip { road, side, name } => {
                let r = road_mut(p, &road)?;
                let list = match side {
                    Side::Left => &mut r.left,
                    Side::Right => &mut r.right,
                };
                remove(list, "strip", &name, |s| &s.name)?;
            }
            Op::PutLine { road, line } => {
                put(&mut road_mut(p, &road)?.lines, line, |l| &l.name, None)
            }
            Op::RemoveLine { road, name } => {
                remove(&mut road_mut(p, &road)?.lines, "line", &name, |l| &l.name)?
            }
            Op::PutBarrier { road, barrier } => put(
                &mut road_mut(p, &road)?.barriers,
                barrier,
                |b| &b.name,
                None,
            ),
            Op::RemoveBarrier { road, name } => {
                remove(&mut road_mut(p, &road)?.barriers, "barrier", &name, |b| {
                    &b.name
                })?
            }
            Op::SetMarkers {
                start,
                sectors,
                grid,
            } => {
                let m = &mut p.markers;
                if let Some(v) = start {
                    m.start = v;
                }
                if let Some(mut v) = sectors {
                    v.sort_by(f64::total_cmp);
                    m.sectors = v;
                }
                if let Some(v) = grid {
                    m.grid = v;
                }
            }
            Op::SetPit { pit } => p.markers.pit = pit,
            Op::SetTerrain { terrain } => p.terrain = terrain,
            Op::SetReference { reference } => p.reference = reference,
            Op::PutSurface { surface } => put(&mut p.surfaces, surface, |s| &s.name, None),
            Op::RemoveSurface { name } => remove(&mut p.surfaces, "surface", &name, |s| &s.name)?,
            Op::PutMaterial { material } => put(&mut p.materials, material, |m| &m.name, None),
            Op::RemoveMaterial { name } => {
                remove(&mut p.materials, "material", &name, |m| &m.name)?
            }
        }
        Ok(())
    }
}

fn curves(r: &mut Road, curve: Curve) -> Vec<&mut StationCurve> {
    match curve {
        Curve::WidthLeft => vec![&mut r.width_left],
        Curve::WidthRight => vec![&mut r.width_right],
        Curve::Width => vec![&mut r.width_left, &mut r.width_right],
        Curve::Bank => vec![&mut r.bank],
    }
}

/// Applies `ops` in order and checks the result; on any error the project is left as
/// it was and the error names the operation that failed.
pub fn apply_all(project: &mut Project, ops: &[Op]) -> Result<(), Error> {
    let mut next = project.clone();
    for (i, op) in ops.iter().enumerate() {
        op.apply(&mut next)
            .map_err(|e| Error::Invalid(format!("operation {} ({}): {e}", i + 1, op.kind())))?;
    }
    next.validate()?;
    *project = next;
    Ok(())
}

impl Op {
    /// The operation's name, for messages.
    pub fn kind(&self) -> &'static str {
        match self {
            Op::SetName { .. } => "SetName",
            Op::AddRoad { .. } => "AddRoad",
            Op::RemoveRoad { .. } => "RemoveRoad",
            Op::RenameRoad { .. } => "RenameRoad",
            Op::SetRoad { .. } => "SetRoad",
            Op::SetMainRoad { .. } => "SetMainRoad",
            Op::AddNode { .. } => "AddNode",
            Op::MoveNode { .. } => "MoveNode",
            Op::SetNodeHandles { .. } => "SetNodeHandles",
            Op::RemoveNode { .. } => "RemoveNode",
            Op::SetNodes { .. } => "SetNodes",
            Op::SetProfile { .. } => "SetProfile",
            Op::SetKey { .. } => "SetKey",
            Op::SetKeyTangents { .. } => "SetKeyTangents",
            Op::PutStrip { .. } => "PutStrip",
            Op::RemoveStrip { .. } => "RemoveStrip",
            Op::PutLine { .. } => "PutLine",
            Op::RemoveLine { .. } => "RemoveLine",
            Op::PutBarrier { .. } => "PutBarrier",
            Op::RemoveBarrier { .. } => "RemoveBarrier",
            Op::PutSpline { .. } => "PutSpline",
            Op::RemoveSpline { .. } => "RemoveSpline",
            Op::RenameSpline { .. } => "RenameSpline",
            Op::PutProp { .. } => "PutProp",
            Op::RemoveProp { .. } => "RemoveProp",
            Op::RenameProp { .. } => "RenameProp",
            Op::MoveProp { .. } => "MoveProp",
            Op::SetMarkers { .. } => "SetMarkers",
            Op::SetPit { .. } => "SetPit",
            Op::SetTerrain { .. } => "SetTerrain",
            Op::SetReference { .. } => "SetReference",
            Op::PutSurface { .. } => "PutSurface",
            Op::RemoveSurface { .. } => "RemoveSurface",
            Op::PutMaterial { .. } => "PutMaterial",
            Op::RemoveMaterial { .. } => "RemoveMaterial",
        }
    }
}

/// Reads a list of operations from RON or JSON (a JSON list starts with `[` and its
/// operations with `{`; RON's with a name).
pub fn parse(src: &str) -> Result<Vec<Op>, Error> {
    let trimmed = src.trim_start();
    let json = trimmed.starts_with('{')
        || (trimmed.starts_with('[') && trimmed[1..].trim_start().starts_with('{'));
    if json {
        let v: OneOrMany = serde_json::from_str(src)
            .map_err(|e| Error::Invalid(format!("operations (JSON): {e}")))?;
        return Ok(v.into_vec());
    }
    let v: OneOrMany =
        ron::from_str(src).map_err(|e| Error::Invalid(format!("operations (RON): {e}")))?;
    Ok(v.into_vec())
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    Many(Vec<Op>),
    One(Op),
}

impl OneOrMany {
    fn into_vec(self) -> Vec<Op> {
        match self {
            Self::Many(v) => v,
            Self::One(op) => vec![op],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{Profile, Range};

    #[test]
    fn edits_by_name_and_rolls_back_on_error() {
        let mut p = Project::new("t");
        let ops = parse(
            r#"[
                AddRoad(name: "pit", closed: false, nodes: [(0, -20, 0), (200, -20, 0)], like: Some("circuit")),
                SetKey(road: "circuit", curve: Width, u: 2.0, value: 7.5),
                PutStrip(road: "circuit", side: Left, strip: (name: "gravel", width: 10, surface: "gravel",
                    material: "gravel", profile: Slope(0.2), ranges: [(from: 3.0, to: 4.0)], fade: 5)),
                RenameRoad(road: "circuit", to: "gp"),
            ]"#,
        )
        .unwrap();
        apply_all(&mut p, &ops).unwrap();
        assert_eq!(p.main_road, "gp");
        assert_eq!(p.roads[1].left.len(), 1, "the grass, not the corner kerbs");
        assert_eq!(p.roads[0].left[2].profile, Profile::Slope(0.2));
        assert_eq!(
            p.roads[0].left[2].ranges,
            vec![Range { from: 3.0, to: 4.0 }]
        );

        // A failing list changes nothing.
        let before = p.clone();
        let bad = parse(
            r#"[{"MoveNode": {"line": "gp", "index": 0, "pos": [1, 2, 3]}},
                {"SetRoad": {"road": "gp", "surface": "ice"}}]"#,
        )
        .unwrap();
        let e = apply_all(&mut p, &bad).unwrap_err().to_string();
        assert!(e.contains("ice"), "{e}");
        assert_eq!(p, before);
    }

    #[test]
    fn node_edits_keep_stretches_in_place() {
        let mut p = Project::new("t");
        let r0 = p.roads[0].left[0].ranges.clone();
        apply_all(
            &mut p,
            &[Op::AddNode {
                line: "circuit".into(),
                pos: DVec3::new(120.0, -5.0, 0.0),
                before: Some(1),
            }],
        )
        .unwrap();
        let r1 = &p.roads[0].left[0].ranges;
        assert_eq!(r1[0].from, r0[0].from + 1.0);
    }

    #[test]
    fn appending_to_a_closed_road_keeps_the_closing_segment_s_keys() {
        let mut p = Project::new("t");
        let n = p.roads[0].nodes.len() as f64;
        // A key on the closing segment, from the last node back to 0.
        apply_all(
            &mut p,
            &[Op::SetKey {
                road: "circuit".into(),
                curve: Curve::Bank,
                u: n - 0.5,
                value: 0.1,
            }],
        )
        .unwrap();
        apply_all(
            &mut p,
            &[Op::AddNode {
                line: "circuit".into(),
                pos: DVec3::new(-60.0, 20.0, 0.0),
                before: None,
            }],
        )
        .unwrap();
        let key = p.roads[0]
            .bank
            .keys
            .iter()
            .find(|k| k.value == 0.1)
            .unwrap();
        assert_eq!(key.u, n + 0.5, "the key stays on the closing segment");

        // Removing node 0 joins the first segment onto the closing one.
        let mut p = Project::new("t");
        let key = |u| Op::SetKey {
            road: "circuit".into(),
            curve: Curve::Bank,
            u,
            value: 0.2,
        };
        let remove = Op::RemoveNode {
            line: "circuit".into(),
            index: 0,
        };
        apply_all(&mut p, &[key(0.5), remove]).unwrap();
        let bank = &p.roads[0].bank.keys;
        let k = bank.iter().find(|k| k.value == 0.2).unwrap();
        assert_eq!(k.u, n - 1.5, "wrapped round, not squashed onto 0");
        assert!(
            bank.windows(2).all(|w| w[0].u <= w[1].u),
            "keys stay in order"
        );
    }

    #[test]
    fn splines_and_props_rename_in_place_and_never_overwrite() {
        let mut p = Project::new("t");
        let ops = parse(
            r#"[
                PutSpline(spline: (name: "a", closed: false, drape: true, resolution: 1.0,
                    nodes: [(pos: (0, 0, 0)), (pos: (10, 0, 0))],
                    shape: Wall(height: 1.0, thickness: 0.5, material: "concrete"))),
                PutSpline(spline: (name: "b", closed: false, drape: true, resolution: 1.0,
                    nodes: [(pos: (0, 5, 0)), (pos: (10, 5, 0))],
                    shape: Wall(height: 1.0, thickness: 0.5, material: "concrete"))),
                RenameSpline(name: "a", to: "first"),
            ]"#,
        )
        .unwrap();
        apply_all(&mut p, &ops).unwrap();
        assert_eq!(p.splines[0].name, "first");
        assert_eq!(p.splines[1].name, "b");
        for to in ["b", "circuit"] {
            let op = Op::RenameSpline {
                name: "first".into(),
                to: to.into(),
            };
            assert!(apply_all(&mut p, &[op]).is_err(), "{to} is taken");
        }
        assert_eq!(p.splines.len(), 2);
    }

    #[test]
    fn handle_modes_and_profile_tangents_apply_from_operations() {
        let mut p = Project::new("t");
        let ops = parse(
            r#"[
            SetNodeHandles(line: "circuit", index: 0, mode: Free,
                incoming: (-2.0, 1.0, 0.0), outgoing: (4.0, 0.0, 0.0)),
            SetKey(road: "circuit", curve: Bank, u: 2.0, value: 0.1),
            SetKeyTangents(road: "circuit", curve: Bank, u: 2.0,
                slope_in: 0.02, slope_out: -0.03),
        ]"#,
        )
        .unwrap();
        apply_all(&mut p, &ops).unwrap();
        let node = p.roads[0].nodes[0];
        assert_eq!(node.handles.mode(), HandleMode::Free);
        assert_eq!(
            node.handles,
            NodeHandles::Free {
                incoming: DVec3::new(-2.0, 1.0, 0.0),
                outgoing: DVec3::new(4.0, 0.0, 0.0)
            }
        );
        let key = p.roads[0].bank.keys.iter().find(|k| k.u == 2.0).unwrap();
        assert_eq!((key.slope_in, key.slope_out), (0.02, -0.03));
        apply_all(
            &mut p,
            &[Op::SetNodeHandles {
                line: "circuit".into(),
                index: 0,
                mode: HandleMode::Auto,
                incoming: DVec3::ZERO,
                outgoing: DVec3::ZERO,
            }],
        )
        .unwrap();
        assert_eq!(p.roads[0].nodes[0].handles, NodeHandles::Auto);
    }
}
