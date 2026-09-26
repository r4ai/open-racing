//! The selected road's elevation along its length. The view keeps its scale until asked
//! to fit, so a node dragged up or down follows the pointer instead of the axes moving
//! under it. Nodes move by how far the pointer moves, not by where it is.
//!
//! Nodes are picked as in the view (a click, Shift + click, a box, A and Alt A) and the
//! selection is the view's; dragging one or G moves every selected node up or down,
//! double-click or Ctrl + click on the line adds a node, X deletes the selected, and the
//! right click opens a menu of what can be done to them.

use bevy_egui::egui;
use open_racing_track_project::curve::Sampled;
use open_racing_track_project::ops::Op;

use crate::commands::{Cmd, Ctx, entry};
use crate::edit;
use crate::graph::{self, Axes, Pick, Plot};
use crate::state::{Editor, Item};

/// Smallest height span a fit shows, m.
const MIN_SPAN: f64 = 10.0;
/// Pointer distance within which a node is grabbed, px.
const GRAB: f32 = 12.0;

/// What part of the profile is shown, and the nodes being dragged.
#[derive(Default)]
pub struct ProfileView {
    /// Road the view was fitted to; another road fits again.
    road: Option<String>,
    /// Distance along the road (x) and height (y) shown, m.
    axes: Axes,
    /// Keep one metre of height as long on screen as one metre of distance.
    true_scale: bool,
    drag: Option<NodeDrag>,
    /// Where a box being dragged out began.
    boxing: Option<egui::Pos2>,
    /// Where the menu was opened: distance along the road and height, m.
    menu_at: Option<(f64, f64)>,
    /// The press that ended a grab (or cancelled it) clicks nothing when let go.
    swallow: bool,
    /// Where the graph was drawn last.
    plot: Option<Plot>,
}

struct NodeDrag {
    /// The road the nodes are on.
    road: String,
    /// The nodes moved and their heights when it began, the grabbed one last.
    start: Vec<(usize, f64)>,
    /// Started with G: follows the pointer without a button held.
    modal: bool,
    /// Height change the drag has reached, before snapping.
    dz: f64,
}

impl ProfileView {
    /// Puts back nodes being dragged, when the view goes away or shows another road.
    pub fn stop(&mut self, editor: &mut Editor) {
        self.boxing = None;
        if self.drag.take().is_some() {
            editor.cancel_drag();
        }
    }

    fn fit(&mut self, smp: &Sampled, nodes: impl Iterator<Item = f64>) {
        let heights = smp.frames.iter().map(|f| f.pos.z).chain(nodes);
        // Room above for the nodes' numbers.
        self.axes = Axes::fit((0.0, smp.length.max(1.0)), heights, 0.2, MIN_SPAN);
        self.axes.y.1 += 0.1 * (self.axes.y.1 - self.axes.y.0);
    }
}

