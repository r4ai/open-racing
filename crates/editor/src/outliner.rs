//! The outliner, as Blender's: the track's roads, kerbs and walls, and props as a tree,
//! with each road's strips, lines and barriers under it. The track's settings are the
//! properties editor's first tabs.
//! Click selects, double-click (or F2) renames, right click opens a menu; what the
//! pointer is over lights up in the 3D view.

use bevy_egui::egui;
use open_racing_track_project::project::Side;

use crate::commands::{self, Cmd, Ctx};
use crate::edit;
use crate::state::Item;
use crate::ui::{Focus, PropTab};

#[derive(Default)]
pub struct State {
    filter: String,
    /// The item being renamed in place, and its new name so far.
    renaming: Option<(Item, String)>,
}

pub fn show(ui: &mut egui::Ui, c: &mut Ctx, state: &mut State) {
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.filter)
                .hint_text("🔍 Filter")
                .desired_width(ui.available_width() - 34.0),
        );
        ui.menu_button("➕", |ui| {
            ui.set_min_width(180.0);
            commands::entry(ui, c, Cmd::DrawRoad);
            ui.separator();
            for i in 0..crate::presets::list(&c.editor.project).len() {
                commands::entry(ui, c, Cmd::DrawSpline(i));
            }
            ui.separator();
            commands::entry(ui, c, Cmd::PlaceProp);
        })
        .response
        .on_hover_text("Add (Shift A in the view)");
    });
    ui.separator();
    let filter = state.filter.trim().to_lowercase();
    let shown = |name: &str| filter.is_empty() || name.to_lowercase().contains(&filter);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let p = &c.editor.project;
            let roads: Vec<(usize, String)> = p
                .roads
                .iter()
                .enumerate()
                .filter(|(_, r)| shown(&r.name))
                .map(|(i, r)| (i, r.name.clone()))
                .collect();
            // Splines and props in no collection; those in one are listed under it.
            let splines: Vec<(usize, String, bool)> = p
                .splines
                .iter()
                .enumerate()
                .filter(|(_, s)| shown(&s.name) && s.group.is_none())
                .map(|(i, s)| {
                    let wall = matches!(
                        s.shape,
                        open_racing_track_project::project::Shape::Wall { .. }
                    );
                    (i, s.name.clone(), wall)
                })
                .collect();
            let props: Vec<(usize, String)> = p
                .props
                .iter()
                .enumerate()
                .filter(|(_, x)| shown(&x.name) && x.group.is_none())
                .map(|(i, x)| (i, x.name.clone()))
                .collect();
            let groups = c.editor.groups();
            let main = p.main_road.clone();

            category(ui, "Roads", roads.len(), |ui| {
                for (i, name) in &roads {
                    let icon = if *name == main { "★" } else { "🚗" };
                    let id = ui.make_persistent_id(("outliner road", i));
                    egui::collapsing_header::CollapsingState::load_with_default_open(
                        ui.ctx(),
                        id,
                        false,
                    )
                    .show_header(ui, |ui| item_row(ui, c, state, Item::Road(*i), icon, name))
                    .body(|ui| road_parts(ui, c, *i));
                }
            });
            if !groups.is_empty() {
                category(ui, "Collections", groups.len(), |ui| {
                    for g in &groups {
                        collection(ui, c, state, g, &shown);
                    }
                });
            }
            category(ui, "Kerbs, walls & fences", splines.len(), |ui| {
                for (i, name, wall) in &splines {
                    ui.horizontal(|ui| {
                        ui.add_space(18.0);
                        item_row(
                            ui,
                            c,
                            state,
                            Item::Spline(*i),
                            if *wall { "🚧" } else { "〰" },
                            name,
                        );
                    });
                }
                if splines.is_empty() && filter.is_empty() {
                    ui.weak("   Shift A in the view to draw one");
                }
            });
            category(ui, "Props", props.len(), |ui| {
                for (i, name) in &props {
                    ui.horizontal(|ui| {
                        ui.add_space(18.0);
                        item_row(ui, c, state, Item::Prop(*i), "📦", name);
                    });
                }
                if props.is_empty() && filter.is_empty() {
                    ui.weak("   Place models from Assets");
                }
            });
            let scatters: Vec<(usize, String)> = c
                .editor
                .project
                .scatter
                .iter()
                .enumerate()
                .filter(|(_, s)| shown(&s.name))
                .map(|(i, s)| (i, s.name.clone()))
                .collect();
            category(ui, "Scatters", scatters.len(), |ui| {
                for (i, name) in &scatters {
                    ui.horizontal(|ui| scatter_row(ui, c, *i, name));
                }
                if scatters.is_empty() && filter.is_empty() {
                    ui.weak("   Woods, bushes, rocks: the Scatter tool");
                }
            });
        });
}

