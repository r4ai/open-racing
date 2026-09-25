//! Working corner by corner on a long track: each road's turns numbered from the start
//! line, labelled in the view, stepped through with Page Up and Page Down, and a tab
//! that gives each its kerbs and shows the stretches of strips and barriers round it.

use bevy_egui::egui;
use glam::DVec3;
use open_racing_track_project::corners::{Corner, Kerb, Kerbs, kerb_ops};
use open_racing_track_project::ops::Op;
use open_racing_track_project::project::{Profile, Road, Side};
use open_racing_track_render::to_bevy;

use crate::commands::Ctx;
use crate::properties::{drag, row, section};
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
    for k in corners {
        let f = smp.frame_at(k.apex);
        let (sign, edge) = match k.outside() {
            Side::Left => (1.0, f.width_left),
            Side::Right => (-1.0, f.width_right),
        };
        let at = f.pos + f.lateral * (sign * (edge + 14.0)) + DVec3::Z;
        let Some(p) = view.screen(at).map(|s| egui::pos2(s.x, s.y)) else {
            continue;
        };
        if !rect.shrink(12.0).contains(p) {
            continue;
        }
        let current = c.shell.corner == Some((r, k.number));
        let text = egui::RichText::new(format!("T{}", k.number))
            .strong()
            .color(if current {
                egui::Color32::BLACK
            } else {
                egui::Color32::WHITE
            });
        let fill = if current {
            egui::Color32::from_rgb(255, 200, 60)
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

/// The corner kerbs a road has for corner `number`, as a `Kerbs`.
fn kerbs_of(road: &Road, number: usize) -> Kerbs {
    let strip = |kerb: Kerb| {
        let name = kerb.strip_name(number);
        road.left.iter().chain(&road.right).find(|s| s.name == name)
    };
    let any = Kerb::ALL.iter().find_map(|&k| strip(k));
    let d = Kerbs::default();
    Kerbs {
        entry: strip(Kerb::Entry).is_some(),
        apex: strip(Kerb::Apex).is_some(),
        exit: strip(Kerb::Exit).is_some(),
        width: any.map_or(d.width, |s| s.width),
        profile: any.map_or(d.profile, |s| s.profile),
    }
}

/// Whether a strip is one of the kerbs laid per corner.
fn is_corner_kerb(name: &str) -> bool {
    name.starts_with('T')
        && Kerb::ALL
            .iter()
            .any(|k| name.ends_with(&format!(" {}", k.label())))
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
        "{} corners, numbered from the start line. Page Up / Page Down in the view steps through them; click a T label to look at one.",
        corners.len()
    ));
    ui.horizontal_wrapped(|ui| {
        if ui
            .button("Kerbs on every corner")
            .on_hover_text(
                "Entry and exit kerbs outside, apex kerbs inside, on each corner that has none",
            )
            .clicked()
        {
            let mut ops = Vec::new();
            for k in &corners {
                let has = kerbs_of(&road, k.number);
                if !(has.entry || has.apex || has.exit) {
                    ops.extend(kerb_ops(
                        &c.editor.project,
                        &road.name,
                        &smp,
                        k,
                        &Kerbs::default(),
                    ));
                }
            }
            if !ops.is_empty() && c.editor.apply(ops, None) {
                c.editor.status = "kerbs laid round the corners".into();
            }
        }
        if ui.button("Remove corner kerbs").clicked() {
            let mut ops = Vec::new();
            for side in [Side::Left, Side::Right] {
                for s in road.strips(side).iter().filter(|s| is_corner_kerb(&s.name)) {
                    ops.push(Op::RemoveStrip {
                        road: road.name.clone(),
                        side,
                        name: s.name.clone(),
                    });
                }
            }
            if !ops.is_empty() {
                c.editor.apply(ops, None);
            }
        }
    });
    for k in &corners {
        let current = c.shell.corner == Some((r, k.number));
        let title = format!(
            "T{}  {}  ·  {:.0}°  ·  R {:.0} m  ·  {:.0} m long",
            k.number,
            arrow(k),
            k.angle.to_degrees(),
            k.radius,
            k.length(smp.length)
        );
        let text = if current {
            egui::RichText::new(title).color(egui::Color32::from_rgb(255, 200, 60))
        } else {
            egui::RichText::new(title)
        };
        let resp = egui::CollapsingHeader::new(text)
            .id_salt(("corner", r, k.number))
            .open(current.then_some(true))
            .show_background(true)
            .show(ui, |ui| corner_ui(ui, c, r, &road, &smp, k));
        if current && resp.header_response.clicked() {
            c.shell.corner = None;
        } else if resp.header_response.clicked() {
            look(c, r, k.number);
        }
    }
}

fn corner_ui(
    ui: &mut egui::Ui,
    c: &mut Ctx,
    r: usize,
    road: &Road,
    smp: &open_racing_track_project::curve::Sampled,
    k: &Corner,
) {
    if ui.small_button("🔍 Look at it").clicked() {
        look(c, r, k.number);
    }
    row(ui, "Places", |ui| {
        ui.weak(format!(
            "entry {:.0} m · apex {:.0} m · exit {:.0} m",
            k.entry.rem_euclid(smp.length),
            k.apex.rem_euclid(smp.length),
            k.exit.rem_euclid(smp.length)
        ))
    });
    section(ui, "Kerbs", ("corner kerbs", r, k.number), true, |ui| {
        let before = kerbs_of(road, k.number);
        let mut kerbs = before;
        row(ui, "Outside", |ui| {
            ui.checkbox(&mut kerbs.entry, "entry");
            ui.checkbox(&mut kerbs.exit, "exit");
        });
        row(ui, "Inside", |ui| ui.checkbox(&mut kerbs.apex, "apex"));
        drag(ui, "Width m", &mut kerbs.width, 0.05, 0.2..=6.0);
        row(ui, "Kind", |ui| {
            for (label, profile) in [
                ("Flat", Profile::Flat),
                ("Rounded", Profile::Crown(0.03)),
                ("Raised", Profile::Crown(0.08)),
                ("Sausage", Profile::Crown(0.15)),
            ] {
                if ui
                    .selectable_label(kerbs.profile == profile, label)
                    .clicked()
                {
                    kerbs.profile = profile;
                }
            }
        });
        if kerbs != before {
            let ops = kerb_ops(&c.editor.project, &road.name, smp, k, &kerbs);
            c.editor.apply(
                ops,
                Some(&format!("corner kerbs {} {}", road.name, k.number)),
            );
        }
        ui.weak("Drag a kerb's ends or its width handle in the view to fit it.");
    });
    // Other parts limited to stretches round here: kerbs, gravel, walls.
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
            if s.ranges.iter().any(|g| near(g.from, g.to)) {
                parts.push((
                    format!("☰ {} ({side:?}, {:.1} m {})", s.name, s.width, s.surface),
                    PropTab::Strips,
                    Focus::Strip(side, i),
                ));
            }
        }
    }
    for (i, b) in road.barriers.iter().enumerate() {
        if b.ranges.iter().any(|g| near(g.from, g.to)) {
            parts.push((
                format!("🚧 {} ({:?}, {:.1} m out)", b.name, b.side, b.offset),
                PropTab::Barriers,
                Focus::Barrier(i),
            ));
        }
    }
    if !parts.is_empty() {
        ui.label("Stretches here:");
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
