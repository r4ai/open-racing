//! Tests of the view: picking, transforms and snapping, on a camera set up headless.

use super::*;
use bevy::camera::RenderTargetInfo;
use open_racing_track_project::Node;

/// The pointer at `cursor`, with a value typed or not, stepping or not, and Ctrl held
/// or not.
fn pointer(view: View, cursor: Vec2, typed: Option<f64>, snap: bool, free: bool) -> Gesture {
    Gesture {
        view,
        cursor,
        typed,
        snap,
        free,
        snapping: Snapping::default(),
        proportional: Proportional::default(),
    }
}

#[test]
fn cursor_keeps_window_coordinates_inside_offset_view() {
    let rect = Rect::from_corners(Vec2::new(200.0, 80.0), Vec2::new(900.0, 600.0));
    assert_eq!(
        cursor_in_view(Vec2::new(250.0, 120.0), Some(rect)),
        Some(Vec2::new(250.0, 120.0))
    );
    assert_eq!(cursor_in_view(Vec2::new(100.0, 120.0), Some(rect)), None);
    let mut tool = Tool::default();
    open_menu(&mut tool, Vec2::new(250.0, 120.0), None, false);
    assert_eq!(
        tool.menu.as_ref().map(|menu| menu.at),
        Some(Vec2::new(250.0, 120.0))
    );
}

#[test]
fn projection_round_trips_with_offset_view_and_dpi_scale() {
    for mut projection in [
        perspective(),
        Projection::Orthographic(OrthographicProjection::default_3d()),
    ] {
        let viewport = Viewport {
            physical_position: UVec2::new(400, 160),
            physical_size: UVec2::new(1400, 1040),
            ..default()
        };
        let mut camera = Camera {
            viewport: Some(viewport),
            ..default()
        };
        camera.computed.target_info = Some(RenderTargetInfo {
            physical_size: UVec2::new(2400, 1600),
            scale_factor: 2.0,
        });
        projection.update(1400.0, 1040.0);
        camera.computed.clip_from_view = projection.get_clip_from_view();
        let transform = GlobalTransform::from(
            Transform::from_xyz(0.0, 100.0, 100.0).looking_at(Vec3::ZERO, Vec3::Y),
        );
        let view = View {
            cam: &camera,
            t: &transform,
        };
        let screen = view.screen(DVec3::ZERO).expect("origin is visible");
        assert!((screen - Vec2::new(550.0, 340.0)).length() < 0.01);
        let (ray_origin, ray_direction) = view.ray(screen).expect("screen ray");
        let to_origin = -ray_origin;
        let miss = to_origin - ray_direction * to_origin.dot(ray_direction);
        assert!(miss.length() < 1e-3);
    }
}

