//! The 3D view's header with its menus, the menu a right click opens for what is under
//! the pointer (Shift + A: the add menu), and the box being dragged out to select.

use bevy_egui::egui;
use open_racing_track_project::ops::Op;
use open_racing_track_project::project::HandleMode;

use crate::commands::{self, Cmd, Ctx, entry};
use crate::edit;
use crate::presets;
use crate::sidebar::overlay_checks;
use crate::state::{Item, item_line};
use crate::viewport::{Hit, Marker, Menu, Part, RangeEnd, ToolKind, ViewDir, add_node_at};

/// The strip above the 3D view: its menus, what a transform is doing, snapping, the
/// projection and the overlays.
pub fn header(root: &mut egui::Ui, c: &mut Ctx) {
    egui::Panel::top("view header").show(root, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            let mode = if c.tool.edit {
                "Edit Mode"
            } else {
                "Object Mode"
            };
            let mut edit = c.tool.edit;
            egui::ComboBox::from_id_salt("mode")
                .selected_text(mode)
                .width(100.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut edit, false, "Object Mode");
                    ui.add_enabled_ui(c.editor.line().is_some(), |ui| {
                        ui.selectable_value(&mut edit, true, "Edit Mode");
                    });
                })
                .response
                .on_hover_text(
                    "Object mode picks whole roads, kerbs and props; edit mode their nodes (Tab)",
                );
            if edit != c.tool.edit {
                crate::viewport::toggle_edit(c.editor, c.tool);
            }
            let tool = c.tool.active;
            egui::ComboBox::from_id_salt("active tool")
                .selected_text(tool.label())
                .width(96.0)
                .show_ui(ui, |ui| {
                    for t in ToolKind::ALL {
                        ui.selectable_value(&mut c.tool.active, t, t.label());
                    }
                })
                .response
                .on_hover_text("The tool a click or drag in the view uses (toolbar: T)");
            ui.separator();
            ui.menu_button("View", |ui| view_menu(ui, c));
            ui.menu_button("Select", |ui| {
                for cmd in [
                    Cmd::SelectAll,
                    Cmd::SelectNone,
                    Cmd::SelectInvert,
                    Cmd::SelectMore,
                    Cmd::SelectLess,
                ] {
                    entry(ui, c, cmd);
                }
                ui.separator();
                ui.menu_button("Select Similar", |ui| {
                    for by in [
                        crate::commands::Similar::Kind,
                        crate::commands::Similar::Type,
                        crate::commands::Similar::Material,
                    ] {
                        entry(ui, c, Cmd::SelectSimilar(by));
                    }
                });
            });
            ui.menu_button("Add", |ui| add_menu(ui, c));
            match c.editor.selection.item {
                Some(item @ (Item::Road(_) | Item::Spline(_))) => {
                    if c.tool.edit {
                        ui.menu_button("Node", |ui| node_menu(ui, c));
                    }
                    ui.menu_button(edit::item_kind(item), |ui| object_menu(ui, c, item));
                }
                Some(item @ Item::Prop(_)) => {
                    ui.menu_button("Prop", |ui| object_menu(ui, c, item));
                }
                None => {}
            }
            if !c.tool.hint.is_empty() {
                ui.separator();
                ui.colored_label(egui::Color32::from_rgb(255, 215, 30), &c.tool.hint);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button("Overlays ⏷", |ui| overlay_checks(ui, c));
                let projection = if c.orbit.ortho { "Ortho" } else { "Persp" };
                if ui
                    .selectable_label(c.orbit.ortho, projection)
                    .on_hover_text("Perspective/Orthographic (Numpad 5)")
                    .clicked()
                {
                    commands::run(Cmd::ToggleOrtho, c);
                }
                ui.menu_button("⏷", |ui| {
                    ui.set_min_width(230.0);
                    crate::sidebar::proportional_ui(ui, c);
                })
                .response
                .on_hover_text("Proportional editing: reach and falloff");
                ui.toggle_value(&mut c.tool.proportional.on, "◎")
                    .on_hover_text("Proportional editing (O): nodes near those moved follow");
                ui.menu_button("⏷", |ui| {
                    ui.set_min_width(230.0);
                    crate::sidebar::snapping_ui(ui, c);
                })
                .response
                .on_hover_text("Snapping: steps, and what dragged nodes catch on");
                let s = c.tool.snapping;
                ui.toggle_value(&mut c.tool.snap, "Snap")
                    .on_hover_text(format!(
                        "Step to {} m, {}° and ×{} (Ctrl while moving does the opposite)",
                        s.grid, s.angle, s.factor
                    ));
            });
        });
    });
}

