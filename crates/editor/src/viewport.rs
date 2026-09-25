//! The 3D view, driven like Blender's.
//!
//! - Camera: middle drag orbits (or right drag, or Alt + left drag), Shift + middle pans,
//!   Ctrl + middle and the wheel zoom. Numpad 1, 3 and 7 look from the front, the right
//!   and the top (with Ctrl, from the other side); numpad 5 switches between perspective
//!   and orthographic; numpad . (or F) frames the selection, Home everything.
//! - Selecting: left click picks a node, a handle, a road or a spline; Shift + click adds
//!   nodes to the selection; dragging over empty space draws a box; A selects all of the
//!   selected road's or spline's nodes, Alt + A none.
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
use open_racing_track_project::curve::{Frame, Sampled, handles, segments};
use open_racing_track_project::ops::{Curve, Op};
use open_racing_track_project::project::{HandleMode, Range, Road, Shape, Side, StationCurve};
use open_racing_track_render::{from_bevy, to_bevy};

use std::path::PathBuf;

use open_racing_track_project::model::Placement;

use crate::presets::{PRESETS, unique_name, unique_prop_name};
use crate::preview::Built;
use crate::state::{Editor, Item, item_line};
use open_racing_track_project::corners::Corner;

/// Screen distance within which the pointer picks a node or handle, logical pixels.
const PICK_RADIUS: f32 = 12.0;
/// How far the pointer moves with a button down before it is a drag, logical pixels.
const DRAG_THRESHOLD: f32 = 4.0;
/// How far above the ground nodes and markers are drawn, m.
const LIFT: f64 = 0.3;
/// Vertical field of view.
const FOV: f32 = 50.0 * std::f32::consts::PI / 180.0;
/// How close to a road's edge a kerb being drawn snaps onto it, m.
const EDGE_SNAP: f64 = 2.5;

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
        }
    }
}

/// A driver's eye height above the road, m, and how far ahead the eye looks.
const EYE: f64 = 1.1;
const LOOK_AHEAD: f64 = 25.0;

/// The main road as last built, for walking it.
pub fn main_sampled<'a>(editor: &Editor, built: &'a Built) -> Option<&'a Sampled> {
    let p = &editor.project;
    p.road_index(&p.main_road).and_then(|i| built.roads.get(i))
}

/// Where the camera is and what it looks at, `s` along a road, at a driver's eye.
fn walk_view(smp: &Sampled, s: f64) -> Transform {
    let f = smp.frame_at(s);
    let ahead = smp.frame_at(s + LOOK_AHEAD);
    let eye = f.pos + f.normal * EYE;
    let at = ahead.pos + ahead.normal * EYE;
    Transform::from_translation(to_bevy(eye)).looking_at(to_bevy(at), to_bevy(f.normal))
}

/// Starts walking the main road at the start line, or stops.
pub fn toggle_walk(editor: &Editor, built: &Built, orbit: &mut Orbit) {
    orbit.walk = match orbit.walk {
        Some(_) => None,
        None => main_sampled(editor, built).map(|smp| smp.s_at(editor.project.markers.start)),
    };
}

/// The part of the window the 3D view covers, logical pixels, set by the UI each frame.
#[derive(Resource, Default)]
pub struct ViewRect(pub Option<Rect>);

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
    /// A road's or spline's body.
    Body(Item),
    /// A part of the active tool's gizmo: an axis, or `Free` for its middle or ring.
    Gizmo(Axis),
}

/// A part of a road limited to stretches of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Strip(Side, usize),
    Barrier(usize),
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

/// The parts of a road limited to stretches, with the stretches.
fn parts(road: &Road) -> impl Iterator<Item = (Part, &[Range])> {
    let strips = [Side::Left, Side::Right].into_iter().flat_map(move |side| {
        road.strips(side)
            .iter()
            .enumerate()
            .map(move |(i, s)| (Part::Strip(side, i), s.ranges.as_slice()))
    });
    let barriers = road
        .barriers
        .iter()
        .enumerate()
        .map(|(i, b)| (Part::Barrier(i), b.ranges.as_slice()));
    strips.chain(barriers).filter(|(_, r)| !r.is_empty())
}

/// Width of a side's strips inside strip `i` at frame `f`: each as wide as it is there,
/// so none where it is limited to stretches elsewhere, as the road is built.
fn inner_width(road: &Road, smp: &Sampled, side: Side, i: usize, f: &Frame) -> f64 {
    road.strips(side)[..i]
        .iter()
        .map(|s| s.width * smp.presence(&s.ranges, s.fade, f.s))
        .sum()
}

fn edge_of(f: &Frame, side: Side) -> f64 {
    match side {
        Side::Left => f.width_left,
        Side::Right => f.width_right,
    }
}

/// How far to the left of the road's centre a part lies in frame `f`: a strip's middle,
/// a barrier's face.
fn part_offset(road: &Road, smp: &Sampled, part: Part, f: &Frame) -> f64 {
    match part {
        Part::Strip(side, i) => {
            let inner = inner_width(road, smp, side, i, f);
            side.sign() * (edge_of(f, side) + inner + 0.5 * road.strips(side)[i].width)
        }
        Part::Barrier(i) => {
            let b = &road.barriers[i];
            b.side.sign() * (edge_of(f, b.side) + b.offset)
        }
    }
}

/// How far to the left of the road's centre a part's outer edge lies: a strip's far
/// side, a barrier's face.
fn part_reach(road: &Road, smp: &Sampled, part: Part, f: &Frame) -> f64 {
    match part {
        Part::Strip(side, i) => {
            let inner = inner_width(road, smp, side, i, f);
            side.sign() * (edge_of(f, side) + inner + road.strips(side)[i].width)
        }
        Part::Barrier(_) => part_offset(road, smp, part, f),
    }
}

/// Distance along the road to a stretch's middle.
fn range_middle(smp: &Sampled, rg: &Range) -> f64 {
    let (a, mut b) = (smp.s_at(rg.from), smp.s_at(rg.to));
    if b < a && smp.closed {
        b += smp.length;
    }
    0.5 * (a + b)
}

/// Where the handle on a stretch's outer edge is drawn.
fn reach_pos(road: &Road, smp: &Sampled, part: Part, rg: &Range) -> DVec3 {
    let f = smp.frame_at(range_middle(smp, rg));
    f.pos + flat_left(&f) * part_reach(road, smp, part, &f)
}

/// A frame's lateral, level.
fn flat_left(f: &Frame) -> DVec3 {
    f.lateral.truncate().normalize_or(DVec2::Y).extend(0.0)
}

/// Where the handle on a road's edge at node `n` is drawn.
fn edge_pos(smp: &Sampled, n: usize, side: Side) -> DVec3 {
    let f = smp.frame_at(smp.s_at(n as f64));
    let w = match side {
        Side::Left => f.width_left,
        Side::Right => f.width_right,
    };
    f.pos + flat_left(&f) * (side.sign() * w)
}

/// Where a stretch's end is drawn.
fn range_end_pos(road: &Road, smp: &Sampled, part: Part, u: f64) -> DVec3 {
    let f = smp.frame_at(smp.s_at(u));
    f.pos + f.lateral * part_offset(road, smp, part, &f)
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
        start: Vec<(usize, DVec3)>,
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
}

/// What the draw tool is laying out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawKind {
    Road,
    /// A spline of `PRESETS[i]`.
    Spline(usize),
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

