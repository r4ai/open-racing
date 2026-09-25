//! What is drawn over the 3D view: the toolbar (T), the navigation gizmo and its
//! buttons, the view's name and what is selected, and labels on the track.

use bevy::math::Vec3;
use bevy_egui::egui;
use glam::DVec3;
use open_racing_track_project::model::Placement;
use open_racing_track_render::to_bevy;

use crate::commands::{self, Cmd, Ctx};
use crate::edit;
use crate::presets::PRESETS;
use crate::state::{Item, item_line};
use crate::viewport::{DrawKind, Orbit, ToolKind, View, ViewDir, look, orbit_by, shown_pos};

pub fn view(ctx: &egui::Context, r: egui::Rect, c: &mut Ctx, view: View) {
    labels(ctx, r, c, view);
    info(ctx, r, c);
    if c.shell.toolbar {
        toolbar(ctx, r, c);
    }
    navigation(ctx, r, c);
}

/// Text with a dark outline, readable over the sky and the grass.
fn outlined(
    painter: &egui::Painter,
    at: egui::Pos2,
    align: egui::Align2,
    text: &str,
    size: f32,
    color: egui::Color32,
) {
    let font = egui::FontId::proportional(size);
    for d in [
        egui::vec2(1.0, 1.0),
        egui::vec2(-1.0, 1.0),
        egui::vec2(0.0, -1.0),
    ] {
        painter.text(
            at + d,
            align,
            text,
            font.clone(),
            egui::Color32::from_black_alpha(170),
        );
    }
    painter.text(at, align, text, font, color);
}

/// The view's name and the selection, top left, as Blender's text info.
fn info(ctx: &egui::Context, r: egui::Rect, c: &Ctx) {
    let painter = ctx
        .layer_painter(egui::LayerId::new(
            egui::Order::Background,
            "view info".into(),
        ))
        .with_clip_rect(r);
    let x = r.min.x + if c.shell.toolbar { 56.0 } else { 12.0 };
    let projection = if c.orbit.ortho {
        "Orthographic"
    } else {
        "Perspective"
    };
    let name = match ViewDir::of(c.orbit) {
        Some(v) => format!("{} {projection}", v.label()),
        None => format!("User {projection}"),
    };
    outlined(
        &painter,
        egui::pos2(x, r.min.y + 8.0),
        egui::Align2::LEFT_TOP,
        &name,
        14.0,
        egui::Color32::WHITE,
    );
    let sel = &c.editor.selection;
    let what = match sel.item {
        Some(item) => {
            let name = edit::item_name(&c.editor.project, item).unwrap_or_default();
            let nodes = match (sel.nodes.len(), sel.node()) {
                (0, _) => String::new(),
                (1, Some(n)) => format!(" › node {n}"),
                (k, Some(n)) => format!(" › {k} nodes, active {n}"),
                _ => String::new(),
            };
            format!("({}) {name}{nodes}", edit::item_kind(item))
        }
        None => "Nothing selected".into(),
    };
    outlined(
        &painter,
        egui::pos2(x, r.min.y + 28.0),
        egui::Align2::LEFT_TOP,
        &what,
        13.0,
        egui::Color32::from_gray(225),
    );
    if let Some(d) = &c.tool.draw {
        let kind = match d.kind {
            DrawKind::Road => "road",
            DrawKind::Spline(i) => PRESETS[i].label,
        };
        let text = format!("Drawing a {kind}: {} points", d.points.len());
        outlined(
            &painter,
            egui::pos2(x, r.min.y + 46.0),
            egui::Align2::LEFT_TOP,
            &text,
            13.0,
            egui::Color32::from_rgb(255, 215, 30),
        );
    }
}

