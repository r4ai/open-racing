//! The 3D view, driven like Blender's.
//!
//! - Camera: middle drag orbits (or right drag, or Alt + left drag), Shift + middle pans,
//!   Ctrl + middle and the wheel zoom. Numpad 1, 3 and 7 look from the front, the right
//!   and the top (with Ctrl, from the other side); numpad 5 switches between perspective
//!   and orthographic; numpad . (or F) frames the selection, Home everything.
//! - Selecting: left click picks a node, a handle, a road or a spline; Shift + click adds
//!   nodes to the selection; dragging over empty space draws a box; A selects all of the
//!   selected road's or spline's nodes (in object mode every item), Alt + A none. In
//!   object mode, dragging an item moves it.
//! - Changing: G grabs, R rotates, S scales what is selected, and dragging a node, handle
//!   or marker grabs it. While transforming, X, Y and Z hold to an axis, Shift is fine,
//!   Ctrl snaps, typed numbers give exact values; a click or Enter confirms, a right
//!   click or Esc puts everything back.
//! - Tools (the toolbar, T): with Move, Rotate or Scale the selection shows a gizmo whose
//!   arms, middle or ring start that transform, held to the axis dragged; with Add Node
//!   a click adds a node to the selected road or spline.
//! - Building: E extrudes the active node, Ctrl + click (left or right) adds a node at
//!   the pointer, X or Delete deletes, Shift + D duplicates a spline, Shift + A opens the
//!   add menu to draw a road, kerb, wall or fence, and a right click opens a menu for
//!   what is under the pointer.

use bevy::camera::visibility::RenderLayers;
use bevy::camera::{CameraOutputMode, ScalingMode, Viewport};
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::prelude::*;
use bevy::render::render_resource::BlendState;
use bevy::window::PrimaryWindow;
use bevy_egui::input::EguiWantsInput;
use bevy_egui::{EguiGlobalSettings, PrimaryEguiContext};
use glam::{DVec2, DVec3};
use open_racing_sim::GroundMesh;
use open_racing_track_project::Node;
use open_racing_track_project::curve::{Frame, Sampled, handles, segments};
use open_racing_track_project::ops::{Curve, Op};
use open_racing_track_project::project::{
    Barrier, HandleMode, PaintLine, PropRow, Range, Road, Shape, Side, StationCurve, Strip,
};
use open_racing_track_render::{from_bevy, to_bevy};

use std::path::PathBuf;

use open_racing_track_project::model::Placement;

use crate::presets::{Preset, unique_name, unique_prop_name};
use crate::preview::Built;
use crate::state::{Editor, Item, item_line};
use crate::theme;
use open_racing_track_project::corners::{self, Corner};
use open_racing_track_project::project::Anchor;

mod camera;
mod geometry;
mod gizmos;
mod input;
mod pick;
mod transform;

pub use camera::*;
pub use geometry::PartValue;
use geometry::*;
pub use gizmos::*;
pub use input::*;
pub use pick::*;
pub use transform::*;

/// Screen distance within which the pointer picks a node or handle, logical pixels.
const PICK_RADIUS: f32 = 12.0;
/// How far the pointer moves with a button down before it is a drag, logical pixels.
const DRAG_THRESHOLD: f32 = 4.0;
/// How far above the ground nodes and markers are drawn, m.
const LIFT: f64 = 0.3;
/// Vertical field of view.
const FOV: f32 = 50.0 * std::f32::consts::PI / 180.0;

#[derive(Component)]
pub struct EditorCamera;

/// The orbit: what the camera looks at, from which direction and how far.
#[derive(Resource)]
pub struct Orbit {
    pub focus: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub ortho: bool,
    /// Orthographic only because a numpad view asked for it: orbiting away goes back to
    /// perspective, as Blender's auto perspective does.
    pub auto_ortho: bool,
    /// Walking the main road at a driver's eye height: how far along it, m.
    pub walk: Option<f64>,
    /// Replaying the last test lap: how far into it, s. The view follows the car.
    pub replay: Option<f64>,
    /// Where the view was before local view: focus, yaw, pitch and distance.
    pub before_local: Option<(Vec3, f32, f32, f32)>,
}

impl Default for Orbit {
    fn default() -> Self {
        Self {
            focus: Vec3::ZERO,
            yaw: 0.6,
            pitch: 0.9,
            distance: 700.0,
            ortho: false,
            auto_ortho: false,
            walk: None,
            replay: None,
            before_local: None,
        }
    }
}

/// The part of the window the 3D view covers, logical pixels, and whether the UI has
/// the pointer: both set by the UI each frame.
#[derive(Resource, Default)]
pub struct ViewRect {
    pub rect: Option<Rect>,
    /// The pointer is on a panel's edge, or the UI is dragging something: the view
    /// takes no clicks.
    pub ui_busy: bool,
    /// The UI is dragging something (a panel being resized, a slider): a press the view
    /// took as well is dropped.
    pub ui_dragging: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marker {
    Start,
    Sector(usize),
}

/// What the pointer is over.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    Node(Item, usize),
    /// A handle of a node: the outgoing one (true) or the incoming one.
    Handle(Item, usize, bool),
    Marker(Marker),
    /// An end of a stretch of the selected road's strip or barrier.
    Range(RangeEnd),
    /// The outer edge of a stretch of a strip (its width) or of a barrier (its offset),
    /// in the stretch's middle.
    Reach(RangeEnd),
    /// A road's edge on one side at one of its selected nodes: its width there.
    Edge(usize, usize, Side),
    /// A line painted along the selected road (edit mode): the road and the line.
    Line(usize, usize),
    /// A node of a strip of the selected road: its width and height there.
    StripKey(KeyRef),
    /// A strip of the selected road (edit mode): the road, its side and the strip.
    Strip(usize, Side, usize),
    /// A road's or spline's body.
    Body(Item),
    /// A part of the active tool's gizmo: an axis, or `Free` for its middle or ring.
    Gizmo(Axis),
    /// A handle of a landform of the terrain.
    Landform(usize, LandformHandle),
}

/// What of a landform is dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LandformHandle {
    /// Its middle, or its start when it is stretched along a line.
    Center,
    /// The far end of the line it is stretched along.
    To,
    /// The edge of its full effect: its radius.
    Edge,
}

/// A part of a road limited to stretches of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Strip(Side, usize),
    Barrier(usize),
    /// A row of models beside the road.
    Row(usize),
    /// A line painted along the road.
    Line(usize),
}

/// One node (key) of a strip beside a road.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyRef {
    pub road: usize,
    pub side: Side,
    pub strip: usize,
    pub key: usize,
}

/// One end of one stretch of a road's part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RangeEnd {
    pub road: usize,
    pub part: Part,
    pub range: usize,
    /// The end (`to`) rather than the start (`from`).
    pub to: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Grab,
    Rotate,
    Scale,
    /// Alt + S: a road's width at its nodes (Blender's shrink/fatten).
    Width,
    /// Ctrl + T: a road's bank at its nodes (Blender's tilt).
    Tilt,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Axis {
    #[default]
    Free,
    X,
    Y,
    Z,
}

