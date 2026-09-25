//! Popups over the whole window, as Blender's: the operator search (F3), renaming (F2),
//! the handle types (V), the view pie (`) and the keyboard shortcuts.

use bevy::math::Vec2;
use bevy_egui::egui;
use open_racing_track_project::project::HandleMode;

use crate::commands::{self, Cmd, Ctx};
use crate::edit;
use crate::ui::Popup;
use crate::viewport::ViewDir;

pub fn show(ctx: &egui::Context, c: &mut Ctx) {
    shortcuts_window(ctx, c);
    let Some(popup) = c.shell.popup.take() else {
        return;
    };
    let (keep, chosen) = match popup {
        Popup::Search { at, text, selected } => search(ctx, c, at, text, selected),
        Popup::Rename { at, item, text } => rename(ctx, c, at, item, text),
        Popup::Handles { at } => handles(ctx, c, at),
        Popup::Pie { at } => pie(ctx, c, at),
    };
    c.shell.popup = keep;
    if let Some(cmd) = chosen {
        commands::run(cmd, c);
    }
}

fn pos(at: Vec2) -> egui::Pos2 {
    egui::pos2(at.x, at.y)
}

/// A popup's frame at a place, and whether a click landed outside it.
fn popup_area<R>(
    ctx: &egui::Context,
    id: &'static str,
    at: egui::Pos2,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> (R, bool) {
    let resp = egui::Area::new(id.into())
        .fixed_pos(at)
        .order(egui::Order::Foreground)
        .constrain(true)
        .show(ctx, |ui| egui::Frame::menu(ui.style()).show(ui, add).inner);
    let outside = ctx.input(|i| i.pointer.any_pressed()) && !resp.response.contains_pointer();
    (resp.inner, outside)
}

fn escape(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.key_pressed(egui::Key::Escape))
}

/// Whether every word typed is in the name.
fn matches(label: &str, text: &str) -> bool {
    let label = label.to_lowercase();
    text.split_whitespace()
        .all(|w| label.contains(&w.to_lowercase()))
}

fn search(
    ctx: &egui::Context,
    c: &mut Ctx,
    at: Vec2,
    mut text: String,
    mut selected: usize,
) -> (Option<Popup>, Option<Cmd>) {
    let found: Vec<Cmd> = Cmd::all()
        .into_iter()
        .filter(|cmd| cmd.enabled(c) && matches(&cmd.search_label(), &text))
        .take(16)
        .collect();
    let (down, up, enter) = ctx.input(|i| {
        (
            i.key_pressed(egui::Key::ArrowDown),
            i.key_pressed(egui::Key::ArrowUp),
            i.key_pressed(egui::Key::Enter),
        )
    });
    if down {
        selected += 1;
    }
    if up {
        selected = selected.saturating_sub(1);
    }
    selected = selected.min(found.len().saturating_sub(1));
    let mut chosen = None;
    let (_, outside) = popup_area(ctx, "search", pos(at) - egui::vec2(170.0, 14.0), |ui| {
        ui.set_width(340.0);
        let edit = ui.add(
            egui::TextEdit::singleline(&mut text)
                .hint_text("🔍 Search commands…")
                .desired_width(f32::INFINITY),
        );
        edit.request_focus();
        ui.separator();
        if found.is_empty() {
            ui.weak("Nothing matches");
        }
        for (i, cmd) in found.iter().enumerate() {
            let resp = ui.add(
                egui::Button::selectable(i == selected, cmd.search_label())
                    .shortcut_text(cmd.shortcut())
                    .min_size(egui::vec2(ui.available_width(), 0.0)),
            );
            if resp.hovered() && ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
                selected = i;
            }
            if resp.clicked() {
                chosen = Some(*cmd);
            }
        }
    });
    if enter {
        chosen = found.get(selected).copied();
    }
    if chosen.is_some() || outside || escape(ctx) {
        return (None, chosen);
    }
    (Some(Popup::Search { at, text, selected }), None)
}

fn rename(
    ctx: &egui::Context,
    c: &mut Ctx,
    at: Vec2,
    item: crate::state::Item,
    mut text: String,
) -> (Option<Popup>, Option<Cmd>) {
    let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
    let (_, outside) = popup_area(ctx, "rename", pos(at) - egui::vec2(110.0, 30.0), |ui| {
        ui.set_width(220.0);
        ui.strong(format!("Rename {}", edit::item_kind(item)));
        ui.add(egui::TextEdit::singleline(&mut text).desired_width(f32::INFINITY))
            .request_focus();
        ui.weak("Enter to rename · Esc to cancel");
    });
    if enter {
        edit::rename(c.editor, item, &text);
        return (None, None);
    }
    if outside || escape(ctx) {
        return (None, None);
    }
    (Some(Popup::Rename { at, item, text }), None)
}

