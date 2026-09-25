//! The 3D view's header, the menus opened in it (right click, Shift + A) and the box
//! being dragged out to select.

use bevy_egui::egui;
use open_racing_track_project::ops::Op;

use crate::presets::PRESETS;
use crate::preview::Built;
use crate::state::{Editor, Item, item_line};
use crate::viewport::{
    Draw, DrawKind, Hit, Marker, Menu, Orbit, Tool, add_node_at, delete, frame_all,
    frame_selection, set_view,
};

const HELP: &str = "\
Select: click · Shift+click: add to it · drag over empty space: box · A: all · Alt+A: none
Change: G move · R rotate · S scale · drag a node, handle or marker to move it
  while changing: X/Y/Z axis · Shift fine · Ctrl snap · type a value · click/Enter done · right click/Esc undo
Build: E extrude a node · Ctrl+click: add a node · X/Delete: delete · Shift+D: duplicate · Shift+A: add
  Alt+click a handle: automatic · right click: menu
View: middle drag orbit · Shift+middle pan · wheel/Ctrl+middle zoom · right or Alt+left drag orbit too
  numpad 1/3/7 front/right/top (Ctrl: opposite) · numpad 5 ortho · F or numpad . frame selection · Home all · N sidebar";

/// The strip above the 3D view: the add and view menus, snapping, and what the view is
/// doing.
pub fn header(root: &mut egui::Ui, editor: &mut Editor, tool: &mut Tool, orbit: &mut Orbit) {
    egui::Panel::top("view header").show(root, |ui| {
        ui.horizontal(|ui| {
            ui.menu_button("Add", |ui| add_items(ui, tool));
            ui.menu_button("View", |ui| view_items(ui, editor, orbit));
            ui.toggle_value(&mut tool.snap, "Snap").on_hover_text(
                "Snap to whole metres, 5° and tenths (Ctrl while moving inverts it)",
            );
            ui.toggle_value(&mut orbit.ortho, "Ortho")
                .on_hover_text("Orthographic view (numpad 5)");
            ui.separator();
            if tool.hint.is_empty() {
                ui.weak("Shortcuts (hover)").on_hover_text(HELP);
            } else {
                ui.colored_label(egui::Color32::from_rgb(255, 215, 30), &tool.hint);
            }
        });
    });
}

fn add_items(ui: &mut egui::Ui, tool: &mut Tool) {
    let mut start = |kind| {
        tool.draw = Some(Draw {
            kind,
            points: Vec::new(),
        });
        tool.menu = None;
    };
    if ui.button("Road").clicked() {
        start(DrawKind::Road);
        ui.close();
    }
    ui.separator();
    for (i, p) in PRESETS.iter().enumerate() {
        if ui.button(p.label).clicked() {
            start(DrawKind::Spline(i));
            ui.close();
        }
    }
}

fn view_items(ui: &mut egui::Ui, editor: &Editor, orbit: &mut Orbit) {
    use std::f32::consts::{FRAC_PI_2, PI};
    for (label, yaw, pitch) in [
        ("Top (numpad 7)", FRAC_PI_2, 1.5695),
        ("Front (numpad 1)", FRAC_PI_2, 0.0),
        ("Right (numpad 3)", 0.0, 0.0),
        ("Back (Ctrl+numpad 1)", -FRAC_PI_2, 0.0),
        ("Left (Ctrl+numpad 3)", PI, 0.0),
    ] {
        if ui.button(label).clicked() {
            set_view(orbit, yaw, pitch);
            ui.close();
        }
    }
    ui.separator();
    if ui.button("Frame selected (F)").clicked() {
        frame_selection(editor, orbit);
        ui.close();
    }
    if ui.button("Frame all (Home)").clicked() {
        frame_all(editor, orbit);
        ui.close();
    }
}

/// The box being dragged out and the open menu, over the 3D view whose top left corner
/// is `origin`.
pub fn overlay(
    ctx: &egui::Context,
    editor: &mut Editor,
    tool: &mut Tool,
    built: &Built,
    orbit: &mut Orbit,
    origin: egui::Pos2,
) {
    if let Some((a, b)) = tool.boxing {
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            "box select".into(),
        ));
        let to = |v: bevy::math::Vec2| origin + egui::vec2(v.x, v.y);
        let r = egui::Rect::from_two_pos(to(a), to(b));
        painter.rect_filled(r, 0.0, egui::Color32::from_white_alpha(16));
        painter.rect_stroke(
            r,
            0.0,
            egui::Stroke::new(1.0, egui::Color32::WHITE),
            egui::StrokeKind::Inside,
        );
    }

    let Some(menu) = tool.menu.take() else {
        return;
    };
    let mut close = false;
    let resp = egui::Area::new("view menu".into())
        .fixed_pos(egui::pos2(menu.at.x, menu.at.y))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::menu(ui.style()).show(ui, |ui| {
                ui.set_min_width(170.0);
                close = menu_items(ui, editor, tool, built, orbit, &menu);
            })
        });
    let elsewhere = ctx.input(|i| i.pointer.any_pressed())
        && !resp.response.contains_pointer()
        && !ctx.is_being_dragged(resp.response.id);
    let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    // Opening a submenu keeps the menu; a click outside everything closes it.
    let in_submenu = ctx.is_pointer_over_egui() && !resp.response.contains_pointer();
    if !(close || escape || (elsewhere && !in_submenu)) && tool.draw.is_none() {
        tool.menu = Some(menu);
    }
}

