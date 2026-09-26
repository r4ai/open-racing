//! Transforms: Blender's grab, rotate and scale, and dragging handles in the view, with
//! snapping.

use super::*;

/// Move one handle while keeping the other independent or aligned as requested.
pub(super) fn dragged_handles(
    mode: HandleMode,
    out: bool,
    moved: DVec3,
    other: DVec3,
) -> (DVec3, DVec3) {
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
        Some(Hit::Landform(index, handle)) => {
            let Some(l) = editor.project.terrain.landforms.get(index) else {
                return;
            };
            Target::Landform {
                index,
                handle,
                start: l.clone(),
            }
        }
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
                other: match side {
                    Side::Left => road.width_right.clone(),
                    Side::Right => road.width_left.clone(),
                },
            }
        }
        _ if !tool.edit && !editor.selection.others.is_empty() => {
            let (mut lines, mut props) = (Vec::new(), Vec::new());
            for item in editor.selection.items() {
                if let Item::Prop(i) = item {
                    if let Some(p) = editor.project.props.get(i) {
                        props.push((i, p.pos, p.yaw, p.scale));
                    }
                } else if let Some((_, nodes, _)) = item_line(&editor.project, item) {
                    lines.push((item, nodes.iter().copied().enumerate().collect()));
                }
            }
            Target::Many { lines, props }
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
            let Some((_, nodes, closed)) = editor.line() else {
                return;
            };
            let prop = tool.proportional;
            let around = if prop.on && mode != Mode::Width && mode != Mode::Tilt {
                let d = distances(nodes, closed, &picked, prop.connected);
                (0..nodes.len())
                    .filter(|i| !picked.contains(i))
                    .map(|i| (i, nodes[i], d[i]))
                    .collect()
            } else {
                vec![]
            };
            Target::Nodes {
                item,
                start: picked.into_iter().map(|i| (i, nodes[i])).collect(),
                around,
            }
        }
    };
    let (mode, pivot) = match &target {
        Target::Nodes { item, start, .. } => {
            let c = start.iter().map(|(_, n)| n.pos).sum::<DVec3>() / start.len().max(1) as f64;
            let active = editor
                .selection
                .node()
                .and_then(|n| start.iter().find(|(i, _)| *i == n))
                .map_or(c, |(_, n)| n.pos);
            let pivot = if mode == Mode::Grab { active } else { c };
            (mode, shown_pos(editor, built, *item, pivot))
        }
        Target::Many { lines, props } => {
            let all: Vec<DVec3> = lines
                .iter()
                .flat_map(|(_, s)| s.iter().map(|(_, n)| n.pos))
                .chain(props.iter().map(|p| p.1))
                .collect();
            (mode, all.iter().sum::<DVec3>() / all.len().max(1) as f64)
        }
        Target::Handle { node, start, .. } => (Mode::Grab, *node + *start),
        Target::Marker { .. } => (Mode::Grab, tool.pointer.unwrap_or_default()),
        Target::Landform { handle, start, .. } => {
            (Mode::Grab, landform_handle(start, built, *handle))
        }
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
        both: false,
        snapped: Default::default(),
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) fn modal(
    editor: &mut Editor,
    tool: &mut Tool,
    built: &Built,
    view: View,
    buttons: &ButtonInput<MouseButton>,
    keys: &ButtonInput<KeyCode>,
    wheel: f32,
    at: Vec2,
    shift: bool,
    ctrl: bool,
) {
    let snap = tool.snap != ctrl;
    let m = tool.modal.as_mut().expect("a transform is running");
    // The wheel or Page Up and Down change a proportional edit's reach; O switches it.
    let prop = &mut tool.proportional;
    if let Target::Nodes { around, .. } = &m.target {
        let grow = keys.just_pressed(KeyCode::PageUp) as i32
            - keys.just_pressed(KeyCode::PageDown) as i32
            + wheel.signum() as i32;
        if prop.on && grow != 0 {
            prop.radius = (prop.radius * 1.15f64.powi(grow)).clamp(0.5, 20_000.0);
        }
        if keys.just_pressed(KeyCode::KeyO) && !around.is_empty() {
            prop.on = !prop.on;
        }
    }
    let prop = *prop;
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
    let edge = matches!(m.target, Target::Edge { .. });
    if edge && keys.just_pressed(KeyCode::KeyB) {
        m.both = !m.both;
    }
    type_value(&mut m.typed, &typed_keys(keys));
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

    let pointer = Gesture {
        view,
        cursor,
        typed,
        snap,
        free: ctrl,
        snapping: tool.snapping,
        proportional: prop,
    };
    let (ops, readout) = transform_ops(editor, built, m, &pointer);
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
        _ if edge => "B both sides",
        _ if matches!(m.target, Target::Nodes { .. }) => {
            "X/Y/Z axis · O proportional · wheel reach"
        }
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

/// Adds typed keys to a value being typed: minus turns its sign round wherever it is
/// typed, as in Blender.
pub(super) fn type_value(typed: &mut String, keys: &str) {
    for ch in keys.chars() {
        if ch != '-' {
            typed.push(ch);
        } else if typed.starts_with('-') {
            typed.remove(0);
        } else {
            typed.insert(0, '-');
        }
    }
}

/// What the pointer and keys say while transforming.
#[derive(Clone, Copy)]
pub(super) struct Gesture<'a> {
    pub view: View<'a>,
    /// Where the pointer has got to, window coordinates (slowed while Shift is held).
    pub cursor: Vec2,
    /// A value typed.
    pub typed: Option<f64>,
    /// Stepping to the snapping's increments.
    pub snap: bool,
    /// Ctrl held: nothing catches.
    pub free: bool,
    pub snapping: Snapping,
    pub proportional: Proportional,
}

