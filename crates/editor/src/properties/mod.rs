//! The properties editor, as Blender's: a column of tabs, the track's settings first,
//! then the selected item's, each a stack of collapsible panels of labelled fields.

use bevy_egui::egui;
use glam::DVec3;
use open_racing_sim::Surface;
use open_racing_track_project::ops::{Curve, Op};
use open_racing_track_project::project::{
    Align, Alpha, BuiltinTexture, Grid, Landform, LandformKind, MaterialDef, ModelRun,
    NamedSurface, PaintLine, Pit, Profile, Range, Shape, Side, StripStyle, TextureSource,
    WallStyle,
};
use open_racing_track_project::{Key, Project};

use crate::assets::Library;
use crate::commands::{self, Cmd, Ctx};
use crate::edit;
use crate::state::{Editor, Item};
use crate::ui::{Focus, PropTab};

mod library;
mod object;
mod road;
mod track;

pub use track::model_button;

use library::*;
pub use object::*;
use road::*;
use track::*;

/// Width of the labels left of the fields.
const LABEL_W: f32 = 104.0;

#[derive(Default)]
pub struct State {
    new_surface: String,
    new_material: String,
    new_strip_style: String,
    new_wall_style: String,
    /// The item being renamed and its new name so far.
    rename: Option<(Item, String)>,
    /// The track's name as typed so far.
    track_name: String,
    /// The pit lane to lay, as set up so far.
    pit_plan: Option<open_racing_track_project::pitlane::Plan>,
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
        (
            PropTab::Reference,
            "🗺",
            "Reference image: a picture of the real circuit to trace",
        ),
        (
            PropTab::Library,
            "📚",
            "Library: kerb and wall types, materials and surfaces",
        ),
    ];
    match c.editor.selection.item {
        Some(Item::Road(_)) => tabs.extend([
            (PropTab::Object, "🚗", "Road"),
            (
                PropTab::Corners,
                "↩",
                "Corners: kerbs and stretches round each turn",
            ),
            (
                PropTab::Strips,
                "☰",
                "Strips: kerbs, grass, gravel and run-off beside the road",
            ),
            (PropTab::Lines, "✏", "Painted lines"),
            (PropTab::Barriers, "🚧", "Barriers along the road"),
            (
                PropTab::Rows,
                "🌲",
                "Rows: trees, cones, boards, lights, stands and garages beside the road",
            ),
        ]),
        Some(Item::Spline(_)) => tabs.push((PropTab::Object, "〰", "Kerb, wall or fence")),
        Some(Item::Prop(_)) => tabs.push((PropTab::Object, "📦", "Prop")),
        None => {}
    }
    tabs
}

