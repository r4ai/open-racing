//! Working corner by corner on a long track: each road's turns numbered from the start
//! line, labelled in the view, stepped through with Page Up and Page Down, and a tab
//! that gives each its kerbs, gravel or run-off and wall, of the Library's types.

use bevy_egui::egui;
use glam::DVec3;
use open_racing_track_project::corners::{Corner, CornerPart, Kit, kit_ops};
use open_racing_track_project::ops::Op;
use open_racing_track_project::project::{Road, Side};
use open_racing_track_render::to_bevy;

use crate::commands::Ctx;
use crate::properties::{row, section};
use crate::state::Item;
use crate::ui::{Focus, PropTab};
use crate::viewport::View;

/// The road whose corners are worked on: the selected one, or else the main road.
fn road_index(c: &Ctx) -> Option<usize> {
    let p = &c.editor.project;
    c.editor
        .selection
        .road()
        .filter(|&r| r < p.roads.len())
        .or_else(|| p.road_index(&p.main_road))
}

/// Selects road `r`, opens its corners and looks at corner `number`.
pub fn look(c: &mut Ctx, r: usize, number: usize) {
    let Some(corner) = c
        .built
        .corners
        .get(r)
        .and_then(|cs| cs.iter().find(|k| k.number == number))
    else {
        return;
    };
    let Some(smp) = c.built.roads.get(r) else {
        return;
    };
    if c.editor.selection.item != Some(Item::Road(r)) {
        c.editor.selection.select(Item::Road(r));
    }
    c.shell.corner = Some((r, number));
    c.shell.tab = PropTab::Corners;
    c.orbit.walk = None;
    c.orbit.focus = to_bevy(smp.frame_at(corner.apex).pos);
    c.orbit.distance = (corner.length(smp.length) * 1.3).clamp(60.0, 600.0) as f32;
}

/// Looks at corner `number` of the road worked on.
pub fn step_to(c: &mut Ctx, number: usize) {
    if let Some(r) = road_index(c) {
        look(c, r, number);
    }
}

/// Looks at the next corner (`by` 1) or the one before (-1) along the road.
pub fn step(c: &mut Ctx, by: isize) {
    let Some(r) = road_index(c) else {
        return;
    };
    let count = c.built.corners.get(r).map_or(0, Vec::len);
    if count == 0 {
        c.editor.status = "this road has no corners".into();
        return;
    }
    let next = match c.shell.corner {
        Some((road, n)) if road == r => (n as isize - 1 + by).rem_euclid(count as isize) as usize,
        _ if by > 0 => 0,
        _ => count - 1,
    };
    look(c, r, next + 1);
}