impl Gesture<'_> {
    /// How far the pointer moved the pivot, in the plane or up and down.
    fn slide(&self, m: &Modal) -> DVec3 {
        let (view, typed, snap) = (self.view, self.typed, self.snap);
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
                    self.snapping.height(dz)
                } else {
                    dz
                };
        }
        let (Some(a), Some(b)) = (
            view.on_plane(m.start_cursor, m.pivot.z),
            view.on_plane(self.cursor, m.pivot.z),
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
            // The pivot lands on the grid.
            let to = self.snapping.grid((m.pivot + d).truncate());
            d = (to - m.pivot.truncate()).extend(0.0);
            match m.axis {
                Axis::X => d.y = 0.0,
                Axis::Y => d.x = 0.0,
                _ => {}
            }
        }
        d
    }
}

/// The operations that put what a transform moves where the pointer now says, and a
/// readout of the change.
pub(super) fn transform_ops(
    editor: &Editor,
    built: &Built,
    m: &Modal,
    p: &Gesture,
) -> (Vec<Op>, String) {
    *m.snapped.lock().expect("snap") = None;
    match &m.target {
        Target::Nodes { .. } => nodes_ops(editor, built, m, p),
        Target::Many { .. } => many_ops(editor, m, p),
        Target::Handle { .. } => handle_ops(editor, m, p),
        Target::Reach { .. } => reach_ops(editor, built, m, p),
        Target::Edge { .. } => edge_ops(built, m, p),
        Target::Range { .. } => range_ops(editor, built, m, p),
        Target::Shape { .. } => shape_ops(m, p),
        Target::Prop { .. } => prop_ops(editor, m, p),
        Target::Marker { .. } => marker_ops(editor, built, m, p),
        Target::Landform { .. } => landform_ops(editor, m, p),
    }
}

