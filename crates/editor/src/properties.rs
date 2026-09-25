//! The properties editor, as Blender's: a column of tabs, the track's settings first,
//! then the selected item's, each a stack of collapsible panels of labelled fields.

use bevy_egui::egui;
use glam::DVec3;
use open_racing_sim::Surface;
use open_racing_track_project::ops::{Curve, Op};
use open_racing_track_project::project::{
    Align, Alpha, Barrier, BuiltinTexture, Grid, MaterialDef, NamedSurface, PaintLine, Pit,
    Profile, Range, Shape, Side, Strip, TextureSource,
};
use open_racing_track_project::{Key, Project};

use crate::assets::Library;
use crate::commands::{self, Cmd, Ctx};
use crate::edit;
use crate::state::{Editor, Item};
use crate::ui::{Focus, PropTab};

/// Width of the labels left of the fields.
const LABEL_W: f32 = 104.0;

#[derive(Default)]
pub struct State {
    new_surface: String,
    new_material: String,
    /// The item being renamed and its new name so far.
    rename: Option<(Item, String)>,
    /// The track's name as typed so far.
    track_name: String,
    /// The track's tab is shown only because nothing was selected.
    stand_in: bool,
}

fn tabs(c: &Ctx) -> Vec<(PropTab, &'static str, &'static str)> {
    let mut tabs = vec![
        (PropTab::Track, "🏁", "Track: name, main road, baking"),
        (
            PropTab::Markers,
            "🚩",
            "Race markers: start, sectors, grid, pit lane",
        ),
        (PropTab::Terrain, "🗻", "Terrain round the roads"),
        (PropTab::Surfaces, "◎", "Surfaces: what the tyres feel"),
        (PropTab::Materials, "🎨", "Materials: how things look"),
    ];
    match c.editor.selection.item {
        Some(Item::Road(_)) => tabs.extend([
            (PropTab::Object, "🚗", "Road"),
            (
                PropTab::Strips,
                "☰",
                "Strips: kerbs, grass, gravel and run-off beside the road",
            ),
            (PropTab::Lines, "✏", "Painted lines"),
            (PropTab::Barriers, "🚧", "Barriers along the road"),
        ]),
        Some(Item::Spline(_)) => tabs.push((PropTab::Object, "〰", "Kerb, wall or fence")),
        Some(Item::Prop(_)) => tabs.push((PropTab::Object, "📦", "Prop")),
        None => {}
    }
    tabs
}

pub fn show(ui: &mut egui::Ui, c: &mut Ctx, state: &mut State, library: &Library) {
    let tabs = tabs(c);
    let selected = c.editor.selection.item.is_some();
    if !tabs.iter().any(|(t, ..)| *t == c.shell.tab) {
        // With nothing selected the track's tab stands in, until something is again.
        state.stand_in = !selected;
        c.shell.tab = if selected {
            PropTab::Object
        } else {
            PropTab::Track
        };
    } else if state.stand_in && selected {
        state.stand_in = false;
        c.shell.tab = PropTab::Object;
    }
    egui::Panel::left("properties tabs")
        .exact_size(36.0)
        .resizable(false)
        .frame(
            egui::Frame::NONE
                .fill(ui.visuals().extreme_bg_color)
                .inner_margin(4.0),
        )
        .show(ui, |ui| {
            for (i, (tab, icon, tip)) in tabs.iter().enumerate() {
                if i == 5 {
                    ui.separator();
                }
                let button = egui::Button::selectable(c.shell.tab == *tab, *icon);
                if ui
                    .add_sized([28.0, 26.0], button)
                    .on_hover_text(*tip)
                    .clicked()
                {
                    c.shell.tab = *tab;
                    state.stand_in = false;
                }
            }
        });
    egui::CentralPanel::default().show(ui, |ui| {
        let title = tabs
            .iter()
            .find(|(t, ..)| *t == c.shell.tab)
            .map_or("", |(_, _, tip)| tip.split(':').next().unwrap_or(tip));
        ui.horizontal(|ui| {
            if let Some(item) = c
                .editor
                .selection
                .item
                .filter(|_| c.shell.tab >= PropTab::Object)
            {
                let name = edit::item_name(&c.editor.project, item).unwrap_or_default();
                ui.strong(name);
                if !matches!(c.shell.tab, PropTab::Object) {
                    ui.weak("›");
                    ui.label(title);
                }
            } else {
                ui.strong(title);
            }
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match c.shell.tab {
                PropTab::Track => track_tab(ui, c, state),
                PropTab::Markers => markers_tab(ui, c.editor),
                PropTab::Terrain => terrain_tab(ui, c.editor),
                PropTab::Surfaces => surfaces_tab(ui, c.editor, state),
                PropTab::Materials => materials_tab(ui, c.editor, state, library),
                PropTab::Object => match c.editor.selection.item {
                    Some(Item::Road(_)) => road_tab(ui, c, state),
                    Some(Item::Spline(_)) => spline_tab(ui, c, state),
                    Some(Item::Prop(_)) => prop_tab(ui, c, state, library),
                    None => {}
                },
                PropTab::Strips => strips_tab(ui, c),
                PropTab::Lines => lines_tab(ui, c),
                PropTab::Barriers => barriers_tab(ui, c),
            });
    });
}

// Layout helpers: Blender's panels of right-aligned labels beside full-width fields.

/// A collapsible panel.
pub fn section<R>(
    ui: &mut egui::Ui,
    title: impl Into<egui::WidgetText>,
    id: impl std::hash::Hash + std::fmt::Debug,
    open: bool,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::CollapsingResponse<R> {
    egui::CollapsingHeader::new(title)
        .id_salt(id)
        .default_open(open)
        .show_background(true)
        .show(ui, body)
}

/// A label and a field beside it.
pub fn row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal(|ui| {
        let h = ui.spacing().interact_size.y;
        ui.allocate_ui_with_layout(
            egui::vec2(LABEL_W, h),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                ui.set_min_width(LABEL_W);
                ui.label(label)
            },
        );
        add(ui)
    })
    .inner
}