/// The right-click menu for what was under the pointer; true once an entry is used.
fn menu_items(
    ui: &mut egui::Ui,
    editor: &mut Editor,
    tool: &mut Tool,
    built: &Built,
    orbit: &mut Orbit,
    menu: &Menu,
) -> bool {
    if menu.add_only {
        ui.strong("Add");
        add_items(ui, tool);
        return tool.draw.is_some();
    }
    let mut used = false;
    let mut entry = |ui: &mut egui::Ui, label: &str| {
        let clicked = ui.button(label).clicked();
        used |= clicked;
        clicked
    };
    match menu.hit {
        Some(Hit::Node(item, n)) => {
            let name = item_line(&editor.project, item)
                .map_or("", |l| l.0)
                .to_string();
            ui.strong(format!("Node {n} of \"{name}\""));
            if entry(ui, "Delete node") {
                editor.selection.select_node(item, n);
                delete(editor);
            }
            if entry(ui, "Automatic handle") {
                editor.apply(
                    vec![Op::SetHandle {
                        line: name.clone(),
                        index: n,
                        handle: None,
                    }],
                    None,
                );
            }
            if entry(ui, "Select all its nodes (A)") {
                let count = item_line(&editor.project, item).map_or(0, |l| l.1.len());
                editor.selection.item = Some(item);
                editor.selection.nodes = (0..count).collect();
            }
        }
        Some(Hit::Handle(item, n, _)) => {
            let name = item_line(&editor.project, item)
                .map_or("", |l| l.0)
                .to_string();
            ui.strong(format!("Handle of node {n}"));
            if entry(ui, "Automatic handle") {
                editor.apply(
                    vec![Op::SetHandle {
                        line: name,
                        index: n,
                        handle: None,
                    }],
                    None,
                );
            }
        }
        Some(Hit::Marker(Marker::Sector(i))) => {
            ui.strong(format!("Sector {}", i + 2));
            if entry(ui, "Remove sector") {
                let mut sectors = editor.project.markers.sectors.clone();
                sectors.remove(i);
                set_markers(editor, None, Some(sectors));
            }
        }
        Some(Hit::Marker(Marker::Start)) => {
            ui.strong("Start/finish line");
            ui.label("Drag it along the road.");
        }
        Some(Hit::Body(item)) => {
            let name = item_line(&editor.project, item)
                .map_or("", |l| l.0)
                .to_string();
            ui.strong(&name);
            if entry(ui, "Insert node here") {
                editor.selection.select(item);
                add_node_at(editor, built, menu.world);
            }
            if let (Item::Road(r), Some(at)) = (item, menu.world)
                && name == editor.project.main_road
                && let Some(u) = built.roads.get(r).map(|s| s.frames[s.nearest(at)].u)
            {
                if entry(ui, "Start/finish line here") {
                    set_markers(editor, Some(u), None);
                }
                if entry(ui, "Sector boundary here") {
                    let mut sectors = editor.project.markers.sectors.clone();
                    sectors.push(u);
                    set_markers(editor, None, Some(sectors));
                }
            }
            if entry(ui, &format!("Delete \"{name}\"")) {
                editor.selection.select(item);
                delete(editor);
            }
        }
        None => {}
    }
    ui.separator();
    ui.menu_button("Add", |ui| add_items(ui, tool));
    if let Some(p) = menu.world
        && entry(ui, "Look here")
    {
        orbit.focus = open_racing_track_render::to_bevy(p);
    }
    if entry(ui, "Frame selected") {
        frame_selection(editor, orbit);
    }
    used || tool.draw.is_some()
}

fn set_markers(editor: &mut Editor, start: Option<f64>, sectors: Option<Vec<f64>>) {
    editor.apply(
        vec![Op::SetMarkers {
            start,
            sectors,
            grid: None,
        }],
        None,
    );
}
