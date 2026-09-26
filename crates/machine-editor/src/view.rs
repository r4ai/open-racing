//! The 3D view of a machine: its parts' stand-in shapes where the assembly puts them.
//!
//! Camera as in Blender: middle drag (or right drag) orbits, Shift + middle pans, the
//! wheel zooms; numpad 1, 3 and 7 look from the front, the side and the top, F frames the
//! machine. A left click selects the part under the pointer (orange); the selected part's
//! mounts show as axes, the machine's centre of gravity as a ball.

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{CameraOutputMode, Viewport};
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::BlendState;
use bevy::window::PrimaryWindow;
use bevy_egui::{EguiGlobalSettings, PrimaryEguiContext};
use glam::{DAffine3, DVec3};
use open_racing_machine_project::assembly::{Assembly, assemble, mount_transform, transform};
use open_racing_machine_project::part::ShapeKind;

use crate::state::{Editor, Workspace};
use crate::ui::ViewRect;

/// Simulation axes (z up) to Bevy's (y up).
pub fn to_bevy(v: DVec3) -> Vec3 {
    Vec3::new(v.x as f32, v.z as f32, -v.y as f32)
}

pub fn from_bevy(v: Vec3) -> DVec3 {
    DVec3::new(v.x as f64, -v.z as f64, v.y as f64)
}

#[derive(Component)]
pub struct MachineCamera;

#[derive(Component)]
pub struct ShapeMesh;

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
            focus: Vec3::new(-1.3, 0.5, 0.0),
            yaw: 0.9,
            pitch: 0.35,
            distance: 7.5,
        }
    }
}

/// What the meshes were built from.
#[derive(Resource, Default)]
pub struct Shown {
    key: Option<(u64, Option<String>, Option<String>)>,
    pub assembly: Option<Assembly>,
    pub cg: Option<DVec3>,
}

pub fn setup(mut commands: Commands, mut egui: ResMut<EguiGlobalSettings>) {
    // The UI gets a camera of its own over the whole window; the 3D camera draws into what
    // the panels leave free.
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
    commands.spawn((MachineCamera, Camera3d::default(), Transform::default()));
    commands.spawn((
        DirectionalLight {
            illuminance: 12_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(0.0, 1.0, 0.0).looking_at(Vec3::new(-0.4, 0.0, 0.6), Vec3::Y),
    ));
    commands.insert_resource(GlobalAmbientLight {
        brightness: 1500.0,
        ..default()
    });
    commands.insert_resource(ClearColor(Color::srgb(0.24, 0.25, 0.27)));
}

fn tris_mesh(t: &open_racing_machine_project::mesh::Tris) -> Mesh {
    let pos: Vec<[f32; 3]> = t
        .positions
        .iter()
        .map(|p| to_bevy(DVec3::from(p.map(f64::from))).to_array())
        .collect();
    let nrm: Vec<[f32; 3]> = t
        .normals
        .iter()
        .map(|p| to_bevy(DVec3::from(p.map(f64::from))).to_array())
        .collect();
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    let n = nrm.len();
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nrm);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0f32, 0.0]; n]);
    m.insert_indices(Indices::U32(t.indices.clone()));
    m
}

/// Rebuilds the meshes when the machine, the library or the selection changes.
pub fn rebuild(
    mut commands: Commands,
    editor: Res<Editor>,
    mut shown: ResMut<Shown>,
    old: Query<Entity, With<ShapeMesh>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let key = (
        editor.revision,
        editor.selection.machine.clone(),
        editor.selection.placed.clone(),
    );
    if shown.key.as_ref() == Some(&key) {
        return;
    }
    shown.key = Some(key);
    for e in &old {
        commands.entity(e).despawn();
    }
    shown.assembly = None;
    shown.cg = None;
    let Some(m) = editor
        .selection
        .machine
        .as_ref()
        .and_then(|m| editor.lib.machines.get(m))
    else {
        return;
    };
    let Ok(asm) = assemble(&editor.lib, m) else {
        return;
    };
    if let Ok(mass) = open_racing_machine_project::mass::machine_mass(&editor.lib, m, &asm) {
        shown.cg = Some(DVec3::from(mass.centre));
    }
    let any_selected = editor.selection.placed.is_some();
    for inst in &asm.instances {
        let Some(part) = editor.lib.parts.get(&inst.part) else {
            continue;
        };
        let selected =
            editor.selection.placed.as_deref() == Some(m.parts[inst.placed].name.as_str());
        // With a part selected the rest turn translucent, so that it shows inside them.
        let ghost = any_selected && !selected;
        for s in &part.physical.shapes {
            let mut t = open_racing_machine_project::mesh::Tris::default();
            t.append(
                &open_racing_machine_project::mesh::shape(s),
                &inst.transform,
            );
            let [r, g, b] = s.colour;
            let base = if selected {
                Color::srgb(1.0, 0.63, 0.16).mix(&Color::srgb(r, g, b), 0.35)
            } else if ghost {
                Color::srgba(r, g, b, 0.18)
            } else {
                Color::srgb(r, g, b)
            };
            commands.spawn((
                ShapeMesh,
                Mesh3d(meshes.add(tris_mesh(&t))),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: base,
                    perceptual_roughness: 0.6,
                    emissive: if selected {
                        LinearRgba::rgb(0.25, 0.12, 0.0)
                    } else {
                        LinearRgba::BLACK
                    },
                    alpha_mode: if ghost {
                        AlphaMode::Blend
                    } else {
                        AlphaMode::Opaque
                    },
                    ..default()
                })),
                Transform::default(),
            ));
        }
    }
    shown.assembly = Some(asm);
}