/// A number field as wide as the panel.
pub fn number(ui: &mut egui::Ui, v: &mut f64, speed: f64, suffix: &str) -> bool {
    let w = ui.available_width().max(40.0);
    ui.add_sized(
        [w, ui.spacing().interact_size.y],
        egui::DragValue::new(v).speed(speed).suffix(suffix),
    )
    .changed()
}

pub fn drag(
    ui: &mut egui::Ui,
    label: &str,
    v: &mut f64,
    speed: f64,
    range: std::ops::RangeInclusive<f64>,
) -> bool {
    row(ui, label, |ui| {
        let w = ui.available_width().max(40.0);
        ui.add_sized(
            [w, ui.spacing().interact_size.y],
            egui::DragValue::new(v).speed(speed).range(range),
        )
        .changed()
    })
}

/// X, Y and Z fields, as Blender's location.
pub fn vector(ui: &mut egui::Ui, label: &str, v: &mut DVec3, speed: f64) -> bool {
    let mut changed = false;
    for (i, (axis, x)) in ["X", "Y", "Z"]
        .iter()
        .zip([&mut v.x, &mut v.y, &mut v.z])
        .enumerate()
    {
        let label = if i == 0 {
            format!("{label} {axis}")
        } else {
            axis.to_string()
        };
        changed |= row(ui, &label, |ui| number(ui, x, speed, " m"));
    }
    changed
}

fn check(ui: &mut egui::Ui, v: &mut bool, label: &str) -> bool {
    row(ui, "", |ui| ui.checkbox(v, label).changed())
}

fn combo(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    value: &mut String,
    options: &[String],
) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id)
        .selected_text(value.as_str())
        .width(ui.available_width().max(60.0))
        .show_ui(ui, |ui| {
            for o in options {
                changed |= ui.selectable_value(value, o.clone(), o).changed();
            }
        });
    changed
}

fn combo_row(
    ui: &mut egui::Ui,
    label: &str,
    id: impl std::hash::Hash + std::fmt::Debug,
    value: &mut String,
    options: &[String],
) -> bool {
    row(ui, label, |ui| combo(ui, id, value, options))
}

fn names(project: &Project) -> (Vec<String>, Vec<String>) {
    (
        project.surfaces.iter().map(|s| s.name.clone()).collect(),
        project.materials.iter().map(|m| m.name.clone()).collect(),
    )
}

/// Buttons that choose one of a few values.
fn choice<T: PartialEq + Copy>(ui: &mut egui::Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let mut changed = false;
    for (v, label) in options {
        changed |= ui.selectable_value(value, *v, *label).changed();
    }
    changed
}

/// The name field of an item: renames it when the field loses focus.
fn name_row(ui: &mut egui::Ui, c: &mut Ctx, state: &mut State, item: Item) {
    let name = edit::item_name(&c.editor.project, item)
        .unwrap_or_default()
        .to_string();
    if !matches!(&state.rename, Some((i, _)) if *i == item) {
        state.rename = Some((item, name.clone()));
    }
    let text = &mut state.rename.as_mut().expect("just set").1;
    let resp = row(ui, "Name", |ui| {
        ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY))
    });
    if resp.lost_focus() {
        let to = text.clone();
        if !edit::rename(c.editor, item, &to) {
            *text = name;
        }
    } else if !resp.has_focus() {
        // Follow renames from elsewhere (undo, the outliner, the file).
        *text = name;
    }
}

/// A stretch of a node's length round node `n`, or along the whole road without one.
/// On an open road it stops at the ends; on a closed one it may run across the start.
fn stretch_round(node: Option<usize>, period: f64, closed: bool) -> Vec<Range> {
    let Some(n) = node else {
        return vec![];
    };
    let u = n as f64;
    let range = if closed {
        Range {
            from: (u - 0.5).rem_euclid(period.max(1.0)),
            to: (u + 0.5).rem_euclid(period.max(1.0)),
        }
    } else {
        Range {
            from: (u - 0.5).max(0.0),
            to: (u + 0.5).min(period),
        }
    };
    vec![range]
}

/// Stretches of the road, in spline parameters, with a button to add one round the
/// selected node.
fn ranges_ui(
    ui: &mut egui::Ui,
    ranges: &mut Vec<Range>,
    node: Option<usize>,
    period: f64,
    closed: bool,
) -> bool {
    let mut changed = false;
    let mut remove = None;
    if ranges.is_empty() {
        row(ui, "Stretches", |ui| ui.weak("the whole road"));
    }
    for (i, r) in ranges.iter_mut().enumerate() {
        row(ui, if i == 0 { "Stretches" } else { "" }, |ui| {
            ui.label("u");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut r.from)
                        .speed(0.01)
                        .range(0.0..=period),
                )
                .changed();
            ui.label("to");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut r.to)
                        .speed(0.01)
                        .range(0.0..=period),
                )
                .changed();
            if ui
                .small_button("✖")
                .on_hover_text("Remove this stretch")
                .clicked()
            {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        ranges.remove(i);
        changed = true;
    }
    let label = match node {
        Some(n) => format!("+ Stretch round node {n}"),
        None => "+ Stretch".into(),
    };
    row(ui, "", |ui| {
        if ui
            .small_button(label)
            .on_hover_text("Limit it to part of the road; drag the ends in the view")
            .clicked()
        {
            ranges.extend(stretch_round(Some(node.unwrap_or(0)), period, closed));
            changed = true;
        }
    });
    changed
}

