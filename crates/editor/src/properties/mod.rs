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
                PropTab::Strips => strips_tab(ui, c, library),
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

/// A name field for something the panel edits a copy of each frame: what is typed is
/// kept until the field loses focus, and then given back if it is a new name.
pub fn rename_field(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    name: &str,
) -> Option<String> {
    let id = ui.make_persistent_id(id);
    let mut text: String = ui
        .data(|d| d.get_temp(id))
        .unwrap_or_else(|| name.to_string());
    let resp = row(ui, "Name", |ui| {
        ui.add(egui::TextEdit::singleline(&mut text).desired_width(f32::INFINITY))
    });
    if resp.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text));
        return None;
    }
    ui.data_mut(|d| d.remove::<String>(id));
    let typed = text.trim();
    (resp.lost_focus() && !typed.is_empty() && typed != name).then(|| typed.to_string())
}

/// The stretch the selected nodes span (a node's length round one alone), or the whole
/// road without any. On an open road it stops at the ends; on a closed one it may run
/// across the start.
fn stretch_round(nodes: &[usize], period: f64, closed: bool) -> Vec<Range> {
    let count = period.round() as usize + usize::from(!closed);
    crate::lay::span(nodes, count, closed).unwrap_or_default()
}

/// Where a part added now goes along the road, in words.
fn where_laid(picked: &[usize]) -> String {
    match picked {
        [] => "Along the whole road".into(),
        [n] => format!("Round node {n}"),
        _ => format!("Over the {} selected nodes", picked.len()),
    }
}