fn handles(ctx: &egui::Context, c: &mut Ctx, at: Vec2) -> (Option<Popup>, Option<Cmd>) {
    let mut chosen = None;
    let (_, outside) = popup_area(ctx, "handles", pos(at) - egui::vec2(20.0, 12.0), |ui| {
        ui.set_min_width(180.0);
        ui.strong("Set Handle Type");
        ui.separator();
        for mode in [HandleMode::Auto, HandleMode::Aligned, HandleMode::Free] {
            if ui.button(commands::handle_label(mode)).clicked() {
                chosen = Some(Cmd::Handles(mode));
            }
        }
        ui.separator();
        if ui
            .add(
                egui::Button::new("Toggle Closed Loop").shortcut_text(Cmd::ToggleClosed.shortcut()),
            )
            .clicked()
        {
            chosen = Some(Cmd::ToggleClosed);
        }
    });
    let _ = c;
    if chosen.is_some() || outside || escape(ctx) {
        return (None, chosen);
    }
    (Some(Popup::Handles { at }), None)
}

/// The view pie's entries, anticlockwise from the right, as Blender's.
const PIE: [(Cmd, &str); 8] = [
    (Cmd::View(ViewDir::Right), "Right"),
    (Cmd::View(ViewDir::Back), "Back"),
    (Cmd::View(ViewDir::Top), "Top"),
    (Cmd::View(ViewDir::Front), "Front"),
    (Cmd::View(ViewDir::Left), "Left"),
    (Cmd::FrameSelected, "Frame Selected"),
    (Cmd::View(ViewDir::Bottom), "Bottom"),
    (Cmd::FrameAll, "Frame All"),
];

/// The pie's entry in the pointer's direction from its middle.
fn pie_sector(at: Vec2, pointer: egui::Pos2) -> Option<usize> {
    let d = egui::vec2(pointer.x - at.x, pointer.y - at.y);
    (d.length() > 24.0).then(|| {
        let angle = (-d.y).atan2(d.x).to_degrees().rem_euclid(360.0);
        ((angle / 45.0).round() as usize) % 8
    })
}

fn pie(ctx: &egui::Context, c: &mut Ctx, at: Vec2) -> (Option<Popup>, Option<Cmd>) {
    let pointer = ctx.pointer_latest_pos().unwrap_or(pos(at));
    let sector = pie_sector(at, pointer);
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, "pie".into()));
    let center = pos(at);
    painter.circle_stroke(
        center,
        16.0,
        egui::Stroke::new(5.0, egui::Color32::from_black_alpha(160)),
    );
    painter.circle_stroke(
        center,
        16.0,
        egui::Stroke::new(2.0, egui::Color32::from_gray(200)),
    );
    if let Some(s) = sector {
        let a = (s as f32 * 45.0).to_radians();
        let dir = egui::vec2(a.cos(), -a.sin());
        painter.line_segment(
            [center + dir * 10.0, center + dir * 22.0],
            egui::Stroke::new(4.0, egui::Color32::from_rgb(71, 114, 179)),
        );
    }
    let font = egui::FontId::proportional(14.0);
    for (i, (cmd, label)) in PIE.iter().enumerate() {
        let a = (i as f32 * 45.0).to_radians();
        let dir = egui::vec2(a.cos(), -a.sin());
        let at = center + dir * egui::vec2(130.0, 105.0);
        let galley = painter.layout_no_wrap(label.to_string(), font.clone(), egui::Color32::WHITE);
        let rect = egui::Rect::from_center_size(at, galley.size() + egui::vec2(20.0, 10.0));
        let fill = if sector == Some(i) {
            egui::Color32::from_rgb(71, 114, 179)
        } else {
            egui::Color32::from_gray(40)
        };
        painter.rect_filled(rect, 6.0, fill);
        painter.rect_stroke(
            rect,
            6.0,
            egui::Stroke::new(1.0, egui::Color32::from_gray(20)),
            egui::StrokeKind::Outside,
        );
        let color = if cmd.enabled(c) {
            egui::Color32::WHITE
        } else {
            egui::Color32::GRAY
        };
        painter.galley(rect.center() - galley.size() * 0.5, galley, color);
    }
    painter.text(
        center + egui::vec2(0.0, 150.0),
        egui::Align2::CENTER_CENTER,
        "View",
        egui::FontId::proportional(12.0),
        egui::Color32::from_gray(200),
    );
    let (click, cancel, released) = ctx.input(|i| {
        (
            i.pointer.primary_pressed(),
            i.pointer.secondary_pressed() || i.key_pressed(egui::Key::Escape),
            i.key_released(egui::Key::Backtick),
        )
    });
    // A click picks; letting go of the key picks what the pointer moved to.
    if cancel {
        return (None, None);
    }
    if click || (released && sector.is_some()) {
        return (None, sector.map(|s| PIE[s].0));
    }
    (Some(Popup::Pie { at }), None)
}