fn profile_ui(
    ui: &mut egui::Ui,
    p: &mut Profile,
    id: impl std::hash::Hash + std::fmt::Debug,
) -> bool {
    let mut changed = false;
    row(ui, "Profile", |ui| {
        let (kind, mut v) = match *p {
            Profile::Flat => (0, 0.0),
            Profile::Crown(h) => (1, h),
            Profile::Slope(d) => (2, d),
        };
        let mut k = kind;
        egui::ComboBox::from_id_salt(id)
            .selected_text(["flat", "crown", "slope"][k])
            .show_ui(ui, |ui| {
                for (i, n) in ["flat", "crown", "slope"].iter().enumerate() {
                    ui.selectable_value(&mut k, i, *n);
                }
            });
        if k != 0 {
            changed |= number(ui, &mut v, 0.005, " m");
        }
        if k != kind || changed {
            *p = match k {
                0 => Profile::Flat,
                1 => Profile::Crown(if k != kind { 0.03 } else { v }),
                _ => Profile::Slope(if k != kind { 0.2 } else { v }),
            };
            changed = true;
        }
    });
    changed
}

/// Opens the panel the outliner pointed at, once.
fn focused(c: &mut Ctx, focus: Focus) -> Option<bool> {
    (c.shell.focus == Some(focus)).then(|| {
        c.shell.focus = None;
        true
    })
}

// The tabs.

fn track_tab(ui: &mut egui::Ui, c: &mut Ctx, state: &mut State) {
    section(ui, "Track", "track", true, |ui| {
        let resp = row(ui, "Name", |ui| {
            ui.add(egui::TextEdit::singleline(&mut state.track_name).desired_width(f32::INFINITY))
        });
        let name = state.track_name.trim();
        if resp.lost_focus() {
            if !name.is_empty() && name != c.editor.project.name {
                let name = name.to_string();
                c.editor.apply(vec![Op::SetName { name }], None);
            }
            state.track_name = c.editor.project.name.clone();
        } else if !resp.has_focus() {
            // Follow changes from elsewhere (undo, the file).
            state.track_name = c.editor.project.name.clone();
        }
        let closed: Vec<String> = c
            .editor
            .project
            .roads
            .iter()
            .filter(|r| r.closed)
            .map(|r| r.name.clone())
            .collect();
        let mut main = c.editor.project.main_road.clone();
        if combo_row(ui, "Main road", "main road", &mut main, &closed) {
            c.editor.apply(vec![Op::SetMainRoad { road: main }], None);
        }
        let p = &c.editor.project;
        if let Some(s) = p
            .road_index(&p.main_road)
            .and_then(|i| c.built.roads.get(i))
        {
            row(ui, "Lap", |ui| ui.label(format!("{:.0} m", s.length)));
        }
        let dir = c.editor.dir.display().to_string();
        row(ui, "Folder", |ui| {
            ui.add(egui::Label::new(egui::RichText::new(&dir).weak()).truncate())
                .on_hover_text(&dir)
        });
    });
    section(ui, "Bake", "bake", true, |ui| {
        ui.label("Build the track package the game drives, check it and drive a test lap.");
        ui.horizontal(|ui| {
            commands::button(ui, c, Cmd::Bake);
            commands::button(ui, c, Cmd::BakeDrive);
        });
        if let Some(r) = &c.jobs.report {
            let first = r.lines().next().unwrap_or_default();
            ui.weak(first);
            if ui.small_button("Show report").clicked() {
                c.shell.bottom = crate::ui::BottomTab::Report;
                c.shell.bottom_open = true;
            }
        }
    });
}

fn road_tab(ui: &mut egui::Ui, c: &mut Ctx, state: &mut State) {
    let Some(r) = c
        .editor
        .selection
        .road()
        .filter(|&r| r < c.editor.project.roads.len())
    else {
        return;
    };
    let road = c.editor.project.roads[r].clone();
    let name = road.name.clone();
    let (surfaces, materials) = names(&c.editor.project);
    let node = c.editor.selection.node();
    let period = road.period();

    section(ui, "Road", ("road", r), true, |ui| {
        name_row(ui, c, state, Item::Road(r));
        if name == c.editor.project.main_road {
            row(ui, "", |ui| ui.label("★ The main road: the circuit"));
        } else {
            row(ui, "", |ui| commands::button(ui, c, Cmd::SetMain));
        }
        let (mut closed, mut crown, mut resolution) = (road.closed, road.crown, road.resolution);
        let (mut surface, mut material) = (road.surface.clone(), road.material.clone());
        let mut changed = check(ui, &mut closed, "Closed loop");
        changed |= drag(ui, "Crown m", &mut crown, 0.005, -0.5..=0.5);
        changed |= drag(ui, "Resolution m", &mut resolution, 0.1, 0.25..=10.0);
        changed |= combo_row(ui, "Surface", ("road surface", r), &mut surface, &surfaces);
        changed |= combo_row(
            ui,
            "Material",
            ("road material", r),
            &mut material,
            &materials,
        );
        if changed {
            c.editor.apply(
                vec![Op::SetRoad {
                    road: name.clone(),
                    closed: Some(closed),
                    crown: Some(crown),
                    surface: Some(surface),
                    material: Some(material),
                    resolution: Some(resolution),
                }],
                Some(&format!("road props {name}")),
            );
        }
    });

    section(ui, "Nodes", ("nodes", r), true, |ui| {
        let length = c.built.roads.get(r).map_or(0.0, |s| s.length);
        row(ui, "Count", |ui| {
            ui.label(format!("{} nodes, {length:.0} m", road.nodes.len()))
        });
        row(ui, "Selected", |ui| {
            ui.label(match c.editor.selection.nodes.len() {
                0 => "none: edits move the whole road".to_string(),
                n => format!("{n}, active node {}", node.unwrap_or(0)),
            })
        });
        ui.horizontal_wrapped(|ui| {
            for cmd in [Cmd::SelectAll, Cmd::Subdivide, Cmd::ToggleClosed] {
                commands::button(ui, c, cmd);
            }
        });
        ui.weak("Edit the selected nodes in the sidebar (N) or drag them in the view.");
    });

    for (curve, title, scale) in [
        (Curve::WidthLeft, "Width left (m)", 1.0),
        (Curve::WidthRight, "Width right (m)", 1.0),
        (Curve::Bank, "Bank (°)", 180.0 / std::f64::consts::PI),
    ] {
        let cv = match curve {
            Curve::WidthLeft => &road.width_left,
            Curve::WidthRight => &road.width_right,
            _ => &road.bank,
        };
        section(ui, title, (title, r), false, |ui| {
            ui.weak("Keys along the road, at a node number (u). Easier: select nodes, then Alt S / Ctrl T in the view, the sidebar (N) or the Curves graph below.");
            let mut keys = cv.keys.clone();
            let mut changed = false;
            let mut remove = None;
            for (i, k) in keys.iter_mut().enumerate() {
                row(ui, &format!("Key {i}"), |ui| {
                    ui.label("node");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut k.u)
                                .speed(0.01)
                                .range(0.0..=period),
                        )
                        .changed();
                    let mut v = k.value * scale;
                    if ui.add(egui::DragValue::new(&mut v).speed(0.05)).changed() {
                        k.value = v / scale;
                        changed = true;
                    }
                    if cv.keys.len() > 1 && ui.small_button("✖").clicked() {
                        remove = Some(i);
                    }
                });
                row(ui, "", |ui| {
                    for (label, slope) in [("in", &mut k.slope_in), ("out", &mut k.slope_out)] {
                        if !road.closed
                            && ((label == "in" && i == 0)
                                || (label == "out" && i + 1 == cv.keys.len()))
                        {
                            continue;
                        }
                        ui.label(label);
                        let mut v = *slope * scale;
                        if ui
                            .add(egui::DragValue::new(&mut v).speed(0.05).suffix(" /u"))
                            .changed()
                        {
                            *slope = v / scale;
                            changed = true;
                        }
                    }
                });
            }
            if let Some(i) = remove {
                keys.remove(i);
                changed = true;
            }
            if let Some(n) = node
                && ui.small_button(format!("+ Key at node {n}")).clicked()
            {
                let v = cv.eval(n as f64, period, road.closed);
                keys.push(Key::new(n as f64, v));
                changed = true;
            }
            if changed {
                c.editor.apply(
                    vec![Op::SetProfile {
                        road: name.clone(),
                        curve,
                        keys,
                    }],
                    Some(&format!("profile {name} {title}")),
                );
            }
        });
    }
}