/// The state of the view's tools.
#[derive(Resource, Default)]
pub struct Tool {
    pub active: ToolKind,
    pub overlays: Overlays,
    /// The item under the pointer in the outliner, lit up in the view.
    pub outliner_hover: Option<Item>,
    /// A popup of the UI is open: the view takes no clicks or keys.
    pub blocked: bool,
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

impl Tool {
    /// Drops what the tools are doing, keeping their settings: another project was
    /// opened.
    pub fn reset(&mut self) {
        *self = Tool {
            active: self.active,
            overlays: self.overlays,
            snap: self.snap,
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

pub fn setup(mut commands: Commands, mut egui: ResMut<EguiGlobalSettings>) {
    // The UI gets a camera of its own over the whole window; the 3D camera's viewport
    // is what the panels leave free.
    egui.auto_create_primary_context = false;
    commands.spawn((
        PrimaryEguiContext,
        Camera2d,
        RenderLayers::none(),
        Camera {
            order: 1,
            output_mode: CameraOutputMode::Write {
                blend_state: Some(BlendState::ALPHA_BLENDING),
                clear_color: ClearColorConfig::None,
            },
            clear_color: ClearColorConfig::Custom(Color::NONE),
            ..default()
        },
    ));
    commands.spawn((
        EditorCamera,
        Camera3d::default(),
        perspective(),
        Transform::default(),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 20_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(0.0, 1.0, 0.0).looking_at(Vec3::new(-0.4, 0.0, 0.5), Vec3::Y),
    ));
    commands.insert_resource(GlobalAmbientLight {
        brightness: 2500.0,
        ..default()
    });
    commands.insert_resource(ClearColor(Color::srgb(0.55, 0.7, 0.88)));
}

fn perspective() -> Projection {
    Projection::Perspective(PerspectiveProjection {
        fov: FOV,
        near: 0.5,
        far: 20_000.0,
        ..default()
    })
}

/// Keeps the camera on its orbit and inside the 3D view's rectangle.
pub fn place_camera(
    orbit: Res<Orbit>,
    rect: Res<ViewRect>,
    editor: Res<Editor>,
    built: Res<Built>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut camera: Single<(&mut Camera, &mut Transform, &mut Projection), With<EditorCamera>>,
) {
    let (cam, transform, projection) = &mut *camera;
    if let Some(s) = orbit.walk
        && let Some(smp) = main_sampled(&editor, &built)
    {
        **transform = walk_view(smp, s);
        if matches!(**projection, Projection::Orthographic(_)) {
            **projection = perspective();
        }
        set_viewport(cam, &rect, &window);
        return;
    }
    let dir = Vec3::new(
        orbit.pitch.cos() * orbit.yaw.cos(),
        orbit.pitch.sin(),
        orbit.pitch.cos() * orbit.yaw.sin(),
    );
    **transform = Transform::from_translation(orbit.focus + dir * orbit.distance)
        .looking_at(orbit.focus, Vec3::Y);
    let ortho = matches!(**projection, Projection::Orthographic(_));
    if orbit.ortho {
        let height = 2.0 * orbit.distance * (0.5 * FOV).tan();
        if let Projection::Orthographic(o) = &mut **projection {
            o.scaling_mode = ScalingMode::FixedVertical {
                viewport_height: height,
            };
        } else {
            **projection = Projection::Orthographic(OrthographicProjection {
                scaling_mode: ScalingMode::FixedVertical {
                    viewport_height: height,
                },
                near: -20_000.0,
                far: 20_000.0,
                ..OrthographicProjection::default_3d()
            });
        }
    } else if ortho {
        **projection = perspective();
    }
    set_viewport(cam, &rect, &window);
}

/// Keeps the 3D camera's viewport on the part of the window the view covers.
fn set_viewport(cam: &mut Camera, rect: &ViewRect, window: &Window) {
    if let Some(r) = rect.0 {
        let scale = window.scale_factor();
        let pos = (r.min * scale).max(Vec2::ZERO).as_uvec2();
        let size = (r.size() * scale).max(Vec2::ONE).as_uvec2();
        let max = UVec2::new(window.physical_width(), window.physical_height());
        if size.x > 1 && size.y > 1 && pos.x + size.x <= max.x && pos.y + size.y <= max.y {
            cam.viewport = Some(Viewport {
                physical_position: pos,
                physical_size: size,
                ..default()
            });
        }
    }
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
    cursor_in_view(p, rect.0)
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

/// What is under the pointer: range ends, the nearest node or active handle,
/// then markers and the roads and splines themselves.
fn pick(
    editor: &Editor,
    built: &Built,
    view: View,
    at: Vec2,
    ground_at: Option<DVec3>,
) -> Option<Hit> {
    let near = |p: DVec3, r: f32| view.screen(p).map(|s| s.distance(at)).filter(|&d| d < r);
    let sel = &editor.selection;
    // Stretch ends of the selected road's strips and barriers.
    if let Some(r) = sel.road()
        && let (Some(road), Some(smp)) = (editor.project.roads.get(r), built.roads.get(r))
    {
        for (part, ranges) in parts(road) {
            for (range, rg) in ranges.iter().enumerate() {
                for (to, u) in [(false, rg.from), (true, rg.to)] {
                    let p = range_end_pos(road, smp, part, u) + DVec3::Z * LIFT;
                    if near(p, PICK_RADIUS).is_some() {
                        return Some(Hit::Range(RangeEnd {
                            road: r,
                            part,
                            range,
                            to,
                        }));
                    }
                }
                let p = reach_pos(road, smp, part, rg) + DVec3::Z * LIFT;
                if near(p, PICK_RADIUS).is_some() {
                    return Some(Hit::Reach(RangeEnd {
                        road: r,
                        part,
                        range,
                        to: false,
                    }));
                }
            }
        }
        // The road's edges at its selected nodes.
        for &n in sel.nodes.iter().filter(|&&n| n < road.nodes.len()) {
            for side in [Side::Left, Side::Right] {
                let p = edge_pos(smp, n, side) + DVec3::Z * LIFT;
                if near(p, PICK_RADIUS).is_some() {
                    return Some(Hit::Edge(r, n, side));
                }
            }
        }
    }
    let mut best: Option<(Hit, f32)> = None;
    for item in items(editor) {
        let Some((_, nodes, _)) = item_line(&editor.project, item) else {
            continue;
        };
        for (i, node) in nodes.iter().enumerate() {
            let p = shown_pos(editor, built, item, node.pos) + DVec3::Z * LIFT;
            consider_pick(&mut best, Hit::Node(item, i), view.screen(p), at);
        }
    }
    if let (Some(item), Some(n)) = (sel.item, sel.node())
        && let Some((_, nodes, closed)) = item_line(&editor.project, item)
        && n < nodes.len()
    {
        let base = shown_pos(editor, built, item, nodes[n].pos);
        for (h, out) in visible_handles(nodes, closed, n) {
            consider_pick(
                &mut best,
                Hit::Handle(item, n, out),
                view.screen(base + h + DVec3::Z * LIFT),
                at,
            );
        }
    }
    if let Some((hit, _)) = best {
        return Some(hit);
    }
    // Props, by where they stand.
    for (i, prop) in editor.project.props.iter().enumerate() {
        let at = Placement::of(prop, built.ground.as_deref());
        if near(at.pos + DVec3::Z * LIFT, 1.5 * PICK_RADIUS).is_some() {
            return Some(Hit::Body(Item::Prop(i)));
        }
    }
    // Markers across the main road.
    let p = &editor.project;
    if let Some(main) = p.road_index(&p.main_road).and_then(|i| built.roads.get(i)) {
        let markers = std::iter::once((Marker::Start, p.markers.start)).chain(
            p.markers
                .sectors
                .iter()
                .enumerate()
                .map(|(i, &u)| (Marker::Sector(i), u)),
        );
        for (m, u) in markers {
            let f = main.frame_at(main.s_at(u));
            let ends = [f.width_left, -f.width_right]
                .map(|d| view.screen(f.pos + f.lateral * d + DVec3::Z * LIFT));
            if let [Some(a), Some(b)] = ends
                && distance_to_segment(at, a, b) < 6.0
            {
                return Some(Hit::Marker(m));
            }
        }
    }
    let g = ground_at?;
    body_at(editor, built, g).map(Hit::Body)
}

/// Where the selection's transform gizmo stands: a prop, or the middle of the selected
/// nodes (all of them with none selected, as moving the whole line).
pub fn selection_pivot(editor: &Editor, built: &Built) -> Option<DVec3> {
    let item = editor.selection.item?;
    if let Item::Prop(i) = item {
        let prop = editor.project.props.get(i)?;
        return Some(Placement::of(prop, built.ground.as_deref()).pos);
    }
    let (_, nodes, _) = item_line(&editor.project, item)?;
    let picked = editor.picked_nodes();
    if picked.is_empty() {
        return None;
    }
    let sum: DVec3 = picked
        .iter()
        .map(|&n| shown_pos(editor, built, item, nodes[n].pos))
        .sum();
    Some(sum / picked.len() as f64)
}

/// The gizmo's middle and the length of its arms, about the same on screen at any
/// distance.
fn gizmo_frame(editor: &Editor, built: &Built, eye: Vec3) -> Option<(DVec3, f64)> {
    let p = selection_pivot(editor, built)? + DVec3::Z * LIFT;
    Some((p, (eye.distance(to_bevy(p)) * 0.11).max(1.0) as f64))
}

const GIZMO_AXES: [(Axis, DVec3); 3] = [
    (Axis::X, DVec3::X),
    (Axis::Y, DVec3::Y),
    (Axis::Z, DVec3::Z),
];

fn axis_color(axis: Axis) -> Color {
    match axis {
        Axis::X => Color::srgb(0.96, 0.25, 0.33),
        Axis::Y => Color::srgb(0.53, 0.84, 0.13),
        Axis::Z => Color::srgb(0.18, 0.52, 1.0),
        Axis::Free => Color::srgb(0.9, 0.9, 0.9),
    }
}

/// Points round the rotate gizmo's ring.
fn gizmo_ring(p: DVec3, size: f64) -> impl Iterator<Item = DVec3> {
    (0..=48).map(move |k| {
        let a = k as f64 / 48.0 * std::f64::consts::TAU;
        p + DVec3::new(a.cos(), a.sin(), 0.0) * size * 0.8
    })
}

/// The part of the active tool's gizmo under the pointer.
fn pick_gizmo(
    editor: &Editor,
    built: &Built,
    view: View,
    tool: ToolKind,
    at: Vec2,
) -> Option<Axis> {
    tool.mode()?;
    let (p, size) = gizmo_frame(editor, built, view.t.translation())?;
    let c = view.screen(p)?;
    if tool == ToolKind::Rotate {
        let ring: Vec<Vec2> = gizmo_ring(p, size).filter_map(|q| view.screen(q)).collect();
        let near = ring
            .windows(2)
            .any(|w| distance_to_segment(at, w[0], w[1]) < 8.0);
        return near.then_some(Axis::Free);
    }
    if c.distance(at) < 14.0 {
        return Some(Axis::Free);
    }
    GIZMO_AXES
        .iter()
        .filter_map(|&(axis, dir)| {
            let end = view.screen(p + dir * size)?;
            let d = distance_to_segment(at, c + (end - c) * 0.2, end);
            (d < 9.0).then_some((axis, d))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(axis, _)| axis)
}

fn consider_pick(best: &mut Option<(Hit, f32)>, hit: Hit, screen: Option<Vec2>, at: Vec2) {
    if let Some(distance) = screen.map(|p| p.distance(at))
        && distance < PICK_RADIUS
        && best.is_none_or(|(_, current)| distance < current)
    {
        *best = Some((hit, distance));
    }
}

/// Only handles with a visible length can be picked or drawn.
fn visible_handles(
    nodes: &[open_racing_track_project::Node],
    closed: bool,
    index: usize,
) -> impl Iterator<Item = (DVec3, bool)> {
    let (inc, out) = handles(nodes, closed, index);
    [(inc, false), (out, true)]
        .into_iter()
        .filter(move |(offset, outgoing)| {
            offset.length_squared() > 1e-12
                && (closed
                    || if *outgoing {
                        index + 1 < nodes.len()
                    } else {
                        index > 0
                    })
        })
}

/// Move one handle while keeping the other independent or aligned as requested.
fn dragged_handles(mode: HandleMode, out: bool, moved: DVec3, other: DVec3) -> (DVec3, DVec3) {
    if out {
        let incoming = if mode == HandleMode::Free {
            other
        } else {
            -moved.normalize_or_zero() * other.length()
        };
        (incoming, moved)
    } else {
        let outgoing = if mode == HandleMode::Free {
            other
        } else {
            -moved.normalize_or_zero()
                * if other.length_squared() > 1e-12 {
                    other.length()
                } else {
                    moved.length()
                }
        };
        (moved, outgoing)
    }
}

/// The spline or road whose body covers `p`, splines first.
fn body_at(editor: &Editor, built: &Built, p: DVec3) -> Option<Item> {
    let p2 = p.truncate();
    let lateral = |smp: &Sampled| {
        let f = smp.frames[smp.nearest(p)];
        ((p2 - f.pos.truncate()).dot(f.lateral.truncate()), f)
    };
    for (i, (sp, smp)) in editor
        .project
        .splines
        .iter()
        .zip(&built.splines)
        .enumerate()
    {
        if smp.frames.is_empty() {
            continue;
        }
        let half = match &sp.shape {
            Shape::Band { width, .. } => 0.5 * width,
            Shape::Wall { thickness, .. } => 0.5 * thickness,
        }
        .max(1.0);
        let (d, f) = lateral(smp);
        if d.abs() <= half + 0.5 && (p2 - f.pos.truncate()).length() <= half + smp.length {
            let along = (p2 - f.pos.truncate()).dot(f.tangent.truncate()).abs();
            if along < 2.0 * sp.resolution + 1.0 {
                return Some(Item::Spline(i));
            }
        }
    }
    let mut best: Option<(usize, f64)> = None;
    for (i, smp) in built.roads.iter().enumerate() {
        if smp.frames.is_empty() {
            continue;
        }
        let (d, f) = lateral(smp);
        let inside = d <= f.width_left + 0.5 && -d <= f.width_right + 0.5;
        let dist = (p2 - f.pos.truncate()).length();
        if inside && dist < 60.0 && best.is_none_or(|b| dist < b.1) {
            best = Some((i, dist));
        }
    }
    best.map(|(i, _)| Item::Road(i))
}

fn distance_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Keys pressed this frame that type a value.
fn typed_keys(keys: &ButtonInput<KeyCode>) -> String {
    use KeyCode as K;
    let digits = [
        (K::Digit0, K::Numpad0),
        (K::Digit1, K::Numpad1),
        (K::Digit2, K::Numpad2),
        (K::Digit3, K::Numpad3),
        (K::Digit4, K::Numpad4),
        (K::Digit5, K::Numpad5),
        (K::Digit6, K::Numpad6),
        (K::Digit7, K::Numpad7),
        (K::Digit8, K::Numpad8),
        (K::Digit9, K::Numpad9),
    ];
    let mut s = String::new();
    for (i, (a, b)) in digits.iter().enumerate() {
        if keys.just_pressed(*a) || keys.just_pressed(*b) {
            s.push(char::from(b'0' + i as u8));
        }
    }
    if keys.any_just_pressed([K::Minus, K::NumpadSubtract]) {
        s.push('-');
    }
    if keys.any_just_pressed([K::Period, K::NumpadDecimal]) {
        s.push('.');
    }
    s
}

#[allow(clippy::too_many_arguments)]
pub fn input(
    mut editor: ResMut<Editor>,
    mut orbit: ResMut<Orbit>,
    mut tool: ResMut<Tool>,
    built: Res<Built>,
    rect: Res<ViewRect>,
    wants: Res<EguiWantsInput>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<EditorCamera>>,
) {
    let (cam, t) = *camera;
    let view = View { cam, t };
    let tool = &mut *tool;
    let editor = &mut *editor;
    // Walking the track takes the keys; the view only looks.
    if orbit.walk.is_some() {
        tool.hover = None;
        tool.pointer = None;
        return;
    }
    let pointer_free = !wants.wants_any_pointer_input() && tool.menu.is_none() && !tool.blocked;
    let keys_free = !wants.wants_any_keyboard_input() && !tool.blocked;
    let anywhere = window.cursor_position();
    let over = cursor(&window, &rect).filter(|_| pointer_free);
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let alt = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
    let ground = built.ground.as_deref();

    tool.pointer = over.and_then(|at| view.on_ground(ground, at));
    tool.hover = None;
    tool.draw_at = None;
    tool.hint.clear();

    if buttons.just_pressed(MouseButton::Middle) {
        tool.middle_press = over.is_some();
    } else if !buttons.pressed(MouseButton::Middle) {
        tool.middle_press = false;
    }
    if tool.modal.is_some() || tool.draw.is_some() || tool.place.is_some() {
        tool.right_press = None;
        tool.press = None;
        tool.boxing = None;
    } else if buttons.just_pressed(MouseButton::Right) {
        tool.right_press = over.map(|at| (at, 0.0));
    }
    if let Some((from, travel)) = &mut tool.right_press {
        *travel += motion.delta.length();
        if let Some(at) = anywhere {
            *travel = (*travel).max(from.distance(at));
        }
    }
    camera_input(
        &mut orbit, tool, &buttons, &motion, &scroll, over, t, alt, shift, ctrl,
    );

    // A transform in progress takes every input.
    if tool.modal.is_some() {
        let at = anywhere.unwrap_or_else(|| tool.modal.as_ref().expect("a transform").last_cursor);
        modal(editor, tool, &built, view, &buttons, &keys, at, shift, ctrl);
        return;
    }

    // Drawing a road or spline.
    if tool.draw.is_some() {
        draw(editor, tool, &built, &buttons, &keys, over, ctrl, keys_free);
        return;
    }

    // Placing a model.
    if let Some(model) = &tool.place {
        tool.hint = format!(
            "Place {}: click where it stands · Esc or right click cancels",
            model.display()
        );
        if over.is_some()
            && buttons.just_pressed(MouseButton::Left)
            && let Some(at) = tool.pointer
        {
            crate::assets::place(editor, model, at);
            tool.place = None;
        } else if keys.just_pressed(KeyCode::Escape) || buttons.just_pressed(MouseButton::Right) {
            tool.place = None;
        }
        return;
    }

    let hover = over.and_then(|at| {
        pick_gizmo(editor, &built, view, tool.active, at)
            .map(Hit::Gizmo)
            .or_else(|| pick(editor, &built, view, at, tool.pointer))
    });
    tool.hover = hover;

    // Left button: a click selects, a drag grabs what it began on or draws a box.
    if buttons.just_pressed(MouseButton::Left) {
        tool.press = over.map(|at| (at, hover, alt));
    }
    if let Some((from, hit, orbiting)) = tool.press {
        if !buttons.pressed(MouseButton::Left) {
            match finish_left(tool, anywhere, over.is_some()) {
                Some(LeftRelease::Box(rect)) => box_select(editor, &built, view, rect, shift),
                Some(LeftRelease::Click(_)) if tool.active == ToolKind::Measure => {
                    if let Some(p) = tool.pointer {
                        if tool.measure.len() >= 2 {
                            tool.measure.clear();
                        }
                        tool.measure.push(p);
                    }
                }
                Some(LeftRelease::Click(hit)) => {
                    let add = ctrl
                        || (tool.active == ToolKind::AddNode
                            && !matches!(
                                hit,
                                Some(Hit::Node(..) | Hit::Handle(..) | Hit::Gizmo(_))
                            ));
                    click(editor, &built, hit, tool.pointer, shift, add, alt);
                }
                None => {}
            }
        } else if !orbiting
            && let Some(at) = anywhere
            && from.distance(at) > DRAG_THRESHOLD
        {
            match hit {
                Some(Hit::Gizmo(axis)) => {
                    tool.press = None;
                    let mode = tool.active.mode().unwrap_or(Mode::Grab);
                    start_modal(editor, tool, &built, mode, None, from, true);
                    if let Some(m) = &mut tool.modal
                        && m.mode != Mode::Rotate
                    {
                        m.axis = axis;
                    }
                }
                Some(Hit::Body(item @ Item::Prop(_))) => {
                    tool.press = None;
                    editor.selection.select(item);
                    start_modal(editor, tool, &built, Mode::Grab, None, from, true);
                }
                Some(
                    h @ (Hit::Node(..)
                    | Hit::Handle(..)
                    | Hit::Marker(_)
                    | Hit::Range(_)
                    | Hit::Reach(_)
                    | Hit::Edge(..)),
                ) => {
                    tool.press = None;
                    if let Hit::Node(item, n) = h
                        && !(editor.selection.item == Some(item)
                            && editor.selection.nodes.contains(&n))
                    {
                        editor.selection.select_node(item, n);
                    }
                    start_modal(editor, tool, &built, Mode::Grab, Some(h), from, true);
                }
                _ => tool.boxing = Some((from, at)),
            }
        }
    }
    if let (Some((_, end)), Some(at)) = (&mut tool.boxing, anywhere) {
        *end = at;
    }

    // Right button: a click opens the menu (with Ctrl, adds a node there).
    if !buttons.pressed(MouseButton::Right)
        && let Some(at) = finish_right(tool, over)
    {
        if ctrl {
            add_node_at(editor, &built, tool.pointer);
        } else {
            open_menu(tool, at, hover, false);
        }
    }

    let Some(at) = over else { return };
    if !keys_free {
        return;
    }
    let pressed = |k| keys.just_pressed(k);
    if pressed(KeyCode::KeyG) {
        start_modal(editor, tool, &built, Mode::Grab, None, at, false);
    } else if pressed(KeyCode::KeyR) {
        start_modal(editor, tool, &built, Mode::Rotate, None, at, false);
    } else if pressed(KeyCode::KeyS) && alt {
        start_modal(editor, tool, &built, Mode::Width, None, at, false);
    } else if pressed(KeyCode::KeyT) && ctrl {
        start_modal(editor, tool, &built, Mode::Tilt, None, at, false);
    } else if pressed(KeyCode::KeyS) && !ctrl {
        start_modal(editor, tool, &built, Mode::Scale, None, at, false);
    } else if pressed(KeyCode::KeyE) {
        extrude(editor, tool, &built, at);
    } else if pressed(KeyCode::KeyD) && shift {
        duplicate(editor, tool, &built, at);
    } else if pressed(KeyCode::KeyA) && shift {
        open_menu(tool, at, hover, true);
    } else if pressed(KeyCode::KeyA) && alt {
        editor.selection.nodes.clear();
    } else if pressed(KeyCode::KeyA) && !ctrl {
        select_all(editor);
    } else if pressed(KeyCode::KeyX) || pressed(KeyCode::Delete) {
        delete(editor);
    } else if pressed(KeyCode::NumpadAdd) && ctrl {
        crate::edit::select_more(editor);
    } else if pressed(KeyCode::NumpadSubtract) && ctrl {
        crate::edit::select_less(editor);
    } else if pressed(KeyCode::Escape) {
        editor.selection.nodes.clear();
    }
}

#[allow(clippy::too_many_arguments)]
fn camera_input(
    orbit: &mut Orbit,
    tool: &Tool,
    buttons: &ButtonInput<MouseButton>,
    motion: &AccumulatedMouseMotion,
    scroll: &AccumulatedMouseScroll,
    over: Option<Vec2>,
    t: &GlobalTransform,
    alt: bool,
    shift: bool,
    ctrl: bool,
) {
    let d = motion.delta;
    let middle = buttons.pressed(MouseButton::Middle) && tool.middle_press;
    let right =
        buttons.pressed(MouseButton::Right) && tool.right_press.is_some() && tool.modal.is_none();
    let alt_left = alt && tool.press.is_some_and(|p| p.2) && buttons.pressed(MouseButton::Left);
    let dragging = middle || right || alt_left;
    if (over.is_some() || dragging) && d != Vec2::ZERO && dragging {
        if shift {
            let right = t.right().as_vec3();
            let up = t.up().as_vec3();
            let k = orbit.distance * 0.0015;
            orbit.focus += (-right * d.x + up * d.y) * k;
        } else if ctrl {
            orbit.distance = (orbit.distance * (1.0 + d.y * 0.005)).clamp(2.0, 15_000.0);
        } else {
            orbit_by(orbit, d.x * 0.005, d.y * 0.005);
        }
    }
    if over.is_some() && scroll.delta.y != 0.0 {
        orbit.distance =
            (orbit.distance * (1.0 - 0.1 * scroll.delta.y.signum())).clamp(2.0, 15_000.0);
    }
}

/// Turns the orbit, leaving an orthographic numpad view for perspective.
pub fn orbit_by(orbit: &mut Orbit, yaw: f32, pitch: f32) {
    orbit.yaw += yaw;
    orbit.pitch = (orbit.pitch + pitch).clamp(-1.5695, 1.5695);
    if orbit.auto_ortho {
        orbit.ortho = false;
        orbit.auto_ortho = false;
    }
}

/// Looking along an axis: from the top, the front and so on, as Blender's numpad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewDir {
    Top,
    Bottom,
    Front,
    Back,
    Right,
    Left,
}

impl ViewDir {
    pub const ALL: [ViewDir; 6] = [
        ViewDir::Top,
        ViewDir::Bottom,
        ViewDir::Front,
        ViewDir::Back,
        ViewDir::Right,
        ViewDir::Left,
    ];

    /// The orbit's yaw and pitch for it.
    pub fn angles(self) -> (f32, f32) {
        use std::f32::consts::{FRAC_PI_2, PI};
        match self {
            ViewDir::Top => (FRAC_PI_2, 1.5695),
            ViewDir::Bottom => (FRAC_PI_2, -1.5695),
            ViewDir::Front => (FRAC_PI_2, 0.0),
            ViewDir::Back => (-FRAC_PI_2, 0.0),
            ViewDir::Right => (0.0, 0.0),
            ViewDir::Left => (PI, 0.0),
        }
    }

    pub fn opposite(self) -> ViewDir {
        match self {
            ViewDir::Top => ViewDir::Bottom,
            ViewDir::Bottom => ViewDir::Top,
            ViewDir::Front => ViewDir::Back,
            ViewDir::Back => ViewDir::Front,
            ViewDir::Right => ViewDir::Left,
            ViewDir::Left => ViewDir::Right,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ViewDir::Top => "Top",
            ViewDir::Bottom => "Bottom",
            ViewDir::Front => "Front",
            ViewDir::Back => "Back",
            ViewDir::Right => "Right",
            ViewDir::Left => "Left",
        }
    }

    pub fn shortcut(self) -> &'static str {
        match self {
            ViewDir::Top => "Numpad 7",
            ViewDir::Bottom => "Ctrl Numpad 7",
            ViewDir::Front => "Numpad 1",
            ViewDir::Back => "Ctrl Numpad 1",
            ViewDir::Right => "Numpad 3",
            ViewDir::Left => "Ctrl Numpad 3",
        }
    }

    /// The axis view the orbit is in, if any.
    pub fn of(orbit: &Orbit) -> Option<ViewDir> {
        let close = |a: f32, b: f32| {
            let d = (a - b).rem_euclid(std::f32::consts::TAU);
            d.min(std::f32::consts::TAU - d) < 1e-3
        };
        ViewDir::ALL.into_iter().find(|v| {
            let (yaw, pitch) = v.angles();
            (pitch - orbit.pitch).abs() < 1e-3 && (pitch.abs() > 1.5 || close(yaw, orbit.yaw))
        })
    }
}

pub fn look(orbit: &mut Orbit, dir: ViewDir) {
    let (yaw, pitch) = dir.angles();
    set_view(orbit, yaw, pitch);
}

/// Numpad views, framing and switching the projection; also called from the menus.
pub fn view_keys(editor: &Editor, orbit: &mut Orbit, keys: &ButtonInput<KeyCode>) {
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let side = |a, b| if ctrl { b } else { a };
    if keys.just_pressed(KeyCode::Numpad1) {
        look(orbit, side(ViewDir::Front, ViewDir::Back));
    }
    if keys.just_pressed(KeyCode::Numpad3) {
        look(orbit, side(ViewDir::Right, ViewDir::Left));
    }
    if keys.just_pressed(KeyCode::Numpad7) {
        look(orbit, side(ViewDir::Top, ViewDir::Bottom));
    }
    if keys.just_pressed(KeyCode::Numpad5) {
        orbit.ortho = !orbit.ortho;
        orbit.auto_ortho = false;
    }
    // Steps of 15 degrees round the focus, and zooming.
    let step = 15f32.to_radians();
    for (key, yaw, pitch) in [
        (KeyCode::Numpad4, -step, 0.0),
        (KeyCode::Numpad6, step, 0.0),
        (KeyCode::Numpad8, 0.0, step),
        (KeyCode::Numpad2, 0.0, -step),
    ] {
        if keys.just_pressed(key) {
            orbit_by(orbit, yaw, pitch);
        }
    }
    if !ctrl && keys.just_pressed(KeyCode::NumpadAdd) {
        orbit.distance = (orbit.distance / 1.2).max(2.0);
    }
    if !ctrl && keys.just_pressed(KeyCode::NumpadSubtract) {
        orbit.distance = (orbit.distance * 1.2).min(15_000.0);
    }
    if keys.any_just_pressed([KeyCode::NumpadDecimal, KeyCode::KeyF]) {
        frame_selection(editor, orbit);
    }
    if keys.just_pressed(KeyCode::Home) {
        frame_all(editor, orbit);
    }
}

/// Looks along a view direction, in orthographic as Blender does.
pub fn set_view(orbit: &mut Orbit, yaw: f32, pitch: f32) {
    orbit.yaw = yaw;
    orbit.pitch = pitch;
    if !orbit.ortho {
        orbit.auto_ortho = true;
    }
    orbit.ortho = true;
}

/// The view keys, outside of transforms and text fields.
pub fn view_input(
    editor: Res<Editor>,
    built: Res<Built>,
    mut orbit: ResMut<Orbit>,
    mut tool: ResMut<Tool>,
    wants: Res<EguiWantsInput>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
) {
    let free = tool.modal.is_none() && !tool.blocked && !wants.wants_any_keyboard_input();
    if let Some(s) = orbit.walk {
        let Some(smp) = main_sampled(&editor, &built) else {
            orbit.walk = None;
            return;
        };
        let (fwd, back) = (
            keys.any_pressed([KeyCode::KeyW, KeyCode::ArrowUp]),
            keys.any_pressed([KeyCode::KeyS, KeyCode::ArrowDown]),
        );
        let speed = if keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]) {
            80.0
        } else if keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]) {
            5.0
        } else {
            25.0
        };
        let step = speed * time.delta_secs_f64();
        let mut s = s;
        if free && fwd {
            s += step;
        }
        if free && back {
            s -= step;
        }
        let s = if smp.closed {
            s.rem_euclid(smp.length.max(1e-9))
        } else {
            s.clamp(0.0, smp.length)
        };
        orbit.walk = Some(s);
        let f = smp.frame_at(s);
        let grade = 100.0 * f.tangent.z / f.tangent.truncate().length().max(1e-9);
        tool.hint = format!(
            "Walking the track: s {s:.0} m, {grade:+.1} %, {:.1} m wide · W/S or ↑/↓ move · Shift fast · Ctrl slow · Esc leaves",
            f.width_left + f.width_right
        );
        if free && keys.just_pressed(KeyCode::Escape) {
            orbit.walk = None;
        }
        return;
    }
    if free {
        view_keys(&editor, &mut orbit, &keys);
    }
}