const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    (
        "View",
        &[
            ("Middle drag", "Orbit (also right drag, Alt+left drag)"),
            ("Shift+Middle drag", "Pan"),
            ("Wheel, Ctrl+Middle drag", "Zoom"),
            ("Numpad 1 / 3 / 7", "Front / Right / Top (Ctrl: opposite)"),
            ("Numpad 2 4 6 8", "Orbit in 15° steps"),
            ("Numpad + / -", "Zoom in / out"),
            ("Numpad 5", "Perspective / Orthographic"),
            ("Numpad . or F", "Frame selected"),
            ("Home", "Frame all"),
            ("`", "View pie"),
            ("T / N", "Toolbar / Sidebar"),
            ("Ctrl Space", "Maximize the 3D view"),
        ],
    ),
    (
        "Select",
        &[
            ("Click", "Select a node, road, kerb or prop"),
            ("Shift+click", "Add a node to the selection"),
            ("Drag on empty space", "Box select"),
            ("A / Alt+A", "All / none of the line's nodes"),
            ("Ctrl I", "Invert"),
            ("Ctrl Numpad + / -", "Select more / less"),
        ],
    ),
    (
        "Transform",
        &[
            ("G / R / S", "Move / Rotate / Scale"),
            ("Alt S / Ctrl T", "Road width / bank at the nodes"),
            ("  X / Y (width)", "Left / right side only"),
            ("Drag a node, handle or marker", "Move it"),
            ("  X / Y / Z", "Hold to an axis"),
            ("  Shift / Ctrl", "Fine / Snap"),
            ("  type a number", "Exact value"),
            ("  Click, Enter / Right click, Esc", "Confirm / Cancel"),
        ],
    ),
    (
        "Edit",
        &[
            ("E", "Extrude the active node"),
            ("Ctrl+click", "Add a node at the pointer"),
            ("X / Delete", "Delete"),
            ("Shift D", "Duplicate a spline or prop"),
            ("Shift A", "Add: draw a road, kerb, wall, fence"),
            ("V", "Handle type"),
            ("Alt+click a handle", "Automatic handle"),
            ("Alt C", "Toggle closed loop"),
            ("F2", "Rename"),
            ("Right click", "Menu for what is under the pointer"),
        ],
    ),
    (
        "Anywhere",
        &[
            ("F3", "Search commands"),
            ("Ctrl Z / Ctrl Shift Z", "Undo / Redo"),
            ("Ctrl Q", "Quit"),
        ],
    ),
];

fn shortcuts_window(ctx: &egui::Context, c: &mut Ctx) {
    let mut open = c.shell.shortcuts;
    egui::Window::new("Keyboard Shortcuts")
        .open(&mut open)
        .default_width(520.0)
        .collapsible(false)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (title, keys) in SHORTCUTS {
                    ui.strong(*title);
                    egui::Grid::new(title)
                        .num_columns(2)
                        .striped(true)
                        .show(ui, |ui| {
                            for (key, what) in *keys {
                                ui.monospace(*key);
                                ui.label(*what);
                                ui.end_row();
                            }
                        });
                    ui.add_space(6.0);
                }
            });
        });
    c.shell.shortcuts = open;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pie_sectors_follow_the_pointer() {
        let at = Vec2::new(100.0, 100.0);
        assert_eq!(pie_sector(at, egui::pos2(105.0, 100.0)), None);
        assert_eq!(pie_sector(at, egui::pos2(200.0, 100.0)), Some(0));
        assert_eq!(pie_sector(at, egui::pos2(100.0, 0.0)), Some(2));
        assert_eq!(pie_sector(at, egui::pos2(0.0, 100.0)), Some(4));
        assert_eq!(pie_sector(at, egui::pos2(100.0, 200.0)), Some(6));
        assert_eq!(pie_sector(at, egui::pos2(0.0, 0.0)), Some(3));
    }

    #[test]
    fn search_matches_every_word() {
        assert!(matches("View › View Top", "view top"));
        assert!(matches("Add › Kerb", "kerb"));
        assert!(!matches("Add › Kerb", "wall"));
        assert!(matches("anything", ""));
    }
}
