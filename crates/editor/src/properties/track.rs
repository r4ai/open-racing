//! The track's own tabs: its name and baking, the race markers and pit lane, the
//! terrain, and the reference image to trace over.

use super::*;

pub(super) fn track_tab(ui: &mut egui::Ui, c: &mut Ctx, state: &mut State) {
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

/// Laying a new pit lane beside the main road.
pub(super) fn pit_generator(ui: &mut egui::Ui, editor: &mut Editor, state: &mut State) {
    use open_racing_track_project::pitlane::{self, Plan};
    let period = editor
        .project
        .road(&editor.project.main_road)
        .map_or(1.0, |r| r.period());
    let plan = state
        .pit_plan
        .get_or_insert_with(|| Plan::around_start(&editor.project));
    egui::CollapsingHeader::new("Lay a new pit lane")
        .id_salt("pit generator")
        .default_open(editor.project.markers.pit.is_none())
        .show(ui, |ui| {
            ui.weak("A road beside the main road that leaves it, runs past the boxes and rejoins it. Places are node numbers (u) on the main road; selected nodes of the main road set them.");
            if let (Some(r), [a, .., b]) = (
                editor.selection.road(),
                &{
                    let mut s = editor.selection.nodes.clone();
                    s.sort_unstable();
                    s
                }[..],
            ) && editor.project.roads.get(r).is_some_and(|r| r.name == editor.project.main_road)
                && ui.small_button(format!("From node {a} to node {b}")).clicked()
            {
                plan.from = *a as f64;
                plan.to = *b as f64;
            }
            drag(ui, "Leaves at u", &mut plan.from, 0.05, 0.0..=period);
            drag(ui, "Rejoins at u", &mut plan.to, 0.05, 0.0..=period);
            row(ui, "Side", |ui| {
                choice(
                    ui,
                    &mut plan.side,
                    &[(Side::Left, "Left"), (Side::Right, "Right")],
                )
            });
            drag(ui, "Gap m", &mut plan.gap, 0.1, 0.0..=100.0);
            drag(ui, "Width m", &mut plan.width, 0.1, 4.0..=30.0);
            let mut boxes = plan.boxes as f64;
            if drag(ui, "Boxes", &mut boxes, 0.2, 0.0..=60.0) {
                plan.boxes = boxes as usize;
            }
            if ui.button("Lay pit lane").clicked() {
                let name = crate::presets::unique_name(&editor.project, "pit");
                match pitlane::ops(&editor.project, &name, plan) {
                    Ok(ops) => {
                        if editor.apply(ops, None) {
                            editor.status = format!("laid pit lane \"{name}\"");
                        }
                    }
                    Err(e) => editor.status = e.to_string(),
                }
            }
        });
}

pub(super) fn markers_tab(ui: &mut egui::Ui, editor: &mut Editor, state: &mut State) {
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
    let mut paint = false;
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
        row(ui, "", |ui| {
            if ui
                .button("Paint start line & grid")
                .on_hover_text("White lines across the main road at the start line and at the front of each slot, in place of those painted before")
                .clicked()
            {
                paint = true;
            }
        });
    });
    if paint {
        let ops = open_racing_track_project::pitlane::start_and_grid(&editor.project);
        if editor.apply(ops, None) {
            editor.status = "painted the start line and grid".into();
        }
    }
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
        pit_generator(ui, editor, state);
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

pub(super) fn reference_tab(
    ui: &mut egui::Ui,
    c: &mut Ctx,
    library: &Library,
    shown: &crate::reference::Shown,
) {
    use crate::reference;
    ui.weak("Trace a real circuit: lay a satellite image or track map (PNG) under the view, line it up, scale it by a distance you know, then draw the roads over it. It is not part of the track.");
    let current = c.editor.project.reference.clone();
    // Import, or pick among the project's textures.
    let mut picked: Option<std::path::PathBuf> = None;
    ui.horizontal(|ui| {
        if ui.button("Import image…").clicked()
            && let Some(file) = rfd::FileDialog::new()
                .add_filter("images", &["png", "jpg", "jpeg", "dds"])
                .pick_file()
        {
            match open_racing_track_project::assets::import(&c.editor.dir, &file) {
                Ok(rel) => picked = Some(rel),
                Err(e) => c.editor.status = e.to_string(),
            }
        }
        let text = current
            .as_ref()
            .map_or("choose a texture".to_string(), |r| {
                r.image.to_string_lossy().into_owned()
            });
        egui::ComboBox::from_id_salt("reference image")
            .selected_text(text)
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for a in library.textures() {
                    let on = current.as_ref().is_some_and(|r| r.image == a.path);
                    if ui.selectable_label(on, a.path.to_string_lossy()).clicked() {
                        picked = Some(a.path.clone());
                    }
                }
            });
    });
    if let Some(image) = picked {
        let r = match &current {
            Some(r) => open_racing_track_project::project::Reference { image, ..r.clone() },
            None => reference::new_reference(c.editor, image),
        };
        reference::set(c.editor, Some(r), None);
        crate::viewport::look(c.orbit, crate::viewport::ViewDir::Top);
        return;
    }
    let Some(before) = current else {
        return;
    };
    if let Some(e) = shown.error() {
        ui.colored_label(egui::Color32::from_rgb(255, 110, 90), e);
    }
    let mut r = before.clone();
    let mut changed = false;
    section(ui, "Placement", "reference placement", true, |ui| {
        changed |= check(ui, &mut r.visible, "Show it");
        let mut percent = r.opacity * 100.0;
        if drag(ui, "Opacity %", &mut percent, 1.0, 0.0..=100.0) {
            r.opacity = percent / 100.0;
            changed = true;
        }
        changed |= row(ui, "Middle X", |ui| number(ui, &mut r.center.x, 1.0, " m"));
        changed |= row(ui, "Y", |ui| number(ui, &mut r.center.y, 1.0, " m"));
        changed |= drag(ui, "Width m", &mut r.width, 1.0, 1.0..=100_000.0);
        let mut deg = r.rotation.to_degrees();
        if row(ui, "Rotation", |ui| number(ui, &mut deg, 0.1, "°")) {
            r.rotation = deg.to_radians();
            changed = true;
        }
        changed |= drag(ui, "Height m", &mut r.height, 0.1, -1000.0..=10_000.0);
        if let Some(size) = shown.size() {
            row(ui, "Resolution", |ui| {
                ui.weak(format!(
                    "{} × {} px, {:.2} m per pixel",
                    size.x,
                    size.y,
                    r.width / size.x as f64
                ))
            });
        }
    });
    if changed {
        reference::set(c.editor, Some(r), Some("reference"));
    }
    section(ui, "Scale", "reference scale", true, |ui| {
        ui.weak("With the Measure tool, click both ends of something whose length you know on the image, then enter it in the sidebar (N) › Tool.");
        if ui.button("📏 Measure on the image").clicked() {
            c.tool.active = crate::viewport::ToolKind::Measure;
            c.tool.measure.clear();
            c.shell.sidebar = true;
            c.shell.sidebar_tab = crate::sidebar::Tab::Tool;
            crate::viewport::look(c.orbit, crate::viewport::ViewDir::Top);
        }
        ui.weak("Hiding the terrain (Terrain tab) while tracing keeps it from covering the image.");
    });
    if ui.button("Remove reference image").clicked() {
        reference::set(c.editor, None, None);
    }
}

pub(super) fn terrain_tab(ui: &mut egui::Ui, editor: &mut Editor) {
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