/// What a transform moves, as it was when it began.
enum Target {
    Nodes {
        item: Item,
        start: Vec<(usize, Node)>,
        /// With proportional editing, the line's other nodes and how far each is from
        /// the nearest selected one, m.
        around: Vec<(usize, Node, f64)>,
    },
    /// Several whole items, in object mode: every node of each line, and props.
    Many {
        lines: Vec<(Item, Vec<(usize, Node)>)>,
        /// (index, place, turn, size)
        props: Vec<(usize, DVec3, f64, f64)>,
    },
    Handle {
        item: Item,
        index: usize,
        out: bool,
        node: DVec3,
        /// The handle's offset from the node.
        start: DVec3,
        other: DVec3,
        handle_mode: HandleMode,
    },
    Marker {
        marker: Marker,
    },
    Landform {
        index: usize,
        handle: LandformHandle,
        start: open_racing_track_project::project::Landform,
    },
    Prop {
        index: usize,
        pos: DVec3,
        yaw: f64,
        scale: f64,
    },
    Range {
        end: RangeEnd,
    },
    /// A strip's width or a barrier's offset, at a stretch of it.
    Reach {
        end: RangeEnd,
    },
    /// A strip's node moved along the road and out (its width), or up (its height),
    /// from the height it had.
    StripKey {
        key: KeyRef,
        height: f64,
    },
    /// A painted line moved across the road, grabbed `s` metres along it.
    Line {
        road: usize,
        line: usize,
        s: f64,
    },
    /// A road's width on one side at some of its nodes.
    Edge {
        road: String,
        index: usize,
        count: usize,
        closed: bool,
        /// The node grabbed, and all it sets.
        node: usize,
        nodes: Vec<usize>,
        side: Side,
        curve: StationCurve,
        /// The other side's width, for setting both alike.
        other: StationCurve,
    },
    /// A road's width or bank at some of its nodes.
    Shape {
        road: String,
        count: usize,
        closed: bool,
        nodes: Vec<usize>,
        left: StationCurve,
        right: StationCurve,
        bank: StationCurve,
    },
}

/// A transform in progress: Blender's G, R and S, or dragging with the mouse.
pub struct Modal {
    pub mode: Mode,
    target: Target,
    /// The pointer where it began, and the pointer motion since (slowed while Shift is
    /// held), in window coordinates.
    start_cursor: Vec2,
    moved: Vec2,
    last_cursor: Vec2,
    /// The point transforms turn and scale about, and grabs slide through.
    pivot: DVec3,
    pub axis: Axis,
    /// A value typed while transforming.
    pub typed: String,
    /// Started by dragging: ends when the button is let go.
    by_drag: bool,
    /// Dragging a road's edge sets both sides alike (B).
    pub both: bool,
    /// What the grabbed node or end has caught on, to show it.
    pub snapped: std::sync::Mutex<Option<DVec3>>,
    /// The operations applied last: the same again (the pointer resting) changes
    /// nothing and starts no rebuild.
    last: Vec<Op>,
}

impl Modal {
    /// Whether proportional editing applies: to nodes being moved, turned or scaled.
    pub fn proportional(&self) -> bool {
        matches!(self.target, Target::Nodes { .. })
            && matches!(self.mode, Mode::Grab | Mode::Rotate | Mode::Scale)
    }
}

/// What the draw tool is laying out.
#[derive(Clone, Debug, PartialEq)]
pub enum DrawKind {
    Road,
    /// A kerb, band or wall of a strip or wall type.
    Spline(Preset),
}

/// The draw tool: a click adds a point, Enter or a right click finishes.
pub struct Draw {
    pub kind: DrawKind,
    pub points: Vec<DVec3>,
}

/// A menu opened in the view, where it was opened.
pub struct Menu {
    /// Window coordinates.
    pub at: Vec2,
    /// The ground under the pointer.
    pub world: Option<DVec3>,
    pub hit: Option<Hit>,
    /// Only the add menu (Shift + A).
    pub add_only: bool,
}

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
}