fn strips_tab(ui: &mut egui::Ui, c: &mut Ctx) {
    let Some(r) = c.editor.selection.road() else {
        return;
    };
    let Some(road) = c.editor.project.roads.get(r).cloned() else {
        return;
    };
    let name = road.name.clone();
    let (surfaces, materials) = names(&c.editor.project);
    let node = c.editor.selection.node();
    let period = road.period();
    ui.weak("Bands beside the road, from its edge outwards.");
    for side in [Side::Left, Side::Right] {
        let title = match side {
            Side::Left => "Left side",
            Side::Right => "Right side",
        };
        section(ui, title, ("strips", r, side as u8), true, |ui| {
            let strips = road.strips(side).clone();
            for (i, strip) in strips.iter().enumerate() {
                let mut s = strip.clone();
                let (mut changed, mut removed) = (false, false);
                let open = focused(c, Focus::Strip(side, i));
                let resp = egui::CollapsingHeader::new(format!(
                    "{}  ·  {:.1} m {}",
                    s.name, s.width, s.surface
                ))
                .id_salt(("strip", r, side as u8, i))
                .open(open)
                .show(ui, |ui| {
                    changed |= drag(ui, "Width m", &mut s.width, 0.05, 0.0..=200.0);
                    changed |= combo_row(
                        ui,
                        "Surface",
                        ("ss", r, side as u8, i),
                        &mut s.surface,
                        &surfaces,
                    );
                    changed |= combo_row(
                        ui,
                        "Material",
                        ("sm", r, side as u8, i),
                        &mut s.material,
                        &materials,
                    );
                    changed |= profile_ui(ui, &mut s.profile, (r, side as u8, i));
                    changed |= drag(ui, "Fade m", &mut s.fade, 0.1, 0.0..=100.0);
                    changed |= ranges_ui(ui, &mut s.ranges, node, period, road.closed);
                    removed = row(ui, "", |ui| ui.button("Remove strip").clicked());
                });
                if open.is_some() {
                    resp.header_response.scroll_to_me(Some(egui::Align::TOP));
                }
                if removed {
                    c.editor.apply(
                        vec![Op::RemoveStrip {
                            road: name.clone(),
                            side,
                            name: s.name,
                        }],
                        None,
                    );
                    return;
                }
                if changed {
                    c.editor.apply(
                        vec![Op::PutStrip {
                            road: name.clone(),
                            side,
                            strip: s,
                            at: None,
                        }],
                        Some(&format!("strip {name} {side:?} {i}")),
                    );
                }
            }
            ui.horizontal_wrapped(|ui| {
                ui.label("Add");
                for (label, surface, material, width, profile) in [
                    ("Kerb", "kerb", "kerb", 1.2, Profile::Crown(0.03)),
                    ("Grass", "grass", "grass", 10.0, Profile::Slope(0.2)),
                    ("Gravel", "gravel", "gravel", 15.0, Profile::Slope(0.3)),
                    ("Run-off", "runoff", "asphalt", 10.0, Profile::Flat),
                ] {
                    if ui
                        .small_button(format!("+ {label}"))
                        .on_hover_text(match node {
                            Some(n) => format!("Round node {n}; drag its ends in the view"),
                            None => "Along the whole road".into(),
                        })
                        .clicked()
                    {
                        let base = label.to_lowercase();
                        let mut k = 1;
                        let mut sname = base.clone();
                        while strips.iter().any(|s| s.name == sname) {
                            k += 1;
                            sname = format!("{base} {k}");
                        }
                        let ranges = stretch_round(node, period, road.closed);
                        let strip = Strip {
                            name: sname,
                            width,
                            surface: surface.into(),
                            material: material.into(),
                            profile,
                            ranges,
                            fade: 3.0,
                        };
                        // Kerbs go against the road; the rest outermost.
                        let at = (label == "Kerb").then_some(0);
                        c.editor.apply(
                            vec![Op::PutStrip {
                                road: name.clone(),
                                side,
                                strip,
                                at,
                            }],
                            None,
                        );
                    }
                }
            });
        });
    }
}

