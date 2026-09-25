//! The 3D view: an orbiting camera, picking and dragging nodes, and gizmos for the
//! nodes, handles and race markers.
//!
//! Mouse: left picks and drags a node (with Shift, up and down), right drag orbits,
//! middle drag (or Shift + right) pans, the wheel zooms. Ctrl + left click adds a node
//! after the selected one where the ground is clicked. Delete removes the selected node,
//! F frames the selected road.

use bevy::camera::visibility::RenderLayers;
use bevy::camera::{CameraOutputMode, Viewport};
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::prelude::*;
use bevy::render::render_resource::BlendState;
use bevy::window::PrimaryWindow;
use bevy_egui::{EguiGlobalSettings, PrimaryEguiContext};
use glam::DVec3;
use open_racing_track_project::ops::Op;
use open_racing_track_render::{from_bevy, to_bevy};

use crate::preview::Built;
use crate::state::{Editor, Selection};

/// Screen distance within which a click picks a node, logical pixels.
const PICK_RADIUS: f32 = 14.0;

#[derive(Component)]
pub struct EditorCamera;

/// The orbit: what the camera looks at, from which direction and how far.
#[derive(Resource)]
pub struct Orbit {
    pub focus: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

impl Default for Orbit {
    fn default() -> Self {
        Self {
            focus: Vec3::ZERO,
            yaw: 0.6,
            pitch: 0.9,
            distance: 700.0,
        }
    }
}

/// The part of the window the 3D view covers, logical pixels, set by the UI each frame.
#[derive(Resource, Default)]
pub struct ViewRect(pub Option<Rect>);

/// A node being dragged: where the grab started.
#[derive(Resource, Default)]
pub struct Drag {
    active: Option<DragState>,
}

struct DragState {
    road: usize,
    node: usize,
    start_pos: DVec3,
    start_cursor: Vec2,
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
        Projection::Perspective(PerspectiveProjection {
            fov: 50f32.to_radians(),
            near: 0.5,
            far: 20_000.0,
            ..default()
        }),
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

/// Keeps the camera on its orbit and inside the 3D view's rectangle.
pub fn place_camera(
    orbit: Res<Orbit>,
    rect: Res<ViewRect>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut camera: Single<(&mut Camera, &mut Transform), With<EditorCamera>>,
) {
    let (cam, transform) = &mut *camera;
    let dir = Vec3::new(
        orbit.pitch.cos() * orbit.yaw.cos(),
        orbit.pitch.sin(),
        orbit.pitch.cos() * orbit.yaw.sin(),
    );
    **transform = Transform::from_translation(orbit.focus + dir * orbit.distance)
        .looking_at(orbit.focus, Vec3::Y);
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

/// The cursor in the 3D view, relative to it, if it is over it.
fn cursor(window: &Window, rect: &ViewRect) -> Option<Vec2> {
    let p = window.cursor_position()?;
    let r = rect.0?;
    r.contains(p).then(|| p - r.min)
}

/// Where a ray from the cursor meets the horizontal plane at height `z` (sim frame).
fn on_plane(camera: &Camera, t: &GlobalTransform, at: Vec2, z: f64) -> Option<DVec3> {
    let ray = camera.viewport_to_world(t, at).ok()?;
    let d = ray.direction.as_vec3();
    if d.y.abs() < 1e-6 {
        return None;
    }
    let k = (z as f32 - ray.origin.y) / d.y;
    (k > 0.0).then(|| from_bevy(ray.origin + d * k))
}

#[allow(clippy::too_many_arguments)]
pub fn input(
    mut editor: ResMut<Editor>,
    mut orbit: ResMut<Orbit>,
    mut drag: ResMut<Drag>,
    built: Res<Built>,
    rect: Res<ViewRect>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<EditorCamera>>,
) {
    let (cam, cam_t) = *camera;
    let over = cursor(&window, &rect);
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);

    // Camera.
    if over.is_some() || buttons.pressed(MouseButton::Right) || buttons.pressed(MouseButton::Middle)
    {
        let d = motion.delta;
        let pan =
            buttons.pressed(MouseButton::Middle) || (shift && buttons.pressed(MouseButton::Right));
        if pan {
            let right = cam_t.right().as_vec3();
            let forward = Vec3::new(cam_t.forward().x, 0.0, cam_t.forward().z).normalize_or_zero();
            let k = orbit.distance * 0.0015;
            orbit.focus += (-right * d.x + forward * d.y) * k;
        } else if buttons.pressed(MouseButton::Right) {
            orbit.yaw += d.x * 0.005;
            orbit.pitch = (orbit.pitch + d.y * 0.005).clamp(0.05, 1.55);
        }
        if over.is_some() && scroll.delta.y != 0.0 {
            orbit.distance =
                (orbit.distance * (1.0 - 0.1 * scroll.delta.y.signum())).clamp(5.0, 15_000.0);
        }
    }

    // Dragging a node.
    if let Some(d) = &drag.active {
        if !buttons.pressed(MouseButton::Left) {
            drag.active = None;
            editor.end_drag();
            return;
        }
        let Some(at) = window
            .cursor_position()
            .map(|p| p - rect.0.map_or(Vec2::ZERO, |r| r.min))
        else {
            return;
        };
        let pos = if shift {
            // Up and down: 1 px is a share of the distance to the node.
            let dist = (cam_t.translation() - to_bevy(d.start_pos)).length() as f64;
            d.start_pos + DVec3::Z * ((d.start_cursor.y - at.y) as f64 * dist * 0.0015)
        } else if let Some(p) = on_plane(cam, cam_t, at, d.start_pos.z) {
            p
        } else {
            return;
        };
        let road = editor.project.roads[d.road].name.clone();
        let node = d.node;
        editor.apply(
            vec![Op::MoveNode {
                line: road,
                index: node,
                pos,
            }],
            None,
        );
        return;
    }

    let Some(at) = over else { return };
    if buttons.just_pressed(MouseButton::Left) {
        // The nearest node on screen.
        let mut best: Option<(usize, usize, f32)> = None;
        for (r, road) in editor.project.roads.iter().enumerate() {
            for (n, node) in road.nodes.iter().enumerate() {
                let Ok(p) = cam.world_to_viewport(cam_t, to_bevy(node.pos)) else {
                    continue;
                };
                let d = p.distance(at);
                if d < PICK_RADIUS && best.is_none_or(|b| d < b.2) {
                    best = Some((r, n, d));
                }
            }
        }
        if ctrl {
            add_node(&mut editor, &built, cam, cam_t, at);
        } else if let Some((road, node, _)) = best {
            editor.selection = Selection {
                road: Some(road),
                node: Some(node),
            };
            editor.begin_drag();
            drag.active = Some(DragState {
                road,
                node,
                start_pos: editor.project.roads[road].nodes[node].pos,
                start_cursor: at,
            });
        } else {
            editor.selection.node = None;
            // Picks the road under the cursor, if any.
            if let Some(p) = on_plane(cam, cam_t, at, 0.0) {
                editor.selection.road = nearest_road(&built, p, 30.0).or(editor.selection.road);
            }
        }
    }
    if keys.just_pressed(KeyCode::Delete)
        && let (Some(road), Some(index)) = (editor.road_name(), editor.selection.node)
    {
        editor.apply(vec![Op::RemoveNode { line: road, index }], None);
        editor.selection.node = None;
    }
    if keys.just_pressed(KeyCode::KeyF) {
        frame_selection(&editor, &mut orbit);
    }
}

/// Index of the road passing within `radius` of `p` in the plane, the nearest.
fn nearest_road(built: &Built, p: DVec3, radius: f64) -> Option<usize> {
    built
        .roads
        .iter()
        .enumerate()
        .filter_map(|(i, smp)| {
            let f = &smp.frames[smp.nearest(p)];
            let d = (f.pos - p).truncate().length();
            (d < radius).then_some((i, d))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// Adds a node where the cursor meets the ground: after the selected node of the
/// selected road, or where it fits best along it.
fn add_node(editor: &mut Editor, built: &Built, cam: &Camera, t: &GlobalTransform, at: Vec2) {
    let Some(r) = editor.selection.road else {
        editor.status = "select a road to add nodes to".into();
        return;
    };
    let road = &editor.project.roads[r];
    // At the height of the road where it passes nearest.
    let z = built.roads.get(r).map_or(0.0, |smp| {
        on_plane(cam, t, at, 0.0).map_or(0.0, |p| smp.frames[smp.nearest(p)].pos.z)
    });
    let Some(pos) = on_plane(cam, t, at, z) else {
        return;
    };
    let before = match editor.selection.node {
        Some(n) => n + 1,
        None => {
            // Into the segment whose middle is nearest.
            let n = road.nodes.len();
            let segs = if road.closed { n } else { n - 1 };
            (0..segs)
                .min_by(|&a, &b| {
                    let mid = |i: usize| (road.nodes[i].pos + road.nodes[(i + 1) % n].pos) * 0.5;
                    mid(a).distance(pos).total_cmp(&mid(b).distance(pos))
                })
                .map_or(n, |i| i + 1)
        }
    };
    let name = road.name.clone();
    if editor.apply(
        vec![Op::AddNode {
            line: name,
            pos,
            before: Some(before),
        }],
        None,
    ) {
        editor.selection.node = Some(before);
    }
}

pub fn frame_selection(editor: &Editor, orbit: &mut Orbit) {
    let Some(road) = editor
        .selection
        .road
        .and_then(|r| editor.project.roads.get(r))
    else {
        return;
    };
    let points: Vec<Vec3> = match editor.selection.node {
        Some(n) => vec![to_bevy(road.nodes[n].pos)],
        None => road.nodes.iter().map(|n| to_bevy(n.pos)).collect(),
    };
    let (lo, hi) = points.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), p| {
        (lo.min(*p), hi.max(*p))
    });
    orbit.focus = (lo + hi) * 0.5;
    orbit.distance = ((hi - lo).length() * 1.1).max(60.0);
}

/// Nodes, the selected node's handles, and the race markers.
pub fn gizmos(editor: Res<Editor>, built: Res<Built>, mut gizmos: Gizmos) {
    let p = &editor.project;
    let lift = |v: DVec3| to_bevy(v + DVec3::Z * 0.3);
    for (r, road) in p.roads.iter().enumerate() {
        let selected_road = editor.selection.road == Some(r);
        let line = if selected_road {
            Color::srgb(0.3, 0.9, 1.0)
        } else {
            Color::srgba(0.3, 0.9, 1.0, 0.35)
        };
        let n = road.nodes.len();
        let segs = if road.closed { n } else { n.saturating_sub(1) };
        for i in 0..segs {
            gizmos.line(
                lift(road.nodes[i].pos),
                lift(road.nodes[(i + 1) % n].pos),
                line.with_alpha(0.3),
            );
        }
        for (i, node) in road.nodes.iter().enumerate() {
            let selected = selected_road && editor.selection.node == Some(i);
            let color = if selected {
                Color::srgb(1.0, 0.85, 0.1)
            } else if i == 0 {
                Color::srgb(0.2, 1.0, 0.4)
            } else {
                line
            };
            gizmos.sphere(
                Isometry3d::from_translation(lift(node.pos)),
                if selected { 2.5 } else { 1.8 },
                color,
            );
            if selected {
                let (inc, out) =
                    open_racing_track_project::curve::handles(&road.nodes, road.closed, i);
                for h in [inc, out] {
                    gizmos.line(
                        lift(node.pos),
                        lift(node.pos + h),
                        Color::srgb(1.0, 0.6, 0.1),
                    );
                    gizmos.sphere(
                        Isometry3d::from_translation(lift(node.pos + h)),
                        1.0,
                        Color::srgb(1.0, 0.6, 0.1),
                    );
                }
            }
        }
    }

    // Markers on the main road, from the last build.
    let Some(main) = p.road_index(&p.main_road).and_then(|i| built.roads.get(i)) else {
        return;
    };
    let across = |gizmos: &mut Gizmos, u: f64, color: Color| {
        let f = main.frame_at(main.s_at(u));
        gizmos.line(
            lift(f.pos + f.lateral * f.width_left),
            lift(f.pos - f.lateral * f.width_right),
            color,
        );
    };
    let m = &p.markers;
    across(&mut gizmos, m.start, Color::WHITE);
    for &u in &m.sectors {
        across(&mut gizmos, u, Color::srgb(1.0, 0.85, 0.1));
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