impl ToolKind {
    pub const ALL: [ToolKind; 6] = [
        ToolKind::Select,
        ToolKind::Move,
        ToolKind::Rotate,
        ToolKind::Scale,
        ToolKind::AddNode,
        ToolKind::Measure,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ToolKind::Select => "Select Box",
            ToolKind::Move => "Move",
            ToolKind::Rotate => "Rotate",
            ToolKind::Scale => "Scale",
            ToolKind::AddNode => "Add Node",
            ToolKind::Measure => "Measure",
        }
    }

    /// The transform its gizmo starts.
    fn mode(self) -> Option<Mode> {
        match self {
            ToolKind::Move => Some(Mode::Grab),
            ToolKind::Rotate => Some(Mode::Rotate),
            ToolKind::Scale => Some(Mode::Scale),
            ToolKind::Select | ToolKind::AddNode | ToolKind::Measure => None,
        }
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

/// The state of the view's tools.
#[derive(Resource, Default)]
pub struct Tool {
    /// Edit mode, as Blender's Tab: the selected line's nodes are shown and picked.
    /// In object mode clicks pick whole roads, kerbs, walls and props.
    pub edit: bool,
    pub active: ToolKind,
    pub overlays: Overlays,
    /// The item under the pointer in the outliner, lit up in the view.
    pub outliner_hover: Option<Item>,
    /// A popup of the UI is open: the view takes no clicks or keys.
    pub blocked: bool,
    /// The terrain's landforms are shown and their handles picked (the Terrain tab is
    /// open).
    pub landforms: bool,
    pub modal: Option<Modal>,
    pub draw: Option<Draw>,
    pub hover: Option<Hit>,
    /// Where the pointer meets the ground, and where a point drawn now would go.
    pub pointer: Option<DVec3>,
    pub draw_at: Option<DVec3>,
    pub menu: Option<Menu>,
    /// Box selection from one corner to the other, window coordinates.
    pub boxing: Option<(Vec2, Vec2)>,
    /// Snap to the grid without holding Ctrl (Ctrl then frees).
    pub snap: bool,
    pub snapping: Snapping,
    pub proportional: Proportional,
    /// What the view is doing, for the header.
    pub hint: String,
    /// A model to place with the next click.
    pub place: Option<PathBuf>,
    /// The points the measure tool was clicked at: none, the start, or both ends.
    pub measure: Vec<DVec3>,
    /// Where the left button went down, on what, and whether it went down with Alt
    /// (orbiting).
    press: Option<(Vec2, Option<Hit>, bool)>,
    /// Right press position and total pointer travel, for distinguishing a menu click
    /// from a camera drag even if the pointer returns to where it began.
    right_press: Option<(Vec2, f32)>,
    middle_press: bool,
}

/// Tab: into edit mode on the selected road or spline, or back to object mode.
pub fn toggle_edit(editor: &mut Editor, tool: &mut Tool) {
    if tool.edit {
        tool.edit = false;
        editor.selection.nodes.clear();
    } else if editor.line().is_some() {
        tool.edit = true;
    } else {
        editor.status = "select a road, kerb or wall to edit its nodes (Tab)".into();
    }
}

/// Keeps the mode in step with the selection: nodes selected (from a graph, say) mean
/// edit mode, and edit mode needs a road or spline.
pub fn sync_mode(editor: &Editor, tool: &mut Tool) {
    if !tool.edit && !editor.selection.nodes.is_empty() {
        tool.edit = true;
    }
    if tool.edit && editor.line().is_none() {
        tool.edit = false;
    }
}

impl Tool {
    /// O: proportional editing on or off; what to tell the user.
    pub fn toggle_proportional(&mut self) -> String {
        self.proportional.on = !self.proportional.on;
        format!(
            "proportional editing {}",
            if self.proportional.on {
                "on: the wheel sets its reach while moving"
            } else {
                "off"
            }
        )
    }

    /// The tools as they start, in edit mode or object mode.
    pub fn editing(edit: bool) -> Self {
        Self { edit, ..default() }
    }

    /// Drops what the tools are doing, keeping their settings: another project was
    /// opened.
    pub fn reset(&mut self) {
        *self = Tool {
            active: self.active,
            overlays: self.overlays,
            snap: self.snap,
            snapping: self.snapping,
            proportional: self.proportional,
            ..default()
        };
    }
}

enum LeftRelease {
    Click(Option<Hit>),
    Box(Rect),
}

fn finish_left(tool: &mut Tool, anywhere: Option<Vec2>, over_view: bool) -> Option<LeftRelease> {
    let press = tool.press.take();
    let boxing = tool.boxing.take();
    match (press, boxing) {
        (Some(_), Some((from, last))) => Some(LeftRelease::Box(Rect::from_corners(
            from,
            anywhere.unwrap_or(last),
        ))),
        (Some((_, hit, false)), None) if over_view => Some(LeftRelease::Click(hit)),
        _ => None,
    }
}

fn finish_right(tool: &mut Tool, over: Option<Vec2>) -> Option<Vec2> {
    let (from, travel) = tool.right_press.take()?;
    let at = over?;
    (travel <= DRAG_THRESHOLD && from.distance(at) <= DRAG_THRESHOLD).then_some(at)
}

/// The camera, for turning between the view and the world.
#[derive(Clone, Copy)]
pub struct View<'a> {
    pub cam: &'a Camera,
    pub t: &'a GlobalTransform,
}

impl View<'_> {
    /// The ray through a point of the view (sim frame).
    fn ray(&self, at: Vec2) -> Option<(DVec3, DVec3)> {
        let ray = self.cam.viewport_to_world(self.t, at).ok()?;
        let origin = from_bevy(ray.origin);
        let dir = from_bevy(ray.origin + ray.direction.as_vec3()) - origin;
        Some((origin, dir.normalize()))
    }

    /// Where the ray through `at` meets the horizontal plane at height `z`.
    fn on_plane(&self, at: Vec2, z: f64) -> Option<DVec3> {
        let (o, d) = self.ray(at)?;
        if d.z.abs() < 1e-6 {
            return None;
        }
        let k = (z - o.z) / d.z;
        (k > 0.0).then(|| o + d * k)
    }

    /// Where the ray through `at` first meets the ground (roads, terrain, splines), or
    /// else the plane at height 0.
    fn on_ground(&self, ground: Option<&GroundMesh>, at: Vec2) -> Option<DVec3> {
        let Some(g) = ground else {
            return self.on_plane(at, 0.0);
        };
        let (o, d) = self.ray(at)?;
        // Marching along the ray, the ground is under the point until the ray passes
        // below it.
        let below = |t: f64| g.raycast_down(o + d * t, 0.0);
        let (mut t0, mut step) = (0.0, 1.0);
        let mut had = below(0.0).is_some();
        while t0 < 30_000.0 {
            let t1 = t0 + step;
            let now = below(t1).is_some();
            if had && !now {
                let (mut a, mut b) = (t0, t1);
                for _ in 0..24 {
                    let m = 0.5 * (a + b);
                    if below(m).is_some() { a = m } else { b = m }
                }
                return below(a).map(|h| h.point);
            }
            had = now;
            t0 = t1;
            step = (t0 * 0.01).max(0.5);
        }
        self.on_plane(at, 0.0)
    }

    pub fn screen(&self, p: DVec3) -> Option<Vec2> {
        self.cam.world_to_viewport(self.t, to_bevy(p)).ok()
    }

    /// Screen motion of a metre up at `p`.
    fn up_on_screen(&self, p: DVec3) -> Option<Vec2> {
        Some(self.screen(p + DVec3::Z)? - self.screen(p)?)
    }
}

/// The cursor in the 3D view, in window coordinates, if it is over it.
fn cursor(window: &Window, rect: &ViewRect) -> Option<Vec2> {
    let p = window.cursor_position()?;
    cursor_in_view(p, rect.rect)
}

fn cursor_in_view(p: Vec2, rect: Option<Rect>) -> Option<Vec2> {
    rect.filter(|r| r.contains(p)).map(|_| p)
}

/// Where a node is drawn: a draped spline's node on the ground under it.
pub fn shown_pos(editor: &Editor, built: &Built, item: Item, pos: DVec3) -> DVec3 {
    let draped =
        matches!(item, Item::Spline(s) if editor.project.splines.get(s).is_some_and(|s| s.drape));
    let ground = built.ground.as_deref().filter(|_| draped);
    drape(ground, pos)
}

fn drape(ground: Option<&GroundMesh>, pos: DVec3) -> DVec3 {
    ground
        .and_then(|g| {
            g.raycast_down(pos, 3.0)
                .or_else(|| g.raycast_down(pos, 1e4))
        })
        .map_or(pos, |h| h.point)
}

fn items(editor: &Editor) -> impl Iterator<Item = Item> {
    let p = &editor.project;
    (0..p.splines.len())
        .map(Item::Spline)
        .chain((0..p.roads.len()).map(Item::Road))
}
#[cfg(test)]
mod tests {
    use super::*;
    use bevy::camera::RenderTargetInfo;
    use open_racing_track_project::Node;

    /// The pointer at `cursor`, with a value typed or not, stepping or not, and Ctrl held
    /// or not.
    fn pointer(view: View, cursor: Vec2, typed: Option<f64>, snap: bool, free: bool) -> Gesture {
        Gesture {
            view,
            cursor,
            typed,
            snap,
            free,
            snapping: Snapping::default(),
            proportional: Proportional::default(),
        }
    }

