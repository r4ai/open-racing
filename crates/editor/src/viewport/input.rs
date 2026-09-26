//! The pointer and keys in the view: selecting, building, deleting and drawing.

use super::*;

/// Keys pressed this frame that type a value.
pub(super) fn typed_keys(keys: &ButtonInput<KeyCode>) -> String {
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
    time: Res<Time>,
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
    sync_mode(editor, tool);
    let pointer_free =
        !wants.wants_any_pointer_input() && !rect.ui_busy && tool.menu.is_none() && !tool.blocked;
    // A panel's edge dragged from beside the view: whatever the press began here is the
    // panel's, not a box to select with.
    if rect.ui_dragging && tool.modal.is_none() {
        tool.press = None;
        tool.boxing = None;
    }
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
        &mut orbit,
        tool,
        &buttons,
        &keys,
        &motion,
        &scroll,
        over,
        t,
        time.delta_secs(),
        alt,
        shift,
        ctrl,
    );
    // Flying takes the keys.
    if tool.flying.is_some() {
        return;
    }
    // Circle select: painting over nodes or items selects them.
    if tool.circle.is_some() {
        circle(
            editor,
            tool,
            &built,
            view,
            &buttons,
            &keys,
            scroll.delta.y,
            over,
            shift,
        );
        return;
    }

    // A transform in progress takes every input.
    if tool.modal.is_some() {
        let at = anywhere.unwrap_or_else(|| tool.modal.as_ref().expect("a transform").last_cursor);
        modal(
            editor,
            tool,
            &built,
            view,
            &buttons,
            &keys,
            scroll.delta.y,
            at,
            shift,
            ctrl,
        );
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

    // A brush paints with the left button; the right button and the keys work as
    // ever, unless a stroke is being painted.
    if tool.active.is_brush() {
        crate::brush::input(
            editor, tool, &built, &buttons, &keys, over, anywhere, keys_free,
        );
        tool.press = None;
        tool.boxing = None;
        if tool.brush.stroke.is_some() || tool.brush.adjust.is_some() {
            return;
        }
    }
    let hover = over.filter(|_| !tool.active.is_brush()).and_then(|at| {
        pick_gizmo(editor, &built, view, tool.active, at)
            .map(Hit::Gizmo)
            .or_else(|| {
                tool.landforms
                    .then(|| pick_landform(editor, &built, view, at))
                    .flatten()
            })
            .or_else(|| pick(editor, &built, view, at, tool.pointer, tool.edit))
    });
    tool.hover = hover;

    // Left button: a click selects, a drag grabs what it began on or draws a box.
    if buttons.just_pressed(MouseButton::Left) && !tool.active.is_brush() {
        tool.press = over.map(|at| (at, hover, alt));
    }
    if let Some((from, hit, orbiting)) = tool.press {
        if !buttons.pressed(MouseButton::Left) {
            match finish_left(tool, anywhere, over.is_some()) {
                Some(LeftRelease::Box(rect)) => {
                    box_select(editor, &built, view, rect, shift, tool.edit)
                }
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
                    click(
                        editor,
                        &built,
                        hit,
                        tool.pointer,
                        shift,
                        add,
                        alt,
                        tool.edit,
                    );
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
                // Object mode, as Blender's tweak: dragging an item moves it, with
                // the rest of the selection when it is part of it.
                Some(Hit::Body(item))
                    if !shift && (!tool.edit || matches!(item, Item::Prop(_))) =>
                {
                    tool.press = None;
                    if !editor.selection.has(item) {
                        editor.selection.select(item);
                    }
                    editor.selection.nodes.clear();
                    tool.edit = false;
                    start_modal(editor, tool, &built, Mode::Grab, None, from, true);
                }
                Some(
                    h @ (Hit::Node(..)
                    | Hit::Handle(..)
                    | Hit::Marker(_)
                    | Hit::Range(_)
                    | Hit::Reach(_)
                    | Hit::Edge(..)
                    | Hit::Line(..)
                    | Hit::StripKey(_)
                    | Hit::Landform(..)),
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

    if !keys_free {
        return;
    }
    // Tab switches modes wherever the pointer is, as long as nothing is being typed.
    if keys.just_pressed(KeyCode::Tab) && !ctrl && !alt {
        toggle_edit(editor, tool);
    }
    let Some(at) = over else { return };
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
        select_none(editor, tool.edit);
    } else if pressed(KeyCode::KeyA) && !ctrl {
        select_all(editor, tool.edit);
    } else if pressed(KeyCode::KeyH) && alt {
        editor.reveal();
    } else if pressed(KeyCode::KeyH) && shift {
        editor.hide_unselected();
    } else if pressed(KeyCode::KeyH) && !ctrl {
        editor.hide_selected();
    } else if pressed(KeyCode::NumpadDivide) || pressed(KeyCode::Slash) {
        toggle_local(editor, &mut orbit);
    } else if pressed(KeyCode::KeyZ) && alt {
        tool.overlays.xray = !tool.overlays.xray;
        editor.status = format!("X-ray {}", if tool.overlays.xray { "on" } else { "off" });
    } else if pressed(KeyCode::KeyC) && !ctrl && !alt && !shift {
        tool.circle = Some(if tool.circle_radius > 0.0 {
            tool.circle_radius
        } else {
            40.0
        });
    } else if pressed(KeyCode::KeyO) && !ctrl && !alt {
        editor.status = tool.toggle_proportional();
    } else if pressed(KeyCode::KeyX) || pressed(KeyCode::Delete) {
        delete_selected(editor, tool);
    } else if pressed(KeyCode::NumpadAdd) && ctrl {
        crate::edit::select_more(editor);
    } else if pressed(KeyCode::NumpadSubtract) && ctrl {
        crate::edit::select_less(editor);
    } else if pressed(KeyCode::Escape) {
        editor.selection.nodes.clear();
    }
}

/// Circle select (C), as Blender's: the left button paints over what to select, Shift
/// or the middle button over what not to; the wheel sizes the circle; a right click,
/// Esc or Enter ends it.
#[allow(clippy::too_many_arguments)]
pub(super) fn circle(
    editor: &mut Editor,
    tool: &mut Tool,
    built: &Built,
    view: View,
    buttons: &ButtonInput<MouseButton>,
    keys: &ButtonInput<KeyCode>,
    wheel: f32,
    over: Option<Vec2>,
    shift: bool,
) {
    let mut r = tool.circle.expect("selecting with a circle");
    if wheel != 0.0 {
        r = (r * 1.15f32.powf(wheel.signum())).clamp(4.0, 400.0);
    }
    tool.circle = Some(r);
    tool.circle_radius = r;
    tool.press = None;
    tool.boxing = None;
    tool.hint = "Circle select: paint over nodes or items · Shift or middle button: deselect · wheel: size · right click, Esc or Enter: done".into();
    if buttons.just_pressed(MouseButton::Right)
        || keys.any_just_pressed([KeyCode::Escape, KeyCode::Enter, KeyCode::NumpadEnter])
    {
        tool.circle = None;
        tool.right_press = None;
        return;
    }
    let Some(at) = over else { return };
    let remove =
        buttons.pressed(MouseButton::Middle) || (shift && buttons.pressed(MouseButton::Left));
    if remove || buttons.pressed(MouseButton::Left) {
        circle_select(editor, built, view, at, r, remove, tool.edit);
    }
}

/// Selects (or with `remove`, deselects) what lies within `r` of `at` on screen: the
/// edited line's nodes in edit mode, else roads, splines and props.
pub(super) fn circle_select(
    editor: &mut Editor,
    built: &Built,
    view: View,
    at: Vec2,
    r: f32,
    remove: bool,
    edit: bool,
) {
    let within = |p: DVec3| {
        view.screen(p + DVec3::Z * LIFT)
            .is_some_and(|s| s.distance(at) <= r)
    };
    if edit {
        let Some(item) = editor.selection.item else {
            return;
        };
        let Some((_, nodes, _)) = item_line(&editor.project, item) else {
            return;
        };
        let hits: Vec<usize> = (0..nodes.len())
            .filter(|&i| within(shown_pos(editor, built, item, nodes[i].pos)))
            .collect();
        let sel = &mut editor.selection.nodes;
        for n in hits {
            if remove {
                sel.retain(|&x| x != n);
            } else if !sel.contains(&n) {
                sel.push(n);
            }
        }
        return;
    }
    // Lines anywhere along them, props where they stand.
    let mut hits: Vec<Item> = items(editor)
        .filter(|&i| editor.pickable(i))
        .filter(|&i| {
            let smp = match i {
                Item::Road(r) => built.roads.get(r),
                Item::Spline(s) => built.splines.get(s),
                Item::Prop(_) => None,
            };
            smp.is_some_and(|s| s.frames.iter().any(|f| within(f.pos)))
        })
        .collect();
    for (i, prop) in editor.project.props.iter().enumerate() {
        if editor.pickable(Item::Prop(i))
            && within(Placement::of(prop, built.ground.as_deref()).pos)
        {
            hits.push(Item::Prop(i));
        }
    }
    let sel = &mut editor.selection;
    for item in hits {
        if remove {
            sel.remove(item);
        } else {
            sel.add(item);
        }
    }
}

pub(super) fn open_menu(tool: &mut Tool, at: Vec2, hit: Option<Hit>, add_only: bool) {
    tool.menu = Some(Menu {
        at,
        world: tool.pointer,
        hit,
        add_only,
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) fn click(
    editor: &mut Editor,
    built: &Built,
    hit: Option<Hit>,
    pointer: Option<DVec3>,
    shift: bool,
    add: bool,
    alt: bool,
    edit: bool,
) {
    if add {
        match (hit, pointer) {
            // Ctrl + click on a strip: a node of the strip there.
            (Some(Hit::Strip(r, side, i)), Some(at)) => {
                add_strip_key(editor, built, r, side, i, at);
            }
            _ => add_node_at(editor, built, pointer),
        }
        return;
    }
    match hit {
        Some(Hit::Node(item, n)) if shift => editor.selection.toggle_node(item, n),
        Some(Hit::Node(item, n)) => editor.selection.select_node(item, n),
        Some(Hit::Handle(item, n, _)) if alt => crate::edit::auto_handles(editor, item, n),
        // In object mode Shift adds items to the selection, or takes the active out.
        Some(Hit::Body(item)) if shift && !edit => editor.selection.toggle_item(item),
        Some(Hit::Body(item)) => {
            if editor.selection.item != Some(item) || !editor.selection.others.is_empty() {
                editor.selection.select(item);
            } else if !shift {
                editor.selection.nodes.clear();
            }
        }
        Some(
            Hit::Handle(..)
            | Hit::Marker(_)
            | Hit::Landform(..)
            | Hit::Range(_)
            | Hit::Reach(_)
            | Hit::Edge(..)
            | Hit::Line(..)
            | Hit::StripKey(_)
            | Hit::Strip(..)
            | Hit::Gizmo(_),
        ) => {}
        // Empty space: in edit mode no nodes, in object mode nothing at all.
        None if shift => {}
        None if edit => editor.selection.nodes.clear(),
        None => editor.selection = Default::default(),
    }
}

pub(super) fn box_select(
    editor: &mut Editor,
    built: &Built,
    view: View,
    r: Rect,
    add: bool,
    edit: bool,
) {
    // In edit mode the nodes of the line being edited; in object mode the line with
    // most nodes in the box.
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
    if edit {
        let Some(item) = editor.selection.item else {
            return;
        };
        let found = inside(item);
        if !add {
            editor.selection.nodes.clear();
        }
        for n in found {
            if !editor.selection.nodes.contains(&n) {
                editor.selection.nodes.push(n);
            }
        }
        return;
    }
    // Every line with nodes in the box, and every prop standing in it; the line with
    // most nodes in it active, unless adding to what is selected.
    let mut found: Vec<(Item, usize)> = items(editor)
        .filter(|&i| editor.pickable(i))
        .map(|i| (i, inside(i).len()))
        .filter(|&(_, n)| n > 0)
        .collect();
    for (i, prop) in editor.project.props.iter().enumerate() {
        if !editor.pickable(Item::Prop(i)) {
            continue;
        }
        let at = Placement::of(prop, built.ground.as_deref()).pos;
        if view
            .screen(at + DVec3::Z * LIFT)
            .is_some_and(|s| r.contains(s))
        {
            found.push((Item::Prop(i), 1));
        }
    }
    found.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
    if !add {
        editor.selection = Default::default();
    }
    for (item, _) in found {
        editor.selection.add(item);
    }
}

/// A: in edit mode every node of the line, in object mode every item that can be
/// picked (the active one staying active).
pub fn select_all(editor: &mut Editor, edit: bool) {
    if edit {
        if let Some((_, nodes, _)) = editor.line() {
            let n = nodes.len();
            editor.selection.nodes = (0..n).collect();
        }
        return;
    }
    let items: Vec<Item> = editor
        .all_items()
        .into_iter()
        .filter(|&i| editor.pickable(i))
        .collect();
    let sel = &mut editor.selection;
    let active = sel.item.filter(|i| items.contains(i));
    *sel = Default::default();
    sel.item = active.or(items.first().copied());
    sel.others = items.into_iter().filter(|&i| Some(i) != sel.item).collect();
}

/// Alt A: in edit mode no nodes, in object mode nothing.
pub fn select_none(editor: &mut Editor, edit: bool) {
    if edit {
        editor.selection.nodes.clear();
    } else {
        editor.selection = Default::default();
    }
}

/// X: in edit mode the selected nodes, in object mode the selected item. Edit mode with
/// no nodes selected deletes nothing, as Blender's, rather than the whole line.
pub fn delete_selected(editor: &mut Editor, tool: &Tool) {
    if tool.edit && editor.selection.nodes.is_empty() {
        editor.status = "no nodes selected (Tab to object mode deletes the whole line)".into();
        return;
    }
    delete(editor);
}

/// Deletes the selected nodes, or else every selected spline, road and prop.
pub fn delete(editor: &mut Editor) {
    let Some(item) = editor.selection.item else {
        return;
    };
    if editor.selection.nodes.is_empty() && !editor.selection.others.is_empty() {
        return delete_items(editor);
    }
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

/// Deletes every selected item but the main road, by name so that their places in
/// the lists do not matter.
fn delete_items(editor: &mut Editor) {
    let p = &editor.project;
    let mut ops = Vec::new();
    let mut kept = false;
    for item in editor.selection.items() {
        match item {
            Item::Road(r) => match p.roads.get(r) {
                Some(road) if road.name == p.main_road => kept = true,
                Some(road) => ops.push(Op::RemoveRoad {
                    road: road.name.clone(),
                }),
                None => {}
            },
            Item::Spline(s) => {
                if let Some(sp) = p.splines.get(s) {
                    ops.push(Op::RemoveSpline {
                        name: sp.name.clone(),
                    });
                }
            }
            Item::Prop(i) => {
                if let Some(x) = p.props.get(i) {
                    ops.push(Op::RemoveProp {
                        name: x.name.clone(),
                    });
                }
            }
        }
    }
    let n = ops.len();
    if !ops.is_empty() && editor.apply(ops, None) {
        editor.selection = Default::default();
        editor.status = if kept {
            format!("deleted {n}; the main road stays")
        } else {
            format!("deleted {n}")
        };
    }
}

/// Where a node goes along a line so that the line passes through `pos`: into the
/// segment nearest to it, in plan.
pub(super) fn insert_index(
    nodes: &[open_racing_track_project::Node],
    closed: bool,
    pos: DVec3,
) -> usize {
    let n = nodes.len();
    let p = pos.truncate();
    let gap = |i: usize| {
        let (a, b) = (nodes[i].pos.truncate(), nodes[(i + 1) % n].pos.truncate());
        let ab = b - a;
        let t = ((p - a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
        p.distance(a + ab * t)
    };
    (0..segments(n, closed))
        .min_by(|&a, &b| gap(a).total_cmp(&gap(b)))
        .map_or(n, |i| i + 1)
}

/// Adds a node at the pointer to the selected road or spline: with the active node at
/// an open line's end, beyond it (extending the line); else into the segment it fits
/// best.
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
        // At an open end: extend it.
        Some(0) if !closed && nodes.len() > 1 => 0,
        Some(n) if !closed && n + 1 == nodes.len() => n + 1,
        _ => insert_index(nodes, closed, pos),
    };
    // A road node at the height of the road where it passes nearest.
    let pos = match item {
        Item::Road(r) => built
            .roads
            .get(r)
            .and_then(|smp| smp.frames.get(smp.nearest(pos)))
            .map_or(pos, |f| pos.with_z(f.pos.z)),
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

/// How far either side of a strip's first node the two nodes holding it as it is go, m.
const KEY_HOLD: f64 = 10.0;

/// Adds a node to strip `i` of road `r` where `at` is along the road, at the width and
/// height it has there. A strip's first node comes with one either side, `KEY_HOLD`
/// along, holding it as it is round them: dragging the node then changes it only
/// nearby.
pub fn add_strip_key(
    editor: &mut Editor,
    built: &Built,
    r: usize,
    side: Side,
    i: usize,
    at: DVec3,
) -> bool {
    use open_racing_track_project::project::StripKey;
    let (Some(road), Some(smp)) = (editor.project.roads.get(r), built.roads.get(r)) else {
        return false;
    };
    let Some(mut strip) = road.strips(side).get(i).cloned() else {
        return false;
    };
    let s = smp.frames[smp.nearest(at)].s;
    let key = |s: f64| {
        let s = if smp.closed {
            s.rem_euclid(smp.length)
        } else {
            s.clamp(0.0, smp.length)
        };
        let (width, height) = smp.strip_shape(&strip, s);
        StripKey {
            u: smp.u_at(s),
            width,
            height,
        }
    };
    let mut keys = vec![key(s)];
    if strip.keys.is_empty() {
        keys.extend([key(s - KEY_HOLD), key(s + KEY_HOLD)]);
    }
    let added = keys.len();
    strip.keys.extend(keys);
    strip.keys.sort_by(|a, b| a.u.total_cmp(&b.u));
    strip.keys.dedup_by(|a, b| (a.u - b.u).abs() < 1e-4);
    let (road, name) = (road.name.clone(), strip.name.clone());
    let done = editor.apply(
        vec![Op::PutStrip {
            road,
            side,
            strip,
            at: None,
        }],
        None,
    );
    if done {
        editor.status = if added > 1 {
            format!(
                "a node on {name}, with two holding it as it is 10 m either side: drag it out to widen it there, Z to raise it"
            )
        } else {
            format!("a node on {name}: drag it out to widen it there, Z to raise it")
        };
    }
    done
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

/// Shift + D: copies of the selected roads, splines and props, grabbed together.
pub fn duplicate(editor: &mut Editor, tool: &mut Tool, built: &Built, at: Vec2) {
    let items = editor.selection.items();
    if items.is_empty() {
        editor.status = "select a road, spline or prop to duplicate".into();
        return;
    }
    // Each copy's name taken before the next is made, so that none clash.
    let mut project = editor.project.clone();
    let mut ops = Vec::new();
    let mut copies = Vec::new();
    for item in items {
        let p = &project;
        let op = match item {
            Item::Road(r) if r < p.roads.len() => {
                let mut copy = p.roads[r].clone();
                copy.name = unique_name(p, &copy.name);
                copies.push(Item::Road(p.roads.len()));
                Op::PutRoad { road: copy }
            }
            Item::Spline(s) if s < p.splines.len() => {
                let mut copy = p.splines[s].clone();
                copy.name = unique_name(p, &copy.name);
                copies.push(Item::Spline(p.splines.len()));
                Op::PutSpline { spline: copy }
            }
            Item::Prop(i) if i < p.props.len() => {
                let mut copy = p.props[i].clone();
                copy.name = unique_prop_name(p, &copy.name);
                copies.push(Item::Prop(p.props.len()));
                Op::PutProp { prop: copy }
            }
            _ => continue,
        };
        if op.apply(&mut project).is_err() {
            continue;
        }
        ops.push(op);
    }
    editor.begin_drag();
    if editor.apply(ops, None) {
        editor.selection.set_items(copies);
        tool.edit = false;
        start_modal(editor, tool, built, Mode::Grab, None, at, false);
    } else {
        editor.cancel_drag();
    }
}

/// Where the draw tool would put a point: the ground under the pointer, or for a band,
/// beside the nearest road edge within reach (Ctrl frees it).
pub(super) fn draw_point(
    editor: &Editor,
    built: &Built,
    kind: &DrawKind,
    pointer: DVec3,
    ctrl: bool,
    snapping: &Snapping,
) -> DVec3 {
    let DrawKind::Spline(preset) = kind else {
        return pointer;
    };
    if ctrl || !preset.is_band() || !snapping.edges {
        return pointer;
    }
    let half = match preset.shape(&editor.project) {
        Some((Shape::Band { width, .. }, _)) => 0.5 * width,
        _ => 0.0,
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
            if gap < snapping.reach && best.is_none_or(|b| gap < b.1) {
                let at = edge + side.sign() * half;
                best = Some((f.pos + f.lateral * at, gap));
            }
        }
    }
    best.map_or(pointer, |(p, _)| p)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw(
    editor: &mut Editor,
    tool: &mut Tool,
    built: &Built,
    buttons: &ButtonInput<MouseButton>,
    keys: &ButtonInput<KeyCode>,
    over: Option<Vec2>,
    ctrl: bool,
    keys_free: bool,
) {
    let kind = tool.draw.as_ref().map(|d| d.kind.clone()).expect("drawing");
    tool.draw_at = tool
        .pointer
        .map(|p| draw_point(editor, built, &kind, p, ctrl, &tool.snapping));
    let area = matches!(&kind, DrawKind::Spline(p) if p.is_area());
    tool.hint = format!(
        "Draw: click to add points, or drag to sketch · Backspace removes the last · Enter or right click finishes{} · Esc cancels · Ctrl: no snapping",
        if area { " (closing the area)" } else { "" }
    );
    let d = tool.draw.as_mut().expect("drawing");
    if over.is_some()
        && buttons.just_pressed(MouseButton::Left)
        && let Some(p) = tool.draw_at
    {
        d.points.push(p);
        d.sketch = over.map(|at| (d.points.len() - 1, at));
    }
    // Dragging sketches: a point every few pixels, simplified once let go to the nodes
    // that keep the line's shape.
    if let Some((first, last)) = d.sketch {
        if buttons.pressed(MouseButton::Left) {
            if let (Some(at), Some(p)) = (over, tool.draw_at)
                && at.distance(last) >= SKETCH_STEP
            {
                d.points.push(p);
                d.sketch = Some((first, at));
            }
        } else {
            d.sketch = None;
            if d.points.len() - first > 2 {
                let sketched = d.points.split_off(first);
                let span = sketched
                    .windows(2)
                    .map(|w| w[0].distance(w[1]))
                    .sum::<f64>();
                // Finer for short sketches (a kerb round an apex) than long ones.
                let tolerance = (span * 0.004).clamp(0.08, 1.5);
                d.points.extend(simplify(&sketched, tolerance));
            }
        }
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
    let fewest = if area { 3 } else { 2 };
    if tool.draw.as_ref().is_some_and(|d| d.points.len() < fewest) {
        // Keep drawing: a stray Enter or right click should not lose the first point.
        editor.status = format!("it needs at least {fewest} points (Esc cancels)");
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
        DrawKind::Spline(preset) => {
            let Some(spline) = preset.spline(&editor.project, d.points) else {
                editor.status = format!("there is no type \"{}\" any more", preset.name());
                return;
            };
            if editor.apply(vec![Op::PutSpline { spline }], None) {
                editor
                    .selection
                    .select(Item::Spline(editor.project.splines.len() - 1));
            }
        }
    }
}