fn view_menu(ui: &mut egui::Ui, c: &mut Ctx) {
    entry(ui, c, Cmd::ToggleToolbar);
    entry(ui, c, Cmd::ToggleSidebar);
    entry(ui, c, Cmd::ToggleMaximize);
    ui.separator();
    entry(ui, c, Cmd::FrameSelected);
    entry(ui, c, Cmd::FrameAll);
    entry(ui, c, Cmd::LocalView);
    ui.separator();
    entry(ui, c, Cmd::Hide);
    entry(ui, c, Cmd::HideOthers);
    entry(ui, c, Cmd::Reveal);
    ui.separator();
    ui.menu_button("Viewpoint", |ui| {
        for v in ViewDir::ALL {
            entry(ui, c, Cmd::View(v));
        }
    });
    entry(ui, c, Cmd::ToggleOrtho);
    entry(ui, c, Cmd::Walk);
    entry(ui, c, Cmd::Replay);
    entry(ui, c, Cmd::ViewPie);
    ui.separator();
    entry(ui, c, Cmd::Search);
    entry(ui, c, Cmd::Shortcuts);
}

pub fn add_menu(ui: &mut egui::Ui, c: &mut Ctx) {
    ui.set_min_width(170.0);
    entry(ui, c, Cmd::DrawRoad);
    ui.separator();
    let list = presets::list(&c.editor.project);
    ui.weak("Kerbs & run-off");
    for (i, p) in list.iter().enumerate() {
        if p.is_band() {
            entry(ui, c, Cmd::DrawSpline(i));
        }
    }
    ui.weak("Walls & fences");
    for (i, p) in list.iter().enumerate() {
        if !p.is_band() {
            entry(ui, c, Cmd::DrawSpline(i));
        }
    }
    ui.separator();
    entry(ui, c, Cmd::PlaceProp);
}

fn node_menu(ui: &mut egui::Ui, c: &mut Ctx) {
    ui.set_min_width(200.0);
    for cmd in [Cmd::Grab, Cmd::Rotate, Cmd::Scale] {
        entry(ui, c, cmd);
    }
    if c.editor.selection.road().is_some() {
        ui.separator();
        entry(ui, c, Cmd::Width);
        entry(ui, c, Cmd::Tilt);
    }
    ui.separator();
    for cmd in [Cmd::Extrude, Cmd::Subdivide, Cmd::Delete] {
        entry(ui, c, cmd);
    }
    ui.separator();
    for cmd in [
        Cmd::SmoothShape,
        Cmd::SmoothHeights,
        Cmd::Flatten,
        Cmd::EvenGrade,
    ] {
        entry(ui, c, cmd);
    }
    ui.separator();
    for cmd in [
        Cmd::Split,
        Cmd::Reverse,
        Cmd::Mirror(true),
        Cmd::Mirror(false),
    ] {
        entry(ui, c, cmd);
    }
    ui.separator();
    ui.menu_button("Handle Type", |ui| {
        for m in [HandleMode::Auto, HandleMode::Aligned, HandleMode::Free] {
            commands::button_as(ui, c, Cmd::Handles(m), commands::handle_label(m));
        }
    });
    entry(ui, c, Cmd::ToggleClosed);
}

fn object_menu(ui: &mut egui::Ui, c: &mut Ctx, item: Item) {
    ui.set_min_width(200.0);
    entry(ui, c, Cmd::Rename);
    entry(ui, c, Cmd::Duplicate);
    entry(ui, c, Cmd::Copy);
    if !matches!(item, Item::Prop(_)) {
        entry(ui, c, Cmd::Join);
        entry(ui, c, Cmd::Reverse);
    }
    entry(ui, c, Cmd::Mirror(true));
    entry(ui, c, Cmd::Mirror(false));
    if matches!(item, Item::Road(_)) {
        entry(ui, c, Cmd::SetMain);
        ui.separator();
        entry(ui, c, Cmd::Width);
        entry(ui, c, Cmd::Tilt);
        ui.separator();
    }
    entry(ui, c, Cmd::FrameSelected);
    ui.separator();
    if ui
        .button(format!("Delete {}", edit::item_kind(item)))
        .clicked()
    {
        c.editor.selection.nodes.clear();
        crate::viewport::delete(c.editor);
        ui.close();
    }
}