/// Names of the roads, splines and props, and numbers of the selected line's nodes.
fn labels(ctx: &egui::Context, r: egui::Rect, c: &Ctx, view: View) {
    let o = c.tool.overlays;
    if !o.names && !o.indices {
        return;
    }
    let painter = ctx
        .layer_painter(egui::LayerId::new(
            egui::Order::Background,
            "view labels".into(),
        ))
        .with_clip_rect(r);
    let p = &c.editor.project;
    let screen = |pos: DVec3| {
        view.screen(pos + DVec3::Z * 0.3)
            .map(|s| egui::pos2(s.x, s.y))
    };
    let sel = &c.editor.selection;
    if o.names {
        let lines = (0..p.roads.len())
            .map(Item::Road)
            .chain((0..p.splines.len()).map(Item::Spline));
        for item in lines {
            let Some((name, nodes, _)) = item_line(p, item) else {
                continue;
            };
            let Some(node) = nodes.get(nodes.len() / 2) else {
                continue;
            };
            let selected = sel.item == Some(item);
            if !o.lines && !selected {
                continue;
            }
            if let Some(at) = screen(shown_pos(c.editor, c.built, item, node.pos)) {
                let color = if selected {
                    egui::Color32::from_rgb(255, 170, 64)
                } else {
                    egui::Color32::from_gray(215)
                };
                outlined(
                    &painter,
                    at + egui::vec2(0.0, -14.0),
                    egui::Align2::CENTER_BOTTOM,
                    name,
                    12.0,
                    color,
                );
            }
        }
        if o.props {
            for prop in &p.props {
                let at = Placement::of(prop, c.built.ground.as_deref());
                if let Some(s) = screen(at.pos) {
                    outlined(
                        &painter,
                        s + egui::vec2(0.0, -16.0),
                        egui::Align2::CENTER_BOTTOM,
                        &prop.name,
                        11.0,
                        egui::Color32::from_rgb(190, 240, 150),
                    );
                }
            }
        }
    }
    if o.indices
        && let Some(item) = sel.item
        && let Some((_, nodes, _)) = item_line(p, item)
    {
        for (i, node) in nodes.iter().enumerate() {
            if let Some(at) = screen(shown_pos(c.editor, c.built, item, node.pos)) {
                let color = if sel.nodes.contains(&i) {
                    egui::Color32::from_rgb(255, 215, 30)
                } else {
                    egui::Color32::from_gray(235)
                };
                outlined(
                    &painter,
                    at + egui::vec2(9.0, -9.0),
                    egui::Align2::LEFT_BOTTOM,
                    &i.to_string(),
                    11.0,
                    color,
                );
            }
        }
    }
}