/// "T3" beside each corner's apex, on the outside; a click looks at it.
pub fn labels(ctx: &egui::Context, rect: egui::Rect, c: &mut Ctx, view: View) {
    if c.orbit.walk.is_some() || !c.tool.overlays.names {
        return;
    }
    let Some(r) = road_index(c) else {
        return;
    };
    let (Some(corners), Some(smp)) = (c.built.corners.get(r), c.built.roads.get(r)) else {
        return;
    };
    let mut clicked = None;
    // Not under the view's own widgets: the navigation at the top right, the mode at
    // the top left and the toolbar down the left.
    let widgets = [
        egui::Rect::from_min_max(
            egui::pos2(rect.max.x - 130.0, rect.min.y),
            egui::pos2(rect.max.x, rect.min.y + 250.0),
        ),
        egui::Rect::from_min_size(rect.min, egui::vec2(260.0, 64.0)),
        egui::Rect::from_min_size(rect.min, egui::vec2(56.0, 380.0)),
    ];
    // The corner looked at first; the others where they do not crowd what is drawn.
    let current = c
        .shell
        .corner
        .filter(|&(road, _)| road == r)
        .map(|(_, n)| n);
    let order = corners
        .iter()
        .filter(|k| Some(k.number) == current)
        .chain(corners.iter().filter(|k| Some(k.number) != current));
    let mut placed: Vec<egui::Pos2> = Vec::new();
    for k in order {
        let f = smp.frame_at(k.apex);
        let (sign, edge) = match k.outside() {
            Side::Left => (1.0, f.width_left),
            Side::Right => (-1.0, f.width_right),
        };
        let at = f.pos + f.lateral * (sign * (edge + 14.0)) + DVec3::Z;
        let Some(p) = view.screen(at).map(|s| egui::pos2(s.x, s.y)) else {
            continue;
        };
        let label = egui::Rect::from_center_size(p, egui::vec2(36.0, 22.0));
        if !rect.shrink(12.0).contains(p)
            || widgets.iter().any(|w| w.intersects(label))
            || placed.iter().any(|q| q.distance(p) < 34.0)
        {
            continue;
        }
        placed.push(p);
        let current = Some(k.number) == current;
        let text = egui::RichText::new(format!("T{}", k.number))
            .strong()
            .color(if current {
                egui::Color32::BLACK
            } else {
                egui::Color32::WHITE
            });
        let fill = if current {
            crate::theme::SELECTED_UI
        } else {
            egui::Color32::from_rgba_unmultiplied(30, 30, 30, 200)
        };
        egui::Area::new(egui::Id::new(("corner label", r, k.number)))
            .fixed_pos(p - egui::vec2(14.0, 10.0))
            .order(egui::Order::Middle)
            .show(ctx, |ui| {
                let resp = ui
                    .add(egui::Button::new(text).fill(fill).corner_radius(8.0))
                    .on_hover_text(summary(k));
                if resp.clicked() {
                    clicked = Some(k.number);
                }
            });
    }
    if let Some(n) = clicked {
        look(c, r, n);
    }
}

fn arrow(k: &Corner) -> &'static str {
    match k.dir {
        Side::Left => "⟲ left",
        Side::Right => "⟳ right",
    }
}

fn summary(k: &Corner) -> String {
    format!(
        "T{} {} · {:.0}° · radius {:.0} m",
        k.number,
        arrow(k),
        k.angle.to_degrees(),
        k.radius
    )
}

/// What each corner of the road worked on has, as a `Kit`.
fn kits(c: &Ctx, r: usize) -> Vec<Kit> {
    let (Some(road), Some(corners), Some(smp)) = (
        c.editor.project.roads.get(r),
        c.built.corners.get(r),
        c.built.roads.get(r),
    ) else {
        return vec![];
    };
    corners
        .iter()
        .map(|k| Kit::of(road, smp, corners, k))
        .collect()
}

/// A part of a kit: on or off, its type and its width (or distance from the edge).
fn part_ui(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug + Copy,
    label: &str,
    part: &mut Option<(String, f64)>,
    styles: &[String],
    fallback: (&str, f64),
    distance: bool,
) -> bool {
    let mut changed = false;
    row(ui, label, |ui| {
        let mut on = part.is_some();
        if ui.checkbox(&mut on, "").changed() {
            *part = on.then(|| {
                let style = styles
                    .iter()
                    .find(|s| *s == fallback.0)
                    .or(styles.first())
                    .cloned()
                    .unwrap_or_default();
                (style, fallback.1)
            });
            changed = true;
        }
        if let Some((style, width)) = part {
            egui::ComboBox::from_id_salt(id)
                .selected_text(style.as_str())
                .width(110.0)
                .show_ui(ui, |ui| {
                    for s in styles {
                        changed |= ui.selectable_value(style, s.clone(), s).changed();
                    }
                });
            let suffix = if distance { " m out" } else { " m" };
            changed |= ui
                .add(
                    egui::DragValue::new(width)
                        .speed(0.05)
                        .range(0.1..=200.0)
                        .suffix(suffix),
                )
                .changed();
        }
    });
    changed
}