/// The box being dragged out and the open menu, in window coordinates.
pub fn overlay(ctx: &egui::Context, c: &mut Ctx) {
    if let Some((a, b)) = c.tool.boxing {
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            "box select".into(),
        ));
        let to = |v: bevy::math::Vec2| egui::pos2(v.x, v.y);
        let r = egui::Rect::from_two_pos(to(a), to(b));
        painter.rect_filled(r, 0.0, egui::Color32::from_white_alpha(16));
        painter.rect_stroke(
            r,
            0.0,
            egui::Stroke::new(1.0, egui::Color32::WHITE),
            egui::StrokeKind::Inside,
        );
    }

    let Some(menu) = c.tool.menu.take() else {
        return;
    };
    let mut close = false;
    let resp = egui::Area::new("view menu".into())
        .fixed_pos(egui::pos2(menu.at.x, menu.at.y))
        .order(egui::Order::Foreground)
        .constrain(true)
        .show(ctx, |ui| {
            egui::Frame::menu(ui.style()).show(ui, |ui| {
                ui.set_min_width(200.0);
                close = menu_items(ui, c, &menu);
            })
        });
    let elsewhere = ctx.input(|i| i.pointer.any_pressed())
        && !resp.response.contains_pointer()
        && !ctx.is_being_dragged(resp.response.id);
    let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    // Opening a submenu keeps the menu; a click outside everything closes it.
    let in_submenu = ctx.is_pointer_over_egui() && !resp.response.contains_pointer();
    if !(close || escape || (elsewhere && !in_submenu)) && c.tool.draw.is_none() {
        c.tool.menu = Some(menu);
    }
}

/// A button in the right-click menu, with a shortcut beside it.
fn item(ui: &mut egui::Ui, label: &str, shortcut: &str) -> bool {
    ui.add(egui::Button::new(label).shortcut_text(shortcut))
        .clicked()
}

/// A command in the right-click menu.
fn command(ui: &mut egui::Ui, c: &mut Ctx, cmd: Cmd) -> bool {
    commands::button(ui, c, cmd)
}

/// The right-click menu for what was under the pointer; true once an entry is used.
fn menu_items(ui: &mut egui::Ui, c: &mut Ctx, menu: &Menu) -> bool {
    if menu.add_only {
        ui.strong("Add");
        ui.separator();
        return add_items(ui, c);
    }
    let mut used = false;
    match menu.hit {
        Some(Hit::Node(it, n)) => {
            let name = item_line(&c.editor.project, it)
                .map_or("", |l| l.0)
                .to_string();
            if !(c.editor.selection.item == Some(it) && c.editor.selection.nodes.contains(&n)) {
                c.editor.selection.select_node(it, n);
            }
            ui.strong(format!("Node {n} of {name}"));
            ui.separator();
            for cmd in [Cmd::Grab, Cmd::Extrude, Cmd::Subdivide] {
                used |= command(ui, c, cmd);
            }
            if matches!(it, Item::Road(_)) {
                used |= command(ui, c, Cmd::Width);
                used |= command(ui, c, Cmd::Tilt);
            }
            ui.menu_button("Handle Type", |ui| {
                for m in [HandleMode::Auto, HandleMode::Aligned, HandleMode::Free] {
                    used |= commands::button_as(ui, c, Cmd::Handles(m), commands::handle_label(m));
                }
            });
            used |= command(ui, c, Cmd::SelectAll);
            ui.separator();
            used |= commands::button_as(ui, c, Cmd::Delete, "Delete Nodes");
        }
        Some(Hit::Handle(it, n, _)) => {
            ui.strong(format!("Handle of node {n}"));
            ui.separator();
            if item(ui, "Automatic handle", "Alt+click") {
                used = true;
                edit::auto_handles(c.editor, it, n);
            }
        }
        Some(Hit::Marker(Marker::Sector(i))) => {
            ui.strong(format!("Sector {}", i + 2));
            ui.separator();
            if item(ui, "Remove sector", "") {
                used = true;
                let mut sectors = c.editor.project.markers.sectors.clone();
                if i < sectors.len() {
                    sectors.remove(i);
                    set_markers(c, None, Some(sectors));
                }
            }
        }
        Some(Hit::Marker(Marker::Start)) => {
            ui.strong("Start/finish line");
            ui.separator();
            ui.weak("Drag it along the road.");
        }
        Some(Hit::Body(it)) => {
            let name = edit::item_name(&c.editor.project, it)
                .unwrap_or_default()
                .to_string();
            if c.editor.selection.item != Some(it) {
                c.editor.selection.select(it);
            }
            ui.strong(format!("{} {name}", edit::item_kind(it)));
            ui.separator();
            if !matches!(it, Item::Prop(_)) && item(ui, "Insert node here", "Ctrl+click") {
                used = true;
                add_node_at(c.editor, c.built, menu.world);
            }
            if let (Item::Road(r), Some(at)) = (it, menu.world)
                && name == c.editor.project.main_road
                && let Some(u) = c.built.roads.get(r).map(|s| s.frames[s.nearest(at)].u)
            {
                if item(ui, "Start/finish line here", "") {
                    used = true;
                    set_markers(c, Some(u), None);
                }
                if item(ui, "Sector boundary here", "") {
                    used = true;
                    let mut sectors = c.editor.project.markers.sectors.clone();
                    sectors.push(u);
                    set_markers(c, None, Some(sectors));
                }
            }
            for cmd in [Cmd::Rename, Cmd::Duplicate] {
                used |= command(ui, c, cmd);
            }
            if matches!(it, Item::Road(_)) {
                used |= command(ui, c, Cmd::SetMain);
            }
            ui.separator();
            if item(ui, &format!("Delete {}", edit::item_kind(it)), "X") {
                used = true;
                c.editor.selection.select(it);
                crate::viewport::delete(c.editor);
            }
        }
        Some(Hit::Range(end)) => {
            ui.strong("Stretch end");
            ui.separator();
            if item(ui, "Remove this stretch", "") {
                used = true;
                remove_stretch(c, end);
            }
        }
        Some(Hit::Landform(i, _)) => {
            let name = c
                .editor
                .project
                .terrain
                .landforms
                .get(i)
                .map_or(String::new(), |l| l.name.clone());
            ui.label(egui::RichText::new(format!("Landform {name}")).strong());
            if ui.button("Remove Landform").clicked() {
                let mut terrain = c.editor.project.terrain.clone();
                if i < terrain.landforms.len() {
                    terrain.landforms.remove(i);
                    c.editor.apply(vec![Op::SetTerrain { terrain }], None);
                }
                ui.close();
            }
        }
        Some(Hit::Gizmo(_) | Hit::Reach(_) | Hit::Edge(..)) | None => {}
    }
    if menu.hit.is_some() {
        ui.separator();
    }
    ui.menu_button("Add", |ui| used |= add_items(ui, c));
    if let Some(p) = menu.world
        && item(ui, "Look here", "")
    {
        used = true;
        c.orbit.focus = open_racing_track_render::to_bevy(p);
    }
    used |= command(ui, c, Cmd::FrameSelected);
    used |= command(ui, c, Cmd::ViewPie);
    used || c.tool.draw.is_some() || c.shell.popup.is_some()
}

