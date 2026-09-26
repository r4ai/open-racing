//! The 3D view, driven like Blender's.
//!
//! - Camera: middle drag orbits (or right drag, or Alt + left drag), Shift + middle pans,
//!   Ctrl + middle and the wheel zoom (towards the pointer). Holding the right button with
//!   W A S D (Q, E down and up) flies, the mouse looking round and the wheel setting the
//!   speed. Numpad 1, 3 and 7 look from the front, the right and the top (with Ctrl, from
//!   the other side); numpad 5 switches between perspective and orthographic; numpad .
//!   (or F) frames the selection, Home everything.
//! - Selecting: left click picks a node, a handle, a road or a spline; Shift + click adds
//!   nodes to the selection; dragging over empty space draws a box; A selects all of the
//!   selected road's or spline's nodes (in object mode every item), Alt + A none; C
//!   selects by painting with a circle. In object mode, dragging an item moves it.
//! - Changing: G grabs, R rotates, S scales what is selected, and dragging a node, handle
//!   or marker grabs it. While transforming, X, Y and Z hold to an axis, Shift is fine,
//!   Ctrl snaps, typed numbers give exact values; a click or Enter confirms, a right
//!   click or Esc puts everything back.
//! - Tools (the toolbar, T): with Move, Rotate or Scale the selection shows a gizmo whose
//!   arms, middle or ring start that transform, held to the axis dragged; with Add Node
//!   a click adds a node to the selected road or spline.
//! - Building: E extrudes the active node, Ctrl + click (left or right) adds a node at
//!   the pointer, X or Delete deletes, Shift + D duplicates a spline, Shift + A opens the
//!   add menu to draw a road, kerb, wall, fence or area (click points, or drag to
//!   sketch), Alt + S sets a kerb's width or a wall's height at its nodes, and a right
//!   click opens a menu for what is under the pointer.
//! - Brushes (the toolbar): Sculpt Terrain, Paint Ground and Scatter paint strokes over
//!   the ground, see `crate::brush`.

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
mod settings;
#[cfg(test)]
mod tests;
mod transform;

pub use camera::*;
pub use geometry::PartValue;
use geometry::*;
pub use gizmos::*;
pub use input::*;
pub use pick::*;
pub use settings::*;
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
    /// The wheel zooms towards what the pointer is over, not the view's middle.
    pub zoom_to_pointer: bool,
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
            zoom_to_pointer: true,
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
    /// Alt + S: a road's width at its nodes, or a spline's radius (Blender's
    /// shrink/fatten).
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
    /// A spline's radius at some of its nodes (Alt S): its kerb's width or its wall's
    /// height there.
    Radius {
        line: String,
        /// Each node and its radius as it was.
        start: Vec<(usize, f64)>,
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
    /// Sketching freehand while the button is held: the first point of the sketch, and
    /// where the pointer was on screen when the last point went down.
    pub sketch: Option<(usize, Vec2)>,
}

impl Draw {
    pub fn new(kind: DrawKind) -> Self {
        Self {
            kind,
            points: Vec::new(),
            sketch: None,
        }
    }
}

/// How far the pointer moves on screen between the points of a sketch, logical pixels.
const SKETCH_STEP: f32 = 6.0;

/// The fewest points that keep a sketched line within `tolerance` of every point of it,
/// its ends kept: Ramer, Douglas and Peucker's simplification, in plan.
pub fn simplify(points: &[DVec3], tolerance: f64) -> Vec<DVec3> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut stack = vec![(0, points.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let (pa, pb) = (points[a].truncate(), points[b].truncate());
        let ab = pb - pa;
        let far = (a + 1..b)
            .map(|i| {
                let p = points[i].truncate();
                let t = ((p - pa).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
                (i, p.distance(pa + ab * t))
            })
            .max_by(|x, y| x.1.total_cmp(&y.1));
        if let Some((i, d)) = far
            && d > tolerance
        {
            keep[i] = true;
            stack.push((a, i));
            stack.push((i, b));
        }
    }
    points
        .iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(p, _)| *p)
        .collect()
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
    /// A part of a road (its index) under the pointer in the outliner or the
    /// properties, lit up in the view.
    pub part_hover: Option<(usize, Part)>,
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
    /// The brushes' settings and the stroke being painted.
    pub brush: crate::brush::Brushes,
    /// Flying with the right button and W A S D: the speed, m/s.
    pub flying: Option<f32>,
    /// Circle select (C): the circle's radius on screen, logical pixels.
    pub circle: Option<f32>,
    /// The circle's radius as last set, for the next time.
    pub circle_radius: f32,
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
            brush: self.brush.settings(),
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
