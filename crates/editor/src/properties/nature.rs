//! The terrain's sculpting and painted ground layers, and the scatters of models over
//! it (woods, bushes, rocks): their settings here, their strokes painted in the view.

use open_racing_track_project::ops::StrokeTarget;
use open_racing_track_project::project::{MAX_LAYERS, ScatterModel, Terrain};

use super::*;
use crate::viewport::ToolKind;

/// A button that picks a brush tool (and what it paints) to paint in the view.
fn brush_button(ui: &mut egui::Ui, c: &mut Ctx, tool: ToolKind, label: &str, tip: &str) -> bool {
    let on = c.tool.active == tool;
    let clicked = ui.selectable_label(on, label).on_hover_text(tip).clicked();
    if clicked {
        c.tool.active = tool;
    }
    clicked
}

/// The Terrain tab's sculpting: the strokes so far, and the brush.
pub(super) fn sculpt_ui(ui: &mut egui::Ui, c: &mut Ctx, t: &mut Terrain) -> bool {
    let mut changed = false;
    section(ui, "Sculpting", "terrain sculpt", true, |ui| {
        ui.weak("Raise, dig, smooth, level or roughen the ground with a brush in the view. The ground by the roads stays where it meets them.");
        row(ui, "Strokes", |ui| {
            ui.label(t.sculpt.len().to_string());
            if !t.sculpt.is_empty()
                && ui
                    .small_button("Clear")
                    .on_hover_text("The ground as it is without sculpting (Ctrl Z brings it back)")
                    .clicked()
            {
                t.sculpt.clear();
                changed = true;
            }
        });
        row(ui, "", |ui| {
            brush_button(
                ui,
                c,
                ToolKind::Sculpt,
                "🗻 Sculpt in the view",
                "The Sculpt Terrain tool: drag over the ground",
            );
        });
        if t.cell > 4.0 {
            ui.weak(format!(
                "The terrain's cells are {:.0} m: a smaller cell (Terrain above) shows finer shapes.",
                t.cell
            ));
        }
    });
    changed
}

/// The Terrain tab's painted layers: each one's name, surface and material.
pub(super) fn layers_ui(ui: &mut egui::Ui, c: &mut Ctx, t: &mut Terrain) -> bool {
    let (surfaces, materials) = names(&c.editor.project);
    let mut changed = false;
    section(ui, "Ground layers", "terrain layers", true, |ui| {
        ui.weak(format!(
            "Up to {MAX_LAYERS} materials painted over the ground's own with a brush in the view: dirt, gravel, sand. Where one covers most of the ground, cars drive on its surface."
        ));
        let mut remove = None;
        for (i, l) in t.layers.iter_mut().enumerate() {
            egui::CollapsingHeader::new(format!("{}  ·  {}", l.name, l.surface))
                .id_salt(("ground layer", i))
                .default_open(true)
                .show(ui, |ui| {
                    if let Some(name) = rename_field(ui, ("ground layer name", i), &l.name) {
                        // The strokes painting it follow the name.
                        for s in t.paint.iter_mut() {
                            if s.layer.as_deref() == Some(l.name.as_str()) {
                                s.layer = Some(name.clone());
                            }
                        }
                        if c.tool.brush.layer.as_deref() == Some(l.name.as_str()) {
                            c.tool.brush.layer = Some(name.clone());
                        }
                        l.name = name;
                        changed = true;
                    }
                    changed |= combo_row(
                        ui,
                        "Surface",
                        ("layer surface", i),
                        &mut l.surface,
                        &surfaces,
                    );
                    changed |= combo_row(
                        ui,
                        "Material",
                        ("layer material", i),
                        &mut l.material,
                        &materials,
                    );
                    let strokes = t
                        .paint
                        .iter()
                        .filter(|s| s.layer.as_deref() == Some(l.name.as_str()))
                        .count();
                    row(ui, "Strokes", |ui| ui.label(strokes.to_string()));
                    row(ui, "", |ui| {
                        if ui
                            .selectable_label(
                                c.tool.active == ToolKind::Paint
                                    && c.tool.brush.layer.as_deref() == Some(l.name.as_str()),
                                "✏ Paint",
                            )
                            .on_hover_text("Paint it in the view")
                            .clicked()
                        {
                            c.tool.active = ToolKind::Paint;
                            c.tool.brush.layer = Some(l.name.clone());
                            c.tool.brush.own = false;
                        }
                        if ui.button("Remove").clicked() {
                            remove = Some(i);
                        }
                    });
                });
        }
        if let Some(i) = remove {
            let name = t.layers.remove(i).name;
            t.paint
                .retain(|s| s.layer.as_deref() != Some(name.as_str()));
            changed = true;
        }
        if t.layers.len() < MAX_LAYERS {
            row(ui, "", |ui| {
                if ui.button("+ Layer").clicked() {
                    let name =
                        crate::presets::free_name("dirt", |n| t.layers.iter().any(|l| l.name == n));
                    let pick = |want: &str, list: &[String]| {
                        list.iter()
                            .find(|n| *n == want)
                            .or(list.first())
                            .cloned()
                            .unwrap_or_default()
                    };
                    t.layers
                        .push(open_racing_track_project::project::GroundLayer {
                            name: name.clone(),
                            surface: pick("dirt", &surfaces),
                            material: pick("dirt", &materials),
                        });
                    c.tool.brush.layer = Some(name);
                    changed = true;
                }
            });
        }
        if !t.layers.is_empty() {
            changed |= drag(ui, "Texel m", &mut t.paint_texel, 0.05, 0.1..=10.0);
            ui.weak("The size of the painted texels: smaller paints finer edges.");
            row(ui, "", |ui| {
                brush_button(
                    ui,
                    c,
                    ToolKind::Paint,
                    "✏ Paint in the view",
                    "The Paint Ground tool: drag over the ground",
                );
                if !t.paint.is_empty() && ui.small_button("Clear all painting").clicked() {
                    t.paint.clear();
                    changed = true;
                }
            });
        }
    });
    changed
}