/// The add menu's entries, without closing an egui menu (the view's menu is an area);
/// true once one is used.
fn add_items(ui: &mut egui::Ui, c: &mut Ctx) -> bool {
    ui.set_min_width(170.0);
    let mut used = command(ui, c, Cmd::DrawRoad);
    ui.separator();
    for i in 0..presets::list(&c.editor.project).len() {
        used |= command(ui, c, Cmd::DrawSpline(i));
    }
    ui.separator();
    used | command(ui, c, Cmd::PlaceProp)
}

fn set_markers(c: &mut Ctx, start: Option<f64>, sectors: Option<Vec<f64>>) {
    c.editor.apply(
        vec![Op::SetMarkers {
            start,
            sectors,
            grid: None,
        }],
        None,
    );
}

/// Removes a stretch of a road's strip or barrier, or the part itself with its last
/// stretch (no stretches would mean everywhere).
fn remove_stretch(c: &mut Ctx, end: RangeEnd) {
    let Some(road) = c.editor.project.roads.get(end.road) else {
        return;
    };
    let name = road.name.clone();
    // The menu may outlive what it was opened on (an undo, a reload).
    let ranges = match end.part {
        Part::Strip(side, i) => road.strips(side).get(i).map(|s| s.ranges.len()),
        Part::Barrier(i) => road.barriers.get(i).map(|b| b.ranges.len()),
    };
    if ranges.is_none_or(|n| end.range >= n) {
        return;
    }
    let op = match end.part {
        Part::Strip(side, i) => {
            let mut strip = road.strips(side)[i].clone();
            strip.ranges.remove(end.range);
            if strip.ranges.is_empty() {
                Op::RemoveStrip {
                    road: name,
                    side,
                    name: strip.name,
                }
            } else {
                Op::PutStrip {
                    road: name,
                    side,
                    strip,
                    at: None,
                }
            }
        }
        Part::Barrier(i) => {
            let mut barrier = road.barriers[i].clone();
            barrier.ranges.remove(end.range);
            if barrier.ranges.is_empty() {
                Op::RemoveBarrier {
                    road: name,
                    name: barrier.name,
                }
            } else {
                Op::PutBarrier {
                    road: name,
                    barrier,
                }
            }
        }
    };
    c.editor.apply(vec![op], None);
}