fn open_menu(tool: &mut Tool, at: Vec2, hit: Option<Hit>, add_only: bool) {
    tool.menu = Some(Menu {
        at,
        world: tool.pointer,
        hit,
        add_only,
    });
}

#[allow(clippy::too_many_arguments)]
fn click(
    editor: &mut Editor,
    built: &Built,
    hit: Option<Hit>,
    pointer: Option<DVec3>,
    shift: bool,
    add: bool,
    alt: bool,
) {
    if add {
        add_node_at(editor, built, pointer);
        return;
    }
    match hit {
        Some(Hit::Node(item, n)) if shift => editor.selection.toggle_node(item, n),
        Some(Hit::Node(item, n)) => editor.selection.select_node(item, n),
        Some(Hit::Handle(item, n, _)) if alt => crate::edit::auto_handles(editor, item, n),
        Some(Hit::Body(item)) => {
            if editor.selection.item != Some(item) {
                editor.selection.select(item);
            } else if !shift {
                editor.selection.nodes.clear();
            }
        }
        Some(
            Hit::Handle(..)
            | Hit::Marker(_)
            | Hit::Range(_)
            | Hit::Reach(_)
            | Hit::Edge(..)
            | Hit::Gizmo(_),
        ) => {}
        None => {
            if !shift {
                editor.selection.nodes.clear();
            }
        }
    }
}