/// The scatters: woods, bushes and rocks painted over the ground.
pub(super) fn scatter_tab(ui: &mut egui::Ui, c: &mut Ctx, library: &Library) {
    ui.weak("Models painted over the ground with the Scatter brush: woods, bushes, rocks, long grass. They keep off the roads, kerbs and ground too steep, and vary in size and turn.");
    ui.horizontal(|ui| {
        brush_button(
            ui,
            c,
            ToolKind::Scatter,
            "🌲 Scatter in the view",
            "The Scatter tool: drag over the ground to plant, Ctrl to wipe out",
        );
        ui.checkbox(&mut c.tool.overlays.scatter, "Show")
            .on_hover_text("Hide them all while working elsewhere: the view is lighter");
    });
    ui.separator();
    let project = c.editor.project.clone();
    let models = library.model_paths();
    for (k, s) in project.scatter.iter().enumerate() {
        let copies = c.built.scattered.get(k).copied().unwrap_or(0);
        let hidden = c.editor.shown.hidden_scatter.contains(&s.name);
        let header = format!(
            "{}{}  ·  {copies} standing",
            if hidden { "(hidden) " } else { "" },
            s.name
        );
        egui::CollapsingHeader::new(header)
            .id_salt(("scatter", k))
            .default_open(project.scatter.len() < 4)
            .show(ui, |ui| scatter_ui(ui, c, s, &models));
    }
    ui.menu_button("+ Scatter", |ui| {
        for (i, (name, models, ..)) in crate::brush::SCATTER_KINDS.iter().enumerate() {
            let what: Vec<&str> = models.iter().map(|m| m.0).collect();
            if ui
                .button(*name)
                .on_hover_text(format!("Built-in {}", what.join(", ")))
                .clicked()
            {
                crate::brush::add_scatter(c, i);
                ui.close();
            }
        }
    });
}

