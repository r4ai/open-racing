//! The tools' settings: which tool is active, what the view draws over the track,
//! snapping and proportional editing.

use glam::DVec2;
use open_racing_track_project::Node;

use super::Mode;

/// The tool a left click or drag uses, picked in the toolbar (T).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolKind {
    #[default]
    Select,
    Move,
    Rotate,
    Scale,
    /// A click adds a node to the selected road or spline.
    AddNode,
    /// Clicks measure the distance between two points.
    Measure,
    /// Strokes shape the terrain.
    Sculpt,
    /// Strokes paint the terrain's ground layers.
    Paint,
    /// Strokes paint models over the ground: woods, bushes, rocks.
    Scatter,
}

impl ToolKind {
    pub const ALL: [ToolKind; 9] = [
        ToolKind::Select,
        ToolKind::Move,
        ToolKind::Rotate,
        ToolKind::Scale,
        ToolKind::AddNode,
        ToolKind::Measure,
        ToolKind::Sculpt,
        ToolKind::Paint,
        ToolKind::Scatter,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ToolKind::Select => "Select Box",
            ToolKind::Move => "Move",
            ToolKind::Rotate => "Rotate",
            ToolKind::Scale => "Scale",
            ToolKind::AddNode => "Add Node",
            ToolKind::Measure => "Measure",
            ToolKind::Sculpt => "Sculpt Terrain",
            ToolKind::Paint => "Paint Ground",
            ToolKind::Scatter => "Scatter",
        }
    }

    /// The transform its gizmo starts.
    pub(super) fn mode(self) -> Option<Mode> {
        match self {
            ToolKind::Move => Some(Mode::Grab),
            ToolKind::Rotate => Some(Mode::Rotate),
            ToolKind::Scale => Some(Mode::Scale),
            _ => None,
        }
    }

    /// Whether it is a brush: dragging paints strokes over the ground.
    pub fn is_brush(self) -> bool {
        matches!(self, ToolKind::Sculpt | ToolKind::Paint | ToolKind::Scatter)
    }
}

/// What the view draws besides the track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Overlays {
    /// Roads' and splines' lines and nodes.
    pub lines: bool,
    /// Names of roads, splines and props.
    pub names: bool,
    /// Numbers of the selected line's nodes.
    pub indices: bool,
    pub markers: bool,
    /// Stretches of the selected road's strips and barriers.
    pub stretches: bool,
    pub props: bool,
    /// The scatters' models (woods, bushes): off, the view is lighter to work in.
    pub scatter: bool,
    /// Lines, nodes and handles drawn through the ground and what stands on it, as
    /// Blender's X-ray (Alt Z): nodes under a sculpted hill stay in sight.
    pub xray: bool,
    /// The view lit by the project's sky and light (the World tab), rather than evenly.
    pub sky: bool,
}

impl Default for Overlays {
    fn default() -> Self {
        Self {
            lines: true,
            names: true,
            indices: true,
            markers: true,
            stretches: true,
            props: true,
            scatter: true,
            xray: false,
            sky: true,
        }
    }
}

/// What snapping steps to while transforming, and what a node or stretch end dragged on
/// its own catches on (the sidebar's Tool tab).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snapping {
    /// Grid moves land on, m.
    pub grid: f64,
    /// Turns, degrees.
    pub angle: f64,
    /// Scale factors.
    pub factor: f64,
    /// Heights, widths, distances and places along a road (in `u`).
    pub fine: f64,
    /// Nodes catch on other lines' nodes (joining them).
    pub nodes: bool,
    /// Nodes catch on roads' edges (kerbs, walls) and centre lines (a pit lane's ends).
    pub edges: bool,
    /// Stretch ends catch on nodes and corners' entries, apexes and exits.
    pub corners: bool,
    /// How near a node must come to what it catches on, m.
    pub reach: f64,
    /// How near along the road a stretch end must come, m.
    pub along: f64,
}

impl Default for Snapping {
    fn default() -> Self {
        Self {
            grid: 1.0,
            angle: 5.0,
            factor: 0.1,
            fine: 0.1,
            nodes: true,
            edges: true,
            corners: true,
            reach: 2.5,
            along: 4.0,
        }
    }
}