/// A ray against the shapes of the shown machine: the placed part hit first.
pub fn pick(editor: &Editor, shown: &Shown, origin: DVec3, dir: DVec3) -> Option<String> {
    let asm = shown.assembly.as_ref()?;
    let m = editor
        .lib
        .machines
        .get(editor.selection.machine.as_ref()?)?;
    let mut best: Option<(f64, String)> = None;
    for inst in &asm.instances {
        let Some(part) = editor.lib.parts.get(&inst.part) else {
            continue;
        };
        for s in &part.physical.shapes {
            let to_local: DAffine3 = (inst.transform * transform(s.at, s.rotation_deg)).inverse();
            let o = to_local.transform_point3(origin);
            let d = to_local.transform_vector3(dir);
            let half = match s.kind {
                ShapeKind::Box { size } => DVec3::from(size) * 0.5,
                ShapeKind::Sphere { radius } => DVec3::splat(radius),
                ShapeKind::Cylinder {
                    radius,
                    length,
                    axis,
                } => match axis {
                    open_racing_machine_project::part::Axis::X => {
                        DVec3::new(0.5 * length, radius, radius)
                    }
                    open_racing_machine_project::part::Axis::Y => {
                        DVec3::new(radius, 0.5 * length, radius)
                    }
                    open_racing_machine_project::part::Axis::Z => {
                        DVec3::new(radius, radius, 0.5 * length)
                    }
                },
            };
            if let Some(t) = slab(o, d, half)
                && best.as_ref().is_none_or(|b| t < b.0)
            {
                best = Some((t, m.parts[inst.placed].name.clone()));
            }
        }
    }
    best.map(|b| b.1)
}

/// Where a ray (along `d`, any length) enters a box of half sizes `h` at the origin.
fn slab(o: DVec3, d: DVec3, h: DVec3) -> Option<f64> {
    let (mut t0, mut t1) = (f64::MIN, f64::MAX);
    for k in 0..3 {
        if d[k].abs() < 1e-12 {
            if o[k].abs() > h[k] {
                return None;
            }
        } else {
            let a = (-h[k] - o[k]) / d[k];
            let b = (h[k] - o[k]) / d[k];
            t0 = t0.max(a.min(b));
            t1 = t1.min(a.max(b));
        }
    }
    (t1 >= t0.max(0.0)).then_some(t0.max(0.0))
}

#[allow(clippy::too_many_arguments)]
pub fn input(
    mut orbit: ResMut<Orbit>,
    mut editor: ResMut<Editor>,
    shown: Res<Shown>,
    rect: Res<ViewRect>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<MachineCamera>>,
    mut contexts: bevy_egui::EguiContexts,
    mut pressed_at: Local<Option<Vec2>>,
) {
    if editor.workspace != Workspace::Assembly {
        return;
    }
    let Some(r) = rect.0 else { return };
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let over = r.contains(bevy_egui::egui::pos2(cursor.x, cursor.y));
    let egui_busy = contexts
        .ctx_mut()
        .is_ok_and(|c| c.egui_is_using_pointer() || c.egui_wants_keyboard_input());
    if !over || egui_busy {
        *pressed_at = None;
        return;
    }
    let d = motion.delta;
    let orbiting = buttons.pressed(MouseButton::Middle) || buttons.pressed(MouseButton::Right);
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    if orbiting && shift {
        let right = Quat::from_rotation_y(orbit.yaw) * Vec3::X;
        let up = Vec3::Y;
        let k = orbit.distance * 0.0015;
        orbit.focus += (-right * d.x + up * d.y) * k;
    } else if orbiting {
        orbit.yaw -= d.x * 0.006;
        orbit.pitch = (orbit.pitch + d.y * 0.006).clamp(-1.5, 1.5);
    }
    if scroll.delta.y != 0.0 {
        orbit.distance = (orbit.distance * (1.0 - scroll.delta.y * 0.1)).clamp(0.3, 60.0);
    }
    if keys.just_pressed(KeyCode::Numpad1) {
        (orbit.yaw, orbit.pitch) = (std::f32::consts::FRAC_PI_2, 0.0);
    }
    if keys.just_pressed(KeyCode::Numpad3) {
        (orbit.yaw, orbit.pitch) = (0.0, 0.0);
    }
    if keys.just_pressed(KeyCode::Numpad7) {
        orbit.pitch = 1.5;
    }
    if keys.just_pressed(KeyCode::KeyF) || keys.just_pressed(KeyCode::NumpadDecimal) {
        *orbit = Orbit {
            yaw: orbit.yaw,
            pitch: orbit.pitch,
            ..Orbit::default()
        };
    }
    if buttons.just_pressed(MouseButton::Left) {
        *pressed_at = Some(cursor);
    }
    if buttons.just_released(MouseButton::Left)
        && pressed_at.take().is_some_and(|p| p.distance(cursor) < 4.0)
    {
        let (cam, t) = *camera;
        let at = cursor - Vec2::new(r.min.x, r.min.y);
        if let Ok(ray) = cam.viewport_to_world(t, at) {
            let hit = pick(
                &editor,
                &shown,
                from_bevy(ray.origin),
                from_bevy(*ray.direction),
            );
            let machine = editor.selection.machine.clone();
            editor.selection.placed = hit.clone();
            editor.selection.part = hit.and_then(|h| {
                machine
                    .as_ref()
                    .and_then(|m| editor.lib.machines.get(m))
                    .and_then(|m| m.placed(&h))
                    .map(|p| p.part.clone())
            });
        }
    }
}