fn scatter_ui(
    ui: &mut egui::Ui,
    c: &mut Ctx,
    before: &open_racing_track_project::project::Scatter,
    models: &[std::path::PathBuf],
) {
    let mut s = before.clone();
    let mut changed = false;
    let k = &before.name;
    if let Some(to) = rename_field(ui, ("scatter name", k), &s.name) {
        let op = Op::RenameScatter {
            name: s.name.clone(),
            to: to.clone(),
        };
        if c.editor.apply(vec![op], None) && c.tool.brush.scatter.as_deref() == Some(k.as_str()) {
            c.tool.brush.scatter = Some(to);
        }
        return;
    }
    ui.label("Models");
    let mut remove = None;
    let n = s.models.len();
    for (i, m) in s.models.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt(("scatter model", k, i))
                .selected_text(crate::assets::model_name(&m.model))
                .width(120.0)
                .show_ui(ui, |ui| {
                    for path in models {
                        let label = path.to_string_lossy().into_owned();
                        changed |= ui
                            .selectable_value(&mut m.model, path.clone(), label)
                            .changed();
                    }
                });
            changed |= ui
                .add(
                    egui::DragValue::new(&mut m.weight)
                        .speed(0.05)
                        .range(0.05..=100.0)
                        .prefix("×"),
                )
                .on_hover_text("How often it is picked, against the others")
                .changed();
            if n > 1 && ui.small_button("✖").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        s.models.remove(i);
        changed = true;
    }
    if ui.small_button("+ Model").clicked() {
        s.models.push(ScatterModel {
            model: models
                .first()
                .cloned()
                .unwrap_or_else(|| open_racing_track_project::shapes::path("pine")),
            weight: 1.0,
        });
        changed = true;
    }
    changed |= drag(ui, "Spacing m", &mut s.spacing, 0.05, 0.2..=200.0);
    let [mut small, mut big] = s.scale;
    if row(ui, "Size", |ui| {
        let a = ui
            .add(
                egui::DragValue::new(&mut small)
                    .speed(0.01)
                    .range(0.05..=20.0)
                    .prefix("×"),
            )
            .changed();
        ui.label("to");
        let b = ui
            .add(
                egui::DragValue::new(&mut big)
                    .speed(0.01)
                    .range(0.05..=20.0)
                    .prefix("×"),
            )
            .changed();
        a | b
    }) {
        s.scale = [small.min(big), big.max(small)];
        changed = true;
    }
    changed |= drag(ui, "Lean", &mut s.tilt, 0.01, 0.0..=1.0);
    changed |= drag(ui, "Clearance m", &mut s.clearance, 0.1, 0.0..=200.0);
    changed |= drag(ui, "Steepest °", &mut s.max_slope, 0.5, 0.0..=90.0);
    changed |= check(ui, &mut s.collide, "Cars collide with them");
    row(ui, "Strokes", |ui| ui.label(s.strokes.len().to_string()));
    ui.horizontal(|ui| {
        let painting = c.tool.active == ToolKind::Scatter
            && c.tool.brush.scatter.as_deref() == Some(k.as_str());
        if ui
            .selectable_label(painting, "🌲 Paint")
            .on_hover_text("Paint it in the view")
            .clicked()
        {
            c.tool.active = ToolKind::Scatter;
            c.tool.brush.scatter = Some(k.clone());
        }
        let hidden = c.editor.shown.hidden_scatter.contains(k);
        if ui
            .selectable_label(!hidden, "👁")
            .on_hover_text("Show or hide it in the view")
            .clicked()
            && !c.editor.shown.hidden_scatter.remove(k)
        {
            c.editor.shown.hidden_scatter.insert(k.clone());
        }
        if !s.strokes.is_empty() && ui.small_button("Clear strokes").clicked() {
            c.editor.apply(
                vec![Op::ClearStrokes {
                    of: StrokeTarget::Scatter(k.clone()),
                }],
                None,
            );
        }
        if ui.small_button("Remove").clicked() {
            c.editor
                .apply(vec![Op::RemoveScatter { name: k.clone() }], None);
        }
    });
    if changed {
        c.editor.apply(
            vec![Op::PutScatter { scatter: s }],
            Some(&format!("scatter {k}")),
        );
    }
}