fn box_select(editor: &mut Editor, built: &Built, view: View, r: Rect, add: bool) {
    // Nodes of the selected road or spline, or else of whichever has most in the box.
    let inside = |item: Item| -> Vec<usize> {
        item_line(&editor.project, item).map_or(vec![], |(_, nodes, _)| {
            nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| {
                    view.screen(shown_pos(editor, built, item, n.pos) + DVec3::Z * LIFT)
                        .is_some_and(|s| r.contains(s))
                })
                .map(|(i, _)| i)
                .collect()
        })
    };
    let (item, found) = match editor.selection.item.map(|i| (i, inside(i))) {
        Some((i, f)) if !f.is_empty() || add => (i, f),
        _ => match items(editor)
            .map(|i| (i, inside(i)))
            .max_by_key(|(_, f)| f.len())
        {
            Some((i, f)) if !f.is_empty() => (i, f),
            _ => {
                if !add {
                    editor.selection.nodes.clear();
                }
                return;
            }
        },
    };
    if add && editor.selection.item == Some(item) {
        for n in found {
            if !editor.selection.nodes.contains(&n) {
                editor.selection.nodes.push(n);
            }
        }
    } else {
        editor.selection.item = Some(item);
        editor.selection.nodes = found;
    }
}

pub fn select_all(editor: &mut Editor) {
    if let Some((_, nodes, _)) = editor.line() {
        let n = nodes.len();
        editor.selection.nodes = (0..n).collect();
    }
}

/// Deletes the selected nodes, or else the selected spline, road or prop.
pub fn delete(editor: &mut Editor) {
    let Some(item) = editor.selection.item else {
        return;
    };
    if let Item::Prop(i) = item {
        let Some(name) = editor.project.props.get(i).map(|p| p.name.clone()) else {
            return;
        };
        if editor.apply(vec![Op::RemoveProp { name }], None) {
            editor.selection = Default::default();
        }
        return;
    }
    let Some((name, ..)) = editor.line() else {
        return;
    };
    let name = name.to_string();
    let mut nodes = editor.selection.nodes.clone();
    let ops = if nodes.is_empty() {
        match item {
            Item::Road(_) => vec![Op::RemoveRoad { road: name }],
            Item::Spline(_) => vec![Op::RemoveSpline { name }],
            Item::Prop(_) => vec![],
        }
    } else {
        nodes.sort_unstable();
        nodes
            .into_iter()
            .rev()
            .map(|index| Op::RemoveNode {
                line: name.clone(),
                index,
            })
            .collect()
    };
    let whole = editor.selection.nodes.is_empty();
    if editor.apply(ops, None) {
        editor.selection.nodes.clear();
        // After removing the whole item its index names the next one in the list.
        if whole || editor.line().is_none() {
            editor.selection = Default::default();
        }
    }
}

/// Where a node goes along a line so that the line passes through `pos`: before the
/// node after the nearest segment's middle.
fn insert_index(nodes: &[open_racing_track_project::Node], closed: bool, pos: DVec3) -> usize {
    let n = nodes.len();
    let segs = segments(n, closed);
    let mid = |i: usize| (nodes[i].pos + nodes[(i + 1) % n].pos) * 0.5;
    (0..segs)
        .min_by(|&a, &b| {
            mid(a)
                .truncate()
                .distance(pos.truncate())
                .total_cmp(&mid(b).truncate().distance(pos.truncate()))
        })
        .map_or(n, |i| i + 1)
}

/// Adds a node at the pointer to the selected road or spline: after the active node,
/// or into the segment it fits best.
pub fn add_node_at(editor: &mut Editor, built: &Built, pointer: Option<DVec3>) {
    let Some(pos) = pointer else { return };
    let Some(item) = editor.selection.item else {
        editor.status = "select a road or spline to add nodes to".into();
        return;
    };
    let Some((name, nodes, closed)) = editor.line() else {
        return;
    };
    let before = match editor.selection.node() {
        // At the open end: extend it.
        Some(0) if !closed && nodes.len() > 1 => 0,
        Some(n) => n + 1,
        None => insert_index(nodes, closed, pos),
    };
    // A road node at the height of the road where it passes nearest.
    let pos = match item {
        Item::Road(r) => built
            .roads
            .get(r)
            .map_or(pos, |smp| pos.with_z(smp.frames[smp.nearest(pos)].pos.z)),
        Item::Spline(_) | Item::Prop(_) => pos,
    };
    let line = name.to_string();
    if editor.apply(
        vec![Op::AddNode {
            line,
            pos,
            before: Some(before),
        }],
        None,
    ) {
        editor.selection.select_node(item, before);
    }
}

/// E: a new node after the active one (before it at the start of an open line), grabbed.
pub fn extrude(editor: &mut Editor, tool: &mut Tool, built: &Built, at: Vec2) {
    let (Some(item), Some(n)) = (editor.selection.item, editor.selection.node()) else {
        return;
    };
    let Some((name, nodes, closed)) = editor.line() else {
        return;
    };
    let before = if n == 0 && !closed && nodes.len() > 1 {
        0
    } else {
        n + 1
    };
    let (line, pos) = (name.to_string(), nodes[n].pos);
    editor.begin_drag();
    if editor.apply(
        vec![Op::AddNode {
            line,
            pos,
            before: Some(before),
        }],
        None,
    ) {
        editor.selection.select_node(item, before);
        start_modal(editor, tool, built, Mode::Grab, None, at, false);
    } else {
        editor.cancel_drag();
    }
}

/// Shift + D: a copy of the selected spline or prop, grabbed.
pub fn duplicate(editor: &mut Editor, tool: &mut Tool, built: &Built, at: Vec2) {
    let p = &editor.project;
    let (op, item) = match editor.selection.item {
        Some(Item::Spline(s)) if s < p.splines.len() => {
            let mut copy = p.splines[s].clone();
            copy.name = unique_name(p, &copy.name);
            (
                Op::PutSpline { spline: copy },
                Item::Spline(p.splines.len()),
            )
        }
        Some(Item::Prop(i)) if i < p.props.len() => {
            let mut copy = p.props[i].clone();
            copy.name = unique_prop_name(p, &copy.name);
            (Op::PutProp { prop: copy }, Item::Prop(p.props.len()))
        }
        _ => {
            editor.status = "select a spline or prop to duplicate".into();
            return;
        }
    };
    editor.begin_drag();
    if editor.apply(vec![op], None) {
        editor.selection.select(item);
        start_modal(editor, tool, built, Mode::Grab, None, at, false);
    } else {
        editor.cancel_drag();
    }
}