/// Selected nodes of a road or spline moved, turned or scaled; one grabbed alone catches on what is near.
fn nodes_ops(editor: &Editor, built: &Built, m: &Modal, p: &Gesture) -> (Vec<Op>, String) {
    let Target::Nodes {
        item,
        start,
        around,
    } = &m.target
    else {
        return (vec![], String::new());
    };
    let Gesture { typed, free, .. } = *p;
    let Some((name, ..)) = item_line(&editor.project, *item) else {
        return (vec![], String::new());
    };
    let draped =
        matches!(item, Item::Spline(s) if editor.project.splines.get(*s).is_some_and(|s| s.drape));
    let ground = built.ground.as_deref().filter(|_| draped);
    let scale_axes = |k: f64| match m.axis {
        Axis::X => DVec3::new(k, 1.0, 1.0),
        Axis::Y => DVec3::new(1.0, k, 1.0),
        Axis::Z => DVec3::new(1.0, 1.0, k),
        Axis::Free => DVec3::new(k, k, 1.0),
    };
    let Some((change, readout)) = change_of(m, p, scale_axes) else {
        return (vec![], String::new());
    };
    // One node grabbed on its own snaps onto what is near it (Ctrl frees it).
    let one = m.mode == Mode::Grab && start.len() == 1 && !free && typed.is_none();
    let mut snapped = None;
    let mut ops = Vec::new();
    for &(index, node) in start {
        let mut pos = change.point(node.pos);
        if one && let Some((to, what)) = snap_node(editor, built, *item, index, pos, &p.snapping) {
            pos = to.with_z(pos.z);
            snapped = Some(what);
            *m.snapped.lock().expect("snap") = Some(to);
        }
        if let Some(g) = ground {
            pos = drape(Some(g), pos.with_z(node.pos.z.max(pos.z)));
        }
        ops.push(Op::MoveNode {
            line: name.to_string(),
            index,
            pos,
        });
        ops.extend(turned_handles(name, index, node, &change));
    }
    // With proportional editing the nodes near the selection follow, less the further
    // they are.
    let prop = p.proportional;
    let mut pulled = 0;
    for &(index, node, d) in around.iter().filter(|_| prop.on) {
        let w = prop.falloff.weight(d, prop.radius);
        if w <= 0.0 {
            continue;
        }
        pulled += 1;
        let part = change.part(w);
        let mut pos = part.point(node.pos);
        if let Some(g) = ground {
            pos = drape(Some(g), pos.with_z(node.pos.z.max(pos.z)));
        }
        ops.push(Op::MoveNode {
            line: name.to_string(),
            index,
            pos,
        });
        ops.extend(turned_handles(name, index, node, &part));
    }
    let readout = if prop.on {
        format!(
            "{readout} · proportional {:.0} m ({pulled} more)",
            prop.radius
        )
    } else {
        readout
    };
    let readout = match snapped {
        Some(what) => format!("{readout} → on {what}"),
        None => readout,
    };
    (ops, readout)
}

/// Several whole items moved, turned about the pivot or scaled from it, all alike.
fn many_ops(editor: &Editor, m: &Modal, p: &Gesture) -> (Vec<Op>, String) {
    let Target::Many { lines, props } = &m.target else {
        return (vec![], String::new());
    };
    // Moved, turned about the pivot or scaled from it, all alike.
    let Some((change, readout)) = change_of(m, p, |k| DVec3::new(k, k, 1.0)) else {
        return (vec![], String::new());
    };
    let mut ops = Vec::new();
    for (item, start) in lines {
        let Some((name, ..)) = item_line(&editor.project, *item) else {
            continue;
        };
        for &(index, node) in start {
            ops.push(Op::MoveNode {
                line: name.to_string(),
                index,
                pos: change.point(node.pos),
            });
            ops.extend(turned_handles(name, index, node, &change));
        }
    }
    for &(i, pos, yaw, scale) in props {
        if let Some(p) = editor.project.props.get(i) {
            ops.push(Op::MoveProp {
                name: p.name.clone(),
                pos: Some(change.point(pos)),
                yaw: Some(yaw + change.angle),
                scale: Some((scale * change.scale.x).max(1e-3)),
            });
        }
    }
    let n = lines.len() + props.len();
    (ops, format!("{n} items {readout}"))
}

