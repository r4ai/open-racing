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
    Barrier, Grid, HandleMode, Key, Mark, MaterialDef, NamedSurface, Node, NodeHandles, PaintLine,
    Pit, Project, Prop, Reference, Road, Shape, Side, Spline, StationCurve, Strip, StripStyle,
    Terrain, WallStyle,
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
    /// Adds a road as given, or replaces the one of the same name.
    PutRoad {
        road: Road,
    },

    /// Cuts a road or spline at node `at`: an open line becomes two, itself up to the
    /// node and `to` from it on; a loop opens there (`to` is not used). What lies along
    /// a road stays where it was.
    SplitLine {
        line: String,
        at: usize,
        #[serde(default)]
        to: String,
    },
    /// Joins open line `with` onto the end of open line `line` (turning either round as
    /// needed so that their nearest ends meet), and removes `with`.
    JoinLines {
        line: String,
        with: String,
    },
    /// Turns a road or spline round, to run the other way: a road's left and right,
    /// banking and corner entries and exits swap with it.
    ReverseLine {
        line: String,
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
    /// Splits each of the segments (segment `i` runs from node `i` to the next) in two
    /// at its middle, with a node that keeps the line's shape; keys, stretches and
    /// markers stay where they were on it.
    Subdivide {
        line: String,
        segments: Vec<usize>,
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
    /// Puts a row of a model beside a road, or replaces the one of the same name.
    PutRow {
        road: String,
        row: crate::project::PropRow,
    },
    RemoveRow {
        road: String,
        name: String,
    },
    /// Paints a mark across a road, or replaces the one of the same name.
    PutMark {
        road: String,
        mark: Mark,
    },
    RemoveMark {
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
    /// Adds a landform to the terrain, or replaces the one of the same name.
    PutLandform {
        landform: crate::project::Landform,
    },
    RemoveLandform {
        name: String,
    },
    /// Sets or removes the reference image the editor shows to trace over.
    SetReference {
        reference: Option<Reference>,
    },
    /// Sets where the project's (0, 0) lies on the Earth.
    SetGeo {
        geo: Option<crate::geo::Geo>,
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
    /// Adds a strip type, or replaces the one of the same name; the strips and splines
    /// made from it take its shape, surface and material (keeping their widths).
    PutStripStyle {
        style: StripStyle,
    },
    /// Removes a strip type; what was made from it keeps its look.
    RemoveStripStyle {
        name: String,
    },
    /// Adds a wall type, or replaces the one of the same name; the barriers and walls
    /// made from it take its shape, material and model (keeping their places).
    PutWallStyle {
        style: WallStyle,
    },
    /// Removes a wall type; what was made from it keeps its look.
    RemoveWallStyle {
        name: String,
    },
    /// Puts a road's strips and barriers laid round corners back round them, as the
    /// road runs now. Every change to a road does this anyway.
    FitCorners {
        road: String,
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

    /// Inserts a node; for a road, gives what became of its spline parameters.
    fn insert(&mut self, index: usize, node: Node) -> Option<ParamMap> {
        match self {
            Line::Road(r) => Some(Box::new(r.insert_node(index, node))),
            Line::Spline(s) => {
                s.nodes.insert(index, node);
                None
            }
        }
    }

    fn remove(&mut self, index: usize) -> Option<ParamMap> {
        match self {
            Line::Road(r) => Some(Box::new(r.remove_node(index))),
            Line::Spline(s) => {
                s.nodes.remove(index);
                None
            }
        }
    }

    /// Splits segment `i` at its middle, keeping the shape.
    fn split(&mut self, i: usize) -> Option<ParamMap> {
        match self {
            Line::Road(r) => Some(Box::new(r.split_segment(i))),
            Line::Spline(s) => {
                s.nodes = crate::project::split_segment(&s.nodes, s.closed, i);
                None
            }
        }
    }
}

/// What an edit of a road's nodes did to its spline parameters.
type ParamMap = Box<dyn Fn(f64) -> f64>;

/// Keeps the race markers on road `road` where they were on it after its nodes changed:
/// the start line and sectors on the main road, the boxes on the pit lane.
fn remap_markers(p: &mut Project, road: &str, f: Option<ParamMap>) {
    let Some(f) = f else { return };
    let m = &mut p.markers;
    if p.main_road == road {
        m.start = f(m.start);
        m.sectors.iter_mut().for_each(|u| *u = f(*u));
        m.sectors.sort_by(f64::total_cmp);
    }
    if let Some(pit) = &mut m.pit
        && pit.road == road
    {
        pit.boxes.iter_mut().for_each(|u| *u = f(*u));
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
        marks: vec![],
        rows: vec![],
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
                        r.marks.clear();
                        r.rows.retain(|w| w.ranges.is_empty() && w.at.is_empty());
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
            Op::PutRoad { road } => put(&mut p.roads, road, |r| &r.name, None),
            Op::SplitLine { line, at, to } => crate::lines::split(p, &line, at, &to)?,
            Op::JoinLines { line, with } => crate::lines::join(p, &line, &with)?,
            Op::ReverseLine { line } => crate::lines::reverse(p, &line)?,
            Op::AddNode { line, pos, before } => {
                let mut l = line_mut(p, &line)?;
                let node = Node::new(pos);
                // At the end through `insert` too, so that a closed road's keys and
                // ranges on its closing segment move onto the new one.
                let at = before.unwrap_or(l.nodes().len());
                if at > l.nodes().len() {
                    l.node_index(&line, at)?;
                }
                let map = l.insert(at, node);
                remap_markers(p, &line, map);
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
                let map = l.remove(i);
                remap_markers(p, &line, map);
            }
            Op::Subdivide { line, segments } => {
                let mut l = line_mut(p, &line)?;
                let n = l.nodes().len();
                let closed = match &l {
                    Line::Road(r) => r.closed,
                    Line::Spline(s) => s.closed,
                };
                let count = crate::curve::segments(n, closed);
                let mut segments = segments;
                segments.sort_unstable();
                segments.dedup();
                if let Some(&bad) = segments.iter().find(|&&i| i >= count) {
                    return Err(Error::Invalid(format!(
                        "\"{line}\" has no segment {bad} (it has {count})"
                    )));
                }
                // From the last back, so that the earlier segments keep their numbers.
                let maps: Vec<Option<ParamMap>> =
                    segments.iter().rev().map(|&i| l.split(i)).collect();
                for map in maps {
                    remap_markers(p, &line, map);
                }
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
            Op::PutRow { road, row } => put(&mut road_mut(p, &road)?.rows, row, |r| &r.name, None),
            Op::RemoveRow { road, name } => {
                remove(&mut road_mut(p, &road)?.rows, "row", &name, |r| &r.name)?
            }
            Op::PutMark { road, mark } => {
                put(&mut road_mut(p, &road)?.marks, mark, |m| &m.name, None)
            }
            Op::RemoveMark { road, name } => {
                remove(&mut road_mut(p, &road)?.marks, "mark", &name, |m| &m.name)?
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
            Op::PutLandform { landform } => {
                put(&mut p.terrain.landforms, landform, |l| &l.name, None)
            }
            Op::RemoveLandform { name } => {
                remove(&mut p.terrain.landforms, "landform", &name, |l| &l.name)?
            }
            Op::SetReference { reference } => p.reference = reference,
            Op::SetGeo { geo } => p.geo = geo,
            Op::PutSurface { surface } => put(&mut p.surfaces, surface, |s| &s.name, None),
            Op::RemoveSurface { name } => remove(&mut p.surfaces, "surface", &name, |s| &s.name)?,
            Op::PutMaterial { material } => put(&mut p.materials, material, |m| &m.name, None),
            Op::RemoveMaterial { name } => {
                remove(&mut p.materials, "material", &name, |m| &m.name)?
            }
            Op::PutStripStyle { style } => {
                for r in &mut p.roads {
                    for s in r.left.iter_mut().chain(&mut r.right) {
                        if s.style.as_deref() == Some(&style.name) {
                            style.restyle(s);
                        }
                    }
                }
                for sp in &mut p.splines {
                    if sp.style.as_deref() != Some(&style.name) {
                        continue;
                    }
                    if let Shape::Band {
                        profile,
                        surface,
                        material,
                        ..
                    } = &mut sp.shape
                    {
                        *profile = style.profile.clone();
                        *surface = style.surface.clone();
                        *material = style.material.clone();
                    }
                }
                put(&mut p.strip_styles, style, |s| &s.name, None);
            }
            Op::RemoveStripStyle { name } => {
                remove(&mut p.strip_styles, "strip type", &name, |s| &s.name)?;
                let styles = p
                    .roads
                    .iter_mut()
                    .flat_map(|r| r.left.iter_mut().chain(&mut r.right).map(|s| &mut s.style));
                for s in styles.chain(p.splines.iter_mut().map(|s| &mut s.style)) {
                    if s.as_deref() == Some(&name) {
                        *s = None;
                    }
                }
            }
            Op::PutWallStyle { style } => {
                for r in &mut p.roads {
                    for b in &mut r.barriers {
                        if b.style.as_deref() == Some(&style.name) {
                            style.restyle(b);
                        }
                    }
                }
                for sp in &mut p.splines {
                    if sp.style.as_deref() != Some(&style.name) {
                        continue;
                    }
                    if let Shape::Wall {
                        height,
                        thickness,
                        material,
                        model,
                        ..
                    } = &mut sp.shape
                    {
                        *height = style.height;
                        *thickness = style.thickness;
                        *material = style.material.clone();
                        *model = style.model.clone();
                    }
                }
                put(&mut p.wall_styles, style, |s| &s.name, None);
            }
            Op::RemoveWallStyle { name } => {
                remove(&mut p.wall_styles, "wall type", &name, |s| &s.name)?;
                let styles = p
                    .roads
                    .iter_mut()
                    .flat_map(|r| r.barriers.iter_mut().map(|b| &mut b.style));
                for s in styles.chain(p.splines.iter_mut().map(|s| &mut s.style)) {
                    if s.as_deref() == Some(&name) {
                        *s = None;
                    }
                }
            }
            Op::FitCorners { road } => {
                let i = p.road_index(&road).ok_or_else(|| missing("road", &road))?;
                crate::corners::fit(p, i);
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
    // What was laid round corners follows the roads' changes, and the main road's
    // corners are numbered from the start line.
    for i in 0..next.roads.len() {
        let r = &next.roads[i];
        let before = project.road(&r.name);
        let numbered_again = r.name == next.main_road
            && (project.main_road != next.main_road || project.markers.start != next.markers.start);
        if r.has_corner_parts() && (before != Some(r) || numbered_again) {
            crate::corners::fit(&mut next, i);
        }
    }
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
            Op::PutRoad { .. } => "PutRoad",
            Op::SplitLine { .. } => "SplitLine",
            Op::JoinLines { .. } => "JoinLines",
            Op::ReverseLine { .. } => "ReverseLine",
            Op::AddNode { .. } => "AddNode",
            Op::MoveNode { .. } => "MoveNode",
            Op::SetNodeHandles { .. } => "SetNodeHandles",
            Op::RemoveNode { .. } => "RemoveNode",
            Op::Subdivide { .. } => "Subdivide",
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
            Op::PutRow { .. } => "PutRow",
            Op::RemoveRow { .. } => "RemoveRow",
            Op::PutMark { .. } => "PutMark",
            Op::RemoveMark { .. } => "RemoveMark",
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
            Op::PutLandform { .. } => "PutLandform",
            Op::RemoveLandform { .. } => "RemoveLandform",
            Op::SetReference { .. } => "SetReference",
            Op::SetGeo { .. } => "SetGeo",
            Op::PutSurface { .. } => "PutSurface",
            Op::RemoveSurface { .. } => "RemoveSurface",
            Op::PutMaterial { .. } => "PutMaterial",
            Op::RemoveMaterial { .. } => "RemoveMaterial",
            Op::PutStripStyle { .. } => "PutStripStyle",
            Op::RemoveStripStyle { .. } => "RemoveStripStyle",
            Op::PutWallStyle { .. } => "PutWallStyle",
            Op::RemoveWallStyle { .. } => "RemoveWallStyle",
            Op::FitCorners { .. } => "FitCorners",
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
    One(Box<Op>),
}

impl OneOrMany {
    fn into_vec(self) -> Vec<Op> {
        match self {
            Self::Many(v) => v,
            Self::One(op) => vec![*op],
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
        let before = p.roads[0].clone();
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
        // The node splits the closing segment; the key stays where it was on it.
        assert!(
            key.u > n - 1.0,
            "on the closing segment's halves: {}",
            key.u
        );
        let moved =
            crate::curve::point(&before, n - 0.5).distance(crate::curve::point(&p.roads[0], key.u));
        assert!(moved < 3.0, "moved {moved} m");

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
        assert!(
            k.u > n - 2.0 && k.u < n - 1.0,
            "on the joined closing segment, not squashed onto 0: {}",
            k.u
        );
        assert!(
            bank.windows(2).all(|w| w[0].u <= w[1].u),
            "keys stay in order"
        );
    }

    #[test]
    fn subdividing_keeps_the_shape_and_what_lies_on_it() {
        let mut p = Project::new("t");
        apply_all(
            &mut p,
            &[
                Op::SetNodeHandles {
                    line: "circuit".into(),
                    index: 2,
                    mode: HandleMode::Free,
                    incoming: DVec3::new(-40.0, -10.0, 0.0),
                    outgoing: DVec3::new(50.0, 30.0, 2.0),
                },
                Op::SetKey {
                    road: "circuit".into(),
                    curve: Curve::Bank,
                    u: 2.75,
                    value: 0.1,
                },
            ],
        )
        .unwrap();
        let before = p.roads[0].clone();
        let start = p.markers.start;
        apply_all(
            &mut p,
            &[Op::Subdivide {
                line: "circuit".into(),
                segments: vec![2, 0],
            }],
        )
        .unwrap();
        let after = &p.roads[0];
        assert_eq!(after.nodes.len(), before.nodes.len() + 2);
        // Old u on segment 2 lies at new u 3 + 2 (u - 2) once segment 0 is split too.
        for k in 0..=10 {
            let u = 2.0 + k as f64 / 10.0;
            let v = 3.0 + 2.0 * (u - 2.0);
            let (a, b) = (
                crate::curve::point(&before, u),
                crate::curve::point(after, v),
            );
            assert!(a.distance(b) < 1e-9, "u {u}: {a} vs {b}");
        }
        let key = after.bank.keys.iter().find(|k| k.value == 0.1).unwrap();
        assert_eq!(key.u, 4.5);
        assert_eq!(p.markers.start, 2.0 * start, "the start line stays put");
    }

    #[test]
    fn node_edits_keep_markers_and_corner_parts_where_they_were() {
        let mut p = Project::new("t");
        let (smp, cs) = crate::corners::of_road(&p, 0);
        let kit = crate::corners::Kit::kerbs(&p, None, 1.5);
        let ops = crate::corners::kit_ops(&p, "circuit", &smp, &cs, &cs[1], &kit);
        apply_all(&mut p, &ops).unwrap();
        let names = |p: &Project| -> Vec<String> {
            let r = &p.roads[0];
            let strips = r.left.iter().chain(&r.right).filter(|s| s.corner.is_some());
            let mut v: Vec<String> = strips.map(|s| s.name.clone()).collect();
            v.sort();
            v
        };
        let (laid, start, sectors) = (names(&p), p.markers.start, p.markers.sectors.clone());
        assert!(laid.iter().all(|n| n.starts_with("T2 ")), "{laid:?}");
        // A node in the middle of the first straight: the corners stay as they were.
        apply_all(
            &mut p,
            &[Op::AddNode {
                line: "circuit".into(),
                pos: DVec3::new(125.0, 0.0, 0.0),
                before: Some(1),
            }],
        )
        .unwrap();
        // The start line lay where the node went in.
        assert!((p.markers.start - 1.0).abs() < 0.01, "{}", p.markers.start);
        assert!(start == 0.5);
        assert_eq!(p.markers.sectors[0], sectors[0] + 1.0);
        assert_eq!(names(&p), laid, "still round the second corner");
        apply_all(
            &mut p,
            &[Op::RemoveNode {
                line: "circuit".into(),
                index: 1,
            }],
        )
        .unwrap();
        assert_eq!(p.markers.sectors, sectors);
        assert_eq!(names(&p), laid);
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
