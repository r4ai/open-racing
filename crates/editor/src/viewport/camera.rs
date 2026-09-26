//! The camera: orbiting, numpad views, framing, walking the track, and keeping it
//! inside the 3D view's rectangle.

use super::*;

/// A driver's eye height above the road, m, and how far ahead the eye looks.
pub(super) const EYE: f64 = 1.1;
pub(super) const LOOK_AHEAD: f64 = 25.0;

/// The main road as last built, for walking it.
pub fn main_sampled<'a>(editor: &Editor, built: &'a Built) -> Option<&'a Sampled> {
    let p = &editor.project;
    p.road_index(&p.main_road).and_then(|i| built.roads.get(i))
}

/// Where the camera is and what it looks at, `s` along a road, at a driver's eye.
pub(super) fn walk_view(smp: &Sampled, s: f64) -> Transform {
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

pub(super) fn perspective() -> Projection {
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
pub(super) fn set_viewport(cam: &mut Camera, rect: &ViewRect, window: &Window) {
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

#[allow(clippy::too_many_arguments)]
pub(super) fn camera_input(
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
    // While transforming with proportional editing the wheel sets its reach.
    let wheel_taken = tool.modal.is_some() && tool.proportional.on;
    if over.is_some() && scroll.delta.y != 0.0 && !wheel_taken {
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
/// The car at `t` seconds into a test lap, between its samples.
pub fn lap_at(
    lap: &[open_racing_track_project::validate::LapSample],
    t: f64,
) -> Option<open_racing_track_project::validate::LapSample> {
    let i = lap.partition_point(|s| s.t <= t);
    let (a, b) = (lap.get(i.checked_sub(1)?)?, lap.get(i).or(lap.last())?);
    let k = ((t - a.t) / (b.t - a.t).max(1e-9)).clamp(0.0, 1.0);
    let turn = (b.heading - a.heading + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU)
        - std::f64::consts::PI;
    Some(open_racing_track_project::validate::LapSample {
        t,
        pos: a.pos.lerp(b.pos, k),
        heading: a.heading + turn * k,
        speed: a.speed + (b.speed - a.speed) * k,
        off: a.off,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn view_input(
    editor: Res<Editor>,
    built: Res<Built>,
    jobs: Res<crate::jobs::Jobs>,
    mut orbit: ResMut<Orbit>,
    mut tool: ResMut<Tool>,
    wants: Res<EguiWantsInput>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
) {
    let free = tool.modal.is_none() && !tool.blocked && !wants.wants_any_keyboard_input();
    // Replaying the test lap: the view follows the car, at the lap's own pace (Shift
    // four times as fast); Esc stops.
    if let Some(t) = orbit.replay {
        let Some(last) = jobs.lap.last() else {
            orbit.replay = None;
            return;
        };
        let fast = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
        let t = t + time.delta_secs_f64() * if fast { 4.0 } else { 1.0 };
        if t > last.t || (free && keys.just_pressed(KeyCode::Escape)) {
            orbit.replay = None;
            return;
        }
        orbit.replay = Some(t);
        if let Some(car) = lap_at(&jobs.lap, t) {
            orbit.focus = to_bevy(car.pos);
            tool.hint = format!(
                "Test lap: {t:.0} s · {:.0} km/h{} · Shift fast · Esc stops",
                car.speed * 3.6,
                if car.off { " · OFF TRACK" } else { "" }
            );
        }
    }
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

/// Frames the selected nodes, or every selected road, spline and prop.
pub fn frame_selection(editor: &Editor, orbit: &mut Orbit) {
    let sel = &editor.selection;
    if let Some((_, nodes, _)) = editor.line()
        && !sel.nodes.is_empty()
    {
        let points: Vec<Vec3> = sel
            .nodes
            .iter()
            .filter_map(|&i| nodes.get(i))
            .map(|n| to_bevy(n.pos))
            .collect();
        return frame(orbit, &points);
    }
    let mut points = Vec::new();
    for item in sel.items() {
        if let Some((_, nodes, _)) = item_line(&editor.project, item) {
            points.extend(nodes.iter().map(|n| to_bevy(n.pos)));
        } else if let Item::Prop(i) = item
            && let Some(prop) = editor.project.props.get(i)
        {
            // Room round a model.
            let p = to_bevy(prop.pos);
            points.extend([p - Vec3::splat(15.0), p + Vec3::splat(15.0)]);
        }
    }
    if points.is_empty() {
        return frame_all(editor, orbit);
    }
    frame(orbit, &points);
}

/// Numpad /: the selected items on their own, framed, or back to everything and the
/// view as it was.
pub fn toggle_local(editor: &mut Editor, orbit: &mut Orbit) {
    if editor.toggle_local() {
        orbit.before_local = Some((orbit.focus, orbit.yaw, orbit.pitch, orbit.distance));
        frame_selection(editor, orbit);
        editor.status = "local view: numpad / goes back".into();
    } else if let Some((focus, yaw, pitch, distance)) = orbit.before_local.take() {
        (orbit.focus, orbit.yaw, orbit.pitch, orbit.distance) = (focus, yaw, pitch, distance);
    }
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

pub(super) fn frame(orbit: &mut Orbit, points: &[Vec3]) {
    if points.is_empty() {
        return;
    }
    let (lo, hi) = points.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), p| {
        (lo.min(*p), hi.max(*p))
    });
    orbit.focus = (lo + hi) * 0.5;
    orbit.distance = ((hi - lo).length() * 1.1).max(40.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_racing_track_project::validate::LapSample;

    #[test]
    fn a_replayed_car_eases_between_samples_the_short_way_round() {
        let at = |t: f64, x: f64, heading: f64| LapSample {
            t,
            pos: DVec3::new(x, 0.0, 0.0),
            heading,
            speed: 10.0 * t,
            off: false,
        };
        let pi = std::f64::consts::PI;
        let lap = [at(0.0, 0.0, pi - 0.1), at(1.0, 10.0, -pi + 0.1)];
        let car = lap_at(&lap, 0.5).unwrap();
        assert!((car.pos.x - 5.0).abs() < 1e-9 && (car.speed - 5.0).abs() < 1e-9);
        // Across ±π, not the long way through 0.
        assert!(car.heading.abs() > 3.0, "{}", car.heading);
        assert!(lap_at(&lap, -1.0).is_none());
    }
}