/// A node's handle dragged.
fn handle_ops(editor: &Editor, m: &Modal, p: &Gesture) -> (Vec<Op>, String) {
    let Target::Handle {
        item,
        index,
        out,
        start,
        other,
        handle_mode,
        ..
    } = &m.target
    else {
        return (vec![], String::new());
    };
    let Some((name, ..)) = item_line(&editor.project, *item) else {
        return (vec![], String::new());
    };
    let h = *start + p.slide(m);
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

/// A strip's width or a barrier's distance from the road, from its outer edge dragged.
fn reach_ops(editor: &Editor, built: &Built, m: &Modal, p: &Gesture) -> (Vec<Op>, String) {
    let Target::Reach { end } = &m.target else {
        return (vec![], String::new());
    };
    let Gesture {
        view,
        cursor,
        typed,
        snap,
        ..
    } = *p;
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
            p.snapping.fine(v)
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
            let readout = format!("{}: {:.2} m from the edge", barrier.name, barrier.offset);
            (
                vec![Op::PutBarrier {
                    road: name,
                    barrier,
                }],
                readout,
            )
        }
        Part::Row(i) => {
            let mut row = road.rows[i].clone();
            let edge = part_reach(road, smp, end.part, &f).abs() - row.offset;
            row.offset = step(row.side.sign() * d - edge).max(0.0);
            let readout = format!("{}: {:.2} m from the edge", row.name, row.offset);
            (vec![Op::PutRow { road: name, row }], readout)
        }
    }
}

/// A road's width on one side at the grabbed nodes, from its edge dragged.
fn edge_ops(built: &Built, m: &Modal, p: &Gesture) -> (Vec<Op>, String) {
    let Target::Edge {
        road,
        index,
        count,
        closed,
        node,
        nodes,
        side,
        curve,
        other,
    } = &m.target
    else {
        return (vec![], String::new());
    };
    let Gesture {
        view,
        cursor,
        typed,
        snap,
        ..
    } = *p;
    let (Some(smp), Some(at)) = (built.roads.get(*index), view.on_plane(cursor, m.pivot.z)) else {
        return (vec![], String::new());
    };
    let f = smp.frame_at(smp.s_at(*node as f64));
    let d = side.sign() * (at - f.pos).dot(flat_left(&f));
    let w = match typed {
        Some(t) => t,
        None if snap => p.snapping.fine(d),
        None => d,
    }
    .max(0.5);
    let changes: Vec<(usize, f64)> = nodes.iter().map(|&n| (n, w)).collect();
    let set = |side: Side, curve: &StationCurve| Op::SetProfile {
        road: road.clone(),
        curve: match side {
            Side::Left => Curve::WidthLeft,
            Side::Right => Curve::WidthRight,
        },
        keys: crate::edit::node_keys(curve, *count, *closed, &changes),
    };
    let mut ops = vec![set(*side, curve)];
    if m.both {
        ops.push(set(side.other(), other));
    }
    let what = match nodes.len() {
        1 => format!("node {node}"),
        n => format!("{n} nodes"),
    };
    let total = if m.both {
        2.0 * w
    } else {
        w + match side {
            Side::Left => f.width_right,
            Side::Right => f.width_left,
        }
    };
    let sides = if m.both {
        "both sides".to_string()
    } else {
        format!("{side:?}")
    };
    (
        ops,
        format!("width {sides} {w:.2} m at {what} (total {total:.2} m)"),
    )
}

