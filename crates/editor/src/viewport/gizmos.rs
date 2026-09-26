//! Drawing lines, nodes, handles, markers and what the tools are doing.

use super::*;

/// Lines, nodes, handles, markers, and what the tools are doing.
pub fn gizmos(
    editor: Res<Editor>,
    built: Res<Built>,
    tool: Res<Tool>,
    jobs: Res<crate::jobs::Jobs>,
    orbit: Res<Orbit>,
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
        if (!overlays.lines && !sel.has(item)) || !editor.visible(item) {
            continue;
        }
        let hovered_body = hover == Some(Hit::Body(item)) || tool.outliner_hover == Some(item);
        let line = if selected {
            theme::SELECTED
        } else if sel.has(item) {
            theme::SELECTED_OTHER
        } else if hovered_body {
            theme::HOVER
        } else {
            theme::UNSELECTED
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
            gizmos.linestrip(pts, line);
        }
        // Only the line being edited shows its nodes.
        if !(selected && tool.edit) {
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
        for (i, &pos) in shown.iter().enumerate() {
            let chosen = sel.nodes.contains(&i);
            let active = sel.node() == Some(i);
            let color = if active {
                theme::ACTIVE_NODE
            } else if chosen {
                theme::SELECTED_NODE
            } else if hover == Some(Hit::Node(item, i)) {
                theme::HOVER
            } else if i == 0 && matches!(item, Item::Road(_)) {
                theme::START
            } else {
                theme::NODE
            };
            let size = if chosen { 1.0 } else { 0.8 };
            let radius = size * node_size(eye, pos);
            gizmos.sphere(Isometry3d::from_translation(lift(pos)), radius, color);
            if active {
                for (h, o) in visible_handles(nodes, closed, i) {
                    let hovered = hover == Some(Hit::Handle(item, i, o));
                    let c = if hovered {
                        theme::HOVER
                    } else {
                        theme::SELECTED_NODE
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
                Part::Strip(..) => theme::STRIP,
                Part::Barrier(_) => theme::BARRIER,
                Part::Row(_) => theme::LANDFORM,
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
                        lit(Hit::Reach(end), theme::STRIP),
                    );
                }
            }
        }
        for &n in sel.nodes.iter().filter(|&&n| n < road.nodes.len()) {
            let centre = smp.frame_at(smp.s_at(n as f64)).pos;
            for side in [Side::Left, Side::Right] {
                let at = edge_pos(smp, n, side);
                let color = lit(Hit::Edge(r, n, side), theme::STRIP);
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
        if (!overlays.props && sel.item != Some(item)) || !editor.visible(item) {
            continue;
        }
        let color = if sel.item == Some(item) {
            theme::SELECTED
        } else if sel.has(item) {
            theme::SELECTED_OTHER
        } else if hover == Some(Hit::Body(item)) || tool.outliner_hover == Some(item) {
            theme::HOVER
        } else {
            theme::UNSELECTED
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
                let hovered = hover == Some(Hit::Landform(i, h));
                gizmos.sphere(
                    Isometry3d::from_translation(lift(at)),
                    node_size(eye, at),
                    if hovered { theme::HOVER } else { color },
                );
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