/// A scatter: its eye, and its name, which picks it to paint.
fn scatter_row(ui: &mut egui::Ui, c: &mut Ctx, i: usize, name: &str) {
    ui.add_space(18.0);
    let hidden = c.editor.shown.hidden_scatter.contains(name);
    let eye = if hidden { "◌" } else { "👁" };
    if ui
        .add(egui::Button::new(eye).frame(false))
        .on_hover_text("Show or hide it in the view")
        .clicked()
        && !c.editor.shown.hidden_scatter.remove(name)
    {
        c.editor.shown.hidden_scatter.insert(name.to_string());
    }
    let copies = c.built.scattered.get(i).copied().unwrap_or(0);
    let painting = c.tool.active == crate::viewport::ToolKind::Scatter
        && c.tool.brush.scatter.as_deref() == Some(name);
    if ui
        .selectable_label(painting, format!("🌲 {name}"))
        .on_hover_text(format!(
            "{copies} copies · click to paint it (Scatter tool)"
        ))
        .clicked()
    {
        c.tool.active = crate::viewport::ToolKind::Scatter;
        c.tool.brush.scatter = Some(name.to_string());
        c.shell.tab = crate::ui::PropTab::Scatter;
    }
}

/// A group of the tree, open by default.
fn category(ui: &mut egui::Ui, title: &str, count: usize, body: impl FnOnce(&mut egui::Ui)) {
    let id = ui.make_persistent_id(("outliner", title));
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
        .show_header(ui, |ui| {
            ui.label(egui::RichText::new(title).strong());
            ui.weak(count.to_string());
        })
        .body(body);
}

