//! The Library: the kinds of kerbs, gravel and verges, walls and fences, and the
//! materials and surfaces everything is made of, each made once and used anywhere.
//! Changing a type changes everything made from it.

use super::*;

pub(super) fn library_tab(
    ui: &mut egui::Ui,
    editor: &mut Editor,
    state: &mut State,
    library: &Library,
) {
    ui.weak(
        "Made once, used anywhere: changing a type changes every kerb, strip or wall made from it.",
    );
    section(ui, "Kerb & strip types", "strip types", true, |ui| {
        strip_types(ui, editor, state)
    });
    section(ui, "Wall types", "wall types", true, |ui| {
        wall_types(ui, editor, state, library)
    });
    section(ui, "Materials", "materials", false, |ui| {
        materials(ui, editor, state, library)
    });
    section(ui, "Surfaces", "surfaces", false, |ui| {
        surfaces(ui, editor, state)
    });
}

/// How many strips, barriers and splines use the type called `name`.
fn uses(project: &Project, name: &str) -> usize {
    let roads = project.roads.iter().flat_map(|r| {
        r.left
            .iter()
            .chain(&r.right)
            .map(|s| &s.style)
            .chain(r.barriers.iter().map(|b| &b.style))
    });
    roads
        .chain(project.splines.iter().map(|s| &s.style))
        .filter(|s| s.as_deref() == Some(name))
        .count()
}

/// A field for a new entry's name and a button to add it; the name when clicked.
fn add_row(ui: &mut egui::Ui, text: &mut String, hint: &str, button: &str) -> Option<String> {
    let mut added = None;
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(text)
                .hint_text(hint)
                .desired_width(150.0),
        );
        let name = text.trim();
        if ui.button(button).clicked() && !name.is_empty() {
            added = Some(name.to_string());
        }
    });
    added
}

fn strip_types(ui: &mut egui::Ui, editor: &mut Editor, state: &mut State) {
    let (surfaces, materials) = names(&editor.project);
    let styles = editor.project.strip_styles.clone();
    for (i, before) in styles.iter().enumerate() {
        let mut s = before.clone();
        let mut remove = false;
        let used = uses(&editor.project, &s.name);
        let title = format!("{}  ·  {:.1} m  ·  used {used}×", s.name, s.width);
        egui::CollapsingHeader::new(title)
            .id_salt(("strip type", i))
            .show(ui, |ui| {
                drag(ui, "Width m", &mut s.width, 0.05, 0.05..=200.0);
                profile_ui(ui, &mut s.profile, ("strip type profile", i));
                combo_row(ui, "Surface", ("sts", i), &mut s.surface, &surfaces);
                combo_row(ui, "Material", ("stm", i), &mut s.material, &materials);
                drag(ui, "Fade m", &mut s.fade, 0.1, 0.0..=100.0);
                ui.weak("New strips start at its width; each keeps its own.");
                remove = row(ui, "", |ui| {
                    ui.button("Remove type")
                        .on_hover_text("What was made from it keeps its look")
                        .clicked()
                });
            });
        if remove {
            let name = s.name.clone();
            editor.apply(vec![Op::RemoveStripStyle { name }], None);
            return;
        }
        if s != *before {
            editor.apply(
                vec![Op::PutStripStyle { style: s }],
                Some(&format!("strip type {i}")),
            );
        }
    }
    if let Some(name) = add_row(
        ui,
        &mut state.new_strip_style,
        "new kerb or strip type",
        "+ Type",
    ) {
        if editor.project.strip_style(&name).is_some() {
            editor.status = format!("a strip type is called \"{name}\" already");
            return;
        }
        // Starting from the plain kerb.
        let mut style = editor
            .project
            .strip_style("kerb")
            .or(editor.project.strip_styles.first())
            .cloned()
            .unwrap_or(StripStyle {
                name: String::new(),
                width: 1.2,
                profile: Profile::Crown(0.03),
                surface: surfaces.first().cloned().unwrap_or_default(),
                material: materials.first().cloned().unwrap_or_default(),
                fade: 2.0,
            });
        style.name = name;
        if editor.apply(vec![Op::PutStripStyle { style }], None) {
            state.new_strip_style.clear();
        }
    }
}