/// A toolbar icon: a glyph of egui's fonts, or one drawn here where the fonts have none.
#[derive(Clone, Copy)]
enum Icon {
    Glyph(&'static str),
    Move,
    Scale,
    Road,
}

/// Paints a drawn icon in a button's square.
fn paint_icon(painter: &egui::Painter, rect: egui::Rect, icon: Icon, color: egui::Color32) {
    let c = rect.center();
    let stroke = egui::Stroke::new(1.6, color);
    let arrow = |from: egui::Pos2, to: egui::Pos2| {
        painter.line_segment([from, to], stroke);
        let d = (to - from).normalized() * 4.0;
        let n = egui::vec2(-d.y, d.x);
        painter.line_segment([to, to - d + n], stroke);
        painter.line_segment([to, to - d - n], stroke);
    };
    match icon {
        Icon::Glyph(_) => {}
        Icon::Move => {
            for d in [
                egui::vec2(9.0, 0.0),
                egui::vec2(-9.0, 0.0),
                egui::vec2(0.0, 9.0),
                egui::vec2(0.0, -9.0),
            ] {
                arrow(c, c + d);
            }
        }
        Icon::Scale => {
            let r = egui::Rect::from_center_size(c + egui::vec2(-3.0, 3.0), egui::vec2(9.0, 9.0));
            painter.rect_stroke(r, 1.0, stroke, egui::StrokeKind::Middle);
            arrow(c + egui::vec2(-1.0, 1.0), c + egui::vec2(8.0, -8.0));
        }
        Icon::Road => {
            painter.line_segment(
                [c + egui::vec2(-9.0, 9.0), c + egui::vec2(-3.0, -9.0)],
                stroke,
            );
            painter.line_segment(
                [c + egui::vec2(9.0, 9.0), c + egui::vec2(3.0, -9.0)],
                stroke,
            );
            for (a, b) in [(9.0, 4.0), (1.0, -3.0), (-6.0, -9.0)] {
                painter.line_segment([c + egui::vec2(0.0, a), c + egui::vec2(0.0, b)], stroke);
            }
        }
    }
}

/// A square button with an icon.
fn tool_button(ui: &mut egui::Ui, selected: bool, icon: Icon, tip: &str) -> egui::Response {
    let text = match icon {
        Icon::Glyph(g) => egui::RichText::new(g).size(18.0),
        _ => egui::RichText::new(""),
    };
    let resp = ui
        .add_sized([32.0, 32.0], egui::Button::selectable(selected, text))
        .on_hover_text(tip);
    let color = if selected {
        ui.visuals().selection.stroke.color
    } else {
        ui.visuals().widgets.style(&resp).fg_stroke.color
    };
    paint_icon(ui.painter(), resp.rect, icon, color);
    resp
}

/// The toolbar, as Blender's T panel: the tools, then what to draw.
fn toolbar(ctx: &egui::Context, r: egui::Rect, c: &mut Ctx) {
    egui::Area::new("toolbar".into())
        .fixed_pos(r.min + egui::vec2(8.0, 8.0))
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(40, 40, 40, 225))
                .corner_radius(6.0)
                .inner_margin(3.0)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(2.0, 2.0);
                    for (t, icon, tip) in [
                        (ToolKind::Select, Icon::Glyph("⬉"), "Select Box\nClick: select · Drag: box select · Shift: extend"),
                        (ToolKind::Move, Icon::Move, "Move\nDrag the gizmo's arrows (G moves at any time)"),
                        (ToolKind::Rotate, Icon::Glyph("⟲"), "Rotate\nDrag the gizmo's ring (R rotates at any time)"),
                        (ToolKind::Scale, Icon::Scale, "Scale\nDrag the gizmo's handles (S scales at any time)"),
                        (ToolKind::AddNode, Icon::Glyph("✚"), "Add Node\nClick: add a node to the selected road or spline (Ctrl+click at any time)"),
                    ] {
                        if tool_button(ui, c.tool.active == t, icon, tip).clicked() {
                            c.tool.active = t;
                        }
                    }
                    ui.separator();
                    let drawing = c.tool.draw.as_ref().map(|d| d.kind);
                    if tool_button(ui, drawing == Some(DrawKind::Road), Icon::Road, "Draw a road\nClick points · Enter finishes · Esc cancels").clicked() {
                        commands::run(Cmd::DrawRoad, c);
                    }
                    let spline = matches!(drawing, Some(DrawKind::Spline(_)));
                    let resp = tool_button(ui, spline, Icon::Glyph("〰"), "Draw a kerb, wall or fence…");
                    egui::Popup::menu(&resp).show(|ui| {
                        ui.set_min_width(150.0);
                        for i in 0..PRESETS.len() {
                            commands::entry(ui, c, Cmd::DrawSpline(i));
                        }
                    });
                    if tool_button(ui, c.tool.place.is_some(), Icon::Glyph("📦"), "Place a prop from Assets").clicked() {
                        commands::run(Cmd::PlaceProp, c);
                    }
                });
        });
}

/// The camera's axes on screen, from the orbit: right, up and towards the camera.
fn basis(orbit: &Orbit) -> (Vec3, Vec3, Vec3) {
    let back = Vec3::new(
        orbit.pitch.cos() * orbit.yaw.cos(),
        orbit.pitch.sin(),
        orbit.pitch.cos() * orbit.yaw.sin(),
    );
    let right = (-back).cross(Vec3::Y).normalize_or(Vec3::X);
    let up = right.cross(-back);
    (right, up, back)
}