/// Starts a transform of `hit`, or of the selection: its selected nodes, or all of them.
#[allow(clippy::too_many_arguments)]
pub fn start_modal(
    editor: &mut Editor,
    tool: &mut Tool,
    built: &Built,
    mode: Mode,
    hit: Option<Hit>,
    at: Vec2,
    by_drag: bool,
) {
    let target = match hit {
        _ if matches!(mode, Mode::Width | Mode::Tilt) => {
            let Some(r) = editor
                .selection
                .road()
                .filter(|&r| r < editor.project.roads.len())
            else {
                editor.status = "Width and tilt change a road: select one, or its nodes".into();
                return;
            };
            let nodes = editor.picked_nodes();
            let road = &editor.project.roads[r];
            Target::Shape {
                road: road.name.clone(),
                count: road.nodes.len(),
                closed: road.closed,
                nodes,
                left: road.width_left.clone(),
                right: road.width_right.clone(),
                bank: road.bank.clone(),
            }
        }
        Some(Hit::Handle(item, index, out)) => {
            let Some((_, nodes, closed)) = item_line(&editor.project, item) else {
                return;
            };
            let (inc, o) = handles(nodes, closed, index);
            Target::Handle {
                item,
                index,
                out,
                node: nodes[index].pos,
                start: if out { o } else { inc },
                other: if out { inc } else { o },
                handle_mode: nodes[index].handles.mode(),
            }
        }
        Some(Hit::Marker(marker)) => Target::Marker { marker },
        Some(Hit::Range(end)) => Target::Range { end },
        Some(Hit::Reach(end)) => Target::Reach { end },
        Some(Hit::Edge(r, node, side)) => {
            let Some(road) = editor.project.roads.get(r) else {
                return;
            };
            // Every selected node takes the width, when the grabbed one is among them.
            let nodes = if editor.selection.nodes.contains(&node) {
                editor.picked_nodes()
            } else {
                vec![node]
            };
            Target::Edge {
                road: road.name.clone(),
                index: r,
                count: road.nodes.len(),
                closed: road.closed,
                node,
                nodes,
                side,
                curve: match side {
                    Side::Left => road.width_left.clone(),
                    Side::Right => road.width_right.clone(),
                },
            }
        }
        _ if editor.selection.prop().is_some() => {
            let index = editor.selection.prop().expect("a prop");
            let Some(p) = editor.project.props.get(index) else {
                return;
            };
            Target::Prop {
                index,
                pos: p.pos,
                yaw: p.yaw,
                scale: p.scale,
            }
        }
        _ => {
            let Some(item) = editor.selection.item else {
                return;
            };
            let picked = editor.picked_nodes();
            let Some((_, nodes, _)) = editor.line() else {
                return;
            };
            Target::Nodes {
                item,
                start: picked.into_iter().map(|i| (i, nodes[i].pos)).collect(),
            }
        }
    };
    let (mode, pivot) = match &target {
        Target::Nodes { item, start } => {
            let c = start.iter().map(|(_, p)| *p).sum::<DVec3>() / start.len().max(1) as f64;
            let active = editor
                .selection
                .node()
                .and_then(|n| start.iter().find(|(i, _)| *i == n))
                .map_or(c, |(_, p)| *p);
            let pivot = if mode == Mode::Grab { active } else { c };
            (mode, shown_pos(editor, built, *item, pivot))
        }
        Target::Handle { node, start, .. } => (Mode::Grab, *node + *start),
        Target::Marker { .. } => (Mode::Grab, tool.pointer.unwrap_or_default()),
        Target::Range { end } => {
            let Some(road) = editor.project.roads.get(end.road) else {
                return;
            };
            let (Some(smp), Some(rg)) = (
                built.roads.get(end.road),
                parts(road)
                    .find(|(p, _)| *p == end.part)
                    .and_then(|(_, r)| r.get(end.range)),
            ) else {
                return;
            };
            let u = if end.to { rg.to } else { rg.from };
            (Mode::Grab, range_end_pos(road, smp, end.part, u))
        }
        Target::Reach { end } => {
            let Some(road) = editor.project.roads.get(end.road) else {
                return;
            };
            let (Some(smp), Some(rg)) = (
                built.roads.get(end.road),
                parts(road)
                    .find(|(p, _)| *p == end.part)
                    .and_then(|(_, r)| r.get(end.range)),
            ) else {
                return;
            };
            (Mode::Grab, reach_pos(road, smp, end.part, rg))
        }
        Target::Edge {
            index, node, side, ..
        } => {
            let Some(smp) = built.roads.get(*index) else {
                return;
            };
            (Mode::Grab, edge_pos(smp, *node, *side))
        }
        Target::Prop { index, .. } => (
            mode,
            Placement::of(&editor.project.props[*index], built.ground.as_deref()).pos,
        ),
        Target::Shape { road, nodes, .. } => {
            let r = editor.project.road_index(road).expect("the selected road");
            let all = &editor.project.roads[r].nodes;
            let c = nodes.iter().map(|&n| all[n].pos).sum::<DVec3>() / nodes.len().max(1) as f64;
            (mode, c)
        }
    };
    if !by_drag || !editor.dragging {
        editor.begin_drag();
    }
    tool.modal = Some(Modal {
        mode,
        target,
        start_cursor: at,
        moved: Vec2::ZERO,
        last_cursor: at,
        pivot,
        axis: Axis::Free,
        typed: String::new(),
        by_drag,
    });
}

#[allow(clippy::too_many_arguments)]
fn modal(
    editor: &mut Editor,
    tool: &mut Tool,
    built: &Built,
    view: View,
    buttons: &ButtonInput<MouseButton>,
    keys: &ButtonInput<KeyCode>,
    at: Vec2,
    shift: bool,
    ctrl: bool,
) {
    let snap = tool.snap != ctrl;
    let m = tool.modal.as_mut().expect("a transform is running");
    // Axes: pressing one again frees it.
    for (k, a) in [
        (KeyCode::KeyX, Axis::X),
        (KeyCode::KeyY, Axis::Y),
        (KeyCode::KeyZ, Axis::Z),
    ] {
        let allowed = match m.mode {
            Mode::Rotate | Mode::Tilt => false,
            Mode::Width => a != Axis::Z,
            _ => true,
        };
        if keys.just_pressed(k) && allowed {
            m.axis = if m.axis == a { Axis::Free } else { a };
        }
    }
    m.typed.push_str(&typed_keys(keys));
    if keys.just_pressed(KeyCode::Backspace) {
        m.typed.pop();
    }
    let step = (at - m.last_cursor) * if shift { 0.1 } else { 1.0 };
    m.moved += step;
    m.last_cursor = at;
    let typed: Option<f64> = m.typed.parse().ok();
    let cursor = m.start_cursor + m.moved;

    let confirm = if m.by_drag {
        !buttons.pressed(MouseButton::Left)
    } else {
        buttons.just_pressed(MouseButton::Left)
    } || keys.any_just_pressed([KeyCode::Enter, KeyCode::NumpadEnter]);
    let cancel = buttons.just_pressed(MouseButton::Right) || keys.just_pressed(KeyCode::Escape);

    let (ops, readout) = transform_ops(editor, built, view, m, cursor, typed, snap, ctrl);
    let label = match m.mode {
        Mode::Grab => format!("Grab {:?}", m.axis),
        Mode::Rotate => format!("Rotate {:?}", m.axis),
        Mode::Scale => format!("Scale {:?}", m.axis),
        Mode::Width => match m.axis {
            Axis::X => "Width, left side".to_string(),
            Axis::Y => "Width, right side".to_string(),
            _ => "Width".to_string(),
        },
        Mode::Tilt => "Tilt (bank)".to_string(),
    };
    let keys_hint = match m.mode {
        Mode::Width => "X left only · Y right only",
        Mode::Tilt => "",
        _ => "X/Y/Z axis",
    };
    tool.hint = format!(
        "{label}: {readout}{}   {keys_hint} · Shift fine · Ctrl snap · type a value · click/Enter confirm · right click/Esc cancel",
        if m.typed.is_empty() {
            String::new()
        } else {
            format!(" [{}]", m.typed)
        }
    );
    if !ops.is_empty() {
        editor.apply(ops, None);
    }
    if cancel {
        tool.modal = None;
        editor.cancel_drag();
    } else if confirm {
        tool.modal = None;
        editor.end_drag();
    }
}