/// Stretches of the road, in spline parameters, with a button to add one over the
/// selected nodes.
fn ranges_ui(
    ui: &mut egui::Ui,
    ranges: &mut Vec<Range>,
    picked: &[usize],
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
    let label = match picked {
        [] => "+ Stretch".to_string(),
        [n] => format!("+ Stretch round node {n}"),
        _ => format!("+ Stretch over the {} selected nodes", picked.len()),
    };
    row(ui, "", |ui| {
        if ui
            .small_button(label)
            .on_hover_text("Limit it to part of the road; drag the ends in the view")
            .clicked()
        {
            let picked = if picked.is_empty() { &[0][..] } else { picked };
            ranges.extend(stretch_round(picked, period, closed));
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

/// Ready shapes for a strip's profile, of a height: (name, points across).
fn shape_presets(h: f64) -> [(&'static str, Vec<[f64; 2]>); 6] {
    let wave = |f: &dyn Fn(f64) -> f64| {
        (0..=16)
            .map(|i| i as f64 / 16.0)
            .map(|x| [x, f(x)])
            .collect()
    };
    [
        ("Step", vec![[0.0, h], [1.0, h]]),
        (
            "Step and crown",
            wave(&|x| h * (0.6 + 0.4 * (std::f64::consts::PI * x).sin())),
        ),
        (
            "Double hump",
            wave(&|x| h * (std::f64::consts::TAU * x).sin().abs()),
        ),
        (
            "Saw-tooth",
            vec![
                [0.0, 0.0],
                [0.3, h],
                [0.34, 0.0],
                [0.64, h],
                [0.68, 0.0],
                [1.0, h],
            ],
        ),
        ("Ramp up", vec![[0.0, 0.0], [0.7, h], [1.0, h]]),
        (
            "Sausage",
            wave(&|x| {
                if (0.3..=0.7).contains(&x) {
                    h * (std::f64::consts::PI * (x - 0.3) / 0.4).sin()
                } else {
                    0.0
                }
            }),
        ),
    ]
}

/// Points of a shaped profile: a drawing of it to drag them in (double-click adds one,
/// a right click removes it), the step up from the road at its inner edge, ready shapes,
/// and each point's place across (%) and height (cm).
fn shape_ui(
    ui: &mut egui::Ui,
    points: &mut Vec<[f64; 2]>,
    id: impl std::hash::Hash + std::fmt::Debug,
) -> bool {
    let mut changed = false;
    let id = ui.make_persistent_id(("shape", id));
    let height = |points: &[[f64; 2]], x: f64| Profile::Shape(points.to_vec()).height(x);
    row(ui, "", |ui| {
        let w = ui.available_width().max(80.0);
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(w, 72.0), egui::Sense::click_and_drag());
        let resp = resp.on_hover_text(
            "Its cross-section, from its inner edge (left) outwards, the road's level dotted: drag a point · double-click: add one · right click: remove one",
        );
        // The heights it spans: kept while a point is dragged, so that it follows the
        // pointer.
        let natural = points.iter().map(|p| p[1].abs()).fold(0.05, f64::max) * 1.3;
        let top: f64 = if resp.dragged() {
            ui.data(|d| d.get_temp(id)).unwrap_or(natural)
        } else {
            natural
        };
        ui.data_mut(|d| d.insert_temp(id, top));
        let span = 1.35 * top;
        let inner = rect.shrink(6.0);
        let at = |p: [f64; 2]| {
            egui::pos2(
                inner.left() + p[0] as f32 * inner.width(),
                inner.bottom() - ((p[1] + 0.35 * top) / span) as f32 * inner.height(),
            )
        };
        let from = |q: egui::Pos2| {
            [
                ((q.x - inner.left()) / inner.width()).clamp(0.0, 1.0) as f64,
                (inner.bottom() - q.y) as f64 / inner.height() as f64 * span - 0.35 * top,
            ]
        };
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        let road = at([0.0, 0.0]).y;
        for k in 0..24 {
            let x = inner.left() + inner.width() * k as f32 / 24.0;
            painter.hline(
                x..=x + 4.0,
                road,
                egui::Stroke::new(1.0, egui::Color32::from_gray(90)),
            );
        }
        // From the road's edge up any step, across, to the outer edge.
        let mut line = vec![egui::pos2(inner.left() - 6.0, road), at([0.0, 0.0])];
        line.push(at([0.0, height(points, 0.0)]));
        line.extend(points.iter().map(|&p| at(p)));
        line.push(at([1.0, height(points, 1.0)]));
        painter.add(egui::Shape::line(
            line,
            egui::Stroke::new(1.5, crate::theme::STRIP_UI),
        ));
        let nearest = |points: &[[f64; 2]], q: egui::Pos2| {
            points
                .iter()
                .enumerate()
                .map(|(i, &p)| (i, at(p).distance(q)))
                .filter(|&(_, d)| d < 9.0)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
        };
        let hover = resp.hover_pos().and_then(|q| nearest(points, q));
        let drag_id = id.with("drag");
        if resp.drag_started()
            && let Some(i) = resp.interact_pointer_pos().and_then(|q| nearest(points, q))
        {
            ui.data_mut(|d| d.insert_temp(drag_id, i));
        }
        let dragged: Option<usize> = ui.data(|d| d.get_temp(drag_id));
        if let (Some(i), Some(q)) = (dragged, resp.interact_pointer_pos())
            && resp.dragged()
            && i < points.len()
        {
            let lo = if i > 0 { points[i - 1][0] } else { 0.0 };
            let hi = points.get(i + 1).map_or(1.0, |p| p[0]);
            let [x, h] = from(q);
            points[i] = [x.clamp(lo, hi), (h * 1000.0).round() / 1000.0];
            changed = true;
        }
        if resp.drag_stopped() {
            ui.data_mut(|d| d.remove::<usize>(drag_id));
        }
        if resp.double_clicked()
            && let Some(q) = resp.interact_pointer_pos()
            && nearest(points, q).is_none()
        {
            points.push(from(q));
            changed = true;
        }
        if resp.secondary_clicked()
            && points.len() > 1
            && let Some(i) = resp.interact_pointer_pos().and_then(|q| nearest(points, q))
        {
            points.remove(i);
            changed = true;
        }
        for (i, &p) in points.iter().enumerate() {
            let lit = hover == Some(i) || dragged == Some(i);
            painter.circle_filled(
                at(p),
                if lit { 4.5 } else { 3.0 },
                if lit {
                    egui::Color32::WHITE
                } else {
                    crate::theme::STRIP_UI
                },
            );
        }
    });
    // The step up from the road at its inner edge: the height its shape starts at.
    let mut step = height(points, 0.0) * 100.0;
    if row(ui, "Step up", |ui| {
        ui.add(egui::DragValue::new(&mut step).speed(0.1).suffix(" cm"))
            .on_hover_text("How high it rises straight up from the road at its inner edge")
            .changed()
    }) {
        match points.first_mut() {
            Some(p) if p[0] <= 1e-9 => p[1] = step / 100.0,
            _ => points.insert(0, [0.0, step / 100.0]),
        }
        changed = true;
    }
    row(ui, "Ready shapes", |ui| {
        let h = points
            .iter()
            .map(|p| p[1].abs())
            .fold(0.0, f64::max)
            .max(0.03);
        ui.horizontal_wrapped(|ui| {
            for (name, shape) in shape_presets(h) {
                if ui.small_button(name).clicked() {
                    *points = shape;
                    changed = true;
                }
            }
        });
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
    if changed {
        points.sort_by(|a, b| a[0].total_cmp(&b[0]));
    }
    changed
}

/// A strip's nodes: where each is along the road, and its width and height there, with
/// a button to add one at the selected node (or the strip's middle). Their handles are
/// dragged in the view too.
fn strip_keys_ui(
    ui: &mut egui::Ui,
    strip: &mut open_racing_track_project::project::Strip,
    period: f64,
) -> bool {
    let mut changed = false;
    let mut remove = None;
    if strip.keys.is_empty() {
        row(ui, "Nodes", |ui| {
            ui.weak("none: as wide all along")
                .on_hover_text("Ctrl+click the strip in the view (edit mode) to add one, then drag it out to widen it there, Z to raise it")
        });
    }
    for (i, k) in strip.keys.iter_mut().enumerate() {
        row(ui, if i == 0 { "Nodes" } else { "" }, |ui| {
            changed |= ui
                .add(
                    egui::DragValue::new(&mut k.u)
                        .speed(0.01)
                        .range(0.0..=period)
                        .prefix("u "),
                )
                .changed();
            changed |= ui
                .add(
                    egui::DragValue::new(&mut k.width)
                        .speed(0.02)
                        .range(0.0..=200.0)
                        .suffix(" m"),
                )
                .changed();
            changed |= ui
                .add(
                    egui::DragValue::new(&mut k.height)
                        .speed(0.02)
                        .range(0.0..=20.0)
                        .prefix("×"),
                )
                .on_hover_text("Its profile's height here, times")
                .changed();
            if ui.small_button("✖").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        strip.keys.remove(i);
        changed = true;
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

/// What a model is repeated along.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Along {
    Wall,
    Strip,
}

/// A wall's or a strip's model: none (its plain shape), or one of the project's models
/// repeated along it, with the length of each copy and how it follows the line.
fn model_ui(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug + Copy,
    model: &mut Option<ModelRun>,
    library: &Library,
    along: Along,
) -> bool {
    let mut changed = false;
    let (plain, tip) = match along {
        Along::Wall => (
            "none: plain wall",
            "A glTF model repeated along the wall: +X along it, +Z up, +Y towards the road. Cars hit the plain wall. Import models under Assets.",
        ),
        Along::Strip => (
            "none: plain strip",
            "A glTF model repeated along it in place of its plain look: +X along it, +Z up, +Y towards the road, its origin at the foot of the inner edge halfway across. Cars drive on its profile. Import models under Assets.",
        ),
    };
    row(ui, "Model", |ui| {
        let text = model.as_ref().map_or(plain.to_string(), |m| {
            m.model.to_string_lossy().into_owned()
        });
        egui::ComboBox::from_id_salt(("model", id))
            .selected_text(text)
            .width(ui.available_width().max(60.0))
            .show_ui(ui, |ui| {
                if ui.selectable_label(model.is_none(), plain).clicked() {
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
            .on_hover_text(tip);
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