pub fn profile(ui: &mut egui::Ui, c: &mut Ctx, view: &mut ProfileView) {
    let Some((r, road)) = c.editor.road() else {
        view.stop(c.editor);
        ui.label("Select a road to see its elevation.");
        return;
    };
    let road = road.clone();
    if view
        .drag
        .as_ref()
        .is_some_and(|d| d.road != road.name || d.start.iter().any(|&(i, _)| i >= road.nodes.len()))
    {
        view.stop(c.editor);
    }
    // Sampled from the project as it is now, so the curve and the nodes agree while
    // dragging.
    let smp = Sampled::new(&road, road.resolution.max(1.0));
    if road.nodes.len() < 2 || smp.frames.is_empty() {
        return;
    }
    let node_s: Vec<f64> = (0..road.nodes.len()).map(|i| smp.s_at(i as f64)).collect();
    let selected: Vec<usize> = if c.editor.selection.item == Some(Item::Road(r)) {
        c.editor.selection.nodes.clone()
    } else {
        vec![]
    };
    let mut fit = view.road.as_deref() != Some(road.name.as_str());

    ui.horizontal(|ui| {
        ui.strong(format!(
            "Elevation of \"{}\" ({:.0} m)",
            road.name, smp.length
        ));
        for (cmd, label, tip) in [
            (
                Cmd::SmoothHeights,
                "Smooth",
                "Smooth the selected nodes' heights (all with none selected)",
            ),
            (
                Cmd::Flatten,
                "Flatten",
                "Put the selected nodes at their mean height (all with none selected)",
            ),
            (
                Cmd::EvenGrade,
                "Even grade",
                "A steady slope from the first selected node to the last",
            ),
        ] {
            if ui
                .add_enabled(cmd.enabled(c), egui::Button::new(label).small())
                .on_hover_text(tip)
                .clicked()
            {
                crate::commands::run(cmd, c);
            }
        }
        if ui
            .add_enabled(!selected.is_empty(), egui::Button::new("Delete").small())
            .on_hover_text("Delete the selected nodes (X)")
            .clicked()
        {
            edit::delete_nodes(c.editor);
        }
        fit |= ui
            .button("Fit")
            .on_hover_text("Home, or double-click the background")
            .clicked();
        ui.checkbox(&mut view.true_scale, "1:1");
        ui.weak("controls").on_hover_text(
            "Click: select a node · Shift: add or take out · drag over empty space: box (Shift adds, Ctrl takes out) · A: all · Alt A: none\n\
             Drag a node, or G over the view: move the selected up or down\n\
             Shift: fine · Ctrl: snap to 0.1 m · click or Enter: done · right click or Esc: undo\n\
             Double-click or Ctrl + click the line: add a node there · X or Delete: delete the selected\n\
             Right click: menu · middle drag: pan · wheel: zoom along the road · Ctrl + wheel: zoom the height\n\
             Home or double-click the background: fit",
        );
    });

    let size = ui.available_size();
    let (resp, painter) = ui.allocate_painter(
        egui::vec2(size.x, size.y.max(60.0)),
        egui::Sense::click_and_drag(),
    );
    let rect = resp.rect.shrink(8.0);
    let hovered = resp.hovered() || view.drag.is_some();
    let key = |k, m| graph::key(ui, hovered, k, m);
    if fit || key(egui::Key::Home, egui::Modifiers::NONE) {
        view.road = Some(road.name.clone());
        view.fit(&smp, road.nodes.iter().map(|n| n.pos.z));
    }
    let plot_of = |axes| Plot { rect, axes };
    if view.drag.is_none() {
        graph::navigate(
            ui,
            &resp,
            &plot_of(view.axes),
            &mut view.axes,
            view.true_scale,
        );
    }
    if view.true_scale {
        let mid = 0.5 * (view.axes.y.0 + view.axes.y.1);
        let half = 0.5 * (view.axes.x.1 - view.axes.x.0) * (rect.height() / rect.width()) as f64;
        view.axes.y = (mid - half, mid + half);
    }
    let plot = plot_of(view.axes);
    view.plot = Some(plot);
    let painter = painter.with_clip_rect(resp.rect);
    painter.rect_filled(resp.rect, 4.0, egui::Color32::from_gray(24));
    graph::grid(&painter, &plot, " m");

    let line: Vec<egui::Pos2> = smp
        .frames
        .iter()
        .map(|f| plot.pos(f.s, f.pos.z))
        .chain(
            smp.closed
                .then(|| plot.pos(smp.length, smp.frames[0].pos.z)),
        )
        .collect();
    painter.add(egui::Shape::line(
        line,
        egui::Stroke::new(2.0, egui::Color32::from_rgb(90, 200, 255)),
    ));

    let nodes: Vec<egui::Pos2> = road
        .nodes
        .iter()
        .zip(&node_s)
        .map(|(n, &s)| plot.pos(s, n.pos.z))
        .collect();
    // Grade between neighbouring nodes.
    for i in 0..road.segments() {
        let j = (i + 1) % road.nodes.len();
        let ds = if j == 0 {
            smp.length - node_s[i]
        } else {
            node_s[j] - node_s[i]
        };
        if ds <= 0.0 {
            continue;
        }
        let grade = 100.0 * (road.nodes[j].pos.z - road.nodes[i].pos.z) / ds;
        let end = if j == 0 {
            egui::pos2(plot.x(smp.length), nodes[0].y)
        } else {
            nodes[j]
        };
        // Only where there is room for it.
        if end.x - nodes[i].x < 44.0 {
            continue;
        }
        let mid = nodes[i].lerp(end, 0.5);
        painter.text(
            mid + egui::vec2(0.0, 4.0),
            egui::Align2::CENTER_TOP,
            format!("{grade:+.1}%"),
            egui::FontId::monospace(10.0),
            grade_color(grade),
        );
    }
    let active = selected.last().copied();
    let mut last_label = f32::NEG_INFINITY;
    for (i, &p) in nodes.iter().enumerate() {
        let lit = selected.contains(&i);
        let color = graph::point_color(
            lit,
            active == Some(i),
            egui::Color32::from_rgb(90, 230, 255),
        );
        painter.circle_filled(p, if lit { 6.0 } else { 4.5 }, color);
        // Numbers where they have room, and always on selected nodes.
        if lit || p.x - last_label > 22.0 {
            last_label = p.x;
            painter.text(
                p + egui::vec2(0.0, -9.0),
                egui::Align2::CENTER_BOTTOM,
                i.to_string(),
                egui::FontId::monospace(10.0),
                if lit { color } else { egui::Color32::WHITE },
            );
        }
    }

    // Where the pointer is along the road.
    if let Some(hover) = resp.hover_pos() {
        let s = plot.x_at(hover.x);
        if (0.0..=smp.length).contains(&s) && view.drag.is_none() {
            let f = smp.frame_at(s);
            let grade = 100.0 * f.tangent.z / f.tangent.truncate().length().max(1e-9);
            painter.vline(
                plot.x(s),
                rect.y_range(),
                egui::Stroke::new(1.0, egui::Color32::from_gray(70)),
            );
            painter.circle_filled(plot.pos(s, f.pos.z), 3.0, egui::Color32::WHITE);
            painter.text(
                rect.right_top(),
                egui::Align2::RIGHT_TOP,
                format!("s {s:.0} m  z {:.2} m  {grade:+.1}%", f.pos.z),
                egui::FontId::monospace(11.0),
                egui::Color32::from_gray(200),
            );
        }
    }

    let nearest = |pos: egui::Pos2| {
        nodes
            .iter()
            .enumerate()
            .map(|(i, p)| (i, p.distance(pos)))
            .filter(|&(_, d)| d < GRAB)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    };
    // Whether a point of the view lies on the road's line.
    let on_line = |pos: egui::Pos2| {
        let s = plot.x_at(pos.x);
        (0.0..=smp.length).contains(&s) && (plot.y(smp.frame_at(s).pos.z) - pos.y).abs() < GRAB
    };
    let modifiers = ui.input(|i| i.modifiers);

    // Moving nodes: dragging one, or G (Blender's grab: follows the pointer until a
    // click or Enter; Esc or a right click puts them back).
    if let Some(mut d) = view.drag.take() {
        let dy = if d.modal {
            ui.input(|i| i.pointer.delta().y)
        } else {
            resp.drag_delta().y
        };
        let (_, per_px) = plot.delta(egui::vec2(0.0, -1.0));
        d.dz -= dy as f64 * per_px * if modifiers.shift { 0.1 } else { 1.0 };
        let &(grabbed, z0) = d.start.last().expect("a node is grabbed");
        let dz = if modifiers.command {
            ((z0 + d.dz) * 10.0).round() / 10.0 - z0
        } else {
            d.dz
        };
        let ops: Vec<Op> = d
            .start
            .iter()
            .filter(|&&(i, z)| road.nodes[i].pos.z != z + dz)
            .map(|&(i, z)| Op::MoveNode {
                line: road.name.clone(),
                index: i,
                pos: road.nodes[i].pos.with_z(z + dz),
            })
            .collect();
        if !ops.is_empty() {
            c.editor.apply(ops, None);
        }
        painter.text(
            rect.left_top(),
            egui::Align2::LEFT_TOP,
            if d.start.len() > 1 {
                format!("{} nodes  {dz:+.2} m", d.start.len())
            } else {
                format!("node {grabbed}  z {:.2} m  ({dz:+.2} m)", z0 + dz)
            },
            egui::FontId::monospace(11.0),
            egui::Color32::from_rgb(255, 215, 30),
        );
        let (confirm, cancel) = if d.modal {
            ui.input(|i| {
                (
                    i.pointer.primary_pressed() || i.key_pressed(egui::Key::Enter),
                    i.pointer.secondary_pressed() || i.key_pressed(egui::Key::Escape),
                )
            })
        } else {
            (
                resp.drag_stopped() || !ui.input(|i| i.pointer.primary_down()),
                ui.input(|i| i.key_pressed(egui::Key::Escape)),
            )
        };
        if cancel {
            view.swallow = true;
            c.editor.cancel_drag();
        } else if confirm {
            view.swallow = d.modal;
            c.editor.end_drag();
            // Make room for nodes dragged out of view.
            view.axes.include(d.start.iter().map(|&(_, z)| z + dz));
        } else {
            view.drag = Some(d);
        }
        return;
    }

    // A press that ended a grab clicks nothing.
    if view.swallow {
        if resp.clicked() || resp.secondary_clicked() {
            view.swallow = false;
            return;
        }
        if !ui.input(|i| i.pointer.any_down()) {
            view.swallow = false;
        }
    }

    // Starts moving the selected nodes, `grabbed` last.
    let grab = |view: &mut ProfileView, editor: &mut Editor, grabbed: usize, modal: bool| {
        let mut moved = editor.selection.nodes.clone();
        moved.retain(|&i| i != grabbed && i < road.nodes.len());
        moved.push(grabbed);
        view.drag = Some(NodeDrag {
            road: road.name.clone(),
            start: moved.iter().map(|&i| (i, road.nodes[i].pos.z)).collect(),
            modal,
            dz: 0.0,
        });
        editor.begin_drag();
    };

    // The right click's menu.
    if resp.secondary_clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        view.menu_at = Some((plot.x_at(p.x), plot.y_at(p.y)));
        if let Some(i) = nearest(p)
            && !selected.contains(&i)
        {
            c.editor.selection.select_node(Item::Road(r), i);
        }
    }
    let mut add_at = None;
    resp.context_menu(|ui| {
        ui.set_min_width(200.0);
        let (s, z) = view.menu_at.unwrap_or((0.0, 0.0));
        if ui.button("Add Node Here").clicked() {
            add_at = Some((s, z));
            ui.close();
        }
        let n = c.editor.selection.nodes.len();
        if ui
            .add_enabled(
                n > 0,
                egui::Button::new(format!("Delete {n} Selected Nodes")).shortcut_text("X"),
            )
            .clicked()
        {
            edit::delete_nodes(c.editor);
            ui.close();
        }
        entry(ui, c, Cmd::Subdivide);
        ui.separator();
        entry(ui, c, Cmd::SmoothHeights);
        entry(ui, c, Cmd::Flatten);
        entry(ui, c, Cmd::EvenGrade);
        ui.separator();
        if ui
            .add(egui::Button::new("Select All").shortcut_text("A"))
            .clicked()
        {
            crate::viewport::select_all(c.editor, true);
            ui.close();
        }
        if ui
            .add(egui::Button::new("Select None").shortcut_text("Alt A"))
            .clicked()
        {
            crate::viewport::select_none(c.editor, true);
            ui.close();
        }
        entry(ui, c, Cmd::SelectMore);
        entry(ui, c, Cmd::SelectLess);
        ui.separator();
        if ui
            .add(egui::Button::new("Fit").shortcut_text("Home"))
            .clicked()
        {
            view.fit(&smp, road.nodes.iter().map(|n| n.pos.z));
            ui.close();
        }
    });

    // The left button: dragging a node moves the selected ones, over empty space draws
    // a box; a click selects.
    // Not while the view is moving something.
    let busy = c.editor.dragging();
    if resp.drag_started_by(egui::PointerButton::Primary)
        && !busy
        && let Some(origin) = graph::press_origin(ui, &resp)
    {
        match nearest(origin) {
            Some(i) => {
                let sel = &mut c.editor.selection;
                if !selected.contains(&i) {
                    if modifiers.shift {
                        sel.toggle_node(Item::Road(r), i);
                    } else {
                        sel.select_node(Item::Road(r), i);
                    }
                }
                grab(view, c.editor, i, false);
                if let Some(d) = &mut view.drag {
                    // The pointer has moved a little from where it went down.
                    let now = ui.input(|i| i.pointer.latest_pos()).unwrap_or(origin);
                    let (_, dz) = plot.delta(now - origin);
                    d.dz = dz;
                }
            }
            None => view.boxing = Some(origin),
        }
    }
    if let Some(from) = view.boxing {
        let to = ui.input(|i| i.pointer.latest_pos()).unwrap_or(from);
        graph::draw_box(&painter, from, to);
        if resp.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
            view.boxing = None;
            let b = egui::Rect::from_two_pos(from, to);
            let found: Vec<usize> = (0..nodes.len()).filter(|&i| b.contains(nodes[i])).collect();
            let sel = &mut c.editor.selection;
            if sel.item != Some(Item::Road(r)) {
                sel.select(Item::Road(r));
            }
            graph::pick(&mut sel.nodes, &found, Pick::of(modifiers));
        }
    }
    if resp.double_clicked()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        match nearest(pos) {
            Some(_) => {}
            None if on_line(pos) => add_at = Some((plot.x_at(pos.x), plot.y_at(pos.y))),
            None => view.fit(&smp, road.nodes.iter().map(|n| n.pos.z)),
        }
    } else if resp.clicked()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let sel = &mut c.editor.selection;
        match nearest(pos) {
            Some(i) if modifiers.shift => sel.toggle_node(Item::Road(r), i),
            Some(i) => sel.select_node(Item::Road(r), i),
            None if modifiers.command && on_line(pos) => {
                add_at = Some((plot.x_at(pos.x), plot.y_at(pos.y)));
            }
            None if !modifiers.shift => sel.nodes.clear(),
            None => {}
        }
    }

    // Keys over the view.
    let none = egui::Modifiers::NONE;
    if key(egui::Key::A, none) {
        crate::viewport::select_all(c.editor, true);
    } else if key(egui::Key::A, egui::Modifiers::ALT) {
        crate::viewport::select_none(c.editor, true);
    } else if key(egui::Key::X, none) || key(egui::Key::Delete, none) {
        edit::delete_nodes(c.editor);
    } else if key(egui::Key::G, none)
        && !busy
        && let Some(i) = c.editor.selection.node().filter(|_| !selected.is_empty())
    {
        grab(view, c.editor, i, true);
    }

    // A node on the road at this distance, at the height asked for.
    if let Some((s, z)) = add_at {
        let s = s.clamp(0.0, smp.length);
        edit::insert_road_node(c.editor, r, smp.u_at(s), Some(z));
    }
}