    #[test]
    fn cursor_keeps_window_coordinates_inside_offset_view() {
        let rect = Rect::from_corners(Vec2::new(200.0, 80.0), Vec2::new(900.0, 600.0));
        assert_eq!(
            cursor_in_view(Vec2::new(250.0, 120.0), Some(rect)),
            Some(Vec2::new(250.0, 120.0))
        );
        assert_eq!(cursor_in_view(Vec2::new(100.0, 120.0), Some(rect)), None);
        let mut tool = Tool::default();
        open_menu(&mut tool, Vec2::new(250.0, 120.0), None, false);
        assert_eq!(
            tool.menu.as_ref().map(|menu| menu.at),
            Some(Vec2::new(250.0, 120.0))
        );
    }

    #[test]
    fn projection_round_trips_with_offset_view_and_dpi_scale() {
        for mut projection in [
            perspective(),
            Projection::Orthographic(OrthographicProjection::default_3d()),
        ] {
            let viewport = Viewport {
                physical_position: UVec2::new(400, 160),
                physical_size: UVec2::new(1400, 1040),
                ..default()
            };
            let mut camera = Camera {
                viewport: Some(viewport),
                ..default()
            };
            camera.computed.target_info = Some(RenderTargetInfo {
                physical_size: UVec2::new(2400, 1600),
                scale_factor: 2.0,
            });
            projection.update(1400.0, 1040.0);
            camera.computed.clip_from_view = projection.get_clip_from_view();
            let transform = GlobalTransform::from(
                Transform::from_xyz(0.0, 100.0, 100.0).looking_at(Vec3::ZERO, Vec3::Y),
            );
            let view = View {
                cam: &camera,
                t: &transform,
            };
            let screen = view.screen(DVec3::ZERO).expect("origin is visible");
            assert!((screen - Vec2::new(550.0, 340.0)).length() < 0.01);
            let (ray_origin, ray_direction) = view.ray(screen).expect("screen ray");
            let to_origin = -ray_origin;
            let miss = to_origin - ray_direction * to_origin.dot(ray_direction);
            assert!(miss.length() < 1e-3);
        }
    }

