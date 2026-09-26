//! A kerb's, wall's or fence's own spline, and a prop.

use super::*;

pub(super) fn spline_tab(ui: &mut egui::Ui, c: &mut Ctx, state: &mut State, library: &Library) {
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
    let (strip_styles, wall_styles) = style_names(&c.editor.project);
    let mut sp = before.clone();
    let mut changed = false;
    section(ui, "Spline", ("spline", i), true, |ui| {
        name_row(ui, c, state, Item::Spline(i));
        collection_row(ui, c, before.group.as_deref());
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
        // Its type: any strip type makes it a band, any wall type a wall.
        let band = matches!(sp.shape, Shape::Band { .. });
        let mut style = sp.style.clone();
        let options: Vec<String> = strip_styles
            .iter()
            .map(|s| format!("{s} (band)"))
            .chain(wall_styles.iter().map(|s| format!("{s} (wall)")))
            .collect();
        let mut shown = style
            .as_ref()
            .map(|s| format!("{s} ({})", if band { "band" } else { "wall" }));
        if row(ui, "Type", |ui| {
            style_combo(ui, ("sptype", i), &mut shown, &options, "custom")
        }) {
            style = shown.as_ref().map(|s| {
                s.trim_end_matches(" (band)")
                    .trim_end_matches(" (wall)")
                    .to_string()
            });
            let preset = match &shown {
                Some(s) if s.ends_with(" (wall)") => {
                    style.clone().map(crate::presets::Preset::Wall)
                }
                Some(_) => style.clone().map(crate::presets::Preset::Strip),
                None => None,
            };
            match preset.and_then(|p| p.shape(&c.editor.project)) {
                Some((shape, _)) => {
                    // A band keeps its width and side when it changes type.
                    sp.shape = match (&sp.shape, shape) {
                        (
                            Shape::Band {
                                width, align, lift, ..
                            },
                            Shape::Band {
                                profile,
                                surface,
                                material,
                                ..
                            },
                        ) => Shape::Band {
                            width: *width,
                            align: *align,
                            profile,
                            surface,
                            material,
                            lift: *lift,
                        },
                        (_, shape) => shape,
                    };
                    sp.style = style;
                }
                None => sp.style = None,
            }
            changed = true;
        }
        // Its own shape and look: changing any of it leaves its type.
        let look = sp.shape.clone();
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
                profile_ui(ui, profile, ("spline profile", i));
                combo_row(ui, "Surface", ("sp surface", i), surface, &surfaces);
                combo_row(ui, "Material", ("sp material", i), material, &materials);
                changed |= drag(ui, "Lift m", lift, 0.005, -1.0..=1.0);
            }
            Shape::Wall {
                height,
                thickness,
                material,
                collide,
                model,
            } => {
                drag(ui, "Height m", height, 0.05, 0.05..=30.0);
                drag(ui, "Thickness m", thickness, 0.05, 0.0..=10.0);
                combo_row(ui, "Material", ("sp material", i), material, &materials);
                model_ui(ui, ("spline", i), model, library);
                changed |= check(ui, collide, "Cars collide with it");
            }
        }
        let own = |s: &Shape| match s {
            Shape::Band {
                profile,
                surface,
                material,
                ..
            } => format!("{profile:?} {surface} {material}"),
            Shape::Wall {
                height,
                thickness,
                material,
                model,
                ..
            } => format!("{height} {thickness} {material} {model:?}"),
        };
        if own(&look) != own(&sp.shape) {
            sp.style = None;
            changed = true;
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

/// The collection an item is kept in, and a button to move it (and the rest of the
/// selection) to another.
fn collection_row(ui: &mut egui::Ui, c: &mut Ctx, group: Option<&str>) {
    row(ui, "Collection", |ui| {
        match group {
            Some(g) => ui.label(format!("🗀 {g}")),
            None => ui.weak("none"),
        };
        commands::button_as(ui, c, Cmd::MoveToCollection, "Move…");
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

pub(super) fn prop_tab(ui: &mut egui::Ui, c: &mut Ctx, state: &mut State, library: &Library) {
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
        let group = c.editor.project.props[i].group.clone();
        collection_row(ui, c, group.as_deref());
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