/// The navigation gizmo, top right: the axes as seen now; click one to look along it,
/// drag to orbit. Below it, buttons to zoom and pan by dragging, switch the
/// projection and frame everything.
fn navigation(ctx: &egui::Context, r: egui::Rect, c: &mut Ctx) {
    const SIZE: f32 = 96.0;
    egui::Area::new("navigation".into())
        .fixed_pos(egui::pos2(r.max.x - SIZE - 10.0, r.min.y + 8.0))
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
            let (resp, painter) =
                ui.allocate_painter(egui::vec2(SIZE, SIZE), egui::Sense::click_and_drag());
            let center = resp.rect.center();
            let (right, up, back) = basis(c.orbit);
            let reach = SIZE * 0.36;
            let mut balls: Vec<(ViewDir, bool, egui::Pos2, f32, egui::Color32, &str)> = [
                (
                    DVec3::X,
                    ViewDir::Right,
                    ViewDir::Left,
                    egui::Color32::from_rgb(245, 64, 84),
                    "X",
                ),
                (
                    DVec3::Y,
                    ViewDir::Back,
                    ViewDir::Front,
                    egui::Color32::from_rgb(135, 214, 34),
                    "Y",
                ),
                (
                    DVec3::Z,
                    ViewDir::Top,
                    ViewDir::Bottom,
                    egui::Color32::from_rgb(46, 133, 255),
                    "Z",
                ),
            ]
            .into_iter()
            .flat_map(|(axis, pos_view, neg_view, color, label)| {
                let v = to_bevy(axis);
                let s = egui::vec2(v.dot(right), -v.dot(up)) * reach;
                let depth = v.dot(back);
                [
                    (pos_view, true, center + s, depth, color, label),
                    (neg_view, false, center - s, -depth, color, label),
                ]
            })
            .collect();
            balls.sort_by(|a, b| a.3.total_cmp(&b.3));
            let hover = resp.hover_pos();
            let hot = hover.and_then(|p| {
                balls
                    .iter()
                    .rev()
                    .find(|b| b.2.distance(p) < 10.0)
                    .map(|b| b.0)
            });
            if resp.hovered() || resp.dragged() {
                painter.circle_filled(center, SIZE * 0.5, egui::Color32::from_white_alpha(22));
            }
            for &(dir, positive, at, _, color, label) in &balls {
                let lit = hot == Some(dir);
                if positive {
                    painter.line_segment([center, at], egui::Stroke::new(2.0, color));
                    painter.circle_filled(at, 9.0, if lit { egui::Color32::WHITE } else { color });
                    painter.text(
                        at,
                        egui::Align2::CENTER_CENTER,
                        label,
                        egui::FontId::proportional(11.0),
                        egui::Color32::BLACK,
                    );
                } else {
                    painter.circle_filled(
                        at,
                        8.0,
                        color.gamma_multiply(if lit { 0.8 } else { 0.35 }),
                    );
                    painter.circle_stroke(at, 8.0, egui::Stroke::new(1.0, color));
                }
            }
            if resp.dragged() {
                let d = resp.drag_delta();
                orbit_by(c.orbit, d.x * 0.01, d.y * 0.01);
            } else if resp.clicked()
                && let Some(dir) = hot
            {
                // Clicking the axis looked along already looks from the other side.
                let dir = if ViewDir::of(c.orbit) == Some(dir) {
                    dir.opposite()
                } else {
                    dir
                };
                look(c.orbit, dir);
            }
            resp.on_hover_text("Click an axis to look along it · drag to orbit");

            ui.vertical_centered(|ui| {
                let round = |ui: &mut egui::Ui, icon: &str, tip: &str, sense: egui::Sense| {
                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), sense);
                    let fill = if resp.hovered() || resp.dragged() {
                        egui::Color32::from_white_alpha(60)
                    } else {
                        egui::Color32::from_white_alpha(22)
                    };
                    ui.painter().circle_filled(rect.center(), 15.0, fill);
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        icon,
                        egui::FontId::proportional(15.0),
                        egui::Color32::WHITE,
                    );
                    resp.on_hover_text(tip)
                };
                let zoom = round(ui, "🔍", "Drag to zoom", egui::Sense::drag());
                if zoom.dragged() {
                    let k = 1.0 + zoom.drag_delta().y * 0.01;
                    c.orbit.distance = (c.orbit.distance * k).clamp(2.0, 15_000.0);
                }
                let pan = round(ui, "✋", "Drag to pan", egui::Sense::drag());
                if pan.dragged() {
                    let d = pan.drag_delta();
                    let k = c.orbit.distance * 0.0015;
                    c.orbit.focus += (-right * d.x + up * d.y) * k;
                }
                let icon = if c.orbit.ortho { "⊞" } else { "🎥" };
                if round(
                    ui,
                    icon,
                    "Perspective/Orthographic (Numpad 5)",
                    egui::Sense::click(),
                )
                .clicked()
                {
                    commands::run(Cmd::ToggleOrtho, c);
                }
                if round(ui, "⛶", "Frame All (Home)", egui::Sense::click()).clicked() {
                    commands::run(Cmd::FrameAll, c);
                }
            });
        });
}