#[test]
fn gizmo_picks_its_middle_and_arms_for_transform_tools_only() {
    let dir = std::env::temp_dir().join(format!("open-racing-editor-gizmo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut editor = Editor::open(dir.clone()).unwrap();
    editor.selection.select_node(Item::Road(0), 0);
    let built = Built::default();
    let pivot = selection_pivot(&editor, &built).expect("a node is selected");

    let mut projection = perspective();
    let mut camera = Camera::default();
    camera.computed.target_info = Some(RenderTargetInfo {
        physical_size: UVec2::new(1200, 800),
        scale_factor: 1.0,
    });
    projection.update(1200.0, 800.0);
    camera.computed.clip_from_view = projection.get_clip_from_view();
    let target = to_bevy(pivot);
    let transform = GlobalTransform::from(
        Transform::from_translation(target + Vec3::new(0.0, 60.0, 60.0))
            .looking_at(target, Vec3::Y),
    );
    let view = View {
        cam: &camera,
        t: &transform,
    };
    let (c, size) = gizmo_frame(&editor, &built, transform.translation()).unwrap();
    let middle = view.screen(c).unwrap();
    let arm = view.screen(c + DVec3::X * size * 0.8).unwrap();
    let pick = |tool, at| pick_gizmo(&editor, &built, view, tool, at);
    assert_eq!(pick(ToolKind::Move, middle), Some(Axis::Free));
    assert_eq!(pick(ToolKind::Move, arm), Some(Axis::X));
    assert_eq!(pick(ToolKind::Scale, arm), Some(Axis::X));
    assert_eq!(pick(ToolKind::Move, middle + Vec2::new(0.0, 300.0)), None);
    assert_eq!(pick(ToolKind::Select, middle), None);
    let ring = view.screen(c + DVec3::Y * size * 0.8).unwrap();
    assert_eq!(pick(ToolKind::Rotate, ring), Some(Axis::Free));
    assert_eq!(pick(ToolKind::Rotate, middle), None);
    std::fs::remove_dir_all(dir).unwrap();
}

/// An editor on a new project with its roads built, and a camera looking straight
/// down at `at` from 300 m.
fn top_down(name: &str, at: DVec3) -> (Editor, Built, Camera, GlobalTransform, PathBuf) {
    let dir =
        std::env::temp_dir().join(format!("open-racing-editor-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let editor = Editor::open(dir.clone()).unwrap();
    let built = built_of(&editor);
    let mut projection = perspective();
    let mut camera = Camera::default();
    camera.computed.target_info = Some(RenderTargetInfo {
        physical_size: UVec2::new(1200, 800),
        scale_factor: 1.0,
    });
    projection.update(1200.0, 800.0);
    camera.computed.clip_from_view = projection.get_clip_from_view();
    let target = to_bevy(at);
    let t = GlobalTransform::from(
        Transform::from_translation(target + Vec3::Y * 300.0).looking_at(target, Vec3::NEG_Z),
    );
    (editor, built, camera, t, dir)
}

#[test]
fn the_nearest_of_a_node_and_a_stretch_end_is_picked() {
    let n2 = DVec3::new(420.0, 20.0, 0.0);
    let (mut editor, _, camera, _, dir) = top_down("pick nearest", n2);
    // Seen from 1 km up, the kerb's end beside node 2 is a few pixels from it.
    let t = GlobalTransform::from(
        Transform::from_translation(to_bevy(n2) + Vec3::Y * 1000.0)
            .looking_at(to_bevy(n2), Vec3::NEG_Z),
    );
    let view = View {
        cam: &camera,
        t: &t,
    };
    let mut kerb = editor.project.roads[0].left[0].clone();
    kerb.ranges = vec![Range { from: 2.0, to: 4.4 }];
    assert!(editor.apply(
        vec![Op::PutStrip {
            road: "circuit".into(),
            side: Side::Left,
            strip: kerb,
            at: None,
        }],
        None
    ));
    let built = built_of(&editor);
    editor.selection.select(Item::Road(0));
    let road = &editor.project.roads[0];
    let lifted = |p: DVec3| view.screen(p + DVec3::Z * LIFT).unwrap();
    let node = lifted(road.nodes[2].pos);
    let end = lifted(range_end_pos(
        road,
        &built.roads[0],
        Part::Strip(Side::Left, 0),
        2.0,
    ));
    assert!(node.distance(end) < PICK_RADIUS, "both within reach");
    assert_eq!(
        pick(&editor, &built, view, node, None, true),
        Some(Hit::Node(Item::Road(0), 2))
    );
    assert!(matches!(
        pick(&editor, &built, view, end, None, true),
        Some(Hit::Range(RangeEnd { to: false, .. }))
    ));
    std::fs::remove_dir_all(dir).unwrap();
}

/// What a build of the editor's project knows.
fn built_of(editor: &Editor) -> Built {
    let scene = open_racing_track_project::bake::build(&editor.project);
    Built {
        corners: (0..editor.project.roads.len())
            .map(|i| corners::of_road(&editor.project, i).1)
            .collect(),
        roads: scene.roads.into_iter().map(|b| b.sampled).collect(),
        ..default()
    }
}

#[test]
fn dragging_a_corner_kerb_s_end_keeps_it_there_as_the_corner_changes() {
    let (mut editor, _, camera, t, dir) = top_down("corner end", DVec3::new(450.0, 130.0, 0.0));
    let (smp, cs) = corners::of_road(&editor.project, 0);
    let kit = corners::Kit::kerbs(&editor.project, None, 1.5);
    let ops = corners::kit_ops(&editor.project, "circuit", &smp, &cs, &cs[0], &kit);
    assert!(editor.apply(ops, None));
    let built = built_of(&editor);
    let view = View {
        cam: &camera,
        t: &t,
    };
    let road = &editor.project.roads[0];
    let i = road.left.iter().position(|s| s.name == "T1 apex").unwrap();
    let before = road.left[i].ranges[0];
    let part = Part::Strip(Side::Left, i);
    let smp = &built.roads[0];
    let at = view
        .screen(range_end_pos(road, smp, part, before.to))
        .unwrap();
    editor.selection.select(Item::Road(0));
    let mut tool = Tool::default();
    let end = RangeEnd {
        road: 0,
        part,
        range: 0,
        to: true,
    };
    start_modal(
        &mut editor,
        &mut tool,
        &built,
        Mode::Grab,
        Some(Hit::Range(end)),
        at,
        true,
    );
    // Its end 20 m further along the road; refitting keeps it there.
    let f = smp.frame_at(smp.s_at(before.to) + 20.0);
    let to = view.screen(f.pos).unwrap();
    let m = tool.modal.as_ref().unwrap();
    let (ops, _) = transform_ops(&editor, &built, m, &pointer(view, to, None, false, true));
    assert!(editor.apply(ops, None));
    let s = &editor.project.roads[0].left[i];
    let moved = smp.s_at(s.ranges[0].to) - smp.s_at(before.to);
    assert!((moved - 20.0).abs() < 3.0, "moved {moved}");
    let shift = s.corner.unwrap().shift;
    assert!(
        shift[0] == 0.0 && (shift[1] - 20.0).abs() < 3.0,
        "{shift:?}"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dragging_a_road_edge_or_a_strip_s_reach_sets_its_width() {
    let n0 = DVec3::new(250.0, 0.0, 0.0);
    let (mut editor, built, camera, t, dir) = top_down("edge", n0);
    let view = View {
        cam: &camera,
        t: &t,
    };
    let mut tool = Tool::default();
    // Node 1 of the oval lies on its bottom straight, driven towards +x: left is +y.
    editor.selection.select_node(Item::Road(0), 1);
    let smp = &built.roads[0];
    let handle = edge_pos(smp, 1, Side::Left);
    let at = view.screen(handle).unwrap();
    start_modal(
        &mut editor,
        &mut tool,
        &built,
        Mode::Grab,
        Some(Hit::Edge(0, 1, Side::Left)),
        at,
        true,
    );
    let m = tool.modal.as_ref().unwrap();
    let f = smp.frame_at(smp.s_at(1.0));
    let to = view.screen(f.pos + flat_left(&f) * 9.0).unwrap();
    let (ops, readout) = transform_ops(&editor, &built, m, &pointer(view, to, None, true, false));
    assert!(readout.contains("9.00"), "{readout}");
    editor.apply(ops, None);
    let r = &editor.project.roads[0];
    let w = r.width_left.eval(1.0, r.period(), true);
    assert!((w - 9.0).abs() < 1e-9, "{w}");
    assert_eq!(r.width_right.eval(1.0, r.period(), true), 6.0);
    // B sets both sides alike.
    let m = tool.modal.as_mut().unwrap();
    m.both = true;
    let (ops, readout) = transform_ops(&editor, &built, m, &pointer(view, to, None, true, false));
    assert!(readout.contains("both sides"), "{readout}");
    editor.apply(ops, None);
    let r = &editor.project.roads[0];
    assert!((r.width_right.eval(1.0, r.period(), true) - 9.0).abs() < 1e-9);
    editor.cancel_drag();
    tool.modal = None;

    // The left kerb's stretch round the first corner: drag its outer edge out.
    let road = &editor.project.roads[0];
    let rg = road.left[0].ranges[0];
    let at = view.screen(reach_pos(road, smp, Part::Strip(Side::Left, 0), &rg));
    let end = RangeEnd {
        road: 0,
        part: Part::Strip(Side::Left, 0),
        range: 0,
        to: false,
    };
    start_modal(
        &mut editor,
        &mut tool,
        &built,
        Mode::Grab,
        Some(Hit::Reach(end)),
        at.unwrap_or_default(),
        true,
    );
    let m = tool.modal.as_ref().unwrap();
    let f = smp.frame_at(range_middle(smp, &rg));
    let to = view.screen(f.pos + flat_left(&f) * (f.width_left + 2.0));
    let (ops, _) = transform_ops(
        &editor,
        &built,
        m,
        &pointer(view, to.unwrap(), None, true, false),
    );
    editor.apply(ops, None);
    let w = editor.project.roads[0].left[0].width;
    assert!((w - 2.0).abs() < 0.05, "{w}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_painted_line_is_picked_in_edit_mode_and_dragged_across() {
    let n0 = DVec3::new(250.0, 0.0, 0.0);
    let (mut editor, built, camera, t, dir) = top_down("paint", n0);
    let view = View {
        cam: &camera,
        t: &t,
    };
    editor.selection.select(Item::Road(0));
    let smp = &built.roads[0];
    let f = smp.frame_at(smp.s_at(1.0));
    let line = |e: &Editor| e.project.roads[0].lines[0].clone();
    let on = f.pos + flat_left(&f) * line(&editor).offset;
    let at = view.screen(on).unwrap();
    assert_eq!(
        pick(&editor, &built, view, at, Some(on), true),
        Some(Hit::Line(0, 0))
    );
    assert_ne!(
        pick(&editor, &built, view, at, Some(on), false),
        Some(Hit::Line(0, 0))
    );

    let mut tool = Tool::editing(true);
    tool.pointer = Some(on);
    start_modal(
        &mut editor,
        &mut tool,
        &built,
        Mode::Grab,
        Some(Hit::Line(0, 0)),
        at,
        true,
    );
    let m = tool.modal.as_ref().unwrap();
    // Near the left edge it lies just inside it; elsewhere it steps.
    let to = view.screen(f.pos + flat_left(&f) * 5.85).unwrap();
    let (ops, readout) = transform_ops(&editor, &built, m, &pointer(view, to, None, true, false));
    assert!(readout.contains("left edge"), "{readout}");
    assert!(editor.apply(ops, None));
    let l = line(&editor);
    assert!(
        (l.offset - (6.0 - 0.5 * l.width)).abs() < 1e-9,
        "{}",
        l.offset
    );
    let to = view.screen(f.pos + flat_left(&f) * 3.02).unwrap();
    let (ops, _) = transform_ops(&editor, &built, m, &pointer(view, to, None, true, false));
    assert!(editor.apply(ops, None));
    assert!((line(&editor).offset - 3.0).abs() < 1e-9);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_kerb_s_node_widens_and_raises_it_there_only() {
    let (mut editor, built, camera, t, dir) = top_down("kerb node", DVec3::new(450.0, 130.0, 0.0));
    let view = View {
        cam: &camera,
        t: &t,
    };
    editor.selection.select(Item::Road(0));
    let smp = &built.roads[0];
    // The left kerb round the first corner (u 1.6 to 4.4): a node at u 3.
    let at = smp.frame_at(smp.s_at(3.0)).pos;
    assert!(add_strip_key(&mut editor, &built, 0, Side::Left, 0, at));
    let keys = editor.project.roads[0].left[0].keys.clone();
    assert_eq!(keys.len(), 3, "{keys:?}");
    let key = KeyRef {
        road: 0,
        side: Side::Left,
        strip: 0,
        key: 1,
    };
    let road = &editor.project.roads[0];
    let handle = key_pos(road, smp, key).unwrap();
    let screen = view.screen(handle + DVec3::Z * LIFT).unwrap();
    assert_eq!(
        pick(&editor, &built, view, screen, Some(handle), false),
        Some(Hit::StripKey(key))
    );

    // Dragged 1.3 m further out: 2.5 m wide there, as it was 10 m away.
    let mut tool = Tool::editing(true);
    start_modal(
        &mut editor,
        &mut tool,
        &built,
        Mode::Grab,
        Some(Hit::StripKey(key)),
        screen,
        true,
    );
    let m = tool.modal.as_ref().unwrap();
    let f = smp.frame_at(smp.s_at(keys[1].u));
    let to = view.screen(handle + flat_left(&f) * 1.3).unwrap();
    let (ops, readout) = transform_ops(&editor, &built, m, &pointer(view, to, None, true, true));
    assert!(readout.contains("2.50 m wide"), "{readout}");
    assert!(editor.apply(ops, None));
    let strip = &editor.project.roads[0].left[0];
    assert_eq!(strip.keys[1].width, 2.5);
    assert!((smp.strip_shape(strip, smp.s_at(keys[1].u) + 10.0).0 - 1.2).abs() < 1e-6);
    // Z raises it instead.
    let m = tool.modal.as_mut().unwrap();
    m.axis = Axis::Z;
    let (ops, readout) = transform_ops(
        &editor,
        &built,
        m,
        &pointer(view, to, Some(2.0), true, true),
    );
    assert!(readout.contains("×2.00"), "{readout}");
    assert!(editor.apply(ops, None));
    assert_eq!(editor.project.roads[0].left[0].keys[1].height, 2.0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn stretch_ends_catch_on_nodes_and_spline_nodes_on_road_edges() {
    let (editor, built, _, _, dir) = top_down("snaps", DVec3::ZERO);
    let smp = &built.roads[0];
    let s2 = smp.s_at(2.0);
    let (u, what) = range_snap(smp, None, s2 + 2.5, &Snapping::default()).unwrap();
    assert_eq!((u, what.as_str()), (2.0, "node 2"));
    assert!(range_snap(smp, None, s2 + 30.0, &Snapping::default()).is_none());

    // A wall's node near the circuit's right edge.
    let mut editor = editor;
    let spline = crate::presets::named(&editor.project, "concrete wall")
        .unwrap()
        .spline(
            &editor.project,
            vec![DVec3::new(100.0, -20.0, 0.0), DVec3::new(150.0, -20.0, 0.0)],
        )
        .unwrap();
    assert!(editor.apply(vec![Op::PutSpline { spline }], None));
    // 1.5 m outside the right edge, halfway along the first straight.
    let f = smp.frame_at(smp.s_at(0.5));
    let edge = f.pos - flat_left(&f) * f.width_right;
    let near = edge - flat_left(&f) * 1.5;
    let (at, what) = snap_node(
        &editor,
        &built,
        Item::Spline(0),
        0,
        near,
        &Snapping::default(),
    )
    .unwrap();
    assert!(at.distance(edge) < 0.3, "{at:?} {edge:?}");
    assert!(what.contains("Right edge"), "{what}");
    // And onto another line's node, joining them.
    let (at, _) = snap_node(
        &editor,
        &built,
        Item::Spline(0),
        1,
        DVec3::new(249.0, 1.0, 0.0),
        &Snapping::default(),
    )
    .unwrap();
    assert_eq!(at, editor.project.roads[0].nodes[1].pos);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn tab_switches_modes_and_clicks_follow_them() {
    let (mut editor, built, _, _, dir) = top_down("modes", DVec3::ZERO);
    let mut tool = Tool::default();
    editor.selection = Default::default();
    toggle_edit(&mut editor, &mut tool);
    assert!(!tool.edit, "nothing to edit");
    editor.selection.select(Item::Road(0));
    toggle_edit(&mut editor, &mut tool);
    assert!(tool.edit);
    // In edit mode empty space drops the nodes only; Tab back drops them too.
    editor.selection.select_node(Item::Road(0), 2);
    click(&mut editor, &built, None, None, false, false, false, true);
    assert_eq!(editor.selection.item, Some(Item::Road(0)));
    assert!(editor.selection.nodes.is_empty());
    editor.selection.select_node(Item::Road(0), 2);
    toggle_edit(&mut editor, &mut tool);
    assert!(!tool.edit && editor.selection.nodes.is_empty());
    // In object mode it drops the selection.
    click(&mut editor, &built, None, None, false, false, false, false);
    assert_eq!(editor.selection.item, None);
    // Nodes selected from elsewhere (a graph) mean edit mode.
    editor.selection.select_node(Item::Road(0), 1);
    sync_mode(&editor, &mut tool);
    assert!(tool.edit);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_sketch_keeps_the_nodes_its_shape_needs() {
    // A straight run, a corner, and a straight run on: wobbles under the tolerance
    // go, the corner stays.
    let mut points: Vec<DVec3> = (0..=20)
        .map(|i| DVec3::new(i as f64, 0.02 * (i % 2) as f64, 0.0))
        .collect();
    points.extend((1..=20).map(|i| DVec3::new(20.0, i as f64, 0.0)));
    let kept = simplify(&points, 0.1);
    assert_eq!(
        kept,
        vec![
            DVec3::ZERO,
            DVec3::new(20.0, 0.0, 0.0),
            DVec3::new(20.0, 20.0, 0.0)
        ]
    );
    assert_eq!(simplify(&points[..2], 0.1), points[..2].to_vec());
}

#[test]
fn a_circle_selects_what_it_paints_over_and_shift_takes_it_out() {
    let (mut editor, _, camera, t, dir) = top_down("circle", DVec3::new(250.0, 0.0, 0.0));
    let built = built_of(&editor);
    let view = View {
        cam: &camera,
        t: &t,
    };
    editor.selection = Default::default();
    let at = view.screen(DVec3::new(250.0, 0.0, 0.0)).unwrap();
    circle_select(&mut editor, &built, view, at, 30.0, false, false);
    assert_eq!(editor.selection.item, Some(Item::Road(0)));
    // In edit mode, the nodes within it.
    circle_select(&mut editor, &built, view, at, 30.0, false, true);
    assert_eq!(editor.selection.nodes, vec![1]);
    circle_select(&mut editor, &built, view, at, 30.0, true, true);
    assert!(editor.selection.nodes.is_empty());
    circle_select(&mut editor, &built, view, at, 30.0, true, false);
    assert_eq!(editor.selection.item, None);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_selects_every_node_in_edit_mode_and_every_item_in_object_mode() {
    let (mut editor, _, _, _, dir) = top_down("select all", DVec3::ZERO);
    let spline = crate::presets::named(&editor.project, "concrete wall")
        .unwrap()
        .spline(
            &editor.project,
            vec![DVec3::new(0.0, -30.0, 0.0), DVec3::new(50.0, -30.0, 0.0)],
        )
        .unwrap();
    assert!(editor.apply(vec![Op::PutSpline { spline }], None));
    editor.selection.select(Item::Road(0));
    select_all(&mut editor, true);
    assert_eq!(
        editor.selection.nodes.len(),
        editor.project.roads[0].nodes.len()
    );
    select_all(&mut editor, false);
    assert_eq!(editor.selection.item, Some(Item::Road(0)));
    assert_eq!(editor.selection.others, vec![Item::Spline(0)]);
    assert!(editor.selection.nodes.is_empty());
    select_none(&mut editor, false);
    assert_eq!(editor.selection.item, None);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn several_items_select_move_and_delete_together() {
    let (mut editor, _, camera, t, dir) = top_down("many", DVec3::new(120.0, -30.0, 0.0));
    for (name, y) in [("a", -30.0), ("b", -45.0)] {
        let spline = crate::presets::named(&editor.project, "concrete wall")
            .unwrap()
            .spline(
                &editor.project,
                vec![DVec3::new(100.0, y, 0.0), DVec3::new(150.0, y, 0.0)],
            )
            .unwrap();
        let spline = open_racing_track_project::project::Spline {
            name: name.into(),
            drape: false,
            ..spline
        };
        assert!(editor.apply(vec![Op::PutSpline { spline }], None));
    }
    let built = built_of(&editor);
    let view = View {
        cam: &camera,
        t: &t,
    };
    // Shift + click adds, the one clicked last active; a click alone selects one.
    click(
        &mut editor,
        &built,
        Some(Hit::Body(Item::Spline(0))),
        None,
        false,
        false,
        false,
        false,
    );
    click(
        &mut editor,
        &built,
        Some(Hit::Body(Item::Spline(1))),
        None,
        true,
        false,
        false,
        false,
    );
    assert_eq!(
        editor.selection.items(),
        vec![Item::Spline(1), Item::Spline(0)]
    );
    // A box round both selects both.
    editor.selection = Default::default();
    let corners = [DVec3::new(90.0, -20.0, 0.0), DVec3::new(160.0, -55.0, 0.0)]
        .map(|p| view.screen(p).unwrap());
    let r = Rect::from_corners(corners[0], corners[1]);
    box_select(&mut editor, &built, view, r, false, false);
    let mut items = editor.selection.items();
    items.sort_by_key(|i| format!("{i:?}"));
    assert_eq!(items, vec![Item::Spline(0), Item::Spline(1)]);
    // Grabbed together by 10 m along x.
    let mut tool = Tool::default();
    let at = view.screen(DVec3::new(125.0, -37.5, 0.0)).unwrap();
    start_modal(&mut editor, &mut tool, &built, Mode::Grab, None, at, false);
    let m = tool.modal.as_ref().unwrap();
    let to = view.screen(DVec3::new(135.0, -37.5, 0.0)).unwrap();
    let (ops, readout) = transform_ops(&editor, &built, m, &pointer(view, to, None, true, false));
    assert!(readout.contains("2 items"), "{readout}");
    assert!(editor.apply(ops, None));
    editor.end_drag();
    for s in &editor.project.splines {
        assert!((s.nodes[0].pos.x - 110.0).abs() < 0.01, "{:?}", s.nodes[0]);
    }
    // Deleted together; the main road never is.
    editor.selection.others.push(Item::Road(0));
    delete(&mut editor);
    assert!(editor.project.splines.is_empty());
    assert_eq!(editor.project.roads.len(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn turning_and_scaling_nodes_takes_their_handles_along() {
    let (mut editor, built, camera, t, dir) = top_down("turn handles", DVec3::ZERO);
    let view = View {
        cam: &camera,
        t: &t,
    };
    assert!(editor.apply(
        vec![Op::SetNodeHandles {
            line: "circuit".into(),
            index: 1,
            mode: HandleMode::Free,
            incoming: DVec3::new(-20.0, 0.0, 0.0),
            outgoing: DVec3::new(30.0, 0.0, 1.0),
        }],
        None
    ));
    editor.selection.item = Some(Item::Road(0));
    editor.selection.nodes = vec![0, 1, 2];
    let mut tool = Tool::editing(true);
    start_modal(
        &mut editor,
        &mut tool,
        &built,
        Mode::Rotate,
        None,
        Vec2::ZERO,
        false,
    );
    let m = tool.modal.as_ref().unwrap();
    let (ops, _) = transform_ops(
        &editor,
        &built,
        m,
        &pointer(view, Vec2::ZERO, Some(90.0), false, false),
    );
    assert!(editor.apply(ops, None));
    let (incoming, outgoing) = handles(&editor.project.roads[0].nodes, true, 1);
    assert!(
        (incoming - DVec3::new(0.0, -20.0, 0.0)).length() < 1e-9,
        "{incoming}"
    );
    assert!(
        (outgoing - DVec3::new(0.0, 30.0, 1.0)).length() < 1e-9,
        "{outgoing}"
    );
    editor.cancel_drag();

    // Scaling by 2 across the plane doubles them, and keeps them level.
    let mut tool = Tool::editing(true);
    start_modal(
        &mut editor,
        &mut tool,
        &built,
        Mode::Scale,
        None,
        Vec2::ZERO,
        false,
    );
    let m = tool.modal.as_ref().unwrap();
    let (ops, _) = transform_ops(
        &editor,
        &built,
        m,
        &pointer(view, Vec2::ZERO, Some(2.0), false, false),
    );
    assert!(editor.apply(ops, None));
    let (_, outgoing) = handles(&editor.project.roads[0].nodes, true, 1);
    assert!(
        (outgoing - DVec3::new(60.0, 0.0, 1.0)).length() < 1e-9,
        "{outgoing}"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn proportional_editing_pulls_the_nodes_near_along_the_line() {
    let (mut editor, built, camera, t, dir) = top_down("proportional", DVec3::ZERO);
    let view = View {
        cam: &camera,
        t: &t,
    };
    let before = editor.project.roads[0].nodes.clone();
    editor.selection.select_node(Item::Road(0), 1);
    let mut tool = Tool::editing(true);
    tool.proportional.on = true;
    tool.proportional.falloff = Falloff::Linear;
    // Node 1 is 250 m from 0 and ~171 m from 2; reach 300 m pulls both.
    tool.proportional.radius = 300.0;
    start_modal(
        &mut editor,
        &mut tool,
        &built,
        Mode::Grab,
        None,
        Vec2::ZERO,
        false,
    );
    tool.modal.as_mut().unwrap().axis = Axis::Y;
    let m = tool.modal.as_ref().unwrap();
    let mut g = pointer(view, Vec2::ZERO, Some(10.0), false, true);
    g.proportional = tool.proportional;
    let (ops, readout) = transform_ops(&editor, &built, m, &g);
    assert!(readout.contains("proportional"), "{readout}");
    assert!(editor.apply(ops, None));
    let after = &editor.project.roads[0].nodes;
    let dy = |i: usize| after[i].pos.y - before[i].pos.y;
    assert!((dy(1) - 10.0).abs() < 1e-9);
    let d0 = before[0].pos.distance(before[1].pos);
    assert!(
        (dy(0) - 10.0 * (1.0 - d0 / 300.0)).abs() < 1e-9,
        "{}",
        dy(0)
    );
    assert!(dy(2) > 0.0 && dy(2) < 10.0);
    // Far along the loop: untouched.
    assert_eq!(dy(5), 0.0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn falloffs_run_from_all_to_nothing() {
    for f in Falloff::ALL {
        assert!((f.weight(0.0, 10.0) - 1.0).abs() < 1e-9, "{f:?}");
        assert_eq!(f.weight(10.0, 10.0), 0.0);
        let mid = f.weight(5.0, 10.0);
        assert!((0.0..=1.0).contains(&mid));
    }
    let nodes: Vec<Node> = (0..5)
        .map(|i| Node::at(i as f64 * 10.0, 0.0, 0.0))
        .collect();
    assert_eq!(
        distances(&nodes, true, &[0], true),
        vec![0.0, 10.0, 20.0, 30.0, 40.0]
    );
}

#[test]
fn mirroring_flips_nodes_and_handles_through_the_middle() {
    let (mut editor, built, _, _, dir) = top_down("mirror", DVec3::ZERO);
    assert!(editor.apply(
        vec![Op::SetNodeHandles {
            line: "circuit".into(),
            index: 1,
            mode: HandleMode::Free,
            incoming: DVec3::new(-20.0, 5.0, 0.0),
            outgoing: DVec3::new(30.0, 0.0, 0.0),
        }],
        None
    ));
    let before = editor.project.roads[0].nodes.clone();
    editor.selection.select(Item::Road(0));
    let pivot = selection_pivot(&editor, &built).unwrap();
    mirror(&mut editor, &Tool::default(), &built, true);
    let after = &editor.project.roads[0].nodes;
    for (a, b) in before.iter().zip(after) {
        assert!((b.pos.x - (2.0 * pivot.x - a.pos.x)).abs() < 1e-9);
        assert_eq!(b.pos.y, a.pos.y);
    }
    let (incoming, outgoing) = handles(after, true, 1);
    assert_eq!(incoming, DVec3::new(20.0, 5.0, 0.0));
    assert_eq!(outgoing, DVec3::new(-30.0, 0.0, 0.0));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn minus_turns_a_typed_value_s_sign_round() {
    let mut typed = String::new();
    for keys in ["1", "2", "-", ".", "5", "-", "-"] {
        type_value(&mut typed, keys);
    }
    assert_eq!(typed, "-12.5");
}

#[test]
fn delete_in_edit_mode_without_nodes_keeps_the_line() {
    let (mut editor, _, _, _, dir) = top_down("delete-edit", DVec3::ZERO);
    let spline = crate::presets::named(&editor.project, "concrete wall")
        .unwrap()
        .spline(
            &editor.project,
            vec![DVec3::new(0.0, -30.0, 0.0), DVec3::new(50.0, -30.0, 0.0)],
        )
        .unwrap();
    assert!(editor.apply(vec![Op::PutSpline { spline }], None));
    editor.selection.select(Item::Spline(0));
    delete_selected(&mut editor, &Tool::editing(true));
    assert_eq!(editor.project.splines.len(), 1);
    delete_selected(&mut editor, &Tool::editing(false));
    assert!(editor.project.splines.is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn view_dirs_are_recognised_and_left_for_perspective() {
    let mut orbit = Orbit::default();
    assert_eq!(ViewDir::of(&orbit), None);
    for v in ViewDir::ALL {
        look(&mut orbit, v);
        assert_eq!(ViewDir::of(&orbit), Some(v));
        assert_eq!(
            ViewDir::of(&orbit)
                .map(ViewDir::opposite)
                .map(ViewDir::opposite),
            Some(v)
        );
    }
    assert!(orbit.ortho);
    orbit_by(&mut orbit, 0.1, 0.0);
    assert!(
        !orbit.ortho,
        "orbiting away from a numpad view goes back to perspective"
    );
    orbit.ortho = true;
    orbit.auto_ortho = false;
    orbit_by(&mut orbit, 0.1, 0.0);
    assert!(orbit.ortho, "an orthographic view chosen by hand stays");
}

#[test]
fn end_node_has_only_its_nonzero_handle() {
    let nodes = [Node::new(DVec3::ZERO), Node::new(DVec3::X * 30.0)];
    let first: Vec<_> = visible_handles(&nodes, false, 0).collect();
    let last: Vec<_> = visible_handles(&nodes, false, 1).collect();
    assert_eq!(first, vec![(DVec3::X * 10.0, true)]);
    assert_eq!(last, vec![(-DVec3::X * 10.0, false)]);
}

#[test]
fn dragging_handle_respects_aligned_and_free_modes() {
    let (incoming, outgoing) =
        dragged_handles(HandleMode::Aligned, true, DVec3::Y * 5.0, -DVec3::X * 2.0);
    assert_eq!(incoming, -DVec3::Y * 2.0);
    assert_eq!(outgoing, DVec3::Y * 5.0);
    let (incoming, outgoing) =
        dragged_handles(HandleMode::Free, false, -DVec3::Y * 3.0, DVec3::X * 4.0);
    assert_eq!(incoming, -DVec3::Y * 3.0);
    assert_eq!(outgoing, DVec3::X * 4.0);
    let (incoming, outgoing) =
        dragged_handles(HandleMode::Auto, false, -DVec3::Y * 3.0, DVec3::ZERO);
    assert_eq!(incoming, -DVec3::Y * 3.0);
    assert_eq!(outgoing, DVec3::Y * 3.0);
}

#[test]
fn nearest_node_or_handle_wins_and_node_wins_a_tie() {
    let node = Hit::Node(Item::Road(0), 0);
    let handle = Hit::Handle(Item::Road(0), 0, true);
    let mut best = None;
    consider_pick(
        &mut best,
        node,
        Some(Vec2::new(250.0, 120.0)),
        Vec2::new(250.0, 120.0),
    );
    consider_pick(
        &mut best,
        handle,
        Some(Vec2::new(254.0, 120.0)),
        Vec2::new(250.0, 120.0),
    );
    assert_eq!(best.map(|(hit, _)| hit), Some(node));
    let mut best = None;
    consider_pick(
        &mut best,
        node,
        Some(Vec2::new(250.0, 120.0)),
        Vec2::new(254.0, 120.0),
    );
    consider_pick(
        &mut best,
        handle,
        Some(Vec2::new(254.0, 120.0)),
        Vec2::new(254.0, 120.0),
    );
    assert_eq!(best.map(|(hit, _)| hit), Some(handle));
    let mut best = None;
    consider_pick(
        &mut best,
        node,
        Some(Vec2::new(250.0, 120.0)),
        Vec2::new(252.0, 120.0),
    );
    consider_pick(
        &mut best,
        handle,
        Some(Vec2::new(254.0, 120.0)),
        Vec2::new(252.0, 120.0),
    );
    assert_eq!(best.map(|(hit, _)| hit), Some(node));
}

#[test]
fn release_outside_finishes_box_and_clears_press_state() {
    let mut tool = Tool {
        press: Some((Vec2::new(250.0, 120.0), None, false)),
        boxing: Some((Vec2::new(250.0, 120.0), Vec2::new(890.0, 580.0))),
        ..default()
    };
    let Some(LeftRelease::Box(rect)) = finish_left(&mut tool, Some(Vec2::new(950.0, 650.0)), false)
    else {
        panic!("box selection should finish outside the view");
    };
    assert_eq!(rect.min, Vec2::new(250.0, 120.0));
    assert_eq!(rect.max, Vec2::new(950.0, 650.0));
    assert!(tool.press.is_none());
    assert!(tool.boxing.is_none());

    tool.press = Some((Vec2::new(250.0, 120.0), None, false));
    assert!(finish_left(&mut tool, None, false).is_none());
    assert!(tool.press.is_none());
}

#[test]
fn right_drag_or_release_outside_does_not_open_menu() {
    let mut tool = Tool {
        right_press: Some((Vec2::new(250.0, 120.0), 8.0)),
        ..default()
    };
    assert_eq!(finish_right(&mut tool, Some(Vec2::new(250.0, 120.0))), None);
    assert!(tool.right_press.is_none());
    tool.right_press = Some((Vec2::new(250.0, 120.0), 0.0));
    assert_eq!(finish_right(&mut tool, None), None);
    assert!(tool.right_press.is_none());
    tool.right_press = Some((Vec2::new(250.0, 120.0), 0.0));
    assert_eq!(
        finish_right(&mut tool, Some(Vec2::new(250.0, 120.0))),
        Some(Vec2::new(250.0, 120.0))
    );
}