fn wall_types(ui: &mut egui::Ui, editor: &mut Editor, state: &mut State, library: &Library) {
    let (_, materials) = names(&editor.project);
    let styles = editor.project.wall_styles.clone();
    for (i, before) in styles.iter().enumerate() {
        let mut w = before.clone();
        let mut remove = false;
        let used = uses(&editor.project, &w.name);
        let what = if w.model.is_some() { "model" } else { "plain" };
        let title = format!("{}  ·  {:.1} m, {what}  ·  used {used}×", w.name, w.height);
        egui::CollapsingHeader::new(title)
            .id_salt(("wall type", i))
            .show(ui, |ui| {
                drag(ui, "Height m", &mut w.height, 0.05, 0.05..=30.0);
                drag(ui, "Thickness m", &mut w.thickness, 0.05, 0.0..=10.0);
                combo_row(ui, "Material", ("wtm", i), &mut w.material, &materials);
                model_ui(ui, ("wall type", i), &mut w.model, library);
                if w.model.is_some() {
                    ui.weak("Cars hit a plain wall this high and thick; the model is what shows.");
                }
                remove = row(ui, "", |ui| {
                    ui.button("Remove type")
                        .on_hover_text("What was made from it keeps its look")
                        .clicked()
                });
            });
        if remove {
            let name = w.name.clone();
            editor.apply(vec![Op::RemoveWallStyle { name }], None);
            return;
        }
        if w != *before {
            editor.apply(
                vec![Op::PutWallStyle { style: w }],
                Some(&format!("wall type {i}")),
            );
        }
    }
    if let Some(name) = add_row(ui, &mut state.new_wall_style, "new wall type", "+ Type") {
        if editor.project.wall_style(&name).is_some() {
            editor.status = format!("a wall type is called \"{name}\" already");
            return;
        }
        let material = if editor.project.material_index("concrete").is_some() {
            "concrete".to_string()
        } else {
            materials.first().cloned().unwrap_or_default()
        };
        let style = WallStyle {
            name,
            height: 1.0,
            thickness: 0.5,
            material,
            model: None,
        };
        if editor.apply(vec![Op::PutWallStyle { style }], None) {
            state.new_wall_style.clear();
        }
    }
}

pub(super) const KINDS: [Surface; 7] = [
    Surface::Asphalt,
    Surface::Kerb,
    Surface::Runoff,
    Surface::Grass,
    Surface::Turf,
    Surface::Gravel,
    Surface::Dirt,
];

fn surfaces(ui: &mut egui::Ui, editor: &mut Editor, state: &mut State) {
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
pub(super) fn texture_picker(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    source: &mut TextureSource,
    library: &Library,
    builtins: bool,
    dir: &std::path::Path,
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
            ui.separator();
            if ui.button("Import image…").clicked()
                && let Some(file) = rfd::FileDialog::new()
                    .add_filter("images", &["png", "jpg", "jpeg", "dds"])
                    .pick_file()
            {
                // Into the project's assets, where the asset list finds it.
                if let Ok(rel) = open_racing_track_project::assets::import(dir, &file) {
                    *source = TextureSource::File(rel);
                    changed = true;
                }
            }
        });
    changed
}

fn materials(ui: &mut egui::Ui, editor: &mut Editor, state: &mut State, library: &Library) {
    ui.weak("PNG, JPEG or DDS textures: import one from a texture list.");
    let materials = editor.project.materials.clone();
    for (i, m) in materials.iter().enumerate() {
        let mut m = m.clone();
        let mut changed = false;
        section(ui, &m.name, ("material", i), false, |ui| {
            changed |= row(ui, "Tint", |ui| {
                ui.color_edit_button_rgb(&mut m.color).changed()
            });
            changed |= row(ui, "Texture", |ui| {
                texture_picker(ui, ("tex", i), &mut m.texture, library, true, &editor.dir)
            });
            changed |= row(ui, "Normal map", |ui| {
                texture_picker(
                    ui,
                    ("normal", i),
                    &mut m.normal,
                    library,
                    false,
                    &editor.dir,
                )
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