/// The operations that put what a transform moves where the pointer now says, and a
/// readout of the change.
#[allow(clippy::too_many_arguments)]
fn transform_ops(
    editor: &Editor,
    built: &Built,
    view: View,
    m: &Modal,
    cursor: Vec2,
    typed: Option<f64>,
    snap: bool,
    free: bool,
) -> (Vec<Op>, String) {
    // How far the pointer moved the pivot, in the plane or up and down.
    let slide = || -> DVec3 {
        if m.axis == Axis::Z {
            let up = view.up_on_screen(m.pivot).filter(|u| u.length() > 0.05);
            let dz = match typed {
                Some(v) => v,
                None => up.map_or(-(m.moved.y as f64) * 0.05, |u| {
                    (m.moved.dot(u) / u.length_squared()) as f64
                }),
            };
            return DVec3::Z
                * if snap && typed.is_none() {
                    (dz * 10.0).round() / 10.0
                } else {
                    dz
                };
        }
        let (Some(a), Some(b)) = (
            view.on_plane(m.start_cursor, m.pivot.z),
            view.on_plane(cursor, m.pivot.z),
        ) else {
            return DVec3::ZERO;
        };
        let mut d = b - a;
        d.z = 0.0;
        match m.axis {
            Axis::X => d.y = 0.0,
            Axis::Y => d.x = 0.0,
            _ => {}
        }
        if let Some(v) = typed {
            d = match m.axis {
                Axis::Y => DVec3::Y * v,
                _ => DVec3::X * v,
            };
        } else if snap {
            // The pivot lands on whole metres.
            let to = (m.pivot + d).truncate().round();
            d = (to - m.pivot.truncate()).extend(0.0);
            match m.axis {
                Axis::X => d.y = 0.0,
                Axis::Y => d.x = 0.0,
                _ => {}
            }
        }
        d
    };
    let center = view.screen(m.pivot);
    match &m.target {
        Target::Nodes { item, start } => {
            let Some((name, ..)) = item_line(&editor.project, *item) else {
                return (vec![], String::new());
            };
            let draped = matches!(item, Item::Spline(s) if editor.project.splines.get(*s).is_some_and(|s| s.drape));
            let ground = built.ground.as_deref().filter(|_| draped);
            let (moved, readout): (Box<dyn Fn(DVec3) -> DVec3>, String) = match m.mode {
                Mode::Grab => {
                    let d = slide();
                    (
                        Box::new(move |p| p + d),
                        format!("Δ ({:.2}, {:.2}, {:.2}) m", d.x, d.y, d.z),
                    )
                }
                Mode::Rotate => {
                    let angle = turn(m, center, cursor, typed, snap);
                    let (pivot, rot) = (m.pivot.truncate(), DVec2::from_angle(angle));
                    (
                        Box::new(move |p: DVec3| {
                            (pivot + rot.rotate(p.truncate() - pivot)).extend(p.z)
                        }),
                        format!("{:.1}°", angle.to_degrees()),
                    )
                }
                Mode::Scale => {
                    let k = stretch(m, center, cursor, typed, snap);
                    let (pivot, axis) = (m.pivot, m.axis);
                    (
                        Box::new(move |p: DVec3| {
                            let d = p - pivot;
                            let s = match axis {
                                Axis::X => DVec3::new(k, 1.0, 1.0),
                                Axis::Y => DVec3::new(1.0, k, 1.0),
                                Axis::Z => DVec3::new(1.0, 1.0, k),
                                Axis::Free => DVec3::new(k, k, 1.0),
                            };
                            pivot + d * s
                        }),
                        format!("×{k:.3}"),
                    )
                }
                Mode::Width | Mode::Tilt => return (vec![], String::new()),
            };
            // One node grabbed on its own snaps onto what is near it (Ctrl frees it).
            let one = m.mode == Mode::Grab && start.len() == 1 && !free && typed.is_none();
            let mut snapped = None;
            let ops = start
                .iter()
                .map(|&(index, p)| {
                    let mut pos = moved(p);
                    if one && let Some((to, what)) = snap_node(editor, built, *item, index, pos) {
                        pos = to.with_z(pos.z);
                        snapped = Some(what);
                    }
                    if let Some(g) = ground {
                        pos = drape(Some(g), pos.with_z(p.z.max(pos.z)));
                    }
                    Op::MoveNode {
                        line: name.to_string(),
                        index,
                        pos,
                    }
                })
                .collect();
            let readout = match snapped {
                Some(what) => format!("{readout} → on {what}"),
                None => readout,
            };
            (ops, readout)
        }
        Target::Handle {
            item,
            index,
            out,
            start,
            other,
            handle_mode,
            ..
        } => {
            let Some((name, ..)) = item_line(&editor.project, *item) else {
                return (vec![], String::new());
            };
            let h = *start + slide();
            let (incoming, outgoing) = dragged_handles(*handle_mode, *out, h, *other);
            (
                vec![Op::SetNodeHandles {
                    line: name.to_string(),
                    index: *index,
                    mode: if *handle_mode == HandleMode::Free {
                        HandleMode::Free
                    } else {
                        HandleMode::Aligned
                    },
                    incoming,
                    outgoing,
                }],
                format!("handle {:.1} m", h.length()),
            )
        }
        Target::Reach { end } => {
            let Some(road) = editor.project.roads.get(end.road) else {
                return (vec![], String::new());
            };
            let (Some(smp), Some(at), Some(rg)) = (
                built.roads.get(end.road),
                view.on_plane(cursor, m.pivot.z),
                parts(road)
                    .find(|(p, _)| *p == end.part)
                    .and_then(|(_, r)| r.get(end.range)),
            ) else {
                return (vec![], String::new());
            };
            let f = smp.frame_at(range_middle(smp, rg));
            let d = (at - f.pos).dot(flat_left(&f));
            let step = |v: f64| {
                if let Some(t) = typed {
                    t
                } else if snap {
                    (v * 10.0).round() / 10.0
                } else {
                    v
                }
            };
            let name = road.name.clone();
            match end.part {
                Part::Strip(side, i) => {
                    let mut strip = road.strips(side)[i].clone();
                    let inner = part_reach(road, smp, end.part, &f).abs() - strip.width;
                    strip.width = step(side.sign() * d - inner).max(0.1);
                    let readout = format!("{}: {:.2} m wide", strip.name, strip.width);
                    let op = Op::PutStrip {
                        road: name,
                        side,
                        strip,
                        at: None,
                    };
                    (vec![op], readout)
                }
                Part::Barrier(i) => {
                    let mut barrier = road.barriers[i].clone();
                    let edge = part_reach(road, smp, end.part, &f).abs() - barrier.offset;
                    barrier.offset = step(barrier.side.sign() * d - edge).max(0.0);
                    let readout =
                        format!("{}: {:.2} m from the edge", barrier.name, barrier.offset);
                    (
                        vec![Op::PutBarrier {
                            road: name,
                            barrier,
                        }],
                        readout,
                    )
                }
            }
        }
        Target::Edge {
            road,
            index,
            count,
            closed,
            node,
            nodes,
            side,
            curve,
        } => {
            let (Some(smp), Some(at)) = (built.roads.get(*index), view.on_plane(cursor, m.pivot.z))
            else {
                return (vec![], String::new());
            };
            let f = smp.frame_at(smp.s_at(*node as f64));
            let d = side.sign() * (at - f.pos).dot(flat_left(&f));
            let w = match typed {
                Some(t) => t,
                None if snap => (d * 10.0).round() / 10.0,
                None => d,
            }
            .max(0.5);
            let changes: Vec<(usize, f64)> = nodes.iter().map(|&n| (n, w)).collect();
            let op = Op::SetProfile {
                road: road.clone(),
                curve: match side {
                    Side::Left => Curve::WidthLeft,
                    Side::Right => Curve::WidthRight,
                },
                keys: crate::edit::node_keys(curve, *count, *closed, &changes),
            };
            let what = match nodes.len() {
                1 => format!("node {node}"),
                n => format!("{n} nodes"),
            };
            (
                vec![op],
                format!("width {side:?} {w:.2} m at {what} (total {:.2} m)", {
                    let other = match side {
                        Side::Left => f.width_right,
                        Side::Right => f.width_left,
                    };
                    w + other
                }),
            )
        }
        Target::Range { end } => {
            let road = &editor.project.roads[end.road];
            let (Some(smp), Some(at)) =
                (built.roads.get(end.road), view.on_plane(cursor, m.pivot.z))
            else {
                return (vec![], String::new());
            };
            let f = &smp.frames[smp.nearest(at)];
            // Nodes and the corners' entries, apexes and exits nearby catch the end.
            let caught = (!free)
                .then(|| range_snap(smp, built.corners.get(end.road), f.s))
                .flatten();
            let u = match &caught {
                Some((u, _)) => *u,
                None if snap => (f.u * 10.0).round() / 10.0,
                None => f.u,
            };
            let set = |ranges: &mut Vec<Range>| {
                if let Some(r) = ranges.get_mut(end.range) {
                    if end.to {
                        r.to = u;
                    } else {
                        r.from = u;
                    }
                }
            };
            let name = road.name.clone();
            let op = match end.part {
                Part::Strip(side, i) => {
                    let mut strip = road.strips(side)[i].clone();
                    set(&mut strip.ranges);
                    Op::PutStrip {
                        road: name,
                        side,
                        strip,
                        at: None,
                    }
                }
                Part::Barrier(i) => {
                    let mut barrier = road.barriers[i].clone();
                    set(&mut barrier.ranges);
                    Op::PutBarrier {
                        road: name,
                        barrier,
                    }
                }
            };
            let at = caught.map_or(String::new(), |(_, what)| format!(" → on {what}"));
            (vec![op], format!("u {u:.2}, s {:.0} m{at}", f.s))
        }
        Target::Shape {
            road,
            count,
            closed,
            nodes,
            left,
            right,
            bank,
        } => {
            let period = if *closed {
                *count
            } else {
                count.saturating_sub(1)
            } as f64;
            let set = |curve: Curve, c: &StationCurve, f: &dyn Fn(f64) -> f64| Op::SetProfile {
                road: road.clone(),
                curve,
                keys: crate::edit::node_keys(
                    c,
                    *count,
                    *closed,
                    &nodes
                        .iter()
                        .map(|&n| (n, f(c.eval(n as f64, period, *closed))))
                        .collect::<Vec<_>>(),
                ),
            };
            if m.mode == Mode::Tilt {
                let angle = turn(m, center, cursor, typed, snap);
                return (
                    vec![set(Curve::Bank, bank, &|b| b + angle)],
                    format!("{:+.1}°", angle.to_degrees()),
                );
            }
            let k = stretch(m, center, cursor, typed, snap);
            let wider = |w: f64| (w * k).max(0.1);
            let mut ops = Vec::new();
            if m.axis != Axis::Y {
                ops.push(set(Curve::WidthLeft, left, &wider));
            }
            if m.axis != Axis::X {
                ops.push(set(Curve::WidthRight, right, &wider));
            }
            let first = nodes.first().copied().unwrap_or(0) as f64;
            (
                ops,
                format!(
                    "×{k:.3}  (node {first}: {:.2} m left, {:.2} m right)",
                    wider(left.eval(first, period, *closed)),
                    wider(right.eval(first, period, *closed))
                ),
            )
        }
        Target::Prop {
            index,
            pos,
            yaw,
            scale,
        } => {
            let name = editor.project.props[*index].name.clone();
            let (op, readout) = match m.mode {
                Mode::Grab => {
                    let d = slide();
                    (
                        Op::MoveProp {
                            name,
                            pos: Some(*pos + d),
                            yaw: None,
                            scale: None,
                        },
                        format!("Δ ({:.2}, {:.2}, {:.2}) m", d.x, d.y, d.z),
                    )
                }
                Mode::Rotate => {
                    let angle = turn(m, center, cursor, typed, snap);
                    (
                        Op::MoveProp {
                            name,
                            pos: None,
                            yaw: Some(yaw + angle),
                            scale: None,
                        },
                        format!("{:.1}°", angle.to_degrees()),
                    )
                }
                Mode::Scale => {
                    let k = stretch(m, center, cursor, typed, snap);
                    (
                        Op::MoveProp {
                            name,
                            pos: None,
                            yaw: None,
                            scale: Some((scale * k).max(1e-3)),
                        },
                        format!("×{k:.3}"),
                    )
                }
                Mode::Width | Mode::Tilt => return (vec![], String::new()),
            };
            (vec![op], readout)
        }
        Target::Marker { marker } => {
            let p = &editor.project;
            let Some(main) = p.road_index(&p.main_road).and_then(|i| built.roads.get(i)) else {
                return (vec![], String::new());
            };
            let Some(at) = view.on_plane(cursor, m.pivot.z) else {
                return (vec![], String::new());
            };
            let f = &main.frames[main.nearest(at)];
            let u = if snap {
                (f.u * 10.0).round() / 10.0
            } else {
                f.u
            };
            let op = match marker {
                Marker::Start => Op::SetMarkers {
                    start: Some(u),
                    sectors: None,
                    grid: None,
                },
                Marker::Sector(i) => {
                    let mut sectors = p.markers.sectors.clone();
                    if let Some(s) = sectors.get_mut(*i) {
                        *s = u;
                    }
                    Op::SetMarkers {
                        start: None,
                        sectors: Some(sectors),
                        grid: None,
                    }
                }
            };
            (vec![op], format!("u {u:.2}, s {:.0} m", f.s))
        }
    }
}

/// How near along the road, m, a node or a corner's place catches a stretch's end.
const CATCH: f64 = 4.0;

/// The node or corner place within `CATCH` of `s` along a road, as its spline
/// parameter and a name for it.
fn range_snap(smp: &Sampled, corners: Option<&Vec<Corner>>, s: f64) -> Option<(f64, String)> {
    let gap = |a: f64| {
        let d = (a - s).abs();
        if smp.closed {
            d.rem_euclid(smp.length)
                .min(smp.length - d.rem_euclid(smp.length))
        } else {
            d
        }
    };
    let node = smp.u_at(s).round();
    let mut best: Option<(f64, f64, String)> =
        Some((gap(smp.s_at(node)), node, format!("node {node}")));
    for c in corners.into_iter().flatten() {
        for (what, at) in [("entry", c.entry), ("apex", c.apex), ("exit", c.exit)] {
            let d = gap(at);
            if best.as_ref().is_none_or(|b| d < b.0) {
                let at = if smp.closed {
                    at.rem_euclid(smp.length)
                } else {
                    at
                };
                best = Some((d, smp.u_at(at), format!("T{} {what}", c.number)));
            }
        }
    }
    best.filter(|b| b.0 < CATCH).map(|(_, u, what)| (u, what))
}

/// Where a single node dragged to `pos` snaps: onto another line's node (joining
/// them), a road's centreline for a road's node (a pit lane's ends), or a road's edge
/// for a kerb's or wall's node.
fn snap_node(
    editor: &Editor,
    built: &Built,
    item: Item,
    index: usize,
    pos: DVec3,
) -> Option<(DVec3, String)> {
    let p = pos.truncate();
    // Nodes of other lines, and this line's own ends (closing it up).
    type Best = Option<(f64, DVec3, String)>;
    fn consider(best: &mut Best, d: f64, at: DVec3, what: String, reach: f64) {
        if d < reach && best.as_ref().is_none_or(|b| d < b.0) {
            *best = Some((d, at, what));
        }
    }
    let mut best: Best = None;
    for other in items(editor) {
        let Some((name, nodes, _)) = item_line(&editor.project, other) else {
            continue;
        };
        for (i, n) in nodes.iter().enumerate() {
            if other == item && (i == index || !(i == 0 || i + 1 == nodes.len())) {
                continue;
            }
            let d = n.pos.truncate().distance(p);
            consider(&mut best, d, n.pos, format!("node {i} of {name}"), 2.0);
        }
    }
    if best.is_some() {
        return best.map(|(_, at, what)| (at, what));
    }
    for (r, smp) in built.roads.iter().enumerate() {
        if smp.frames.is_empty() || item == Item::Road(r) {
            continue;
        }
        let name = &editor.project.roads.get(r)?.name;
        let f = smp.frames[smp.nearest(pos)];
        let left = flat_left(&f);
        let d = (pos - f.pos).dot(left);
        match item {
            // Straight across onto the line, keeping the node's place along it.
            Item::Road(_) => consider(
                &mut best,
                d.abs(),
                pos - left * d,
                format!("{name}'s centre line"),
                3.0,
            ),
            Item::Spline(s) => {
                let half = match editor.project.splines.get(s).map(|s| &s.shape) {
                    Some(Shape::Band { width, .. }) => 0.5 * width,
                    _ => 0.0,
                };
                for (side, edge) in [(Side::Left, f.width_left), (Side::Right, -f.width_right)] {
                    let at = pos + left * (edge + side.sign() * half - d);
                    consider(
                        &mut best,
                        (d - edge).abs(),
                        at,
                        format!("{name}'s {side:?} edge"),
                        EDGE_SNAP,
                    );
                }
            }
            Item::Prop(_) => {}
        }
    }
    best.map(|(_, at, what)| (at, what))
}