fn lines_tab(ui: &mut egui::Ui, c: &mut Ctx) {
    let Some(r) = c.editor.selection.road() else {
        return;
    };
    let Some(road) = c.editor.project.roads.get(r).cloned() else {
        return;
    };
    let name = road.name.clone();
    let (_, materials) = names(&c.editor.project);
    let node = c.editor.selection.node();
    let period = road.period();
    for (i, line) in road.lines.iter().enumerate() {
        let mut l = line.clone();
        let (mut changed, mut removed) = (false, false);
        let open = focused(c, Focus::Line(i));
        let resp = egui::CollapsingHeader::new(&l.name)
            .id_salt(("line", r, i))
            .default_open(true)
            .show_background(true)
            .open(open)
            .show(ui, |ui| {
                changed |= drag(ui, "Offset m", &mut l.offset, 0.05, -100.0..=100.0);
                changed |= drag(ui, "Width m", &mut l.width, 0.01, 0.01..=5.0);
                changed |= combo_row(ui, "Material", ("lm", r, i), &mut l.material, &materials);
                let mut dashed = l.dash.is_some();
                if check(ui, &mut dashed, "Dashed") {
                    l.dash = dashed.then_some((3.0, 9.0));
                    changed = true;
                }
                if let Some((on, off)) = &mut l.dash {
                    changed |= drag(ui, "Dash m", on, 0.1, 0.1..=100.0);
                    changed |= drag(ui, "Gap m", off, 0.1, 0.1..=100.0);
                }
                changed |= ranges_ui(ui, &mut l.ranges, node, period, road.closed);
                removed = row(ui, "", |ui| ui.button("Remove line").clicked());
            });
        if open.is_some() {
            resp.header_response.scroll_to_me(Some(egui::Align::TOP));
        }
        if removed {
            c.editor.apply(
                vec![Op::RemoveLine {
                    road: name.clone(),
                    name: l.name,
                }],
                None,
            );
            return;
        }
        if changed {
            c.editor.apply(
                vec![Op::PutLine {
                    road: name.clone(),
                    line: l,
                }],
                Some(&format!("line {name} {i}")),
            );
        }
    }
    ui.weak("Offsets are from the road's centre, positive to the left.");
    if ui.button("+ Line").clicked() {
        let n = (1..)
            .map(|k| format!("line {k}"))
            .find(|n| road.lines.iter().all(|l| &l.name != n))
            .expect("some name is free");
        let line = PaintLine {
            name: n,
            offset: 0.0,
            width: 0.12,
            material: "paint".into(),
            ranges: vec![],
            dash: Some((3.0, 9.0)),
        };
        c.editor.apply(
            vec![Op::PutLine {
                road: name.clone(),
                line,
            }],
            None,
        );
    }
}

fn barriers_tab(ui: &mut egui::Ui, c: &mut Ctx) {
    let Some(r) = c.editor.selection.road() else {
        return;
    };
    let Some(road) = c.editor.project.roads.get(r).cloned() else {
        return;
    };
    let name = road.name.clone();
    let (_, materials) = names(&c.editor.project);
    let node = c.editor.selection.node();
    let period = road.period();
    for (i, barrier) in road.barriers.iter().enumerate() {
        let mut b = barrier.clone();
        let (mut changed, mut removed) = (false, false);
        let open = focused(c, Focus::Barrier(i));
        let resp = egui::CollapsingHeader::new(format!("{}  ·  {:?}", b.name, b.side))
            .id_salt(("barrier", r, i))
            .default_open(true)
            .show_background(true)
            .open(open)
            .show(ui, |ui| {
                changed |= row(ui, "Side", |ui| {
                    choice(
                        ui,
                        &mut b.side,
                        &[(Side::Left, "Left"), (Side::Right, "Right")],
                    )
                });
                changed |= drag(ui, "From edge m", &mut b.offset, 0.1, 0.0..=500.0);
                changed |= drag(ui, "Height m", &mut b.height, 0.05, 0.1..=20.0);
                changed |= drag(ui, "Thickness m", &mut b.thickness, 0.05, 0.0..=5.0);
                changed |= combo_row(ui, "Material", ("bm", r, i), &mut b.material, &materials);
                changed |= ranges_ui(ui, &mut b.ranges, node, period, road.closed);
                removed = row(ui, "", |ui| ui.button("Remove barrier").clicked());
            });
        if open.is_some() {
            resp.header_response.scroll_to_me(Some(egui::Align::TOP));
        }
        if removed {
            c.editor.apply(
                vec![Op::RemoveBarrier {
                    road: name.clone(),
                    name: b.name,
                }],
                None,
            );
            return;
        }
        if changed {
            c.editor.apply(
                vec![Op::PutBarrier {
                    road: name.clone(),
                    barrier: b,
                }],
                Some(&format!("barrier {name} {i}")),
            );
        }
    }
    if ui.button("+ Barrier").clicked() {
        let n = (1..)
            .map(|k| format!("barrier {k}"))
            .find(|n| road.barriers.iter().all(|b| &b.name != n))
            .expect("some name is free");
        let barrier = Barrier {
            name: n,
            side: Side::Left,
            offset: 10.0,
            height: 1.0,
            thickness: 0.0,
            material: "armco".into(),
            ranges: vec![],
        };
        c.editor.apply(
            vec![Op::PutBarrier {
                road: name.clone(),
                barrier,
            }],
            None,
        );
    }
}