/// A road, spline or prop: select, rename in place, menu.
fn item_row(ui: &mut egui::Ui, c: &mut Ctx, state: &mut State, item: Item, icon: &str, name: &str) {
    if let Some((it, text)) = &mut state.renaming
        && *it == item
    {
        let resp = ui.add(egui::TextEdit::singleline(text).desired_width(160.0));
        resp.request_focus();
        let (enter, escape) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if escape {
            state.renaming = None;
        } else if enter || resp.lost_focus() {
            let to = text.clone();
            state.renaming = None;
            edit::rename(c.editor, item, &to);
        }
        return;
    }
    // Blender's eye and lock: hidden from the view, or shown but not picked there.
    let hidden = c.editor.is_hidden(item);
    let eye = egui::Button::new(if hidden { "◌" } else { "👁" }).frame(false);
    if ui
        .add(eye)
        .on_hover_text(if hidden {
            "Show (Alt H shows all)"
        } else {
            "Hide (H)"
        })
        .clicked()
    {
        c.editor.toggle_hidden(item);
    }
    let locked = c.editor.is_locked(item);
    let lock = egui::Button::new(if locked { "🔒" } else { "🔓" }).frame(false);
    if ui
        .add(lock)
        .on_hover_text(if locked {
            "Locked: not picked in the view. Click to unlock"
        } else {
            "Lock: not picked in the view (still selectable here)"
        })
        .clicked()
    {
        c.editor.toggle_locked(item);
    }
    let selected = c.editor.selection.item == Some(item);
    let text = if selected {
        egui::RichText::new(format!("{icon} {name}")).color(crate::theme::SELECTED_UI)
    } else if c.editor.selection.has(item) {
        egui::RichText::new(format!("{icon} {name}")).color(crate::theme::SELECTED_OTHER_UI)
    } else if hidden {
        egui::RichText::new(format!("{icon} {name}")).weak()
    } else {
        egui::RichText::new(format!("{icon} {name}"))
    };
    let resp = ui.selectable_label(selected, text);
    if resp.hovered() {
        c.tool.outliner_hover = Some(item);
    }
    if resp.clicked() {
        // Ctrl or Shift adds to the selection, as Blender's outliner.
        let m = ui.input(|i| i.modifiers);
        if m.ctrl || m.shift {
            c.editor.selection.toggle_item(item);
        } else if !selected || !c.editor.selection.others.is_empty() {
            c.editor.selection.select(item);
        }
        c.shell.tab = PropTab::Object;
    }
    if resp.double_clicked() {
        state.renaming = Some((item, name.to_string()));
    }
    resp.context_menu(|ui| {
        ui.set_min_width(200.0);
        if c.editor.selection.item != Some(item) {
            c.editor.selection.select(item);
        }
        ui.label(egui::RichText::new(format!("{} {name}", edit::item_kind(item))).strong());
        ui.separator();
        if ui
            .add(egui::Button::new("Rename").shortcut_text("Double-click"))
            .clicked()
        {
            state.renaming = Some((item, name.to_string()));
            ui.close();
        }
        commands::entry(ui, c, Cmd::FrameSelected);
        commands::entry(ui, c, Cmd::Duplicate);
        if matches!(item, Item::Road(_)) {
            commands::entry(ui, c, Cmd::SetMain);
        }
        if !matches!(item, Item::Prop(_)) {
            commands::entry(ui, c, Cmd::ToggleClosed);
        }
        ui.separator();
        if ui
            .button(format!("Delete {}", edit::item_kind(item)))
            .clicked()
        {
            c.editor.selection.nodes.clear();
            crate::viewport::delete(c.editor);
            ui.close();
        }
    });
}

/// A road's strips, lines and barriers; a click opens them in the properties.
fn road_parts(ui: &mut egui::Ui, c: &mut Ctx, r: usize) {
    let Some(road) = c.editor.project.roads.get(r) else {
        return;
    };
    let mut parts: Vec<(Option<Focus>, PropTab, String)> = Vec::new();
    // What was laid round corners is one entry: on a long track there are dozens.
    let cornered = road
        .left
        .iter()
        .chain(&road.right)
        .filter(|s| crate::corners::held(c.built, r, &s.corner))
        .count()
        + road
            .barriers
            .iter()
            .filter(|b| crate::corners::held(c.built, r, &b.corner))
            .count();
    if cornered > 0 {
        parts.push((
            None,
            PropTab::Corners,
            format!("↩ Corner kerbs, gravel & walls ({cornered})"),
        ));
    }
    for side in [Side::Left, Side::Right] {
        let s = match side {
            Side::Left => "L",
            Side::Right => "R",
        };
        for (i, strip) in road.strips(side).iter().enumerate() {
            if crate::corners::held(c.built, r, &strip.corner) {
                continue;
            }
            parts.push((
                Some(Focus::Strip(side, i)),
                PropTab::Strips,
                format!("☰ {} ({s})", strip.name),
            ));
        }
    }
    for (i, line) in road.lines.iter().enumerate() {
        parts.push((
            Some(Focus::Line(i)),
            PropTab::Lines,
            format!("✏ {}", line.name),
        ));
    }
    for (i, b) in road.barriers.iter().enumerate() {
        if crate::corners::held(c.built, r, &b.corner) {
            continue;
        }
        parts.push((
            Some(Focus::Barrier(i)),
            PropTab::Barriers,
            format!("🚧 {}", b.name),
        ));
    }
    for (i, w) in road.rows.iter().enumerate() {
        parts.push((Some(Focus::Row(i)), PropTab::Rows, format!("🌲 {}", w.name)));
    }
    if parts.is_empty() {
        ui.weak("no strips, lines or barriers");
    }
    for (focus, tab, label) in parts {
        let resp = ui.selectable_label(false, egui::RichText::new(label).weak());
        if resp.hovered() {
            c.tool.outliner_hover = Some(Item::Road(r));
        }
        if resp.clicked() {
            if c.editor.selection.item != Some(Item::Road(r)) {
                c.editor.selection.select(Item::Road(r));
            }
            c.shell.tab = tab;
            c.shell.focus = focus;
        }
    }
}