    #[test]
    fn gizmo_picks_its_middle_and_arms_for_transform_tools_only() {
        let dir =
            std::env::temp_dir().join(format!("open-racing-editor-gizmo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut editor = Editor::open(dir.clone()).unwrap();
        editor.selection.select_node(Item::Road(0), 0);
        let built = Built::default();
        let pivot = selection_pivot(&editor, &built).expect("a node is selected");

        let mut projection = perspective();
        let mut camera = Camera::default();
        camera.computed.target_info = Some(RenderTargetInfo {
            physical_size: UVec2::new(1200, 800),
            scale_factor: 1.0,
        });
        projection.update(1200.0, 800.0);
        camera.computed.clip_from_view = projection.get_clip_from_view();
        let target = to_bevy(pivot);
        let transform = GlobalTransform::from(
            Transform::from_translation(target + Vec3::new(0.0, 60.0, 60.0))
                .looking_at(target, Vec3::Y),
        );
        let view = View {
            cam: &camera,
            t: &transform,
        };
        let (c, size) = gizmo_frame(&editor, &built, transform.translation()).unwrap();
        let middle = view.screen(c).unwrap();
        let arm = view.screen(c + DVec3::X * size * 0.8).unwrap();
        let pick = |tool, at| pick_gizmo(&editor, &built, view, tool, at);
        assert_eq!(pick(ToolKind::Move, middle), Some(Axis::Free));
        assert_eq!(pick(ToolKind::Move, arm), Some(Axis::X));
        assert_eq!(pick(ToolKind::Scale, arm), Some(Axis::X));
        assert_eq!(pick(ToolKind::Move, middle + Vec2::new(0.0, 300.0)), None);
        assert_eq!(pick(ToolKind::Select, middle), None);
        let ring = view.screen(c + DVec3::Y * size * 0.8).unwrap();
        assert_eq!(pick(ToolKind::Rotate, ring), Some(Axis::Free));
        assert_eq!(pick(ToolKind::Rotate, middle), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// An editor on a new project with its roads built, and a camera looking straight
    /// down at `at` from 300 m.
    fn top_down(name: &str, at: DVec3) -> (Editor, Built, Camera, GlobalTransform, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("open-racing-editor-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let editor = Editor::open(dir.clone()).unwrap();
        let built = built_of(&editor);
        let mut projection = perspective();
        let mut camera = Camera::default();
        camera.computed.target_info = Some(RenderTargetInfo {
            physical_size: UVec2::new(1200, 800),
            scale_factor: 1.0,
        });
        projection.update(1200.0, 800.0);
        camera.computed.clip_from_view = projection.get_clip_from_view();
        let target = to_bevy(at);
        let t = GlobalTransform::from(
            Transform::from_translation(target + Vec3::Y * 300.0).looking_at(target, Vec3::NEG_Z),
        );
        (editor, built, camera, t, dir)
    }

    /// What a build of the editor's project knows.
    fn built_of(editor: &Editor) -> Built {
        let scene = open_racing_track_project::bake::build(&editor.project);
        Built {
            corners: (0..editor.project.roads.len())
                .map(|i| corners::of_road(&editor.project, i).1)
                .collect(),
            roads: scene.roads.into_iter().map(|b| b.sampled).collect(),
            ..default()
        }
    }

    #[test]
    fn dragging_a_corner_kerb_s_end_keeps_it_there_as_the_corner_changes() {
        let (mut editor, _, camera, t, dir) = top_down("corner end", DVec3::new(450.0, 130.0, 0.0));
        let (smp, cs) = corners::of_road(&editor.project, 0);
        let kit = corners::Kit::kerbs(&editor.project, None, 1.5);
        let ops = corners::kit_ops(&editor.project, "circuit", &smp, &cs, &cs[0], &kit);
        assert!(editor.apply(ops, None));
        let built = built_of(&editor);
        let view = View {
            cam: &camera,
            t: &t,
        };
        let road = &editor.project.roads[0];
        let i = road.left.iter().position(|s| s.name == "T1 apex").unwrap();
        let before = road.left[i].ranges[0];
        let part = Part::Strip(Side::Left, i);
        let smp = &built.roads[0];
        let at = view
            .screen(range_end_pos(road, smp, part, before.to))
            .unwrap();
        editor.selection.select(Item::Road(0));
        let mut tool = Tool::default();
        let end = RangeEnd {
            road: 0,
            part,
            range: 0,
            to: true,
        };
        start_modal(
            &mut editor,
            &mut tool,
            &built,
            Mode::Grab,
            Some(Hit::Range(end)),
            at,
            true,
        );
        // Its end 20 m further along the road; refitting keeps it there.
        let f = smp.frame_at(smp.s_at(before.to) + 20.0);
        let to = view.screen(f.pos).unwrap();
        let m = tool.modal.as_ref().unwrap();
        let (ops, _) = transform_ops(&editor, &built, m, &pointer(view, to, None, false, true));
        assert!(editor.apply(ops, None));
        let s = &editor.project.roads[0].left[i];
        let moved = smp.s_at(s.ranges[0].to) - smp.s_at(before.to);
        assert!((moved - 20.0).abs() < 3.0, "moved {moved}");
        let shift = s.corner.unwrap().shift;
        assert!(
            shift[0] == 0.0 && (shift[1] - 20.0).abs() < 3.0,
            "{shift:?}"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn dragging_a_road_edge_or_a_strip_s_reach_sets_its_width() {
        let n0 = DVec3::new(250.0, 0.0, 0.0);
        let (mut editor, built, camera, t, dir) = top_down("edge", n0);
        let view = View {
            cam: &camera,
            t: &t,
        };
        let mut tool = Tool::default();
        // Node 1 of the oval lies on its bottom straight, driven towards +x: left is +y.
        editor.selection.select_node(Item::Road(0), 1);
        let smp = &built.roads[0];
        let handle = edge_pos(smp, 1, Side::Left);
        let at = view.screen(handle).unwrap();
        start_modal(
            &mut editor,
            &mut tool,
            &built,
            Mode::Grab,
            Some(Hit::Edge(0, 1, Side::Left)),
            at,
            true,
        );
        let m = tool.modal.as_ref().unwrap();
        let f = smp.frame_at(smp.s_at(1.0));
        let to = view.screen(f.pos + flat_left(&f) * 9.0).unwrap();
        let (ops, readout) =
            transform_ops(&editor, &built, m, &pointer(view, to, None, true, false));
        assert!(readout.contains("9.00"), "{readout}");
        editor.apply(ops, None);
        let r = &editor.project.roads[0];
        let w = r.width_left.eval(1.0, r.period(), true);
        assert!((w - 9.0).abs() < 1e-9, "{w}");
        assert_eq!(r.width_right.eval(1.0, r.period(), true), 6.0);
        // B sets both sides alike.
        let m = tool.modal.as_mut().unwrap();
        m.both = true;
        let (ops, readout) =
            transform_ops(&editor, &built, m, &pointer(view, to, None, true, false));
        assert!(readout.contains("both sides"), "{readout}");
        editor.apply(ops, None);
        let r = &editor.project.roads[0];
        assert!((r.width_right.eval(1.0, r.period(), true) - 9.0).abs() < 1e-9);
        editor.cancel_drag();
        tool.modal = None;

        // The left kerb's stretch round the first corner: drag its outer edge out.
        let road = &editor.project.roads[0];
        let rg = road.left[0].ranges[0];
        let at = view.screen(reach_pos(road, smp, Part::Strip(Side::Left, 0), &rg));
        let end = RangeEnd {
            road: 0,
            part: Part::Strip(Side::Left, 0),
            range: 0,
            to: false,
        };
        start_modal(
            &mut editor,
            &mut tool,
            &built,
            Mode::Grab,
            Some(Hit::Reach(end)),
            at.unwrap_or_default(),
            true,
        );
        let m = tool.modal.as_ref().unwrap();
        let f = smp.frame_at(range_middle(smp, &rg));
        let to = view.screen(f.pos + flat_left(&f) * (f.width_left + 2.0));
        let (ops, _) = transform_ops(
            &editor,
            &built,
            m,
            &pointer(view, to.unwrap(), None, true, false),
        );
        editor.apply(ops, None);
        let w = editor.project.roads[0].left[0].width;
        assert!((w - 2.0).abs() < 0.05, "{w}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_painted_line_is_picked_in_edit_mode_and_dragged_across() {
        let n0 = DVec3::new(250.0, 0.0, 0.0);
        let (mut editor, built, camera, t, dir) = top_down("paint", n0);
        let view = View {
            cam: &camera,
            t: &t,
        };
        editor.selection.select(Item::Road(0));
        let smp = &built.roads[0];
        let f = smp.frame_at(smp.s_at(1.0));
        let line = |e: &Editor| e.project.roads[0].lines[0].clone();
        let on = f.pos + flat_left(&f) * line(&editor).offset;
        let at = view.screen(on).unwrap();
        assert_eq!(
            pick(&editor, &built, view, at, Some(on), true),
            Some(Hit::Line(0, 0))
        );
        assert_ne!(
            pick(&editor, &built, view, at, Some(on), false),
            Some(Hit::Line(0, 0))
        );

        let mut tool = Tool::editing(true);
        tool.pointer = Some(on);
        start_modal(
            &mut editor,
            &mut tool,
            &built,
            Mode::Grab,
            Some(Hit::Line(0, 0)),
            at,
            true,
        );
        let m = tool.modal.as_ref().unwrap();
        // Near the left edge it lies just inside it; elsewhere it steps.
        let to = view.screen(f.pos + flat_left(&f) * 5.85).unwrap();
        let (ops, readout) =
            transform_ops(&editor, &built, m, &pointer(view, to, None, true, false));
        assert!(readout.contains("left edge"), "{readout}");
        assert!(editor.apply(ops, None));
        let l = line(&editor);
        assert!(
            (l.offset - (6.0 - 0.5 * l.width)).abs() < 1e-9,
            "{}",
            l.offset
        );
        let to = view.screen(f.pos + flat_left(&f) * 3.02).unwrap();
        let (ops, _) = transform_ops(&editor, &built, m, &pointer(view, to, None, true, false));
        assert!(editor.apply(ops, None));
        assert!((line(&editor).offset - 3.0).abs() < 1e-9);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_kerb_s_node_widens_and_raises_it_there_only() {
        let (mut editor, built, camera, t, dir) =
            top_down("kerb node", DVec3::new(450.0, 130.0, 0.0));
        let view = View {
            cam: &camera,
            t: &t,
        };
        editor.selection.select(Item::Road(0));
        let smp = &built.roads[0];
        // The left kerb round the first corner (u 1.6 to 4.4): a node at u 3.
        let at = smp.frame_at(smp.s_at(3.0)).pos;
        assert!(add_strip_key(&mut editor, &built, 0, Side::Left, 0, at));
        let keys = editor.project.roads[0].left[0].keys.clone();
        assert_eq!(keys.len(), 3, "{keys:?}");
        let key = KeyRef {
            road: 0,
            side: Side::Left,
            strip: 0,
            key: 1,
        };
        let road = &editor.project.roads[0];
        let handle = key_pos(road, smp, key).unwrap();
        let screen = view.screen(handle + DVec3::Z * LIFT).unwrap();
        assert_eq!(
            pick(&editor, &built, view, screen, Some(handle), false),
            Some(Hit::StripKey(key))
        );

        // Dragged 1.3 m further out: 2.5 m wide there, as it was 10 m away.
        let mut tool = Tool::editing(true);
        start_modal(
            &mut editor,
            &mut tool,
            &built,
            Mode::Grab,
            Some(Hit::StripKey(key)),
            screen,
            true,
        );
        let m = tool.modal.as_ref().unwrap();
        let f = smp.frame_at(smp.s_at(keys[1].u));
        let to = view.screen(handle + flat_left(&f) * 1.3).unwrap();
        let (ops, readout) =
            transform_ops(&editor, &built, m, &pointer(view, to, None, true, true));
        assert!(readout.contains("2.50 m wide"), "{readout}");
        assert!(editor.apply(ops, None));
        let strip = &editor.project.roads[0].left[0];
        assert_eq!(strip.keys[1].width, 2.5);
        assert!((smp.strip_shape(strip, smp.s_at(keys[1].u) + 10.0).0 - 1.2).abs() < 1e-6);
        // Z raises it instead.
        let m = tool.modal.as_mut().unwrap();
        m.axis = Axis::Z;
        let (ops, readout) = transform_ops(
            &editor,
            &built,
            m,
            &pointer(view, to, Some(2.0), true, true),
        );
        assert!(readout.contains("×2.00"), "{readout}");
        assert!(editor.apply(ops, None));
        assert_eq!(editor.project.roads[0].left[0].keys[1].height, 2.0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stretch_ends_catch_on_nodes_and_spline_nodes_on_road_edges() {
        let (editor, built, _, _, dir) = top_down("snaps", DVec3::ZERO);
        let smp = &built.roads[0];
        let s2 = smp.s_at(2.0);
        let (u, what) = range_snap(smp, None, s2 + 2.5, &Snapping::default()).unwrap();
        assert_eq!((u, what.as_str()), (2.0, "node 2"));
        assert!(range_snap(smp, None, s2 + 30.0, &Snapping::default()).is_none());

        // A wall's node near the circuit's right edge.
        let mut editor = editor;
        let spline = crate::presets::named(&editor.project, "concrete wall")
            .unwrap()
            .spline(
                &editor.project,
                vec![DVec3::new(100.0, -20.0, 0.0), DVec3::new(150.0, -20.0, 0.0)],
            )
            .unwrap();
        assert!(editor.apply(vec![Op::PutSpline { spline }], None));
        // 1.5 m outside the right edge, halfway along the first straight.
        let f = smp.frame_at(smp.s_at(0.5));
        let edge = f.pos - flat_left(&f) * f.width_right;
        let near = edge - flat_left(&f) * 1.5;
        let (at, what) = snap_node(
            &editor,
            &built,
            Item::Spline(0),
            0,
            near,
            &Snapping::default(),
        )
        .unwrap();
        assert!(at.distance(edge) < 0.3, "{at:?} {edge:?}");
        assert!(what.contains("Right edge"), "{what}");
        // And onto another line's node, joining them.
        let (at, _) = snap_node(
            &editor,
            &built,
            Item::Spline(0),
            1,
            DVec3::new(249.0, 1.0, 0.0),
            &Snapping::default(),
        )
        .unwrap();
        assert_eq!(at, editor.project.roads[0].nodes[1].pos);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tab_switches_modes_and_clicks_follow_them() {
        let (mut editor, built, _, _, dir) = top_down("modes", DVec3::ZERO);
        let mut tool = Tool::default();
        editor.selection = Default::default();
        toggle_edit(&mut editor, &mut tool);
        assert!(!tool.edit, "nothing to edit");
        editor.selection.select(Item::Road(0));
        toggle_edit(&mut editor, &mut tool);
        assert!(tool.edit);
        // In edit mode empty space drops the nodes only; Tab back drops them too.
        editor.selection.select_node(Item::Road(0), 2);
        click(&mut editor, &built, None, None, false, false, false, true);
        assert_eq!(editor.selection.item, Some(Item::Road(0)));
        assert!(editor.selection.nodes.is_empty());
        editor.selection.select_node(Item::Road(0), 2);
        toggle_edit(&mut editor, &mut tool);
        assert!(!tool.edit && editor.selection.nodes.is_empty());
        // In object mode it drops the selection.
        click(&mut editor, &built, None, None, false, false, false, false);
        assert_eq!(editor.selection.item, None);
        // Nodes selected from elsewhere (a graph) mean edit mode.
        editor.selection.select_node(Item::Road(0), 1);
        sync_mode(&editor, &mut tool);
        assert!(tool.edit);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_selects_every_node_in_edit_mode_and_every_item_in_object_mode() {
        let (mut editor, _, _, _, dir) = top_down("select all", DVec3::ZERO);
        let spline = crate::presets::named(&editor.project, "concrete wall")
            .unwrap()
            .spline(
                &editor.project,
                vec![DVec3::new(0.0, -30.0, 0.0), DVec3::new(50.0, -30.0, 0.0)],
            )
            .unwrap();
        assert!(editor.apply(vec![Op::PutSpline { spline }], None));
        editor.selection.select(Item::Road(0));
        select_all(&mut editor, true);
        assert_eq!(
            editor.selection.nodes.len(),
            editor.project.roads[0].nodes.len()
        );
        select_all(&mut editor, false);
        assert_eq!(editor.selection.item, Some(Item::Road(0)));
        assert_eq!(editor.selection.others, vec![Item::Spline(0)]);
        assert!(editor.selection.nodes.is_empty());
        select_none(&mut editor, false);
        assert_eq!(editor.selection.item, None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn several_items_select_move_and_delete_together() {
        let (mut editor, _, camera, t, dir) = top_down("many", DVec3::new(120.0, -30.0, 0.0));
        for (name, y) in [("a", -30.0), ("b", -45.0)] {
            let spline = crate::presets::named(&editor.project, "concrete wall")
                .unwrap()
                .spline(
                    &editor.project,
                    vec![DVec3::new(100.0, y, 0.0), DVec3::new(150.0, y, 0.0)],
                )
                .unwrap();
            let spline = open_racing_track_project::project::Spline {
                name: name.into(),
                drape: false,
                ..spline
            };
            assert!(editor.apply(vec![Op::PutSpline { spline }], None));
        }
        let built = built_of(&editor);
        let view = View {
            cam: &camera,
            t: &t,
        };
        // Shift + click adds, the one clicked last active; a click alone selects one.
        click(
            &mut editor,
            &built,
            Some(Hit::Body(Item::Spline(0))),
            None,
            false,
            false,
            false,
            false,
        );
        click(
            &mut editor,
            &built,
            Some(Hit::Body(Item::Spline(1))),
            None,
            true,
            false,
            false,
            false,
        );
        assert_eq!(
            editor.selection.items(),
            vec![Item::Spline(1), Item::Spline(0)]
        );
        // A box round both selects both.
        editor.selection = Default::default();
        let corners = [DVec3::new(90.0, -20.0, 0.0), DVec3::new(160.0, -55.0, 0.0)]
            .map(|p| view.screen(p).unwrap());
        let r = Rect::from_corners(corners[0], corners[1]);
        box_select(&mut editor, &built, view, r, false, false);
        let mut items = editor.selection.items();
        items.sort_by_key(|i| format!("{i:?}"));
        assert_eq!(items, vec![Item::Spline(0), Item::Spline(1)]);
        // Grabbed together by 10 m along x.
        let mut tool = Tool::default();
        let at = view.screen(DVec3::new(125.0, -37.5, 0.0)).unwrap();
        start_modal(&mut editor, &mut tool, &built, Mode::Grab, None, at, false);
        let m = tool.modal.as_ref().unwrap();
        let to = view.screen(DVec3::new(135.0, -37.5, 0.0)).unwrap();
        let (ops, readout) =
            transform_ops(&editor, &built, m, &pointer(view, to, None, true, false));
        assert!(readout.contains("2 items"), "{readout}");
        assert!(editor.apply(ops, None));
        editor.end_drag();
        for s in &editor.project.splines {
            assert!((s.nodes[0].pos.x - 110.0).abs() < 0.01, "{:?}", s.nodes[0]);
        }
        // Deleted together; the main road never is.
        editor.selection.others.push(Item::Road(0));
        delete(&mut editor);
        assert!(editor.project.splines.is_empty());
        assert_eq!(editor.project.roads.len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn turning_and_scaling_nodes_takes_their_handles_along() {
        let (mut editor, built, camera, t, dir) = top_down("turn handles", DVec3::ZERO);
        let view = View {
            cam: &camera,
            t: &t,
        };
        assert!(editor.apply(
            vec![Op::SetNodeHandles {
                line: "circuit".into(),
                index: 1,
                mode: HandleMode::Free,
                incoming: DVec3::new(-20.0, 0.0, 0.0),
                outgoing: DVec3::new(30.0, 0.0, 1.0),
            }],
            None
        ));
        editor.selection.item = Some(Item::Road(0));
        editor.selection.nodes = vec![0, 1, 2];
        let mut tool = Tool::editing(true);
        start_modal(
            &mut editor,
            &mut tool,
            &built,
            Mode::Rotate,
            None,
            Vec2::ZERO,
            false,
        );
        let m = tool.modal.as_ref().unwrap();
        let (ops, _) = transform_ops(
            &editor,
            &built,
            m,
            &pointer(view, Vec2::ZERO, Some(90.0), false, false),
        );
        assert!(editor.apply(ops, None));
        let (incoming, outgoing) = handles(&editor.project.roads[0].nodes, true, 1);
        assert!(
            (incoming - DVec3::new(0.0, -20.0, 0.0)).length() < 1e-9,
            "{incoming}"
        );
        assert!(
            (outgoing - DVec3::new(0.0, 30.0, 1.0)).length() < 1e-9,
            "{outgoing}"
        );
        editor.cancel_drag();

        // Scaling by 2 across the plane doubles them, and keeps them level.
        let mut tool = Tool::editing(true);
        start_modal(
            &mut editor,
            &mut tool,
            &built,
            Mode::Scale,
            None,
            Vec2::ZERO,
            false,
        );
        let m = tool.modal.as_ref().unwrap();
        let (ops, _) = transform_ops(
            &editor,
            &built,
            m,
            &pointer(view, Vec2::ZERO, Some(2.0), false, false),
        );
        assert!(editor.apply(ops, None));
        let (_, outgoing) = handles(&editor.project.roads[0].nodes, true, 1);
        assert!(
            (outgoing - DVec3::new(60.0, 0.0, 1.0)).length() < 1e-9,
            "{outgoing}"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn proportional_editing_pulls_the_nodes_near_along_the_line() {
        let (mut editor, built, camera, t, dir) = top_down("proportional", DVec3::ZERO);
        let view = View {
            cam: &camera,
            t: &t,
        };
        let before = editor.project.roads[0].nodes.clone();
        editor.selection.select_node(Item::Road(0), 1);
        let mut tool = Tool::editing(true);
        tool.proportional.on = true;
        tool.proportional.falloff = Falloff::Linear;
        // Node 1 is 250 m from 0 and ~171 m from 2; reach 300 m pulls both.
        tool.proportional.radius = 300.0;
        start_modal(
            &mut editor,
            &mut tool,
            &built,
            Mode::Grab,
            None,
            Vec2::ZERO,
            false,
        );
        tool.modal.as_mut().unwrap().axis = Axis::Y;
        let m = tool.modal.as_ref().unwrap();
        let mut g = pointer(view, Vec2::ZERO, Some(10.0), false, true);
        g.proportional = tool.proportional;
        let (ops, readout) = transform_ops(&editor, &built, m, &g);
        assert!(readout.contains("proportional"), "{readout}");
        assert!(editor.apply(ops, None));
        let after = &editor.project.roads[0].nodes;
        let dy = |i: usize| after[i].pos.y - before[i].pos.y;
        assert!((dy(1) - 10.0).abs() < 1e-9);
        let d0 = before[0].pos.distance(before[1].pos);
        assert!(
            (dy(0) - 10.0 * (1.0 - d0 / 300.0)).abs() < 1e-9,
            "{}",
            dy(0)
        );
        assert!(dy(2) > 0.0 && dy(2) < 10.0);
        // Far along the loop: untouched.
        assert_eq!(dy(5), 0.0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn falloffs_run_from_all_to_nothing() {
        for f in Falloff::ALL {
            assert!((f.weight(0.0, 10.0) - 1.0).abs() < 1e-9, "{f:?}");
            assert_eq!(f.weight(10.0, 10.0), 0.0);
            let mid = f.weight(5.0, 10.0);
            assert!((0.0..=1.0).contains(&mid));
        }
        let nodes: Vec<Node> = (0..5)
            .map(|i| Node::at(i as f64 * 10.0, 0.0, 0.0))
            .collect();
        assert_eq!(
            distances(&nodes, true, &[0], true),
            vec![0.0, 10.0, 20.0, 30.0, 40.0]
        );
    }

    #[test]
    fn mirroring_flips_nodes_and_handles_through_the_middle() {
        let (mut editor, built, _, _, dir) = top_down("mirror", DVec3::ZERO);
        assert!(editor.apply(
            vec![Op::SetNodeHandles {
                line: "circuit".into(),
                index: 1,
                mode: HandleMode::Free,
                incoming: DVec3::new(-20.0, 5.0, 0.0),
                outgoing: DVec3::new(30.0, 0.0, 0.0),
            }],
            None
        ));
        let before = editor.project.roads[0].nodes.clone();
        editor.selection.select(Item::Road(0));
        let pivot = selection_pivot(&editor, &built).unwrap();
        mirror(&mut editor, &Tool::default(), &built, true);
        let after = &editor.project.roads[0].nodes;
        for (a, b) in before.iter().zip(after) {
            assert!((b.pos.x - (2.0 * pivot.x - a.pos.x)).abs() < 1e-9);
            assert_eq!(b.pos.y, a.pos.y);
        }
        let (incoming, outgoing) = handles(after, true, 1);
        assert_eq!(incoming, DVec3::new(20.0, 5.0, 0.0));
        assert_eq!(outgoing, DVec3::new(-30.0, 0.0, 0.0));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn minus_turns_a_typed_value_s_sign_round() {
        let mut typed = String::new();
        for keys in ["1", "2", "-", ".", "5", "-", "-"] {
            type_value(&mut typed, keys);
        }
        assert_eq!(typed, "-12.5");
    }

    #[test]
    fn delete_in_edit_mode_without_nodes_keeps_the_line() {
        let (mut editor, _, _, _, dir) = top_down("delete-edit", DVec3::ZERO);
        let spline = crate::presets::named(&editor.project, "concrete wall")
            .unwrap()
            .spline(
                &editor.project,
                vec![DVec3::new(0.0, -30.0, 0.0), DVec3::new(50.0, -30.0, 0.0)],
            )
            .unwrap();
        assert!(editor.apply(vec![Op::PutSpline { spline }], None));
        editor.selection.select(Item::Spline(0));
        delete_selected(&mut editor, &Tool::editing(true));
        assert_eq!(editor.project.splines.len(), 1);
        delete_selected(&mut editor, &Tool::editing(false));
        assert!(editor.project.splines.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn view_dirs_are_recognised_and_left_for_perspective() {
        let mut orbit = Orbit::default();
        assert_eq!(ViewDir::of(&orbit), None);
        for v in ViewDir::ALL {
            look(&mut orbit, v);
            assert_eq!(ViewDir::of(&orbit), Some(v));
            assert_eq!(
                ViewDir::of(&orbit)
                    .map(ViewDir::opposite)
                    .map(ViewDir::opposite),
                Some(v)
            );
        }
        assert!(orbit.ortho);
        orbit_by(&mut orbit, 0.1, 0.0);
        assert!(
            !orbit.ortho,
            "orbiting away from a numpad view goes back to perspective"
        );
        orbit.ortho = true;
        orbit.auto_ortho = false;
        orbit_by(&mut orbit, 0.1, 0.0);
        assert!(orbit.ortho, "an orthographic view chosen by hand stays");
    }

    #[test]
    fn end_node_has_only_its_nonzero_handle() {
        let nodes = [Node::new(DVec3::ZERO), Node::new(DVec3::X * 30.0)];
        let first: Vec<_> = visible_handles(&nodes, false, 0).collect();
        let last: Vec<_> = visible_handles(&nodes, false, 1).collect();
        assert_eq!(first, vec![(DVec3::X * 10.0, true)]);
        assert_eq!(last, vec![(-DVec3::X * 10.0, false)]);
    }

    #[test]
    fn dragging_handle_respects_aligned_and_free_modes() {
        let (incoming, outgoing) =
            dragged_handles(HandleMode::Aligned, true, DVec3::Y * 5.0, -DVec3::X * 2.0);
        assert_eq!(incoming, -DVec3::Y * 2.0);
        assert_eq!(outgoing, DVec3::Y * 5.0);
        let (incoming, outgoing) =
            dragged_handles(HandleMode::Free, false, -DVec3::Y * 3.0, DVec3::X * 4.0);
        assert_eq!(incoming, -DVec3::Y * 3.0);
        assert_eq!(outgoing, DVec3::X * 4.0);
        let (incoming, outgoing) =
            dragged_handles(HandleMode::Auto, false, -DVec3::Y * 3.0, DVec3::ZERO);
        assert_eq!(incoming, -DVec3::Y * 3.0);
        assert_eq!(outgoing, DVec3::Y * 3.0);
    }

    #[test]
    fn nearest_node_or_handle_wins_and_node_wins_a_tie() {
        let node = Hit::Node(Item::Road(0), 0);
        let handle = Hit::Handle(Item::Road(0), 0, true);
        let mut best = None;
        consider_pick(
            &mut best,
            node,
            Some(Vec2::new(250.0, 120.0)),
            Vec2::new(250.0, 120.0),
        );
        consider_pick(
            &mut best,
            handle,
            Some(Vec2::new(254.0, 120.0)),
            Vec2::new(250.0, 120.0),
        );
        assert_eq!(best.map(|(hit, _)| hit), Some(node));
        let mut best = None;
        consider_pick(
            &mut best,
            node,
            Some(Vec2::new(250.0, 120.0)),
            Vec2::new(254.0, 120.0),
        );
        consider_pick(
            &mut best,
            handle,
            Some(Vec2::new(254.0, 120.0)),
            Vec2::new(254.0, 120.0),
        );
        assert_eq!(best.map(|(hit, _)| hit), Some(handle));
        let mut best = None;
        consider_pick(
            &mut best,
            node,
            Some(Vec2::new(250.0, 120.0)),
            Vec2::new(252.0, 120.0),
        );
        consider_pick(
            &mut best,
            handle,
            Some(Vec2::new(254.0, 120.0)),
            Vec2::new(252.0, 120.0),
        );
        assert_eq!(best.map(|(hit, _)| hit), Some(node));
    }

    #[test]
    fn release_outside_finishes_box_and_clears_press_state() {
        let mut tool = Tool {
            press: Some((Vec2::new(250.0, 120.0), None, false)),
            boxing: Some((Vec2::new(250.0, 120.0), Vec2::new(890.0, 580.0))),
            ..default()
        };
        let Some(LeftRelease::Box(rect)) =
            finish_left(&mut tool, Some(Vec2::new(950.0, 650.0)), false)
        else {
            panic!("box selection should finish outside the view");
        };
        assert_eq!(rect.min, Vec2::new(250.0, 120.0));
        assert_eq!(rect.max, Vec2::new(950.0, 650.0));
        assert!(tool.press.is_none());
        assert!(tool.boxing.is_none());

        tool.press = Some((Vec2::new(250.0, 120.0), None, false));
        assert!(finish_left(&mut tool, None, false).is_none());
        assert!(tool.press.is_none());
    }

    #[test]
    fn right_drag_or_release_outside_does_not_open_menu() {
        let mut tool = Tool {
            right_press: Some((Vec2::new(250.0, 120.0), 8.0)),
            ..default()
        };
        assert_eq!(finish_right(&mut tool, Some(Vec2::new(250.0, 120.0))), None);
        assert!(tool.right_press.is_none());
        tool.right_press = Some((Vec2::new(250.0, 120.0), 0.0));
        assert_eq!(finish_right(&mut tool, None), None);
        assert!(tool.right_press.is_none());
        tool.right_press = Some((Vec2::new(250.0, 120.0), 0.0));
        assert_eq!(
            finish_right(&mut tool, Some(Vec2::new(250.0, 120.0))),
            Some(Vec2::new(250.0, 120.0))
        );
    }
}
