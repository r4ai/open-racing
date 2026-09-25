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
pub(super) fn modal(
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
pub(super) fn transform_ops(
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
pub(super) const CATCH: f64 = 4.0;

/// The node or corner place within `CATCH` of `s` along a road, as its spline
/// parameter and a name for it.
pub(super) fn range_snap(
    smp: &Sampled,
    corners: Option<&Vec<Corner>>,
    s: f64,
) -> Option<(f64, String)> {
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
pub(super) fn snap_node(
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
pub(super) fn turn(
    m: &Modal,
    center: Option<Vec2>,
    cursor: Vec2,
    typed: Option<f64>,
    snap: bool,
) -> f64 {
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
pub(super) fn stretch(
    m: &Modal,
    center: Option<Vec2>,
    cursor: Vec2,
    typed: Option<f64>,
    snap: bool,
) -> f64 {
    let k = match (typed, center) {
        (Some(k), _) => return k,
        (None, Some(c)) => (cursor.distance(c) / m.start_cursor.distance(c).max(1.0)) as f64,
        _ => 1.0,
    };
    if snap { (k * 10.0).round() / 10.0 } else { k }
}