pub fn place_camera(
    orbit: Res<Orbit>,
    rect: Res<ViewRect>,
    editor: Res<Editor>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut q: Query<(&mut Transform, &mut Camera), With<MachineCamera>>,
) {
    let Ok((mut t, mut cam)) = q.single_mut() else {
        return;
    };
    let dir = Quat::from_rotation_y(orbit.yaw) * Quat::from_rotation_x(-orbit.pitch) * Vec3::Z;
    *t = Transform::from_translation(orbit.focus + dir * orbit.distance)
        .looking_at(orbit.focus, Vec3::Y);
    cam.is_active = editor.workspace == Workspace::Assembly;
    if let Some(r) = rect.0 {
        let scale = window.scale_factor();
        let pos = (Vec2::new(r.min.x, r.min.y) * scale)
            .max(Vec2::ZERO)
            .as_uvec2();
        let size = (Vec2::new(r.width(), r.height()) * scale)
            .max(Vec2::ONE)
            .as_uvec2();
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

pub fn gizmos(mut g: Gizmos, editor: Res<Editor>, shown: Res<Shown>) {
    if editor.workspace != Workspace::Assembly {
        return;
    }
    let grey = Color::srgba(0.6, 0.6, 0.6, 0.35);
    for i in -6..=3 {
        let x = i as f32;
        g.line(
            to_bevy(DVec3::new(x as f64, -2.0, 0.0)),
            to_bevy(DVec3::new(x as f64, 2.0, 0.0)),
            grey,
        );
    }
    for j in -2..=2 {
        let y = j as f64;
        g.line(
            to_bevy(DVec3::new(-6.0, y, 0.0)),
            to_bevy(DVec3::new(3.0, y, 0.0)),
            grey,
        );
    }
    g.line(
        Vec3::ZERO,
        to_bevy(DVec3::X * 0.5),
        Color::srgb(1.0, 0.3, 0.3),
    );
    g.line(
        Vec3::ZERO,
        to_bevy(DVec3::Y * 0.5),
        Color::srgb(0.3, 1.0, 0.3),
    );
    g.line(
        Vec3::ZERO,
        to_bevy(DVec3::Z * 0.5),
        Color::srgb(0.3, 0.5, 1.0),
    );
    if let Some(cg) = shown.cg {
        g.sphere(
            Isometry3d::from_translation(to_bevy(cg)),
            0.06,
            Color::srgb(1.0, 1.0, 0.2),
        );
        g.line(
            to_bevy(cg),
            to_bevy(DVec3::new(cg.x, cg.y, 0.0)),
            Color::srgba(1.0, 1.0, 0.2, 0.5),
        );
    }
    let (Some(asm), Some(sel), Some(m)) = (
        shown.assembly.as_ref(),
        editor.selection.placed.as_ref(),
        editor
            .selection
            .machine
            .as_ref()
            .and_then(|m| editor.lib.machines.get(m)),
    ) else {
        return;
    };
    for inst in asm
        .instances
        .iter()
        .filter(|i| &m.parts[i.placed].name == sel)
    {
        let Some(part) = editor.lib.parts.get(&inst.part) else {
            continue;
        };
        for mount in &part.physical.mounts {
            let t = inst.transform * mount_transform(mount);
            let o = to_bevy(t.translation);
            for (axis, c) in [
                (DVec3::X, Color::srgb(1.0, 0.3, 0.3)),
                (DVec3::Y, Color::srgb(0.3, 1.0, 0.3)),
                (DVec3::Z, Color::srgb(0.3, 0.5, 1.0)),
            ] {
                g.line(o, to_bevy(t.transform_point3(axis * 0.15)), c);
            }
        }
    }
}
