//! Drawing lines, nodes, handles, markers and what the tools are doing.

use bevy::ecs::system::SystemParam;
use bevy::gizmos::config::{GizmoConfig, GizmoLineConfig, GizmoLineJoint};
use theme::{Look, NodeLook};

use super::*;

/// Gizmos drawn with bold lines: what is selected, and what the pointer is over.
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct Bold;

/// Dots of nodes and handles.
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct Dots;

/// How far in front of the ground bold lines and dots are drawn, as a gizmo's depth
/// bias: a dot on the road would sink half into it, and a selected line into a banked
/// or crowned road. Dots come further, over the lines through them.
const FORWARD: f32 = -0.02;
const DOTS_FORWARD: f32 = -0.03;

/// The view's gizmo groups beside the default one.
pub struct GizmoGroups;

impl Plugin for GizmoGroups {
    fn build(&self, app: &mut App) {
        add_gizmo_groups(app);
    }
}

fn add_gizmo_groups(app: &mut App) {
    let bold = GizmoConfig {
        line: GizmoLineConfig {
            width: 4.0,
            joints: GizmoLineJoint::Round(4),
            ..default()
        },
        depth_bias: FORWARD,
        ..default()
    };
    let dots = GizmoConfig {
        depth_bias: DOTS_FORWARD,
        ..default()
    };
    app.insert_gizmo_config(Bold, bold)
        .insert_gizmo_config(Dots, dots);
}

/// X-ray (Alt Z): what the view draws over the track shows through the ground.
pub fn xray(tool: Res<Tool>, mut store: ResMut<GizmoConfigStore>) {
    let set = |config: &mut GizmoConfig, forward: f32| {
        let bias = if tool.overlays.xray { -1.0 } else { forward };
        if config.depth_bias != bias {
            config.depth_bias = bias;
        }
    };
    set(store.config_mut::<DefaultGizmoConfigGroup>().0, 0.0);
    set(store.config_mut::<Bold>().0, FORWARD);
    set(store.config_mut::<Dots>().0, DOTS_FORWARD);
}

/// A plane at `at` facing the camera.
fn facing(eye: Vec3, at: Vec3) -> Isometry3d {
    Isometry3d::new(
        at,
        Quat::from_rotation_arc(Vec3::Z, (eye - at).normalize_or(Vec3::Y)),
    )
}

/// A dot facing the camera, filled with `fill` and ringed with `rim`, as Blender draws
/// vertices: seen at a glance on asphalt, grass or sky.
fn dot(dots: &mut Gizmos<Dots>, eye: Vec3, at: Vec3, r: f32, (fill, rim): (Color, Color)) {
    let plane = facing(eye, at);
    for k in [0.12, 0.26, 0.4, 0.54, 0.68, 0.8] {
        dots.circle(plane, r * k, fill).resolution(10);
    }
    dots.circle(plane, r, rim).resolution(16);
}

/// A ring round what the pointer is over, whatever colour that is drawn in.
fn hover_ring(dots: &mut Gizmos<Dots>, eye: Vec3, at: Vec3, r: f32) {
    let plane = facing(eye, at);
    for k in [1.45, 1.55, 1.65] {
        dots.circle(plane, k * r, theme::HOVER).resolution(20);
    }
}

/// The view's gizmos: thin lines, bold ones and dots.
#[derive(SystemParam)]
pub struct Pens<'w, 's> {
    thin: Gizmos<'w, 's>,
    bold: Gizmos<'w, 's, Bold>,
    dots: Gizmos<'w, 's, Dots>,
}

/// Points along a stretch of a road's part, where the view draws it.
fn stretch(road: &Road, smp: &Sampled, part: Part, rg: &Range) -> Vec<Vec3> {
    let (a, mut b) = (smp.s_at(rg.from), smp.s_at(rg.to));
    if b < a && smp.closed {
        b += smp.length;
    }
    let steps = ((b - a) / 3.0).ceil().max(1.0) as usize;
    (0..=steps)
        .map(|k| {
            let f = smp.frame_at(a + (b - a) * k as f64 / steps as f64);
            to_bevy(f.pos + f.lateral * part_offset(road, smp, part, &f) + DVec3::Z * LIFT)
        })
        .collect()
}

