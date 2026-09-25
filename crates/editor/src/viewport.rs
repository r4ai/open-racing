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
use open_racing_track_project::curve::{Sampled, handles};
use open_racing_track_project::ops::Op;
use open_racing_track_project::project::{Shape, Side};
use open_racing_track_render::{from_bevy, to_bevy};

use std::path::PathBuf;

use open_racing_track_project::model::Placement;

use crate::presets::{PRESETS, unique_name};
use crate::preview::Built;
use crate::state::{Editor, Item, item_line};

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
}

impl Default for Orbit {
    fn default() -> Self {
        Self {
            focus: Vec3::ZERO,
            yaw: 0.6,
            pitch: 0.9,
            distance: 700.0,
            ortho: false,
        }
    }
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
    /// A road's or spline's body.
    Body(Item),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Grab,
    Rotate,
    Scale,
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
}

/// A transform in progress: Blender's G, R and S, or dragging with the mouse.
pub struct Modal {
    pub mode: Mode,
    target: Target,
    /// The pointer where it began, and the pointer motion since (slowed while Shift is
    /// held), in view coordinates.
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

/// The state of the view's tools.
#[derive(Resource, Default)]
pub struct Tool {
    pub modal: Option<Modal>,
    pub draw: Option<Draw>,
    pub hover: Option<Hit>,
    /// Where the pointer meets the ground, and where a point drawn now would go.
    pub pointer: Option<DVec3>,
    pub draw_at: Option<DVec3>,
    pub menu: Option<Menu>,
    /// Box selection from one corner to the other, view coordinates.
    pub boxing: Option<(Vec2, Vec2)>,
    /// Snap to the grid without holding Ctrl (Ctrl then frees).
    pub snap: bool,
    /// What the view is doing, for the header.
    pub hint: String,
    /// A model to place with the next click.
    pub place: Option<PathBuf>,
    /// Where the left button went down, on what, and whether it went down with Alt
    /// (orbiting).
    press: Option<(Vec2, Option<Hit>, bool)>,
    right_press: Option<Vec2>,
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
    window: Single<&Window, With<PrimaryWindow>>,
    mut camera: Single<(&mut Camera, &mut Transform, &mut Projection), With<EditorCamera>>,
) {
    let (cam, transform, projection) = &mut *camera;
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
struct View<'a> {
    cam: &'a Camera,
    t: &'a GlobalTransform,
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

    fn screen(&self, p: DVec3) -> Option<Vec2> {
        self.cam.world_to_viewport(self.t, to_bevy(p)).ok()
    }

    /// Screen motion of a metre up at `p`.
    fn up_on_screen(&self, p: DVec3) -> Option<Vec2> {
        Some(self.screen(p + DVec3::Z)? - self.screen(p)?)
    }
}

/// The cursor in the 3D view, relative to it, if it is over it.
fn cursor(window: &Window, rect: &ViewRect) -> Option<Vec2> {
    let p = window.cursor_position()?;
    let r = rect.0?;
    r.contains(p).then(|| p - r.min)
}

/// The cursor relative to the 3D view, wherever it is.
fn cursor_anywhere(window: &Window, rect: &ViewRect) -> Option<Vec2> {
    Some(window.cursor_position()? - rect.0.map_or(Vec2::ZERO, |r| r.min))
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

/// What is under the pointer: the active node's handles first, then nodes, markers,
/// and the roads and splines themselves.
fn pick(
    editor: &Editor,
    built: &Built,
    view: View,
    at: Vec2,
    ground_at: Option<DVec3>,
) -> Option<Hit> {
    let near = |p: DVec3, r: f32| view.screen(p).map(|s| s.distance(at)).filter(|&d| d < r);
    let sel = &editor.selection;
    if let (Some(item), Some(n)) = (sel.item, sel.node())
        && let Some((_, nodes, closed)) = item_line(&editor.project, item)
        && n < nodes.len()
    {
        let base = shown_pos(editor, built, item, nodes[n].pos);
        let (inc, out) = handles(nodes, closed, n);
        for (h, o) in [(out, true), (inc, false)] {
            if near(base + h + DVec3::Z * LIFT, PICK_RADIUS).is_some() {
                return Some(Hit::Handle(item, n, o));
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
            if let Some(d) = near(p, PICK_RADIUS)
                && best.is_none_or(|b| d < b.1)
            {
                best = Some((Hit::Node(item, i), d));
            }
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
    let pointer_free = !wants.wants_any_pointer_input() && tool.menu.is_none();
    let keys_free = !wants.wants_any_keyboard_input();
    let over = cursor(&window, &rect).filter(|_| pointer_free);
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let alt = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
    let ground = built.ground.as_deref();

    tool.pointer = over.and_then(|at| view.on_ground(ground, at));
    tool.hover = None;
    tool.draw_at = None;
    tool.hint.clear();

    camera_input(
        &mut orbit, tool, &buttons, &motion, &scroll, over, t, alt, shift, ctrl,
    );

    // A transform in progress takes every input.
    if tool.modal.is_some() {
        let at = cursor_anywhere(&window, &rect).unwrap_or(Vec2::ZERO);
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

    let Some(at) = over else {
        tool.press = None;
        return;
    };
    let hover = pick(editor, &built, view, at, tool.pointer);
    tool.hover = hover;

    // Left button: a click selects, a drag grabs what it began on or draws a box.
    if buttons.just_pressed(MouseButton::Left) {
        tool.press = Some((at, hover, alt));
    }
    if let Some((from, hit, orbiting)) = tool.press {
        if orbiting {
            if !buttons.pressed(MouseButton::Left) {
                tool.press = None;
            }
        } else if buttons.pressed(MouseButton::Left) {
            if from.distance(at) > DRAG_THRESHOLD {
                match hit {
                    Some(Hit::Body(item @ Item::Prop(_))) => {
                        tool.press = None;
                        editor.selection.select(item);
                        start_modal(editor, tool, &built, Mode::Grab, None, from, true);
                    }
                    Some(h @ (Hit::Node(..) | Hit::Handle(..) | Hit::Marker(_))) => {
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
        } else {
            tool.press = None;
            match tool.boxing.take() {
                Some((a, b)) => box_select(editor, &built, view, Rect::from_corners(a, b), shift),
                None => click(editor, &built, hit, tool.pointer, shift, ctrl, alt),
            }
        }
        if let Some(b) = &mut tool.boxing {
            b.1 = at;
        }
    }

    // Right button: a click opens the menu (with Ctrl, adds a node there).
    if buttons.just_pressed(MouseButton::Right) {
        tool.right_press = Some(at);
    }
    if buttons.just_released(MouseButton::Right)
        && let Some(from) = tool.right_press.take()
        && from.distance(at) <= DRAG_THRESHOLD
    {
        if ctrl {
            add_node_at(editor, &built, tool.pointer);
        } else {
            open_menu(tool, &rect, at, hover, false);
        }
    }

    if !keys_free {
        return;
    }
    let pressed = |k| keys.just_pressed(k);
    if pressed(KeyCode::KeyG) {
        start_modal(editor, tool, &built, Mode::Grab, None, at, false);
    } else if pressed(KeyCode::KeyR) {
        start_modal(editor, tool, &built, Mode::Rotate, None, at, false);
    } else if pressed(KeyCode::KeyS) && !ctrl {
        start_modal(editor, tool, &built, Mode::Scale, None, at, false);
    } else if pressed(KeyCode::KeyE) {
        extrude(editor, tool, &built, at);
    } else if pressed(KeyCode::KeyD) && shift {
        duplicate(editor, tool, &built, at);
    } else if pressed(KeyCode::KeyA) && shift {
        open_menu(tool, &rect, at, hover, true);
    } else if pressed(KeyCode::KeyA) && alt {
        editor.selection.nodes.clear();
    } else if pressed(KeyCode::KeyA) && !ctrl {
        select_all(editor);
    } else if pressed(KeyCode::KeyX) || pressed(KeyCode::Delete) {
        delete(editor);
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
    let middle = buttons.pressed(MouseButton::Middle);
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
            orbit.yaw += d.x * 0.005;
            orbit.pitch = (orbit.pitch + d.y * 0.005).clamp(-1.55, 1.5695);
        }
    }
    if over.is_some() && scroll.delta.y != 0.0 {
        orbit.distance =
            (orbit.distance * (1.0 - 0.1 * scroll.delta.y.signum())).clamp(2.0, 15_000.0);
    }
}

/// Numpad views, framing and switching the projection; also called from the menus.
pub fn view_keys(editor: &Editor, orbit: &mut Orbit, keys: &ButtonInput<KeyCode>) {
    use std::f32::consts::{FRAC_PI_2, PI};
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let side = |a: f32, b: f32| if ctrl { b } else { a };
    if keys.just_pressed(KeyCode::Numpad1) {
        set_view(orbit, side(FRAC_PI_2, -FRAC_PI_2), 0.0);
    }
    if keys.just_pressed(KeyCode::Numpad3) {
        set_view(orbit, side(0.0, PI), 0.0);
    }
    if keys.just_pressed(KeyCode::Numpad7) {
        set_view(orbit, FRAC_PI_2, side(1.5695, -1.5695));
    }
    if keys.just_pressed(KeyCode::Numpad5) {
        orbit.ortho = !orbit.ortho;
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
    orbit.ortho = true;
}

/// The view keys, outside of transforms and text fields.
pub fn view_input(
    editor: Res<Editor>,
    mut orbit: ResMut<Orbit>,
    tool: Res<Tool>,
    wants: Res<EguiWantsInput>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    if tool.modal.is_none() && !wants.wants_any_keyboard_input() {
        view_keys(&editor, &mut orbit, &keys);
    }
}

fn open_menu(tool: &mut Tool, rect: &ViewRect, at: Vec2, hit: Option<Hit>, add_only: bool) {
    tool.menu = Some(Menu {
        at: at + rect.0.map_or(Vec2::ZERO, |r| r.min),
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
    ctrl: bool,
    alt: bool,
) {
    if ctrl {
        add_node_at(editor, built, pointer);
        return;
    }
    match hit {
        Some(Hit::Node(item, n)) if shift => editor.selection.toggle_node(item, n),
        Some(Hit::Node(item, n)) => editor.selection.select_node(item, n),
        Some(Hit::Handle(item, n, _)) if alt => {
            if let Some((name, ..)) = item_line(&editor.project, item) {
                let line = name.to_string();
                editor.apply(
                    vec![Op::SetHandle {
                        line,
                        index: n,
                        handle: None,
                    }],
                    None,
                );
            }
        }
        Some(Hit::Body(item)) => {
            if editor.selection.item != Some(item) {
                editor.selection.select(item);
            } else if !shift {
                editor.selection.nodes.clear();
            }
        }
        Some(Hit::Handle(..) | Hit::Marker(_)) => {}
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

fn select_all(editor: &mut Editor) {
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
        let name = editor.project.props[i].name.clone();
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
    if editor.apply(ops, None) {
        editor.selection.nodes.clear();
        if editor.line().is_none() {
            editor.selection = Default::default();
        }
    }
}

/// Where a node goes along a line so that the line passes through `pos`: before the
/// node after the nearest segment's middle.
fn insert_index(nodes: &[open_racing_track_project::Node], closed: bool, pos: DVec3) -> usize {
    let n = nodes.len();
    let segs = if closed { n } else { n.saturating_sub(1) };
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
fn extrude(editor: &mut Editor, tool: &mut Tool, built: &Built, at: Vec2) {
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
fn duplicate(editor: &mut Editor, tool: &mut Tool, built: &Built, at: Vec2) {
    let p = &editor.project;
    let (op, item) = match editor.selection.item {
        Some(Item::Spline(s)) => {
            let mut copy = p.splines[s].clone();
            copy.name = unique_name(p, &copy.name);
            (
                Op::PutSpline { spline: copy },
                Item::Spline(p.splines.len()),
            )
        }
        Some(Item::Prop(i)) => {
            let mut copy = p.props[i].clone();
            let base = copy.name.clone();
            copy.name = (1..)
                .map(|k| format!("{base}.{k:03}"))
                .find(|n| p.props.iter().all(|x| &x.name != n))
                .expect("some name is free");
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
fn start_modal(
    editor: &mut Editor,
    tool: &mut Tool,
    built: &Built,
    mode: Mode,
    hit: Option<Hit>,
    at: Vec2,
    by_drag: bool,
) {
    let target = match hit {
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
            }
        }
        Some(Hit::Marker(marker)) => Target::Marker { marker },
        _ if editor.selection.prop().is_some() => {
            let index = editor.selection.prop().expect("a prop");
            let p = &editor.project.props[index];
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
            let Some((_, nodes, _)) = editor.line() else {
                return;
            };
            let picked: Vec<usize> = if editor.selection.nodes.is_empty() {
                (0..nodes.len()).collect()
            } else {
                editor.selection.nodes.clone()
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
        Target::Prop { index, .. } => (
            mode,
            Placement::of(&editor.project.props[*index], built.ground.as_deref()).pos,
        ),
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
        if keys.just_pressed(k) && m.mode != Mode::Rotate {
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

    let (ops, readout) = transform_ops(editor, built, view, m, cursor, typed, snap);
    let label = match m.mode {
        Mode::Grab => "Grab",
        Mode::Rotate => "Rotate",
        Mode::Scale => "Scale",
    };
    tool.hint = format!(
        "{label} {:?}: {readout}{}   X/Y/Z axis · Shift fine · Ctrl snap · type a value · click/Enter confirm · right click/Esc cancel",
        m.axis,
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
fn transform_ops(
    editor: &Editor,
    built: &Built,
    view: View,
    m: &Modal,
    cursor: Vec2,
    typed: Option<f64>,
    snap: bool,
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
            };
            let ops = start
                .iter()
                .map(|&(index, p)| {
                    let mut pos = moved(p);
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
            (ops, readout)
        }
        Target::Handle {
            item,
            index,
            out,
            start,
            ..
        } => {
            let Some((name, ..)) = item_line(&editor.project, *item) else {
                return (vec![], String::new());
            };
            let h = *start + slide();
            let handle = if *out { h } else { -h };
            (
                vec![Op::SetHandle {
                    line: name.to_string(),
                    index: *index,
                    handle: Some(handle),
                }],
                format!("handle {:.1} m", h.length()),
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
    let d = tool.draw.take().expect("drawing");
    if d.points.len() < 2 {
        editor.status = "a line needs at least two points".into();
        return;
    }
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
    if let Some(i) = editor.selection.prop() {
        let p = to_bevy(editor.project.props[i].pos);
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
    for item in items(editor) {
        let Some((_, nodes, closed)) = item_line(p, item) else {
            continue;
        };
        let selected = sel.item == Some(item);
        let base = match item {
            Item::Road(_) => Color::srgb(0.3, 0.9, 1.0),
            Item::Spline(_) | Item::Prop(_) => Color::srgb(1.0, 0.45, 0.8),
        };
        let hovered_body = hover == Some(Hit::Body(item));
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
        let segs = if closed { n } else { n.saturating_sub(1) };
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
                let (inc, out) = handles(nodes, closed, i);
                for (h, o) in [(inc, false), (out, true)] {
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

    // Props: a ring where each stands and a line the way it faces.
    for (i, prop) in p.props.iter().enumerate() {
        let at = Placement::of(prop, built.ground.as_deref());
        let item = Item::Prop(i);
        let color = if sel.item == Some(item) {
            Color::srgb(1.0, 0.6, 0.1)
        } else if hover == Some(Hit::Body(item)) {
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
    if let Some(main) = p.road_index(&p.main_road).and_then(|i| built.roads.get(i)) {
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