pub fn show(
    ui: &mut egui::Ui,
    c: &mut Ctx,
    state: &mut State,
    library: &Library,
    reference: &crate::reference::Shown,
) {
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
        let more = crate::batch::followers(c.editor);
        if more > 0 && c.shell.tab == PropTab::Object {
            ui.colored_label(
                crate::theme::SELECTED_OTHER_UI,
                format!(
                    "Changes here are made to the {more} other selected too (not names or places)"
                ),
            );
        }
        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match c.shell.tab {
                PropTab::Track => track_tab(ui, c, state),
                PropTab::Markers => markers_tab(ui, c.editor, state, library),
                PropTab::Terrain => terrain_tab(ui, c),
                PropTab::Reference => reference_tab(ui, c, library, reference),
                PropTab::Library => library_tab(ui, c.editor, state, library),
                PropTab::Object => match c.editor.selection.item {
                    Some(Item::Road(_)) => road_tab(ui, c, state),
                    Some(Item::Spline(_)) => spline_tab(ui, c, state, library),
                    Some(Item::Prop(_)) => prop_tab(ui, c, state, library),
                    None => {}
                },
                PropTab::Corners => crate::corners::tab(ui, c, library),
                PropTab::Strips => strips_tab(ui, c),
                PropTab::Lines => lines_tab(ui, c),
                PropTab::Barriers => barriers_tab(ui, c, library),
                PropTab::Rows => rows_tab(ui, c, library),
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

/// A strip's cross-section: flat, a crown, a slope, or any shape of points across, drawn
/// beside its fields.
fn profile_ui(
    ui: &mut egui::Ui,
    p: &mut Profile,
    id: impl std::hash::Hash + std::fmt::Debug + Copy,
) -> bool {
    const KINDS: [&str; 4] = ["flat", "crown", "slope", "shape"];
    let mut changed = false;
    row(ui, "Profile", |ui| {
        let (kind, mut v) = match p {
            Profile::Flat => (0, 0.0),
            Profile::Crown(h) => (1, *h),
            Profile::Slope(d) => (2, *d),
            Profile::Shape(_) => (3, 0.0),
        };
        let mut k = kind;
        egui::ComboBox::from_id_salt(id)
            .selected_text(KINDS[k])
            .width(70.0)
            .show_ui(ui, |ui| {
                for (i, n) in KINDS.iter().enumerate() {
                    ui.selectable_value(&mut k, i, *n);
                }
            });
        if k == 1 || k == 2 {
            changed |= number(ui, &mut v, 0.005, " m");
        }
        if k != kind || changed {
            *p = match k {
                0 => Profile::Flat,
                1 => Profile::Crown(if k != kind { 0.03 } else { v }),
                2 => Profile::Slope(if k != kind { 0.2 } else { v }),
                // Starting from the shape it had.
                _ => Profile::Shape(
                    (0..=8)
                        .map(|i| {
                            let x = i as f64 / 8.0;
                            [x, p.height(x)]
                        })
                        .collect(),
                ),
            };
            changed = true;
        }
    });
    if let Profile::Shape(points) = p {
        changed |= shape_ui(ui, points, id);
    }
    changed
}

/// Points of a shaped profile: a drawing of it, and each point's place across (%) and
/// height (cm).
fn shape_ui(
    ui: &mut egui::Ui,
    points: &mut Vec<[f64; 2]>,
    id: impl std::hash::Hash + std::fmt::Debug,
) -> bool {
    let mut changed = false;
    row(ui, "", |ui| {
        let w = ui.available_width().max(80.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 44.0), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        let top = points.iter().map(|p| p[1].abs()).fold(0.05, f64::max);
        let at = |p: [f64; 2]| {
            egui::pos2(
                rect.left() + 4.0 + p[0] as f32 * (rect.width() - 8.0),
                rect.center().y + 6.0 - (p[1] / top) as f32 * (0.5 * rect.height() - 8.0),
            )
        };
        let line: Vec<egui::Pos2> = points.iter().map(|&p| at(p)).collect();
        painter.add(egui::Shape::line(
            line,
            egui::Stroke::new(1.5, crate::theme::STRIP_UI),
        ));
    });
    let mut remove = None;
    let n = points.len();
    for (i, pt) in points.iter_mut().enumerate() {
        row(ui, if i == 0 { "Points" } else { "" }, |ui| {
            let mut x = pt[0] * 100.0;
            let mut h = pt[1] * 100.0;
            if ui
                .add(
                    egui::DragValue::new(&mut x)
                        .speed(0.5)
                        .range(0.0..=100.0)
                        .suffix(" %"),
                )
                .changed()
            {
                pt[0] = x / 100.0;
                changed = true;
            }
            if ui
                .add(egui::DragValue::new(&mut h).speed(0.1).suffix(" cm"))
                .changed()
            {
                pt[1] = h / 100.0;
                changed = true;
            }
            if n > 1 && ui.small_button("✖").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        points.remove(i);
        changed = true;
    }
    row(ui, "", |ui| {
        if ui
            .small_button("+ Point")
            .on_hover_text("Halfway along the widest gap")
            .clicked()
        {
            let i = (1..points.len())
                .max_by(|&a, &b| {
                    let gap = |i: usize| points[i][0] - points[i - 1][0];
                    gap(a).total_cmp(&gap(b))
                })
                .unwrap_or(1);
            let (a, b) = (
                points[i - 1],
                points.get(i).copied().unwrap_or([1.0, points[i - 1][1]]),
            );
            points.insert(i, [0.5 * (a[0] + b[0]), 0.5 * (a[1] + b[1])]);
            changed = true;
        }
    });
    let _ = id;
    if changed {
        points.sort_by(|a, b| a[0].total_cmp(&b[0]));
    }
    changed
}

/// A choice of one of `options` by name, or none (`none` says what that means).
fn style_combo(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    value: &mut Option<String>,
    options: &[String],
    none: &str,
) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id)
        .selected_text(value.as_deref().unwrap_or(none))
        .width(ui.available_width().max(60.0))
        .show_ui(ui, |ui| {
            changed |= ui.selectable_value(value, None, none).changed();
            for o in options {
                changed |= ui.selectable_value(value, Some(o.clone()), o).changed();
            }
        });
    changed
}

/// A wall's model: none (its plain shape), or one of the project's models repeated
/// along it, with the length of each copy and how it follows the line.
fn model_ui(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug + Copy,
    model: &mut Option<ModelRun>,
    library: &Library,
) -> bool {
    let mut changed = false;
    row(ui, "Model", |ui| {
        let text = model.as_ref().map_or("none: plain wall".to_string(), |m| {
            m.model.to_string_lossy().into_owned()
        });
        egui::ComboBox::from_id_salt(("model", id))
            .selected_text(text)
            .width(ui.available_width().max(60.0))
            .show_ui(ui, |ui| {
                if ui.selectable_label(model.is_none(), "none: plain wall").clicked() {
                    *model = None;
                    changed = true;
                }
                for a in library.models() {
                    let on = model.as_ref().is_some_and(|m| m.model == a.path);
                    if ui.selectable_label(on, a.path.to_string_lossy()).clicked() && !on {
                        *model = Some(ModelRun {
                            model: a.path.clone(),
                            length: 0.0,
                            bend: true,
                            flip: false,
                        });
                        changed = true;
                    }
                }
            })
            .response
            .on_hover_text(
                "A glTF model repeated along the wall: +X along it, +Z up, +Y towards the road. Import models under Assets.",
            );
    });
    if let Some(m) = model {
        changed |= row(ui, "Copy length", |ui| {
            let mut own = m.length == 0.0;
            let mut c = ui.checkbox(&mut own, "the model's own").changed();
            if c {
                m.length = if own { 0.0 } else { 4.0 };
            }
            if !own {
                c |= ui
                    .add(
                        egui::DragValue::new(&mut m.length)
                            .speed(0.05)
                            .range(0.1..=100.0)
                            .suffix(" m"),
                    )
                    .changed();
            }
            c
        });
        changed |= row(ui, "", |ui| {
            ui.checkbox(&mut m.bend, "Bend along the line")
                .on_hover_text("Each copy follows the curve; else each stands straight")
                .changed()
                | ui.checkbox(&mut m.flip, "Turn round").changed()
        });
    }
    changed
}

/// The names of the project's strip and wall types.
fn style_names(project: &Project) -> (Vec<String>, Vec<String>) {
    (
        project
            .strip_styles
            .iter()
            .map(|s| s.name.clone())
            .collect(),
        project.wall_styles.iter().map(|s| s.name.clone()).collect(),
    )
}

/// Opens the panel the outliner pointed at, once.
fn focused(c: &mut Ctx, focus: Focus) -> Option<bool> {
    (c.shell.focus == Some(focus)).then(|| {
        c.shell.focus = None;
        true
    })
}