/// Lines, nodes, handles, markers, and what the tools are doing.
pub fn gizmos(
    editor: Res<Editor>,
    built: Res<Built>,
    tool: Res<Tool>,
    jobs: Res<crate::jobs::Jobs>,
    orbit: Res<Orbit>,
    camera: Single<&GlobalTransform, With<EditorCamera>>,
    pens: Pens,
) {
    let Pens {
        thin: mut gizmos,
        mut bold,
        mut dots,
    } = pens;
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
        let look = sel.look(
            item,
            hover == Some(Hit::Body(item)) || tool.outliner_hover == Some(item),
        );
        if (!overlays.lines && !look.selected()) || !editor.visible(item) {
            continue;
        }
        let shown: Vec<DVec3> = nodes
            .iter()
            .map(|n| shown_pos(editor, &built, item, n.pos))
            .collect();
        // The spline itself, from the last build.
        if let Some(smp) = built.sampled(item).filter(|s| s.frames.len() > 1) {
            let pts = smp
                .frames
                .iter()
                .map(|f| lift(f.pos))
                .chain(closed.then(|| lift(smp.frames[0].pos)));
            if look.bold() {
                bold.linestrip(pts, look.line());
            } else {
                gizmos.linestrip(pts, look.line());
            }
        }
        // Only the line being edited shows its nodes.
        if !(look == Look::Active && tool.edit) {
            continue;
        }
        let n = nodes.len();
        let segs = segments(n, closed);
        for i in 0..segs {
            gizmos.line(
                lift(shown[i]),
                lift(shown[(i + 1) % n]),
                theme::UNSELECTED.with_alpha(0.3),
            );
        }
        // The selected nodes last, over the others.
        let mut order: Vec<usize> = (0..shown.len()).collect();
        order.sort_by_key(|&i| sel.node_look(item, i).selected());
        for i in order {
            let pos = shown[i];
            let look = sel.node_look(item, i);
            let radius = look.size() * node_size(eye, pos);
            dot(&mut dots, eye, lift(pos), radius, look.colours());
            if hover == Some(Hit::Node(item, i)) {
                hover_ring(&mut dots, eye, lift(pos), radius);
            }
            if look == NodeLook::Active {
                for (h, o) in visible_handles(nodes, closed, i) {
                    let r = 0.65 * radius;
                    bold.line(lift(pos), lift(pos + h), theme::SELECTED_NODE);
                    dot(
                        &mut dots,
                        eye,
                        lift(pos + h),
                        r,
                        (theme::SELECTED_NODE, theme::RIM_DARK),
                    );
                    if hover == Some(Hit::Handle(item, i, o)) {
                        hover_ring(&mut dots, eye, lift(pos + h), r);
                    }
                }
            }
        }
    }

    // The part of a road the pointer is over, in the view, the outliner or the
    // properties: lit all along its stretches.
    let hot = tool.part_hover.or(match hover {
        Some(Hit::Range(e) | Hit::Reach(e)) => Some((e.road, e.part)),
        Some(Hit::Line(r, i)) => Some((r, Part::Line(i))),
        Some(Hit::Strip(r, side, i)) => Some((r, Part::Strip(side, i))),
        Some(Hit::StripKey(k)) => Some((k.road, Part::Strip(k.side, k.strip))),
        _ => None,
    });

    // Stretches of the selected road's strips, barriers, rows and painted lines, each
    // kind in a colour of its own, with their ends to drag.
    let stretches = sel.road().filter(|_| overlays.stretches);
    if let Some(r) = stretches
        && let (Some(road), Some(smp)) = (p.roads.get(r), built.roads.get(r))
        && smp.frames.len() > 1
    {
        for (part, ranges) in parts(road) {
            let color = match part {
                Part::Strip(..) => theme::STRIP,
                Part::Barrier(_) => theme::BARRIER,
                Part::Row(_) => theme::LANDFORM,
                Part::Line(_) => theme::PAINT,
            };
            for (range, rg) in ranges.iter().enumerate() {
                let pts = stretch(road, smp, part, rg);
                if hot == Some((r, part)) {
                    bold.linestrip(pts, theme::HOVER);
                } else {
                    gizmos.linestrip(pts, color);
                }
                for (to, u) in [(false, rg.from), (true, rg.to)] {
                    let end = RangeEnd {
                        road: r,
                        part,
                        range,
                        to,
                    };
                    let at = range_end_pos(road, smp, part, u);
                    let r = 0.7 * node_size(eye, at);
                    dot(&mut dots, eye, lift(at), r, (color, theme::RIM_DARK));
                    if hover == Some(Hit::Range(end)) {
                        hover_ring(&mut dots, eye, lift(at), r);
                    }
                }
            }
        }
    }
    // A part of another road, from the outliner.
    if let Some((r, part)) = hot.filter(|&(r, _)| Some(r) != stretches)
        && let (Some(road), Some(smp)) = (p.roads.get(r), built.roads.get(r))
        && smp.frames.len() > 1
        && let Some((_, ranges)) = parts(road).find(|&(q, _)| q == part)
    {
        for rg in ranges {
            bold.linestrip(stretch(road, smp, part, rg), theme::HOVER);
        }
    }

    // Handles for widths: the outer edge of each stretch (a strip's width, a barrier's
    // distance) and the road's edges at its selected nodes.
    if let Some(r) = sel.road()
        && let (Some(road), Some(smp)) = (p.roads.get(r), built.roads.get(r))
        && smp.frames.len() > 1
    {
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
                        theme::STRIP,
                    );
                    if hover == Some(Hit::Reach(end)) {
                        hover_ring(&mut dots, eye, lift(at), size);
                    }
                }
            }
        }
        // The strips' nodes, on their outer edges, joined across to the inner edge.
        if overlays.stretches {
            for key in strip_keys(road, r) {
                let Some(at) = key_pos(road, smp, key) else {
                    continue;
                };
                let f = smp.frame_at(smp.s_at(road.strips(key.side)[key.strip].keys[key.key].u));
                let inner = key.side.sign()
                    * (edge_of(&f, key.side) + inner_width(road, smp, key.side, key.strip, &f));
                gizmos.line(
                    lift(f.pos + flat_left(&f) * inner),
                    lift(at),
                    theme::STRIP_KEY,
                );
                let r = 0.8 * node_size(eye, at);
                dot(
                    &mut dots,
                    eye,
                    lift(at),
                    r,
                    (theme::STRIP_KEY, theme::RIM_DARK),
                );
                if hover == Some(Hit::StripKey(key)) {
                    hover_ring(&mut dots, eye, lift(at), r);
                }
            }
        }
        for &n in sel.nodes.iter().filter(|&&n| n < road.nodes.len()) {
            let centre = smp.frame_at(smp.s_at(n as f64)).pos;
            for side in [Side::Left, Side::Right] {
                let at = edge_pos(smp, n, side);
                gizmos.line(lift(centre), lift(at), theme::STRIP.with_alpha(0.4));
                let size = node_size(eye, at);
                gizmos.cube(
                    Transform::from_translation(lift(at)).with_scale(Vec3::splat(size * 1.2)),
                    theme::STRIP,
                );
                if hover == Some(Hit::Edge(r, n, side)) {
                    hover_ring(&mut dots, eye, lift(at), size);
                }
            }
        }
    }

    // Props: a ring where each stands and a line the way it faces.
    for (i, prop) in p.props.iter().enumerate() {
        let at = Placement::of(prop, built.ground.as_deref());
        let item = Item::Prop(i);
        let look = sel.look(
            item,
            hover == Some(Hit::Body(item)) || tool.outliner_hover == Some(item),
        );
        if (!overlays.props && !look.selected()) || !editor.visible(item) {
            continue;
        }
        let r = 1.5 * node_size(eye, at.pos);
        let base = lift(at.pos);
        let ring = Isometry3d::new(base, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2));
        let facing = DVec3::new(at.yaw.cos(), at.yaw.sin(), 0.0);
        let tip = lift(at.pos + facing * (2.0 * r as f64));
        if look.bold() {
            bold.circle(ring, r, look.line());
            bold.line(base, tip, look.line());
        } else {
            gizmos.circle(ring, r, look.line());
            gizmos.line(base, tip, look.line());
        }
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
        // An area closes back to its first point.
        let close = matches!(&d.kind, DrawKind::Spline(p) if p.is_area())
            .then(|| d.points.first().copied())
            .flatten();
        let pts: Vec<Vec3> = d
            .points
            .iter()
            .copied()
            .chain(next)
            .chain(close)
            .map(lift)
            .collect();
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

    crate::brush::draw(&tool, &built, &mut gizmos, &mut bold);

    // The last test lap: where the car went, coloured by its speed (blue slow, yellow
    // fast; darker where it brakes), red where it was off the track, and the car itself
    // while it is replayed.
    if overlays.markers && jobs.lap.len() > 1 {
        let lift_car = |p: DVec3| to_bevy(p + DVec3::Z * 0.6);
        let top = jobs.lap.iter().map(|s| s.speed).fold(1.0, f64::max);
        for w in jobs.lap.windows(2) {
            let color = if w[0].off {
                theme::OFF_TRACK
            } else {
                let k = (w[0].speed / top) as f32;
                let braking = w[1].speed < w[0].speed - 0.5;
                let c = Color::srgb(0.2 + 0.8 * k, 0.4 + 0.5 * k, 1.0 - 0.8 * k);
                if braking { c.darker(0.25) } else { c }
            };
            gizmos.line(lift_car(w[0].pos), lift_car(w[1].pos), color);
        }
    }
    if let Some(car) = orbit.replay.and_then(|t| lap_at(&jobs.lap, t)) {
        let turn = Quat::from_rotation_y(car.heading as f32);
        gizmos.cube(
            Transform::from_translation(to_bevy(car.pos + DVec3::Z * 0.6))
                .with_rotation(turn)
                .with_scale(Vec3::new(4.6, 1.2, 2.0)),
            if car.off {
                theme::OFF_TRACK
            } else {
                theme::SELECTED
            },
        );
    }

    // The terrain's landforms: their full effect and, faint, how far it eases out.
    if tool.landforms {
        let flat = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
        for (i, l) in p.terrain.landforms.iter().enumerate() {
            let color = theme::LANDFORM;
            let c = landform_handle(l, &built, LandformHandle::Center);
            let ends: Vec<DVec3> = std::iter::once(c)
                .chain(l.to.map(|_| landform_handle(l, &built, LandformHandle::To)))
                .collect();
            for &e in &ends {
                gizmos.circle(Isometry3d::new(lift(e), flat), l.radius as f32, color);
                gizmos.circle(
                    Isometry3d::new(lift(e), flat),
                    (l.radius + l.falloff) as f32,
                    color.with_alpha(0.35),
                );
            }
            if let [a, b] = ends[..] {
                let across = (b - a).truncate().perp().normalize_or(DVec2::X).extend(0.0);
                for side in [-1.0, 1.0] {
                    let o = across * side * l.radius;
                    gizmos.line(lift(a + o), lift(b + o), color);
                    let o = across * side * (l.radius + l.falloff);
                    gizmos.line(lift(a + o), lift(b + o), color.with_alpha(0.35));
                }
            }
            for h in [
                LandformHandle::Center,
                LandformHandle::To,
                LandformHandle::Edge,
            ] {
                if h == LandformHandle::To && l.to.is_none() {
                    continue;
                }
                let at = landform_handle(l, &built, h);
                let r = node_size(eye, at);
                dot(&mut dots, eye, lift(at), r, (color, theme::RIM_DARK));
                if hover == Some(Hit::Landform(i, h)) {
                    hover_ring(&mut dots, eye, lift(at), r);
                }
            }
        }
    }

    // What a grabbed node or stretch end has caught on.
    if let Some(at) = tool
        .modal
        .as_ref()
        .and_then(|m| *m.snapped.lock().expect("snap"))
    {
        let r = 1.8 * node_size(eye, at);
        let c = lift(at);
        gizmos.circle(
            Isometry3d::new(c, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
            r,
            theme::SNAP,
        );
        gizmos.circle(
            Isometry3d::new(c, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
            1.4 * r,
            theme::SNAP.with_alpha(0.5),
        );
    }

    // How far a proportional edit reaches.
    if let Some(m) = &tool.modal
        && tool.proportional.on
        && m.proportional()
    {
        gizmos.circle(
            Isometry3d::new(
                lift(m.pivot),
                Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
            ),
            tool.proportional.radius as f32,
            Color::srgba(1.0, 1.0, 1.0, 0.6),
        );
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
pub(super) fn node_size(eye: Vec3, pos: DVec3) -> f32 {
    (eye.distance(to_bevy(pos)) * 0.008).clamp(0.3, 40.0)
}