fn spline_tab(ui: &mut egui::Ui, c: &mut Ctx, state: &mut State) {
    let Some(i) = c
        .editor
        .selection
        .spline()
        .filter(|&i| i < c.editor.project.splines.len())
    else {
        return;
    };
    let before = c.editor.project.splines[i].clone();
    let name = before.name.clone();
    let (surfaces, materials) = names(&c.editor.project);
    let mut sp = before.clone();
    let mut changed = false;
    section(ui, "Spline", ("spline", i), true, |ui| {
        name_row(ui, c, state, Item::Spline(i));
        changed |= check(ui, &mut sp.closed, "Closed loop");
        changed |= row(ui, "", |ui| {
            ui.checkbox(&mut sp.drape, "Follow the ground")
                .on_hover_text(
                    "Lay it on the roads and terrain under its line instead of at the nodes' heights",
                )
                .changed()
        });
        changed |= drag(ui, "Resolution m", &mut sp.resolution, 0.05, 0.1..=10.0);
        row(ui, "Nodes", |ui| ui.label(before.nodes.len().to_string()));
    });
    section(ui, "Shape", ("spline shape", i), true, |ui| {
        row(ui, "Kind", |ui| {
            let band = matches!(sp.shape, Shape::Band { .. });
            let preset = |label| {
                crate::presets::PRESETS
                    .iter()
                    .find(|p| p.label == label)
                    .map(|p| (p.shape)(&c.editor.project))
            };
            if ui
                .selectable_label(band, "Band")
                .on_hover_text("Kerb, run-off, gravel")
                .clicked()
                && !band
            {
                sp.shape = preset("Kerb").expect("preset");
                changed = true;
            }
            if ui
                .selectable_label(!band, "Wall")
                .on_hover_text("Barrier, fence")
                .clicked()
                && band
            {
                sp.shape = preset("Concrete wall").expect("preset");
                changed = true;
            }
        });
        match &mut sp.shape {
            Shape::Band {
                width,
                align,
                profile,
                surface,
                material,
                lift,
            } => {
                changed |= drag(ui, "Width m", width, 0.05, 0.0..=100.0);
                changed |= row(ui, "Lies", |ui| {
                    choice(
                        ui,
                        align,
                        &[
                            (Align::Right, "Right"),
                            (Align::Center, "Centred"),
                            (Align::Left, "Left"),
                        ],
                    )
                });
                changed |= profile_ui(ui, profile, ("spline profile", i));
                changed |= combo_row(ui, "Surface", ("sp surface", i), surface, &surfaces);
                changed |= combo_row(ui, "Material", ("sp material", i), material, &materials);
                changed |= drag(ui, "Lift m", lift, 0.005, -1.0..=1.0);
            }
            Shape::Wall {
                height,
                thickness,
                material,
                collide,
            } => {
                changed |= drag(ui, "Height m", height, 0.05, 0.05..=30.0);
                changed |= drag(ui, "Thickness m", thickness, 0.05, 0.0..=10.0);
                changed |= combo_row(ui, "Material", ("sp material", i), material, &materials);
                changed |= check(ui, collide, "Cars collide with it");
            }
        }
    });
    if changed {
        c.editor.apply(
            vec![Op::PutSpline { spline: sp }],
            Some(&format!("spline {name}")),
        );
    }
    ui.horizontal(|ui| {
        commands::button(ui, c, Cmd::Duplicate);
        if ui.button("Delete Spline").clicked() {
            c.editor.selection.nodes.clear();
            crate::viewport::delete(c.editor);
        }
    });
}

/// A prop's place, turn, size and flags; the sidebar shows them too.
pub fn prop_fields(ui: &mut egui::Ui, editor: &mut Editor, i: usize) {
    let Some(before) = editor.project.props.get(i).cloned() else {
        return;
    };
    let mut p = before.clone();
    let mut changed = vector(ui, "Location", &mut p.pos, 0.25);
    let mut deg = p.yaw.to_degrees();
    if row(ui, "Rotation Z", |ui| number(ui, &mut deg, 1.0, "°")) {
        p.yaw = deg.to_radians();
        changed = true;
    }
    changed |= drag(ui, "Scale", &mut p.scale, 0.01, 0.01..=100.0);
    changed |= row(ui, "", |ui| {
        ui.checkbox(&mut p.drape, "Stand on the ground")
            .on_hover_text("Its height comes from the road or terrain under it")
            .changed()
    });
    changed |= check(ui, &mut p.collide, "Cars collide with it");
    if changed {
        editor.apply(
            vec![Op::PutProp { prop: p }],
            Some(&format!("prop {}", before.name)),
        );
    }
}

fn prop_tab(ui: &mut egui::Ui, c: &mut Ctx, state: &mut State, library: &Library) {
    let Some(i) = c
        .editor
        .selection
        .prop()
        .filter(|&i| i < c.editor.project.props.len())
    else {
        return;
    };
    section(ui, "Prop", ("prop", i), true, |ui| {
        name_row(ui, c, state, Item::Prop(i));
        let mut p = c.editor.project.props[i].clone();
        let mut changed = false;
        row(ui, "Model", |ui| {
            egui::ComboBox::from_id_salt(("prop model", i))
                .selected_text(p.model.to_string_lossy())
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for a in library.models() {
                        changed |= ui
                            .selectable_value(
                                &mut p.model,
                                a.path.clone(),
                                a.path.to_string_lossy(),
                            )
                            .changed();
                    }
                });
        });
        if changed {
            c.editor.apply(vec![Op::PutProp { prop: p }], None);
        }
    });
    section(ui, "Transform", ("prop transform", i), true, |ui| {
        prop_fields(ui, c.editor, i);
    });
    ui.horizontal(|ui| {
        commands::button(ui, c, Cmd::Duplicate);
        if ui.button("Delete Prop").clicked() {
            crate::viewport::delete(c.editor);
        }
    });
}