/// The turn a rotation has reached, radians anticlockwise seen from above: typed in
/// degrees, or the pointer's angle round the pivot on screen.
fn turn(m: &Modal, center: Option<Vec2>, cursor: Vec2, typed: Option<f64>, snap: bool) -> f64 {
    let angle = match (typed, center) {
        (Some(deg), _) => return deg.to_radians(),
        (None, Some(c)) => {
            let a0 = (m.start_cursor - c).to_angle();
            let a1 = (cursor - c).to_angle();
            // The screen's y points down: clockwise on screen.
            -(a1 - a0) as f64
        }
        _ => 0.0,
    };
    if snap {
        (angle.to_degrees() / 5.0).round() * 5f64.to_radians()
    } else {
        angle
    }
}

/// The factor a scaling has reached: typed, or the pointer's distance from the pivot on
/// screen against where it began.
fn stretch(m: &Modal, center: Option<Vec2>, cursor: Vec2, typed: Option<f64>, snap: bool) -> f64 {
    let k = match (typed, center) {
        (Some(k), _) => return k,
        (None, Some(c)) => (cursor.distance(c) / m.start_cursor.distance(c).max(1.0)) as f64,
        _ => 1.0,
    };
    if snap { (k * 10.0).round() / 10.0 } else { k }
}

/// Where the draw tool would put a point: the ground under the pointer, or for a band,
/// beside the nearest road edge within reach (Ctrl frees it).
fn draw_point(editor: &Editor, built: &Built, kind: DrawKind, pointer: DVec3, ctrl: bool) -> DVec3 {
    let DrawKind::Spline(i) = kind else {
        return pointer;
    };
    let preset = &PRESETS[i];
    if ctrl || !preset.is_band(&editor.project) {
        return pointer;
    }
    let half = match (preset.shape)(&editor.project) {
        Shape::Band { width, .. } => 0.5 * width,
        Shape::Wall { .. } => 0.0,
    };
    let mut best: Option<(DVec3, f64)> = None;
    for smp in &built.roads {
        if smp.frames.is_empty() {
            continue;
        }
        let f = &smp.frames[smp.nearest(pointer)];
        let d = (pointer - f.pos).truncate().dot(f.lateral.truncate());
        for (side, edge) in [(Side::Left, f.width_left), (Side::Right, -f.width_right)] {
            let gap = (d - edge).abs();
            if gap < EDGE_SNAP && best.is_none_or(|b| gap < b.1) {
                let at = edge + side.sign() * half;
                best = Some((f.pos + f.lateral * at, gap));
            }
        }
    }
    best.map_or(pointer, |(p, _)| p)
}

#[allow(clippy::too_many_arguments)]
fn draw(
    editor: &mut Editor,
    tool: &mut Tool,
    built: &Built,
    buttons: &ButtonInput<MouseButton>,
    keys: &ButtonInput<KeyCode>,
    over: Option<Vec2>,
    ctrl: bool,
    keys_free: bool,
) {
    let kind = tool.draw.as_ref().map(|d| d.kind).expect("drawing");
    tool.draw_at = tool
        .pointer
        .map(|p| draw_point(editor, built, kind, p, ctrl));
    tool.hint = "Draw: click to add points · Backspace removes the last · Enter or right click finishes · Esc cancels · Ctrl: no snapping".into();
    let d = tool.draw.as_mut().expect("drawing");
    if over.is_some()
        && buttons.just_pressed(MouseButton::Left)
        && let Some(p) = tool.draw_at
    {
        d.points.push(p);
    }
    if keys_free && keys.just_pressed(KeyCode::Backspace) {
        d.points.pop();
    }
    if keys_free && keys.just_pressed(KeyCode::Escape) {
        tool.draw = None;
        return;
    }
    let finish = (keys_free && keys.any_just_pressed([KeyCode::Enter, KeyCode::NumpadEnter]))
        || (over.is_some() && buttons.just_pressed(MouseButton::Right));
    if !finish {
        return;
    }
    if tool.draw.as_ref().is_some_and(|d| d.points.len() < 2) {
        // Keep drawing: a stray Enter or right click should not lose the first point.
        editor.status = "a line needs at least two points (Esc cancels)".into();
        return;
    }
    let d = tool.draw.take().expect("drawing");
    match d.kind {
        DrawKind::Road => {
            let name = unique_name(&editor.project, "road");
            if editor.apply(
                vec![Op::AddRoad {
                    name,
                    closed: false,
                    nodes: d.points,
                    like: None,
                }],
                None,
            ) {
                editor
                    .selection
                    .select(Item::Road(editor.project.roads.len() - 1));
            }
        }
        DrawKind::Spline(i) => {
            let spline = PRESETS[i].spline(&editor.project, d.points);
            if editor.apply(vec![Op::PutSpline { spline }], None) {
                editor
                    .selection
                    .select(Item::Spline(editor.project.splines.len() - 1));
            }
        }
    }
}

/// Frames the selected nodes, or the selected road or spline.
pub fn frame_selection(editor: &Editor, orbit: &mut Orbit) {
    if let Some(prop) = editor
        .selection
        .prop()
        .and_then(|i| editor.project.props.get(i))
    {
        let p = to_bevy(prop.pos);
        return frame(orbit, &[p - Vec3::splat(15.0), p + Vec3::splat(15.0)]);
    }
    let Some((_, nodes, _)) = editor.line() else {
        return frame_all(editor, orbit);
    };
    let points: Vec<Vec3> = if editor.selection.nodes.is_empty() {
        nodes.iter().map(|n| to_bevy(n.pos)).collect()
    } else {
        editor
            .selection
            .nodes
            .iter()
            .filter_map(|&i| nodes.get(i))
            .map(|n| to_bevy(n.pos))
            .collect()
    };
    frame(orbit, &points);
}

pub fn frame_all(editor: &Editor, orbit: &mut Orbit) {
    let p = &editor.project;
    let points: Vec<Vec3> = p
        .roads
        .iter()
        .flat_map(|r| &r.nodes)
        .chain(p.splines.iter().flat_map(|s| &s.nodes))
        .map(|n| to_bevy(n.pos))
        .collect();
    frame(orbit, &points);
}

fn frame(orbit: &mut Orbit, points: &[Vec3]) {
    if points.is_empty() {
        return;
    }
    let (lo, hi) = points.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), p| {
        (lo.min(*p), hi.max(*p))
    });
    orbit.focus = (lo + hi) * 0.5;
    orbit.distance = ((hi - lo).length() * 1.1).max(40.0);
}