/// An end of a stretch dragged along the road; it catches on nodes and corners.
fn range_ops(editor: &Editor, built: &Built, m: &Modal, p: &Gesture) -> (Vec<Op>, String) {
    let Target::Range { end } = &m.target else {
        return (vec![], String::new());
    };
    let Gesture {
        view,
        cursor,
        snap,
        free,
        ..
    } = *p;
    let road = &editor.project.roads[end.road];
    let (Some(smp), Some(at)) = (built.roads.get(end.road), view.on_plane(cursor, m.pivot.z))
    else {
        return (vec![], String::new());
    };
    let f = &smp.frames[smp.nearest(at)];
    // Nodes and the corners' entries, apexes and exits nearby catch the end.
    let caught = (!free)
        .then(|| range_snap(smp, built.corners.get(end.road), f.s, &p.snapping))
        .flatten();
    let u = match &caught {
        Some((u, _)) => *u,
        None if snap => p.snapping.fine(f.u),
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
    // A part laid round a corner keeps where its end now is from the corner,
    // so that it stays there as the corner changes.
    let anchored = |anchor: &mut Option<Anchor>| {
        if let Some(a) = anchor
            && let Some(c) = built
                .corners
                .get(end.road)
                .and_then(|cs| corners::owner(smp, cs, a))
        {
            a.shift[end.to as usize] = corners::shift_to(smp, c, a, end.to, smp.s_at(u));
        }
    };
    let name = road.name.clone();
    let op = match end.part {
        Part::Strip(side, i) => {
            let mut strip = road.strips(side)[i].clone();
            set(&mut strip.ranges);
            anchored(&mut strip.corner);
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
            anchored(&mut barrier.corner);
            Op::PutBarrier {
                road: name,
                barrier,
            }
        }
        Part::Row(i) => {
            let mut row = road.rows[i].clone();
            set(&mut row.ranges);
            Op::PutRow { road: name, row }
        }
    };
    if caught.is_some() {
        *m.snapped.lock().expect("snap") = Some(range_end_pos(road, smp, end.part, u));
    }
    let at = caught.map_or(String::new(), |(_, what)| format!(" → on {what}"));
    (vec![op], format!("u {u:.2}, s {:.0} m{at}", f.s))
}

/// A road's width scaled (Alt S) or its bank tilted (Ctrl T) at the selected nodes.
fn shape_ops(m: &Modal, p: &Gesture) -> (Vec<Op>, String) {
    let Target::Shape {
        road,
        count,
        closed,
        nodes,
        left,
        right,
        bank,
    } = &m.target
    else {
        return (vec![], String::new());
    };
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
        let angle = p.turn(m);
        return (
            vec![set(Curve::Bank, bank, &|b| b + angle)],
            format!("{:+.1}°", angle.to_degrees()),
        );
    }
    let k = p.stretch(m);
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

/// A prop moved, turned or scaled.
fn prop_ops(editor: &Editor, m: &Modal, p: &Gesture) -> (Vec<Op>, String) {
    let Target::Prop {
        index,
        pos,
        yaw,
        scale,
    } = &m.target
    else {
        return (vec![], String::new());
    };
    let name = editor.project.props[*index].name.clone();
    let (op, readout) = match m.mode {
        Mode::Grab => {
            let d = p.slide(m);
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
            let angle = p.turn(m);
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
            let k = p.stretch(m);
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

/// The start line or a sector boundary slid along the main road.
fn marker_ops(editor: &Editor, built: &Built, m: &Modal, p: &Gesture) -> (Vec<Op>, String) {
    let Target::Marker { marker } = &m.target else {
        return (vec![], String::new());
    };
    let Gesture {
        view, cursor, snap, ..
    } = *p;
    let project = &editor.project;
    let Some(main) = project
        .road_index(&project.main_road)
        .and_then(|i| built.roads.get(i))
    else {
        return (vec![], String::new());
    };
    let Some(at) = view.on_plane(cursor, m.pivot.z) else {
        return (vec![], String::new());
    };
    let f = &main.frames[main.nearest(at)];
    let u = if snap { p.snapping.fine(f.u) } else { f.u };
    let op = match marker {
        Marker::Start => Op::SetMarkers {
            start: Some(u),
            sectors: None,
            grid: None,
        },
        Marker::Sector(i) => {
            let mut sectors = project.markers.sectors.clone();
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

/// A move, a turn about the pivot seen from above, or a scaling from it: what G, R and
/// S do to points and to the handles' offsets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Change {
    pub pivot: DVec3,
    pub shift: DVec3,
    /// Radians anticlockwise seen from above.
    pub angle: f64,
    pub scale: DVec3,
}

impl Change {
    fn identity(pivot: DVec3) -> Self {
        Self {
            pivot,
            shift: DVec3::ZERO,
            angle: 0.0,
            scale: DVec3::ONE,
        }
    }

    /// An offset turned and scaled, not moved.
    pub fn vector(&self, v: DVec3) -> DVec3 {
        let v = v * self.scale;
        DVec2::from_angle(self.angle)
            .rotate(v.truncate())
            .extend(v.z)
    }

    pub fn point(&self, p: DVec3) -> DVec3 {
        self.pivot + self.vector(p - self.pivot) + self.shift
    }

    /// The change done `w` of the way (0 nothing, 1 all of it).
    pub fn part(&self, w: f64) -> Self {
        Self {
            pivot: self.pivot,
            shift: self.shift * w,
            angle: self.angle * w,
            scale: DVec3::ONE + (self.scale - DVec3::ONE) * w,
        }
    }
}

/// The change a grab, turn or scale has reached, and a readout of it; `axes` gives the
/// scaling for a factor.
fn change_of(m: &Modal, p: &Gesture, axes: impl Fn(f64) -> DVec3) -> Option<(Change, String)> {
    let mut c = Change::identity(m.pivot);
    let readout = match m.mode {
        Mode::Grab => {
            c.shift = p.slide(m);
            let d = c.shift;
            format!("Δ ({:.2}, {:.2}, {:.2}) m", d.x, d.y, d.z)
        }
        Mode::Rotate => {
            c.angle = p.turn(m);
            format!("{:.1}°", c.angle.to_degrees())
        }
        Mode::Scale => {
            let k = p.stretch(m);
            c.scale = axes(k);
            format!("×{k:.3}")
        }
        Mode::Width | Mode::Tilt => return None,
    };
    Some((c, readout))
}

/// The operation that turns and scales a node's own handles with it; automatic ones
/// need none, and a grab moves them with the node.
fn turned_handles(line: &str, index: usize, node: Node, change: &Change) -> Option<Op> {
    if node.handles.is_auto() || (change.angle == 0.0 && change.scale == DVec3::ONE) {
        return None;
    }
    let (mode, incoming, outgoing) = node.handles.map(|v| change.vector(v)).offsets();
    Some(Op::SetNodeHandles {
        line: line.to_string(),
        index,
        mode,
        incoming,
        outgoing,
    })
}

/// The node or corner place within the snapping's reach along a road of `s`, as its
/// spline parameter and a name for it.
pub(super) fn range_snap(
    smp: &Sampled,
    corners: Option<&Vec<Corner>>,
    s: f64,
    snapping: &Snapping,
) -> Option<(f64, String)> {
    if !snapping.corners {
        return None;
    }
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
    best.filter(|b| b.0 < snapping.along)
        .map(|(_, u, what)| (u, what))
}

/// Where a single node dragged to `pos` snaps: onto another line's node (joining
/// them), a road's centreline for a road's node (a pit lane's ends), or a road's edge
/// for a kerb's or wall's node.
pub(super) fn snap_node(
    editor: &Editor,
    built: &Built,
    item: Item,
    index: usize,
    pos: DVec3,
    snapping: &Snapping,
) -> Option<(DVec3, String)> {
    let p = pos.truncate();
    let reach = snapping.reach;
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
            if snapping.nodes {
                consider(&mut best, d, n.pos, format!("node {i} of {name}"), reach);
            }
        }
    }
    if best.is_some() {
        return best.map(|(_, at, what)| (at, what));
    }
    if !snapping.edges {
        return None;
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
                reach,
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
                        reach,
                    );
                }
            }
            Item::Prop(_) => {}
        }
    }
    best.map(|(_, at, what)| (at, what))
}