/// A collection: its eye and lock, a click selecting all it holds, and what it holds.
fn collection(
    ui: &mut egui::Ui,
    c: &mut Ctx,
    state: &mut State,
    group: &str,
    shown: &dyn Fn(&str) -> bool,
) {
    let p = &c.editor.project;
    // Everything it holds; the filter only narrows what is listed.
    let all: Vec<(Item, String, &'static str)> = p
        .splines
        .iter()
        .enumerate()
        .filter(|(_, s)| s.group.as_deref() == Some(group))
        .map(|(i, s)| {
            let icon = match s.shape {
                open_racing_track_project::project::Shape::Wall { .. } => "🚧",
                _ => "〰",
            };
            (Item::Spline(i), s.name.clone(), icon)
        })
        .chain(
            p.props
                .iter()
                .enumerate()
                .filter(|(_, x)| x.group.as_deref() == Some(group))
                .map(|(i, x)| (Item::Prop(i), x.name.clone(), "📦")),
        )
        .collect();
    let items: Vec<Item> = all.iter().map(|(item, ..)| *item).collect();
    let members: Vec<&(Item, String, &'static str)> =
        all.iter().filter(|(_, name, _)| shown(name)).collect();
    let id = ui.make_persistent_id(("outliner collection", group));
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
        .show_header(ui, |ui| {
            let sh = &mut c.editor.shown;
            let hidden = sh.hidden_groups.contains(group);
            if ui
                .add(egui::Button::new(if hidden { "◌" } else { "👁" }).frame(false))
                .on_hover_text("Hide or show the whole collection")
                .clicked()
                && !sh.hidden_groups.remove(group)
            {
                sh.hidden_groups.insert(group.to_string());
            }
            let locked = sh.locked_groups.contains(group);
            if ui
                .add(egui::Button::new(if locked { "🔒" } else { "🔓" }).frame(false))
                .on_hover_text("Keep the whole collection from being picked in the view")
                .clicked()
                && !sh.locked_groups.remove(group)
            {
                sh.locked_groups.insert(group.to_string());
            }
            let resp = ui
                .selectable_label(false, egui::RichText::new(format!("🗀 {group}")).strong())
                .on_hover_text("Click: select all it holds · right click: menu");
            if resp.clicked() {
                c.editor.selection.set_items(items.iter().copied());
                c.tool.edit = false;
            }
            ui.weak(items.len().to_string());
            resp.context_menu(|ui| {
                if ui.button("Select all").clicked() {
                    c.editor.selection.set_items(items.iter().copied());
                    c.tool.edit = false;
                    ui.close();
                }
                if ui
                    .button("Remove collection (keep what it holds)")
                    .clicked()
                {
                    c.editor.selection.set_items(items.iter().copied());
                    c.editor.set_group(None);
                    ui.close();
                }
            });
        })
        .body(|ui| {
            for (item, name, icon) in &members {
                ui.horizontal(|ui| {
                    ui.add_space(18.0);
                    item_row(ui, c, state, *item, icon, name);
                });
            }
        });
}