/// Which kerbs, what outside them and what wall a corner has.
fn kit_ui(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug + Copy,
    kit: &mut Kit,
    project: &open_racing_track_project::Project,
) -> bool {
    let strips: Vec<String> = project
        .strip_styles
        .iter()
        .map(|s| s.name.clone())
        .collect();
    let walls: Vec<String> = project.wall_styles.iter().map(|s| s.name.clone()).collect();
    let mut changed = false;
    for (part, label, fallback) in [
        (CornerPart::Entry, "Entry kerb", ("kerb", 1.5)),
        (CornerPart::Apex, "Apex kerb", ("kerb", 1.5)),
        (CornerPart::Exit, "Exit kerb", ("kerb", 1.5)),
        (CornerPart::Outside, "Outside", ("gravel", 12.0)),
    ] {
        changed |= part_ui(
            ui,
            (id, part),
            label,
            kit.strip_mut(part),
            &strips,
            fallback,
            false,
        );
    }
    changed |= part_ui(
        ui,
        (id, "wall"),
        "Wall",
        &mut kit.wall,
        &walls,
        ("tyre wall", 25.0),
        true,
    );
    changed
}

/// The Corners tab of a road.
pub fn tab(ui: &mut egui::Ui, c: &mut Ctx) {
    let Some(r) = c.editor.selection.road() else {
        return;
    };
    let (Some(road), Some(corners), Some(smp)) = (
        c.editor.project.roads.get(r).cloned(),
        c.built.corners.get(r).cloned(),
        c.built.roads.get(r).cloned(),
    ) else {
        ui.weak("Building…");
        return;
    };
    ui.weak(format!(
        "{} corners, numbered from the start line. Page Up / Page Down in the view steps through them; click a T label to look at one. What is laid round a corner stays with it as the road changes.",
        corners.len()
    ));
    let have = kits(c, r);

    // One kit for many corners at once.
    let id = ui.make_persistent_id("corner kit for all");
    let mut all: Kit = ui
        .data(|d| d.get_temp::<Kit>(id))
        .unwrap_or_else(|| Kit::kerbs(&c.editor.project, None, 1.5));
    section(ui, "Every corner", "corners all", false, |ui| {
        kit_ui(ui, "all", &mut all, &c.editor.project);
        ui.horizontal_wrapped(|ui| {
            let bare = have.iter().filter(|k| k.is_empty()).count();
            let lay = |c: &mut Ctx, only_bare: bool| {
                let ops: Vec<Op> = corners
                    .iter()
                    .zip(&have)
                    .filter(|(_, k)| !only_bare || k.is_empty())
                    .flat_map(|(k, _)| {
                        kit_ops(&c.editor.project, &road.name, &smp, &corners, k, &all)
                    })
                    .collect();
                if !ops.is_empty() && c.editor.apply(ops, None) {
                    c.editor.status = "laid round the corners".into();
                }
            };
            if ui
                .add_enabled(
                    bare > 0,
                    egui::Button::new(format!("Lay on {bare} bare corners")),
                )
                .on_hover_text("Corners with nothing laid round them yet")
                .clicked()
            {
                lay(c, true);
            }
            if ui
                .button(format!("Lay on all {}", corners.len()))
                .on_hover_text(
                    "Every corner gets this, keeping where each part's ends were dragged",
                )
                .clicked()
            {
                lay(c, false);
            }
            if ui.button("Clear all corners").clicked() {
                let ops: Vec<Op> = corners
                    .iter()
                    .flat_map(|k| {
                        kit_ops(
                            &c.editor.project,
                            &road.name,
                            &smp,
                            &corners,
                            k,
                            &Kit::default(),
                        )
                    })
                    .collect();
                if !ops.is_empty() {
                    c.editor.apply(ops, None);
                }
            }
        });
    });
    ui.data_mut(|d| d.insert_temp(id, all));

    for (k, kit) in corners.iter().zip(&have) {
        let current = c.shell.corner == Some((r, k.number));
        let parts = [
            kit.entry.is_some(),
            kit.apex.is_some(),
            kit.exit.is_some(),
            kit.outside.is_some(),
            kit.wall.is_some(),
        ]
        .iter()
        .filter(|&&p| p)
        .count();
        let title = format!(
            "T{}  {}  ·  {:.0}°  ·  R {:.0} m  ·  {:.0} m{}",
            k.number,
            arrow(k),
            k.angle.to_degrees(),
            k.radius,
            k.length(smp.length),
            if parts > 0 {
                format!("  ·  {parts} parts")
            } else {
                String::new()
            }
        );
        let text = if current {
            egui::RichText::new(title).color(crate::theme::SELECTED_UI)
        } else {
            egui::RichText::new(title)
        };
        let resp = egui::CollapsingHeader::new(text)
            .id_salt(("corner", r, k.number))
            .open(current.then_some(true))
            .show_background(true)
            .show(ui, |ui| corner_ui(ui, c, r, &road, &smp, &corners, k, kit));
        if current && resp.header_response.clicked() {
            c.shell.corner = None;
        } else if resp.header_response.clicked() {
            look(c, r, k.number);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn corner_ui(
    ui: &mut egui::Ui,
    c: &mut Ctx,
    r: usize,
    road: &Road,
    smp: &open_racing_track_project::curve::Sampled,
    corners: &[Corner],
    k: &Corner,
    kit: &Kit,
) {
    ui.horizontal(|ui| {
        if ui.small_button("🔍 Look at it").clicked() {
            look(c, r, k.number);
        }
        ui.weak(format!(
            "entry {:.0} m · apex {:.0} m · exit {:.0} m",
            k.entry.rem_euclid(smp.length),
            k.apex.rem_euclid(smp.length),
            k.exit.rem_euclid(smp.length)
        ));
    });
    let mut want = kit.clone();
    if kit_ui(ui, ("corner", r, k.number), &mut want, &c.editor.project) {
        let ops = kit_ops(&c.editor.project, &road.name, smp, corners, k, &want);
        c.editor
            .apply(ops, Some(&format!("corner kit {} {}", road.name, k.number)));
    }
    ui.weak("Drag a part's ends or its outer edge in the view to fit it: it keeps that as the corner changes.");
    // Other parts limited to stretches round here, not laid with the corner.
    let near = |from: f64, to: f64| {
        let (a, b) = (smp.s_at(from), smp.s_at(to));
        let inside = if b >= a {
            (a..=b).contains(&k.apex)
        } else {
            k.apex >= a || k.apex <= b
        };
        inside
            || k.covers(a, 40.0, smp.length, smp.closed)
            || k.covers(b, 40.0, smp.length, smp.closed)
    };
    let mut parts: Vec<(String, PropTab, Focus)> = Vec::new();
    for side in [Side::Left, Side::Right] {
        for (i, s) in road.strips(side).iter().enumerate() {
            if s.corner.is_none() && s.ranges.iter().any(|g| near(g.from, g.to)) {
                parts.push((
                    format!("☰ {} ({side:?}, {:.1} m)", s.name, s.width),
                    PropTab::Strips,
                    Focus::Strip(side, i),
                ));
            }
        }
    }
    for (i, b) in road.barriers.iter().enumerate() {
        if b.corner.is_none() && b.ranges.iter().any(|g| near(g.from, g.to)) {
            parts.push((
                format!("🚧 {} ({:?}, {:.1} m out)", b.name, b.side, b.offset),
                PropTab::Barriers,
                Focus::Barrier(i),
            ));
        }
    }
    if !parts.is_empty() {
        ui.label("Also here:");
        for (label, tab, focus) in parts {
            if ui
                .add(egui::Button::new(label).frame(false))
                .on_hover_text("Edit it")
                .clicked()
            {
                c.shell.tab = tab;
                c.shell.focus = Some(focus);
            }
        }
    }
}