impl Gesture<'_> {
    /// The turn a rotation has reached, radians anticlockwise seen from above: typed in
    /// degrees, or the pointer's angle round the pivot on screen.
    fn turn(&self, m: &Modal) -> f64 {
        let angle = match (self.typed, self.view.screen(m.pivot)) {
            (Some(deg), _) => return deg.to_radians(),
            (None, Some(c)) => {
                let a0 = (m.start_cursor - c).to_angle();
                let a1 = (self.cursor - c).to_angle();
                // The screen's y points down: clockwise on screen.
                -(a1 - a0) as f64
            }
            _ => 0.0,
        };
        if self.snap {
            self.snapping.angle(angle)
        } else {
            angle
        }
    }

    /// The factor a scaling has reached: typed, or the pointer's distance from the
    /// pivot on screen against where it began.
    fn stretch(&self, m: &Modal) -> f64 {
        let k = match (self.typed, self.view.screen(m.pivot)) {
            (Some(k), _) => return k,
            (None, Some(c)) => {
                (self.cursor.distance(c) / m.start_cursor.distance(c).max(1.0)) as f64
            }
            _ => 1.0,
        };
        if self.snap {
            self.snapping.factor(k)
        } else {
            k
        }
    }
}

/// Ctrl M: the selected nodes (in edit mode), or the selected roads, splines and props,
/// mirrored through their middle: east for west (`x`), or north for south.
pub fn mirror(editor: &mut Editor, tool: &Tool, built: &Built, x: bool) {
    let Some(pivot) = selection_pivot(editor, built) else {
        return;
    };
    let scale = if x {
        DVec3::new(-1.0, 1.0, 1.0)
    } else {
        DVec3::new(1.0, -1.0, 1.0)
    };
    let change = Change {
        scale,
        ..Change::identity(pivot)
    };
    let mut ops = Vec::new();
    let mut line_ops = |item: Item, picked: &[usize]| {
        if let Some((name, nodes, _)) = item_line(&editor.project, item) {
            for &i in picked {
                ops.push(Op::MoveNode {
                    line: name.to_string(),
                    index: i,
                    pos: change.point(nodes[i].pos),
                });
                ops.extend(turned_handles(name, i, nodes[i], &change));
            }
        }
    };
    if tool.edit {
        if let Some(item) = editor.selection.item {
            line_ops(item, &editor.picked_nodes());
        }
    } else {
        for item in editor.selection.items() {
            let count = item_line(&editor.project, item).map_or(0, |(_, n, _)| n.len());
            line_ops(item, &(0..count).collect::<Vec<_>>());
        }
        for item in editor.selection.items() {
            if let Item::Prop(i) = item
                && let Some(p) = editor.project.props.get(i)
            {
                let facing = DVec2::from_angle(p.yaw) * scale.truncate();
                ops.push(Op::MoveProp {
                    name: p.name.clone(),
                    pos: Some(change.point(p.pos)),
                    yaw: Some(facing.to_angle()),
                    scale: None,
                });
            }
        }
    }
    if !ops.is_empty() && editor.apply(ops, None) {
        editor.status = format!(
            "mirrored {}",
            if x {
                "east for west"
            } else {
                "north for south"
            }
        );
    }
}