/// Lines, nodes, handles, markers, and what the tools are doing.
pub fn gizmos(
    editor: Res<Editor>,
    built: Res<Built>,
    tool: Res<Tool>,
    camera: Single<&GlobalTransform, With<EditorCamera>>,
    mut gizmos: Gizmos,
) {
    let eye = camera.translation();
    let editor = &*editor;
    let p = &editor.project;
    let lift = |v: DVec3| to_bevy(v + DVec3::Z * LIFT);
    let sel = &editor.selection;
    let hover = tool.hover;
    let overlays = tool.overlays;
    for item in items(editor) {
        let Some((_, nodes, closed)) = item_line(p, item) else {
            continue;
        };
        let selected = sel.item == Some(item);
        if !overlays.lines && !selected {
            continue;
        }
        let base = match item {
            Item::Road(_) => Color::srgb(0.3, 0.9, 1.0),
            Item::Spline(_) | Item::Prop(_) => Color::srgb(1.0, 0.45, 0.8),
        };
        let hovered_body = hover == Some(Hit::Body(item)) || tool.outliner_hover == Some(item);
        let line = if selected || hovered_body {
            base
        } else {
            base.with_alpha(0.35)
        };
        let shown: Vec<DVec3> = nodes
            .iter()
            .map(|n| shown_pos(editor, &built, item, n.pos))
            .collect();
        // The spline itself, from the last build.
        let sampled = match item {
            Item::Road(r) => built.roads.get(r),
            Item::Spline(s) => built.splines.get(s),
            Item::Prop(_) => None,
        };
        if let Some(smp) = sampled.filter(|s| s.frames.len() > 1) {
            let pts = smp
                .frames
                .iter()
                .map(|f| lift(f.pos))
                .chain(closed.then(|| lift(smp.frames[0].pos)));
            gizmos.linestrip(pts, line.with_alpha(if selected { 0.9 } else { 0.4 }));
        }
        let n = nodes.len();
        let segs = segments(n, closed);
        if selected {
            for i in 0..segs {
                gizmos.line(
                    lift(shown[i]),
                    lift(shown[(i + 1) % n]),
                    line.with_alpha(0.25),
                );
            }
        }
        for (i, &pos) in shown.iter().enumerate() {
            let chosen = selected && sel.nodes.contains(&i);
            let active = selected && sel.node() == Some(i);
            let color = if active {
                Color::srgb(1.0, 1.0, 1.0)
            } else if chosen {
                Color::srgb(1.0, 0.6, 0.1)
            } else if hover == Some(Hit::Node(item, i)) {
                Color::srgb(1.0, 0.95, 0.6)
            } else if i == 0 && matches!(item, Item::Road(_)) {
                Color::srgb(0.2, 1.0, 0.4)
            } else {
                line
            };
            let size = if chosen { 1.0 } else { 0.8 };
            let radius = size * node_size(eye, pos);
            gizmos.sphere(Isometry3d::from_translation(lift(pos)), radius, color);
            if active {
                for (h, o) in visible_handles(nodes, closed, i) {
                    let hovered = hover == Some(Hit::Handle(item, i, o));
                    let c = if hovered {
                        Color::srgb(1.0, 0.95, 0.6)
                    } else {
                        Color::srgb(1.0, 0.6, 0.1)
                    };
                    gizmos.line(lift(pos), lift(pos + h), c);
                    gizmos.sphere(Isometry3d::from_translation(lift(pos + h)), 0.6 * radius, c);
                }
            }
        }
    }

    // Stretches of the selected road's strips (orange) and barriers (grey), with their
    // ends to drag.
    if let Some(r) = sel.road().filter(|_| overlays.stretches)
        && let (Some(road), Some(smp)) = (p.roads.get(r), built.roads.get(r))
        && smp.frames.len() > 1
    {
        for (part, ranges) in parts(road) {
            let color = match part {
                Part::Strip(..) => Color::srgb(1.0, 0.55, 0.2),
                Part::Barrier(_) => Color::srgb(0.8, 0.8, 0.9),
            };
            for (range, rg) in ranges.iter().enumerate() {
                let (a, mut b) = (smp.s_at(rg.from), smp.s_at(rg.to));
                if b < a && smp.closed {
                    b += smp.length;
                }
                let steps = ((b - a) / 3.0).ceil().max(1.0) as usize;
                let pts = (0..=steps).map(|k| {
                    let f = smp.frame_at(a + (b - a) * k as f64 / steps as f64);
                    lift(f.pos + f.lateral * part_offset(road, smp, part, &f))
                });
                gizmos.linestrip(pts, color);
                for (to, u) in [(false, rg.from), (true, rg.to)] {
                    let end = RangeEnd {
                        road: r,
                        part,
                        range,
                        to,
                    };
                    let at = range_end_pos(road, smp, part, u);
                    let c = if hover == Some(Hit::Range(end)) {
                        Color::WHITE
                    } else {
                        color
                    };
                    gizmos.sphere(
                        Isometry3d::from_translation(lift(at)),
                        0.7 * node_size(eye, at),
                        c,
                    );
                }
            }
        }
    }

    // Handles for widths: the outer edge of each stretch (a strip's width, a barrier's
    // distance) and the road's edges at its selected nodes.
    if let Some(r) = sel.road()
        && let (Some(road), Some(smp)) = (p.roads.get(r), built.roads.get(r))
        && smp.frames.len() > 1
    {
        let lit = |hit: Hit, c: Color| if hover == Some(hit) { Color::WHITE } else { c };
        if overlays.stretches {
            for (part, ranges) in parts(road) {
                for (range, rg) in ranges.iter().enumerate() {
                    let at = reach_pos(road, smp, part, rg);
                    let end = RangeEnd {
                        road: r,
                        part,
                        range,
                        to: false,
                    };
                    let size = 0.6 * node_size(eye, at);
                    gizmos.cube(
                        Transform::from_translation(lift(at)).with_scale(Vec3::splat(size * 1.6)),
                        lit(Hit::Reach(end), Color::srgb(1.0, 0.85, 0.3)),
                    );
                }
            }
        }
        for &n in sel.nodes.iter().filter(|&&n| n < road.nodes.len()) {
            let centre = smp.frame_at(smp.s_at(n as f64)).pos;
            for side in [Side::Left, Side::Right] {
                let at = edge_pos(smp, n, side);
                let color = lit(Hit::Edge(r, n, side), Color::srgb(0.3, 0.9, 1.0));
                gizmos.line(lift(centre), lift(at), color.with_alpha(0.4));
                gizmos.cube(
                    Transform::from_translation(lift(at))
                        .with_scale(Vec3::splat(node_size(eye, at) * 1.2)),
                    color,
                );
            }
        }
    }

    // Props: a ring where each stands and a line the way it faces.
    for (i, prop) in p.props.iter().enumerate() {
        let at = Placement::of(prop, built.ground.as_deref());
        let item = Item::Prop(i);
        if !overlays.props && sel.item != Some(item) {
            continue;
        }
        let color = if sel.item == Some(item) {
            Color::srgb(1.0, 0.6, 0.1)
        } else if hover == Some(Hit::Body(item)) || tool.outliner_hover == Some(item) {
            Color::srgb(1.0, 0.95, 0.6)
        } else {
            Color::srgb(0.7, 1.0, 0.4)
        };
        let r = 1.5 * node_size(eye, at.pos);
        let base = lift(at.pos);
        gizmos.circle(
            Isometry3d::new(base, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
            r,
            color,
        );
        let facing = DVec3::new(at.yaw.cos(), at.yaw.sin(), 0.0);
        gizmos.line(base, lift(at.pos + facing * (2.0 * r as f64)), color);
    }

    // Markers on the main road, from the last build.
    if let Some(main) = p
        .road_index(&p.main_road)
        .and_then(|i| built.roads.get(i))
        .filter(|_| overlays.markers)
    {
        let across = |gizmos: &mut Gizmos, u: f64, color: Color| {
            let f = main.frame_at(main.s_at(u));
            gizmos.line(
                lift(f.pos + f.lateral * f.width_left),
                lift(f.pos - f.lateral * f.width_right),
                color,
            );
        };
        let m = &p.markers;
        let hot = |mk: Marker, c: Color| {
            if hover == Some(Hit::Marker(mk)) {
                Color::WHITE
            } else {
                c
            }
        };
        across(
            &mut gizmos,
            m.start,
            hot(Marker::Start, Color::srgb(0.9, 0.9, 0.9)),
        );
        for (i, &u) in m.sectors.iter().enumerate() {
            across(
                &mut gizmos,
                u,
                hot(Marker::Sector(i), Color::srgb(1.0, 0.85, 0.1)),
            );
        }
        let start = main.s_at(m.start);
        for k in 0..m.grid.count {
            let side = if k % 2 == 0 { 1.0 } else { -1.0 };
            let f = main.frame_at(start - m.grid.behind - k as f64 * m.grid.spacing);
            let c = f.pos + f.lateral * (m.grid.pole.sign() * side * m.grid.stagger);
            let (fw, lt) = (f.tangent * 2.3, f.lateral * 1.0);
            let corners = [
                c + fw + lt,
                c + fw - lt,
                c - fw - lt,
                c - fw + lt,
                c + fw + lt,
            ]
            .map(lift);
            gizmos.linestrip(corners, Color::srgb(0.2, 0.6, 1.0));
        }
        if let Some(pit) = &m.pit
            && let Some(lane) = p.road_index(&pit.road).and_then(|i| built.roads.get(i))
        {
            for &u in &pit.boxes {
                let f = lane.frame_at(lane.s_at(u));
                let c = f.pos + f.lateral * (pit.box_side.sign() * pit.box_offset);
                gizmos.sphere(
                    Isometry3d::from_translation(lift(c)),
                    1.5,
                    Color::srgb(1.0, 0.5, 0.1),
                );
            }
        }
    }

    // What the measure tool measured, or is measuring to the pointer.
    if tool.active == ToolKind::Measure
        && let Some(&a) = tool.measure.first()
    {
        let b = tool.measure.get(1).copied().or(tool.pointer);
        let color = Color::srgb(1.0, 0.85, 0.2);
        gizmos.sphere(
            Isometry3d::from_translation(lift(a)),
            node_size(eye, a),
            color,
        );
        if let Some(b) = b {
            gizmos.line(lift(a), lift(b), color);
            gizmos.sphere(
                Isometry3d::from_translation(lift(b)),
                node_size(eye, b),
                color,
            );
        }
    }

    // The draw tool's points so far and the next one.
    if let Some(d) = &tool.draw {
        let next = tool.draw_at;
        let pts: Vec<Vec3> = d.points.iter().copied().chain(next).map(lift).collect();
        gizmos.linestrip(pts, Color::srgb(1.0, 0.95, 0.3));
        for &p in &d.points {
            gizmos.sphere(
                Isometry3d::from_translation(lift(p)),
                0.8,
                Color::srgb(1.0, 0.95, 0.3),
            );
        }
        if let Some(p) = next {
            gizmos.sphere(Isometry3d::from_translation(lift(p)), 0.6, Color::WHITE);
        }
    }
    // The active tool's gizmo, hidden while transforming.
    if let Some(mode) = tool.active.mode()
        && tool.modal.is_none()
        && tool.draw.is_none()
        && let Some((c, size)) = gizmo_frame(editor, &built, eye)
    {
        let lit = |axis: Axis, color: Color| {
            if hover == Some(Hit::Gizmo(axis)) {
                Color::srgb(1.0, 0.95, 0.6)
            } else {
                color
            }
        };
        let facing = Quat::from_rotation_arc(Vec3::Z, (eye - to_bevy(c)).normalize_or(Vec3::Y));
        match mode {
            Mode::Rotate => {
                let ring: Vec<Vec3> = gizmo_ring(c, size).map(to_bevy).collect();
                gizmos.linestrip(ring, lit(Axis::Free, axis_color(Axis::Z)));
            }
            Mode::Width | Mode::Tilt => {}
            Mode::Grab | Mode::Scale => {
                for (axis, dir) in GIZMO_AXES {
                    let color = lit(axis, axis_color(axis));
                    let (from, to) = (to_bevy(c + dir * size * 0.2), to_bevy(c + dir * size));
                    if mode == Mode::Grab {
                        gizmos
                            .arrow(from, to, color)
                            .with_tip_length(0.18 * size as f32);
                    } else {
                        gizmos.line(from, to, color);
                        gizmos.cube(
                            Transform::from_translation(to)
                                .with_scale(Vec3::splat(0.1 * size as f32)),
                            color,
                        );
                    }
                }
                gizmos.circle(
                    Isometry3d::new(to_bevy(c), facing),
                    0.1 * size as f32,
                    lit(Axis::Free, Color::WHITE),
                );
            }
        }
    }

    // Axis lines of a transform.
    if let Some(m) = &tool.modal {
        let dir = match m.axis {
            Axis::X => Some((DVec3::X, Color::srgb(1.0, 0.3, 0.3))),
            Axis::Y => Some((DVec3::Y, Color::srgb(0.4, 1.0, 0.4))),
            Axis::Z => Some((DVec3::Z, Color::srgb(0.4, 0.6, 1.0))),
            Axis::Free => None,
        };
        if let Some((d, c)) = dir {
            gizmos.line(
                to_bevy(m.pivot - d * 5000.0),
                to_bevy(m.pivot + d * 5000.0),
                c,
            );
        }
    }
}

/// A node sphere's radius at `pos`, seen from `eye`: about the same size on screen at
/// any distance.
fn node_size(eye: Vec3, pos: DVec3) -> f32 {
    (eye.distance(to_bevy(pos)) * 0.008).clamp(0.3, 40.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::camera::RenderTargetInfo;
    use open_racing_track_project::Node;

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
        let scene = open_racing_track_project::bake::build(&editor.project);
        let built = Built {
            roads: scene.roads.into_iter().map(|b| b.sampled).collect(),
            corners: vec![],
            ..default()
        };
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
        let (ops, readout) = transform_ops(&editor, &built, view, m, to, None, true, false);
        assert!(readout.contains("9.00"), "{readout}");
        editor.apply(ops, None);
        let r = &editor.project.roads[0];
        let w = r.width_left.eval(1.0, r.period(), true);
        assert!((w - 9.0).abs() < 1e-9, "{w}");
        assert_eq!(r.width_right.eval(1.0, r.period(), true), 6.0);
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
        let (ops, _) = transform_ops(&editor, &built, view, m, to.unwrap(), None, true, false);
        editor.apply(ops, None);
        let w = editor.project.roads[0].left[0].width;
        assert!((w - 2.0).abs() < 0.05, "{w}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stretch_ends_catch_on_nodes_and_spline_nodes_on_road_edges() {
        let (editor, built, _, _, dir) = top_down("snaps", DVec3::ZERO);
        let smp = &built.roads[0];
        let s2 = smp.s_at(2.0);
        let (u, what) = range_snap(smp, None, s2 + 2.5).unwrap();
        assert_eq!((u, what.as_str()), (2.0, "node 2"));
        assert!(range_snap(smp, None, s2 + 30.0).is_none());

        // A wall's node near the circuit's right edge.
        let mut editor = editor;
        let spline = crate::presets::PRESETS[4].spline(
            &editor.project,
            vec![DVec3::new(100.0, -20.0, 0.0), DVec3::new(150.0, -20.0, 0.0)],
        );
        assert!(editor.apply(vec![Op::PutSpline { spline }], None));
        // 1.5 m outside the right edge, halfway along the first straight.
        let f = smp.frame_at(smp.s_at(0.5));
        let edge = f.pos - flat_left(&f) * f.width_right;
        let near = edge - flat_left(&f) * 1.5;
        let (at, what) = snap_node(&editor, &built, Item::Spline(0), 0, near).unwrap();
        assert!(at.distance(edge) < 0.3, "{at:?} {edge:?}");
        assert!(what.contains("Right edge"), "{what}");
        // And onto another line's node, joining them.
        let (at, _) = snap_node(
            &editor,
            &built,
            Item::Spline(0),
            1,
            DVec3::new(249.0, 1.0, 0.0),
        )
        .unwrap();
        assert_eq!(at, editor.project.roads[0].nodes[1].pos);
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