fn step(v: f64, by: f64) -> f64 {
    if by > 0.0 { (v / by).round() * by } else { v }
}

impl Snapping {
    pub fn grid(&self, p: DVec2) -> DVec2 {
        DVec2::new(step(p.x, self.grid), step(p.y, self.grid))
    }

    /// A turn in radians, stepped.
    pub fn angle(&self, a: f64) -> f64 {
        step(a.to_degrees(), self.angle).to_radians()
    }

    pub fn factor(&self, k: f64) -> f64 {
        step(k, self.factor)
    }

    /// A height, width or distance, m, or a place along a road.
    pub fn fine(&self, v: f64) -> f64 {
        step(v, self.fine)
    }

    pub fn height(&self, v: f64) -> f64 {
        self.fine(v)
    }
}

/// How a proportional edit's pull fades towards the edge of its reach, as Blender's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Falloff {
    #[default]
    Smooth,
    Sphere,
    Root,
    Linear,
    Sharp,
    Constant,
}

impl Falloff {
    pub const ALL: [Falloff; 6] = [
        Falloff::Smooth,
        Falloff::Sphere,
        Falloff::Root,
        Falloff::Linear,
        Falloff::Sharp,
        Falloff::Constant,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Falloff::Smooth => "Smooth",
            Falloff::Sphere => "Sphere",
            Falloff::Root => "Root",
            Falloff::Linear => "Linear",
            Falloff::Sharp => "Sharp",
            Falloff::Constant => "Constant",
        }
    }

    /// How much of the change a node `d` from the selection gets, with a reach of
    /// `radius`: 1 at the selection, nothing at the edge and beyond.
    pub fn weight(self, d: f64, radius: f64) -> f64 {
        if radius <= 0.0 || d >= radius {
            return 0.0;
        }
        let t = 1.0 - d / radius;
        match self {
            Falloff::Smooth => t * t * (3.0 - 2.0 * t),
            Falloff::Sphere => (2.0 * t - t * t).max(0.0).sqrt(),
            Falloff::Root => t.sqrt(),
            Falloff::Linear => t,
            Falloff::Sharp => t * t,
            Falloff::Constant => 1.0,
        }
    }
}

/// Proportional editing (O): moving nodes pulls the others near them along, less the
/// further away they are, so that a road's shape or its heights change smoothly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Proportional {
    pub on: bool,
    /// How far the pull reaches, m; the wheel changes it while transforming.
    pub radius: f64,
    pub falloff: Falloff,
    /// Distances along the line rather than straight across: a road passing close by
    /// on its way back is not pulled.
    pub connected: bool,
}

impl Default for Proportional {
    fn default() -> Self {
        Self {
            on: false,
            radius: 60.0,
            falloff: Falloff::Smooth,
            connected: true,
        }
    }
}

/// How far each node of a line is from the nearest selected one: along the line
/// through the nodes, or straight across. Selected nodes are 0 away.
pub fn distances(nodes: &[Node], closed: bool, selected: &[usize], connected: bool) -> Vec<f64> {
    let n = nodes.len();
    if selected.is_empty() {
        return vec![f64::INFINITY; n];
    }
    if !connected {
        return nodes
            .iter()
            .map(|a| {
                selected
                    .iter()
                    .map(|&i| a.pos.distance(nodes[i].pos))
                    .fold(f64::INFINITY, f64::min)
            })
            .collect();
    }
    // Distance along the line to each node, and the whole loop's length.
    let mut along = vec![0.0; n];
    for i in 1..n {
        along[i] = along[i - 1] + nodes[i - 1].pos.distance(nodes[i].pos);
    }
    let total = along[n - 1]
        + if closed {
            nodes[n - 1].pos.distance(nodes[0].pos)
        } else {
            0.0
        };
    (0..n)
        .map(|j| {
            selected
                .iter()
                .map(|&i| {
                    let d = (along[j] - along[i]).abs();
                    if closed { d.min(total - d) } else { d }
                })
                .fold(f64::INFINITY, f64::min)
        })
        .collect()
}