/// Green on the flat, through yellow to red on steep grades either way.
fn grade_color(grade: f64) -> egui::Color32 {
    let t = (grade.abs() / 12.0).clamp(0.0, 1.0) as f32;
    let g = egui::Color32::from_rgb(120, 220, 120);
    let r = egui::Color32::from_rgb(255, 110, 90);
    g.lerp_to_gamma(r, t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::sim::Sim;
    use std::cell::RefCell;

    #[test]
    fn nodes_are_picked_raised_together_boxed_deleted_and_added() {
        let dir = std::env::temp_dir().join(format!(
            "open-racing-editor-elevation-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = Editor::open(dir.clone()).unwrap();
        e.selection.select(Item::Road(0));
        let view = RefCell::new(ProfileView::default());
        let mut draw = |ui: &mut egui::Ui, c: &mut Ctx| profile(ui, c, &mut view.borrow_mut());
        let at = |e: &Editor, i: usize| {
            let road = &e.project.roads[0];
            let smp = Sampled::new(road, road.resolution.max(1.0));
            let plot = view.borrow().plot.expect("drawn");
            plot.pos(smp.s_at(i as f64), road.nodes[i].pos.z)
        };
        let z =
            |e: &Editor| -> Vec<f64> { e.project.roads[0].nodes.iter().map(|n| n.pos.z).collect() };
        let mut sim = Sim::default();
        sim.frame(&mut e, vec![], &mut draw);

        // A click picks a node, Shift + click another; dragging one raises both.
        let primary = egui::PointerButton::Primary;
        let (n2, n3) = (at(&e, 2), at(&e, 3));
        sim.click(&mut e, n2, primary, &mut draw);
        sim.modifiers = egui::Modifiers::SHIFT;
        sim.click(&mut e, n3, primary, &mut draw);
        sim.modifiers = egui::Modifiers::NONE;
        assert_eq!(e.selection.nodes, vec![2, 3]);
        let steps = e.history().0.len();
        let from = at(&e, 3);
        sim.drag(&mut e, from, from - egui::vec2(0.0, 30.0), &mut draw);
        let (_, up) = view
            .borrow()
            .plot
            .expect("drawn")
            .delta(egui::vec2(0.0, -30.0));
        let after = z(&e);
        assert!(
            (after[3] - up).abs() < 0.02 * up && after[2] == after[3],
            "{after:?}, {up}"
        );
        assert_eq!(after[4], 0.0);
        assert_eq!(e.history().0.len(), steps + 1, "one step");

        // A box over nodes 5 to 7 selects them, and X deletes them.
        let plot = view.borrow().plot.expect("drawn");
        let (a, b) = (at(&e, 5), at(&e, 7));
        sim.drag(
            &mut e,
            egui::pos2(a.x - 10.0, plot.rect.top() + 5.0),
            egui::pos2(b.x + 10.0, plot.rect.bottom() - 5.0),
            &mut draw,
        );
        assert_eq!(e.selection.nodes, vec![5, 6, 7]);
        sim.key(&mut e, egui::Key::X, &mut draw);
        assert_eq!(e.project.roads[0].nodes.len(), 7);

        // A double-click on the line between nodes 0 and 1 adds a node there.
        let (a, b) = (at(&e, 0), at(&e, 1));
        sim.double_click(&mut e, a.lerp(b, 0.5), &mut draw);
        assert_eq!(e.project.roads[0].nodes.len(), 8);
        assert_eq!(e.selection.nodes, vec![1]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