fn markers_tab(ui: &mut egui::Ui, editor: &mut Editor) {
    let p = &editor.project;
    let mut m = p.markers.clone();
    let main_period = p.road(&p.main_road).map_or(1.0, |r| r.period());
    let node = editor.selection.node();
    let mut changed = false;
    ui.weak("Places on the main road are spline parameters u (node index + fraction). Drag the lines in the view, or right-click the road to put them there.");
    section(ui, "Start & sectors", "start", true, |ui| {
        changed |= drag(ui, "Start/finish u", &mut m.start, 0.01, 0.0..=main_period);
        let mut remove = None;
        for (i, s) in m.sectors.iter_mut().enumerate() {
            row(ui, &format!("Sector {}", i + 2), |ui| {
                changed |= ui
                    .add(egui::DragValue::new(s).speed(0.01).range(0.0..=main_period))
                    .changed();
                if ui.small_button("✖").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            m.sectors.remove(i);
            changed = true;
        }
        row(ui, "", |ui| {
            if ui.small_button("+ Sector").clicked() {
                m.sectors.push(node.map_or(main_period * 0.5, |n| n as f64));
                changed = true;
            }
        });
    });
    section(ui, "Grid", "grid", true, |ui| {
        let g: &mut Grid = &mut m.grid;
        let mut count = g.count as f64;
        changed |= drag(ui, "Slots", &mut count, 0.2, 0.0..=60.0);
        g.count = count as usize;
        changed |= drag(ui, "Spacing m", &mut g.spacing, 0.1, 2.0..=30.0);
        changed |= drag(ui, "Stagger m", &mut g.stagger, 0.05, 0.0..=10.0);
        changed |= drag(ui, "Pole behind m", &mut g.behind, 0.1, 0.0..=200.0);
        changed |= row(ui, "Pole side", |ui| {
            choice(
                ui,
                &mut g.pole,
                &[(Side::Left, "Left"), (Side::Right, "Right")],
            )
        });
    });
    if changed {
        editor.apply(
            vec![Op::SetMarkers {
                start: Some(m.start),
                sectors: Some(m.sectors.clone()),
                grid: Some(m.grid.clone()),
            }],
            Some("markers"),
        );
    }

    section(ui, "Pit lane", "pit", true, |ui| {
        let others: Vec<String> = editor
            .project
            .roads
            .iter()
            .filter(|r| r.name != editor.project.main_road && !r.closed)
            .map(|r| r.name.clone())
            .collect();
        let mut pit = editor.project.markers.pit.clone();
        let mut pchanged = false;
        let mut on = pit.is_some();
        if check(ui, &mut on, "Has a pit lane") {
            pit = match (on, others.first()) {
                (true, Some(road)) => Some(Pit {
                    road: road.clone(),
                    speed_limit: 80.0 / 3.6,
                    boxes: vec![],
                    box_side: Side::Right,
                    box_offset: 4.0,
                }),
                (true, None) => {
                    editor.status = "add an open road for the pit lane first".into();
                    None
                }
                (false, _) => None,
            };
            pchanged = true;
        }
        if let Some(p) = &mut pit {
            pchanged |= combo_row(ui, "Road", "pit road", &mut p.road, &others);
            let mut kmh = p.speed_limit * 3.6;
            if drag(ui, "Limit km/h", &mut kmh, 0.5, 20.0..=200.0) {
                p.speed_limit = kmh / 3.6;
                pchanged = true;
            }
            pchanged |= drag(ui, "Box offset m", &mut p.box_offset, 0.1, 0.0..=30.0);
            pchanged |= row(ui, "Boxes on the", |ui| {
                choice(
                    ui,
                    &mut p.box_side,
                    &[(Side::Left, "Left"), (Side::Right, "Right")],
                )
            });
            let period = editor.project.road(&p.road).map_or(1.0, |r| r.period());
            let mut remove = None;
            for (i, u) in p.boxes.iter_mut().enumerate() {
                row(ui, &format!("Box {}", i + 1), |ui| {
                    pchanged |= ui
                        .add(egui::DragValue::new(u).speed(0.005).range(0.0..=period))
                        .changed();
                    if ui.small_button("✖").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                p.boxes.remove(i);
                pchanged = true;
            }
            row(ui, "", |ui| {
                if ui.small_button("+ Box").clicked() {
                    let last = p.boxes.last().copied().unwrap_or(period * 0.3);
                    p.boxes.push((last + 0.05).min(period));
                    pchanged = true;
                }
                if ui.small_button("+ 10 along the lane").clicked() {
                    let (a, b) = (period * 0.3, period * 0.7);
                    p.boxes = (0..10).map(|i| a + (b - a) * i as f64 / 9.0).collect();
                    pchanged = true;
                }
            });
        }
        if pchanged {
            editor.apply(vec![Op::SetPit { pit }], Some("pit"));
        }
    });
}

fn terrain_tab(ui: &mut egui::Ui, editor: &mut Editor) {
    let (surfaces, materials) = names(&editor.project);
    let mut t = editor.project.terrain.clone();
    let mut changed = false;
    section(ui, "Terrain", "terrain", true, |ui| {
        ui.weak("Ground round the roads, just under them and meeting their outer edges.");
        changed |= check(ui, &mut t.enabled, "Enabled");
        changed |= combo_row(ui, "Surface", "terrain surface", &mut t.surface, &surfaces);
        changed |= combo_row(
            ui,
            "Material",
            "terrain material",
            &mut t.material,
            &materials,
        );
        changed |= drag(ui, "Margin m", &mut t.margin, 1.0, 0.0..=2000.0);
        changed |= drag(ui, "Cell m", &mut t.cell, 0.5, 2.0..=50.0);
    });
    if changed {
        editor.apply(vec![Op::SetTerrain { terrain: t }], Some("terrain"));
    }
}

const KINDS: [Surface; 7] = [
    Surface::Asphalt,
    Surface::Kerb,
    Surface::Runoff,
    Surface::Grass,
    Surface::Turf,
    Surface::Gravel,
    Surface::Dirt,
];

fn surfaces_tab(ui: &mut egui::Ui, editor: &mut Editor, state: &mut State) {
    ui.weak("What the tyres feel. Asphalt and kerb count as track.");
    let surfaces = editor.project.surfaces.clone();
    for (i, s) in surfaces.iter().enumerate() {
        let mut s = s.clone();
        let mut changed = false;
        section(ui, &s.name, ("surface", i), false, |ui| {
            row(ui, "Kind", |ui| {
                egui::ComboBox::from_id_salt(("kind", i))
                    .selected_text(format!("{:?}", s.props.kind))
                    .show_ui(ui, |ui| {
                        for k in KINDS {
                            changed |= ui
                                .selectable_value(&mut s.props.kind, k, format!("{k:?}"))
                                .changed();
                        }
                    });
            });
            changed |= drag(ui, "Grip ×", &mut s.props.grip, 0.005, 0.05..=1.5);
            changed |= drag(ui, "Rolling drag", &mut s.props.drag, 0.002, 0.0..=0.5);
            if row(ui, "", |ui| ui.button("Remove").clicked()) {
                let name = s.name.clone();
                editor.apply(vec![Op::RemoveSurface { name }], None);
            }
        });
        if changed {
            editor.apply(
                vec![Op::PutSurface { surface: s }],
                Some(&format!("surface {i}")),
            );
        }
    }
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.new_surface)
                .hint_text("new surface")
                .desired_width(150.0),
        );
        let name = state.new_surface.trim();
        if ui.button("+ Surface").clicked() && !name.is_empty() {
            if editor.project.surface_index(name).is_some() {
                editor.status = format!("a surface is called \"{name}\" already");
                return;
            }
            let surface = NamedSurface {
                name: name.to_string(),
                props: open_racing_sim::SurfaceProps::of(Surface::Asphalt),
            };
            if editor.apply(vec![Op::PutSurface { surface }], None) {
                state.new_surface.clear();
            }
        }
    });
}

