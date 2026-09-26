//! What is under the pointer: nodes, handles, stretch ends, markers, bodies and the
//! active tool's gizmo.

use super::*;

/// What is under the pointer: range ends, the nearest node or active handle,
/// then markers and the roads and splines themselves.
pub(super) fn pick(
    editor: &Editor,
    built: &Built,
    view: View,
    at: Vec2,
    ground_at: Option<DVec3>,
    edit: bool,
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
    // In edit mode, the nodes of the line being edited; none in object mode.
    if let Some(item) = sel.item.filter(|_| edit)
        && let Some((_, nodes, _)) = item_line(&editor.project, item)
    {
        for (i, node) in nodes.iter().enumerate() {
            let p = shown_pos(editor, built, item, node.pos) + DVec3::Z * LIFT;
            consider_pick(&mut best, Hit::Node(item, i), view.screen(p), at);
        }
    }
    if let (Some(item), Some(n)) = (sel.item.filter(|_| edit), sel.node())
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
    if edit && let Some(hit) = pick_line(editor, built, view, at, ground_at) {
        return Some(hit);
    }
    // Props, by where they stand.
    for (i, prop) in editor.project.props.iter().enumerate() {
        if !editor.pickable(Item::Prop(i)) {
            continue;
        }
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

/// The painted line of the selected road under the pointer: near it on screen, or on
/// it where it is wide.
fn pick_line(
    editor: &Editor,
    built: &Built,
    view: View,
    at: Vec2,
    ground_at: Option<DVec3>,
) -> Option<Hit> {
    let r = editor.selection.road()?;
    let (road, smp) = (editor.project.roads.get(r)?, built.roads.get(r)?);
    let g = ground_at?;
    let f = smp.frames.get(smp.nearest(g))?;
    let along = (g - f.pos).dot(f.tangent);
    let across = (g - f.pos).dot(flat_left(f));
    let mut best = None;
    for (i, l) in road.lines.iter().enumerate() {
        if smp.presence(&l.ranges, 0.0, f.s) <= 0.5 {
            continue;
        }
        if (across - l.offset).abs() <= 0.5 * l.width {
            return Some(Hit::Line(r, i));
        }
        let p = f.pos + f.tangent * along + flat_left(f) * l.offset;
        consider_pick(&mut best, Hit::Line(r, i), view.screen(p), at);
    }
    best.map(|(hit, _)| hit)
}

/// Where the selection's transform gizmo stands: a prop, or the middle of the selected
/// nodes (all of them with none selected, as moving the whole line).
pub fn selection_pivot(editor: &Editor, built: &Built) -> Option<DVec3> {
    let item = editor.selection.item?;
    // Several items: the middle of all of them.
    if !editor.selection.others.is_empty() && editor.selection.nodes.is_empty() {
        let points: Vec<DVec3> = editor
            .selection
            .items()
            .into_iter()
            .flat_map(|i| match i {
                Item::Prop(p) => editor
                    .project
                    .props
                    .get(p)
                    .map(|x| Placement::of(x, built.ground.as_deref()).pos)
                    .into_iter()
                    .collect::<Vec<_>>(),
                _ => item_line(&editor.project, i).map_or(vec![], |(_, nodes, _)| {
                    nodes
                        .iter()
                        .map(|n| shown_pos(editor, built, i, n.pos))
                        .collect()
                }),
            })
            .collect();
        if !points.is_empty() {
            return Some(points.iter().sum::<DVec3>() / points.len() as f64);
        }
    }
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
pub(super) fn gizmo_frame(editor: &Editor, built: &Built, eye: Vec3) -> Option<(DVec3, f64)> {
    let p = selection_pivot(editor, built)? + DVec3::Z * LIFT;
    Some((p, (eye.distance(to_bevy(p)) * 0.11).max(1.0) as f64))
}

pub(super) const GIZMO_AXES: [(Axis, DVec3); 3] = [
    (Axis::X, DVec3::X),
    (Axis::Y, DVec3::Y),
    (Axis::Z, DVec3::Z),
];

pub(super) fn axis_color(axis: Axis) -> Color {
    match axis {
        Axis::X => Color::srgb(0.96, 0.25, 0.33),
        Axis::Y => Color::srgb(0.53, 0.84, 0.13),
        Axis::Z => Color::srgb(0.18, 0.52, 1.0),
        Axis::Free => Color::srgb(0.9, 0.9, 0.9),
    }
}

/// Points round the rotate gizmo's ring.
pub(super) fn gizmo_ring(p: DVec3, size: f64) -> impl Iterator<Item = DVec3> {
    (0..=48).map(move |k| {
        let a = k as f64 / 48.0 * std::f64::consts::TAU;
        p + DVec3::new(a.cos(), a.sin(), 0.0) * size * 0.8
    })
}

/// The part of the active tool's gizmo under the pointer.
pub(super) fn pick_gizmo(
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

pub(super) fn consider_pick(
    best: &mut Option<(Hit, f32)>,
    hit: Hit,
    screen: Option<Vec2>,
    at: Vec2,
) {
    if let Some(distance) = screen.map(|p| p.distance(at))
        && distance < PICK_RADIUS
        && best.is_none_or(|(_, current)| distance < current)
    {
        *best = Some((hit, distance));
    }
}

/// Only handles with a visible length can be picked or drawn.
pub(super) fn visible_handles(
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

/// The spline or road whose body covers `p`, splines first.
pub(super) fn body_at(editor: &Editor, built: &Built, p: DVec3) -> Option<Item> {
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
        if smp.frames.is_empty() || !editor.pickable(Item::Spline(i)) {
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
        if smp.frames.is_empty() || !editor.pickable(Item::Road(i)) {
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

pub(super) fn distance_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Where a landform's handle is: on the ground under it.
pub fn landform_handle(
    l: &open_racing_track_project::project::Landform,
    built: &Built,
    handle: LandformHandle,
) -> DVec3 {
    let across = match l.to {
        Some(to) => (to - l.center).perp().normalize_or(DVec2::X),
        None => DVec2::X,
    };
    let p = match handle {
        LandformHandle::Center => l.center,
        LandformHandle::To => l.to.unwrap_or(l.center),
        LandformHandle::Edge => l.center + across * l.radius,
    };
    drape(built.ground.as_deref(), p.extend(1e4))
}

/// The landform handle under the pointer.
pub(super) fn pick_landform(editor: &Editor, built: &Built, view: View, at: Vec2) -> Option<Hit> {
    let mut best = None;
    for (i, l) in editor.project.terrain.landforms.iter().enumerate() {
        let handles = [
            LandformHandle::Center,
            LandformHandle::To,
            LandformHandle::Edge,
        ];
        for h in handles {
            if h == LandformHandle::To && l.to.is_none() {
                continue;
            }
            let p = landform_handle(l, built, h) + DVec3::Z * LIFT;
            consider_pick(&mut best, Hit::Landform(i, h), view.screen(p), at);
        }
    }
    best.map(|(hit, _)| hit)
}