/// A landform's middle or end moved, or its edge dragged out or in.
fn landform_ops(editor: &Editor, m: &Modal, p: &Gesture) -> (Vec<Op>, String) {
    let Target::Landform {
        index,
        handle,
        start,
    } = &m.target
    else {
        return (vec![], String::new());
    };
    let mut l = start.clone();
    let readout = match handle {
        LandformHandle::Center | LandformHandle::To => {
            let d = p.slide(m).truncate();
            if *handle == LandformHandle::Center {
                // Stretched along a line, the whole line moves.
                l.center += d;
                l.to = l.to.map(|t| t + d);
            } else {
                l.to = l.to.map(|t| t + d);
            }
            format!("{}: Δ ({:.1}, {:.1}) m", l.name, d.x, d.y)
        }
        LandformHandle::Edge => {
            let Some(at) = p.view.on_plane(p.cursor, m.pivot.z) else {
                return (vec![], String::new());
            };
            let r = match l.to {
                Some(to) => {
                    let ab = to - l.center;
                    let t = ((at.truncate() - l.center).dot(ab) / ab.length_squared().max(1e-9))
                        .clamp(0.0, 1.0);
                    at.truncate().distance(l.center + ab * t)
                }
                None => at.truncate().distance(l.center),
            };
            l.radius = match p.typed {
                Some(v) => v.max(0.0),
                None if p.snap => p.snapping.fine(r),
                None => r,
            };
            format!("{}: radius {:.1} m", l.name, l.radius)
        }
    };
    let mut terrain = editor.project.terrain.clone();
    if let Some(slot) = terrain.landforms.get_mut(*index) {
        *slot = l;
    }
    (vec![Op::SetTerrain { terrain }], readout)
}