/// A texture: none, a built-in one, or a file among the project's textures.
fn texture_picker(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    source: &mut TextureSource,
    library: &Library,
    builtins: bool,
) -> bool {
    let text = match &*source {
        TextureSource::None => "none".to_string(),
        TextureSource::Builtin(t) => format!("{t:?} (built in)"),
        TextureSource::File(p) => p.to_string_lossy().into_owned(),
    };
    let mut changed = false;
    egui::ComboBox::from_id_salt(id)
        .selected_text(text)
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            changed |= ui
                .selectable_value(source, TextureSource::None, "none")
                .changed();
            if builtins {
                for t in BuiltinTexture::ALL {
                    changed |= ui
                        .selectable_value(
                            source,
                            TextureSource::Builtin(t),
                            format!("{t:?} (built in)"),
                        )
                        .changed();
                }
            }
            for a in library.textures() {
                changed |= ui
                    .selectable_value(
                        source,
                        TextureSource::File(a.path.clone()),
                        a.path.to_string_lossy(),
                    )
                    .changed();
            }
        });
    changed
}

fn materials_tab(ui: &mut egui::Ui, editor: &mut Editor, state: &mut State, library: &Library) {
    ui.weak("Import textures under Assets below to use them here.");
    let materials = editor.project.materials.clone();
    for (i, m) in materials.iter().enumerate() {
        let mut m = m.clone();
        let mut changed = false;
        section(ui, &m.name, ("material", i), false, |ui| {
            changed |= row(ui, "Tint", |ui| {
                ui.color_edit_button_rgb(&mut m.color).changed()
            });
            changed |= row(ui, "Texture", |ui| {
                texture_picker(ui, ("tex", i), &mut m.texture, library, true)
            });
            changed |= row(ui, "Normal map", |ui| {
                texture_picker(ui, ("normal", i), &mut m.normal, library, false)
            });
            row(ui, "Alpha", |ui| {
                let mut kind = match m.alpha {
                    Alpha::Opaque => 0,
                    Alpha::Mask(_) => 1,
                    Alpha::Blend => 2,
                };
                let before = kind;
                for (k, label) in ["Opaque", "Cut out", "Blend"].iter().enumerate() {
                    ui.selectable_value(&mut kind, k, *label);
                }
                if kind != before {
                    m.alpha = [Alpha::Opaque, Alpha::Mask(0.5), Alpha::Blend][kind];
                    changed = true;
                }
                if let Alpha::Mask(c) = &mut m.alpha {
                    changed |= ui
                        .add(egui::DragValue::new(c).speed(0.01).range(0.0..=1.0))
                        .changed();
                }
            });
            row(ui, "Tile m", |ui| {
                for t in &mut m.tile {
                    changed |= ui
                        .add(egui::DragValue::new(t).speed(0.05).range(0.05..=100.0))
                        .changed();
                }
            });
            let (mut rough, mut refl) = (m.roughness as f64, m.reflectance as f64);
            if drag(ui, "Roughness", &mut rough, 0.01, 0.0..=1.0) {
                m.roughness = rough as _;
                changed = true;
            }
            if drag(ui, "Reflectance", &mut refl, 0.01, 0.0..=1.0) {
                m.reflectance = refl as _;
                changed = true;
            }
            changed |= check(ui, &mut m.double_sided, "Double sided");
            if row(ui, "", |ui| ui.button("Remove").clicked()) {
                let name = m.name.clone();
                editor.apply(vec![Op::RemoveMaterial { name }], None);
            }
        });
        if changed {
            editor.apply(
                vec![Op::PutMaterial { material: m }],
                Some(&format!("material {i}")),
            );
        }
    }
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.new_material)
                .hint_text("new material")
                .desired_width(150.0),
        );
        let name = state.new_material.trim();
        if ui.button("+ Material").clicked() && !name.is_empty() {
            if editor.project.material_index(name).is_some() {
                editor.status = format!("a material is called \"{name}\" already");
                return;
            }
            let material = MaterialDef {
                name: name.to_string(),
                color: [1.0; 3],
                texture: TextureSource::Builtin(BuiltinTexture::Concrete),
                tile: [2.0, 2.0],
                roughness: 0.8,
                reflectance: 0.5,
                double_sided: false,
                normal: TextureSource::None,
                alpha: Alpha::Opaque,
            };
            if editor.apply(vec![Op::PutMaterial { material }], None) {
                state.new_material.clear();
            }
        }
    });
}
