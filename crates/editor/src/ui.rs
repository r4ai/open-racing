//! The panels round the 3D view: menus, the outliner, the inspector of what is selected,
//! and the road's elevation profile. Every change is an operation applied to the editor.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::{EguiContexts, egui};
use glam::DVec3;
use open_racing_sim::Surface;
use open_racing_track_project::ops::{Curve, Op};
use open_racing_track_project::project::{
    Align, Alpha, Barrier, BuiltinTexture, Grid, MaterialDef, NamedSurface, PaintLine, Pit,
    Profile, Range, Shape, Side, Spline, Strip, TextureSource,
};
use open_racing_track_project::{Project, projects_dir};

use crate::assets::{self, Library};
use crate::jobs::Jobs;
use crate::menus;
use crate::preview::Built;
use crate::preview::Props;
use crate::profile::{ProfileView, profile};
use crate::state::{Editor, Item};
use crate::viewport::{Orbit, Tool, ViewRect, frame_selection};

/// What the inspector shows.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Road,
    Markers,
    Terrain,
    Surfaces,
    Materials,
    Assets,
}

#[derive(Default)]
pub struct UiState {
    tab: Tab,
    new_project: String,
    new_surface: String,
    new_material: String,
    rename: Option<(Item, String)>,
    profile: ProfileView,
    /// The inspector is hidden (N).
    sidebar_hidden: bool,
    assets: assets::Panel,
}

#[allow(clippy::too_many_arguments)]
pub fn ui(
    mut contexts: EguiContexts,
    mut editor: ResMut<Editor>,
    mut jobs: ResMut<Jobs>,
    mut orbit: ResMut<Orbit>,
    mut rect: ResMut<ViewRect>,
    mut tool: ResMut<Tool>,
    built: Res<Built>,
    mut library: ResMut<Library>,
    props: Res<Props>,
    mut state: Local<UiState>,
    window: Single<&Window, With<PrimaryWindow>>,
) -> Result {
    let ctx = contexts.ctx_mut()?.clone();
    // The window starts out tiny on some systems; the panels need room.
    if ctx.viewport_rect().width() < 800.0 || ctx.viewport_rect().height() < 500.0 {
        rect.0 = None;
        return Ok(());
    }
    let mut root = egui::Ui::new(
        ctx.clone(),
        "root".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );
    let editor = &mut *editor;

    // Shortcuts.
    let (undo, redo) = ctx.input(|i| {
        let z = i.modifiers.command && i.key_pressed(egui::Key::Z);
        (
            z && !i.modifiers.shift,
            (z && i.modifiers.shift) || (i.modifiers.command && i.key_pressed(egui::Key::Y)),
        )
    });
    if undo && !ctx.egui_wants_keyboard_input() && tool.modal.is_none() {
        editor.undo();
    }
    if redo && !ctx.egui_wants_keyboard_input() && tool.modal.is_none() {
        editor.redo();
    }
    if ctx.input(|i| i.key_pressed(egui::Key::N) && i.modifiers.is_none())
        && !ctx.egui_wants_keyboard_input()
        && tool.modal.is_none()
    {
        state.sidebar_hidden = !state.sidebar_hidden;
    }

    egui::Panel::top("menu").show(&mut root, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("Project", |ui| {
                ui.label(format!("{}", editor.dir.display()));
                ui.separator();
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut state.new_project);
                    if ui.button("New / open").clicked() && !state.new_project.is_empty() {
                        let dir = projects_dir().join(state.new_project.trim());
                        editor.switch(dir);
                        ui.close();
                    }
                });
                ui.separator();
                for dir in list_projects() {
                    let name = dir
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    if ui.button(name).clicked() {
                        editor.switch(dir);
                        ui.close();
                    }
                }
            });
            ui.menu_button("Edit", |ui| {
                if ui
                    .add_enabled(editor.can_undo(), egui::Button::new("Undo (Ctrl+Z)"))
                    .clicked()
                {
                    editor.undo();
                }
                if ui
                    .add_enabled(editor.can_redo(), egui::Button::new("Redo (Ctrl+Y)"))
                    .clicked()
                {
                    editor.redo();
                }
            });
            ui.separator();
            if ui
                .add_enabled(!jobs.running, egui::Button::new("Bake"))
                .clicked()
            {
                jobs.bake(&editor.project, &editor.dir, false);
            }
            if ui
                .add_enabled(!jobs.running, egui::Button::new("▶ Bake & drive"))
                .clicked()
            {
                jobs.bake(&editor.project, &editor.dir, true);
            }
            if ui.button("Frame (F)").clicked() {
                frame_selection(editor, &mut orbit);
            }
            ui.separator();
            ui.label(&editor.status);
        });
    });

    egui::Panel::left("outliner")
        .resizable(true)
        .default_size(200.0)
        .show(&mut root, |ui| {
            outliner(ui, editor, &mut state);
        });

    let mut sidebar = !state.sidebar_hidden;
    egui::Panel::right("inspector")
        .resizable(true)
        .default_size(340.0)
        .show_collapsible(&mut root, &mut sidebar, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| match state.tab {
                Tab::Road => match editor.selection.item {
                    Some(Item::Spline(_)) => spline_inspector(ui, editor, &mut state),
                    Some(Item::Prop(_)) => prop_inspector(ui, editor, &library),
                    _ => road_inspector(ui, editor, &mut state),
                },
                Tab::Markers => markers_inspector(ui, editor),
                Tab::Terrain => terrain_inspector(ui, editor),
                Tab::Surfaces => surfaces_inspector(ui, editor, &mut state),
                Tab::Materials => materials_inspector(ui, editor, &mut state, &library),
                Tab::Assets => assets::panel(
                    ui,
                    editor,
                    &mut library,
                    &props,
                    &mut tool,
                    &mut state.assets,
                ),
            });
        });

    egui::Panel::bottom("profile")
        .resizable(true)
        .default_size(190.0)
        .show(&mut root, |ui| {
            ui.columns(2, |cols| {
                profile(&mut cols[0], editor, &mut state.profile);
                egui::ScrollArea::vertical()
                    .id_salt("report")
                    .show(&mut cols[1], |ui| {
                        ui.strong("Bake report");
                        match &jobs.report {
                            Some(r) => ui.monospace(r),
                            None => ui.label("Bake to check the track and drive a test lap."),
                        };
                    });
            });
        });

    menus::header(&mut root, editor, &mut tool, &mut orbit);

    // What is left is the 3D view.
    let free = root.available_rect_before_wrap();
    let _ = window;
    rect.0 = Some(Rect::new(free.min.x, free.min.y, free.max.x, free.max.y));
    menus::overlay(&ctx, editor, &mut tool, &built, &mut orbit);
    Ok(())
}

fn list_projects() -> Vec<std::path::PathBuf> {
    let mut dirs: Vec<_> = std::fs::read_dir(projects_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join(open_racing_track_project::PROJECT_FILE).is_file())
        .collect();
    dirs.sort();
    dirs
}

fn outliner(ui: &mut egui::Ui, editor: &mut Editor, state: &mut UiState) {
    ui.heading(&editor.project.name);
    ui.separator();
    ui.strong("Roads");
    let main = editor.project.main_road.clone();
    for (i, road) in editor.project.roads.iter().enumerate() {
        let label = if road.name == main {
            format!("★ {}", road.name)
        } else {
            road.name.clone()
        };
        let selected = state.tab == Tab::Road && editor.selection.road() == Some(i);
        if ui.selectable_label(selected, label).clicked() {
            editor.selection.select(Item::Road(i));
            state.tab = Tab::Road;
        }
    }
    ui.horizontal(|ui| {
        if ui.button("+ road").clicked() {
            add_road(editor);
            state.tab = Tab::Road;
        }
        if let Some(name) = editor.road_name()
            && name != main
            && ui.button("remove").clicked()
        {
            editor.apply(vec![Op::RemoveRoad { road: name }], None);
            editor.selection.select(Item::Road(0));
        }
    });
    ui.separator();
    ui.strong("Kerbs, walls, fences");
    for (i, sp) in editor.project.splines.iter().enumerate() {
        let selected = state.tab == Tab::Road && editor.selection.spline() == Some(i);
        if ui.selectable_label(selected, &sp.name).clicked() {
            editor.selection.select(Item::Spline(i));
            state.tab = Tab::Road;
        }
    }
    if editor.project.splines.is_empty() {
        ui.weak("Shift+A in the view to draw one.");
    }
    ui.separator();
    ui.strong("Props");
    for (i, p) in editor.project.props.iter().enumerate() {
        let selected = state.tab == Tab::Road && editor.selection.prop() == Some(i);
        if ui.selectable_label(selected, &p.name).clicked() {
            editor.selection.select(Item::Prop(i));
            state.tab = Tab::Road;
        }
    }
    if editor.project.props.is_empty() {
        ui.weak("Place models from Assets.");
    }
    ui.separator();
    for (tab, name) in [
        (Tab::Markers, "Race markers"),
        (Tab::Terrain, "Terrain"),
        (Tab::Surfaces, "Surfaces"),
        (Tab::Materials, "Materials"),
        (Tab::Assets, "Assets"),
    ] {
        if ui.selectable_label(state.tab == tab, name).clicked() {
            state.tab = tab;
        }
    }
}

/// Adds a short straight road beside the selected one's selected node, like it.
fn add_road(editor: &mut Editor) {
    let base = editor
        .selection
        .road()
        .and_then(|r| editor.project.roads.get(r))
        .and_then(|r| r.nodes.get(editor.selection.node().unwrap_or(0)))
        .map_or(DVec3::ZERO, |n| n.pos);
    let mut k = editor.project.roads.len();
    let name = loop {
        let n = format!("road {k}");
        if editor.project.road(&n).is_none() {
            break n;
        }
        k += 1;
    };
    let offset = DVec3::new(0.0, -30.0, 0.0);
    let ops = vec![Op::AddRoad {
        name,
        closed: false,
        nodes: vec![base + offset, base + offset + DVec3::X * 100.0],
        like: None,
    }];
    if editor.apply(ops, None) {
        editor
            .selection
            .select(Item::Road(editor.project.roads.len() - 1));
    }
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
        .show_ui(ui, |ui| {
            for o in options {
                changed |= ui.selectable_value(value, o.clone(), o).changed();
            }
        });
    changed
}

fn names(project: &Project) -> (Vec<String>, Vec<String>) {
    (
        project.surfaces.iter().map(|s| s.name.clone()).collect(),
        project.materials.iter().map(|m| m.name.clone()).collect(),
    )
}

fn drag(
    ui: &mut egui::Ui,
    label: &str,
    v: &mut f64,
    speed: f64,
    range: std::ops::RangeInclusive<f64>,
) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::DragValue::new(v).speed(speed).range(range))
            .changed()
    })
    .inner
}

/// Stretches of the road, in spline parameters, with buttons to add one round the
/// selected node.
fn ranges_ui(ui: &mut egui::Ui, ranges: &mut Vec<Range>, node: Option<usize>, period: f64) -> bool {
    let mut changed = false;
    let mut remove = None;
    if ranges.is_empty() {
        ui.small("everywhere");
    }
    for (i, r) in ranges.iter_mut().enumerate() {
        ui.horizontal(|ui| {
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
            if ui.small_button("✕").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        ranges.remove(i);
        changed = true;
    }
    let label = match node {
        Some(n) => format!("+ stretch round node {n}"),
        None => "+ stretch".into(),
    };
    if ui.small_button(label).clicked() {
        let u = node.map_or(0.0, |n| n as f64);
        ranges.push(Range {
            from: (u - 0.5).rem_euclid(period.max(1.0)),
            to: (u + 0.5).min(period),
        });
        changed = true;
    }
    changed
}

fn road_inspector(ui: &mut egui::Ui, editor: &mut Editor, state: &mut UiState) {
    let Some(r) = editor
        .selection
        .road()
        .filter(|&r| r < editor.project.roads.len())
    else {
        ui.label("Select a road.");
        return;
    };
    let road = editor.project.roads[r].clone();
    let name = road.name.clone();
    let (surfaces, materials) = names(&editor.project);
    let node = editor.selection.node();
    let period = road.period();

    // Name.
    let text = rename_text(state, Item::Road(r), &name);
    let resp = ui.horizontal(|ui| {
        ui.label("Road");
        ui.text_edit_singleline(text)
    });
    if resp.inner.lost_focus() && *text != name && !text.is_empty() {
        let to = text.clone();
        editor.apply(
            vec![Op::RenameRoad {
                road: name.clone(),
                to,
            }],
            None,
        );
        return;
    }
    if name != editor.project.main_road && ui.button("Make this the main road").clicked() {
        editor.apply(vec![Op::SetMainRoad { road: name.clone() }], None);
    }

    // Properties.
    let (mut closed, mut crown, mut resolution) = (road.closed, road.crown, road.resolution);
    let (mut surface, mut material) = (road.surface.clone(), road.material.clone());
    let mut changed = ui.checkbox(&mut closed, "closed loop").changed();
    changed |= drag(ui, "crown m", &mut crown, 0.005, -0.5..=0.5);
    changed |= drag(ui, "resolution m", &mut resolution, 0.1, 0.25..=10.0);
    ui.horizontal(|ui| {
        ui.label("surface");
        changed |= combo(ui, ("road surface", r), &mut surface, &surfaces);
        ui.label("material");
        changed |= combo(ui, ("road material", r), &mut material, &materials);
    });
    if changed {
        editor.apply(
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

    ui.separator();
    node_ui(ui, editor, &name, &road.nodes, road.closed);

    // Profiles.
    ui.separator();
    for (curve, title, c, scale) in [
        (Curve::WidthLeft, "Width left (m)", &road.width_left, 1.0),
        (Curve::WidthRight, "Width right (m)", &road.width_right, 1.0),
        (
            Curve::Bank,
            "Bank (°)",
            &road.bank,
            180.0 / std::f64::consts::PI,
        ),
    ] {
        egui::CollapsingHeader::new(title)
            .id_salt((title, r))
            .show(ui, |ui| {
                let mut keys = c.keys.clone();
                let mut changed = false;
                let mut remove = None;
                for (i, k) in keys.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label("u");
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
                        if keys_len_gt_one(c.keys.len()) && ui.small_button("✕").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    keys.remove(i);
                    changed = true;
                }
                if let Some(n) = node
                    && ui.small_button(format!("+ key at node {n}")).clicked()
                {
                    let v = c.eval(n as f64, period, road.closed);
                    keys.push(open_racing_track_project::Key {
                        u: n as f64,
                        value: v,
                    });
                    changed = true;
                }
                if changed {
                    editor.apply(
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

    // Strips.
    for side in [Side::Left, Side::Right] {
        ui.separator();
        ui.strong(match side {
            Side::Left => "Strips left (outwards)",
            Side::Right => "Strips right (outwards)",
        });
        let strips = road.strips(side).clone();
        for (i, strip) in strips.iter().enumerate() {
            let mut s = strip.clone();
            let mut changed = false;
            let mut removed = false;
            egui::CollapsingHeader::new(format!("{} — {:.1} m {}", s.name, s.width, s.surface))
                .id_salt(("strip", r, side as u8, i))
                .show(ui, |ui| {
                    changed |= drag(ui, "width m", &mut s.width, 0.05, 0.0..=200.0);
                    ui.horizontal(|ui| {
                        changed |= combo(ui, ("ss", r, side as u8, i), &mut s.surface, &surfaces);
                        changed |= combo(ui, ("sm", r, side as u8, i), &mut s.material, &materials);
                    });
                    changed |= profile_ui(ui, &mut s.profile, (r, side as u8, i));
                    changed |= drag(ui, "fade m", &mut s.fade, 0.1, 0.0..=100.0);
                    changed |= ranges_ui(ui, &mut s.ranges, node, period);
                    removed = ui.button("Remove strip").clicked();
                });
            if removed {
                editor.apply(
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
                editor.apply(
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
        ui.horizontal(|ui| {
            for (label, surface, material, width, profile) in [
                ("+ kerb", "kerb", "kerb", 1.2, Profile::Crown(0.03)),
                ("+ grass", "grass", "grass", 10.0, Profile::Slope(0.2)),
                ("+ gravel", "gravel", "gravel", 15.0, Profile::Slope(0.3)),
                ("+ run-off", "runoff", "asphalt", 10.0, Profile::Flat),
            ] {
                if ui.small_button(label).clicked() {
                    let base = &label[2..];
                    let mut k = 1;
                    let mut sname = base.to_string();
                    while strips.iter().any(|s| s.name == sname) {
                        k += 1;
                        sname = format!("{base} {k}");
                    }
                    let ranges = node
                        .map(|n| {
                            vec![Range {
                                from: (n as f64 - 0.5).max(0.0),
                                to: (n as f64 + 0.5).min(period),
                            }]
                        })
                        .unwrap_or_default();
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
                    let at = (label == "+ kerb").then_some(0);
                    editor.apply(
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
    }

    // Lines.
    ui.separator();
    ui.strong("Painted lines");
    for (i, line) in road.lines.iter().enumerate() {
        let mut l = line.clone();
        let mut changed = false;
        let mut removed = false;
        egui::CollapsingHeader::new(&l.name)
            .id_salt(("line", r, i))
            .show(ui, |ui| {
                changed |= drag(ui, "offset m (+ left)", &mut l.offset, 0.05, -100.0..=100.0);
                changed |= drag(ui, "width m", &mut l.width, 0.01, 0.01..=5.0);
                changed |= combo(ui, ("lm", r, i), &mut l.material, &materials);
                let mut dashed = l.dash.is_some();
                if ui.checkbox(&mut dashed, "dashed").changed() {
                    l.dash = dashed.then_some((3.0, 9.0));
                    changed = true;
                }
                if let Some((on, off)) = &mut l.dash {
                    changed |= drag(ui, "on m", on, 0.1, 0.1..=100.0);
                    changed |= drag(ui, "off m", off, 0.1, 0.1..=100.0);
                }
                changed |= ranges_ui(ui, &mut l.ranges, node, period);
                removed = ui.button("Remove line").clicked();
            });
        if removed {
            editor.apply(
                vec![Op::RemoveLine {
                    road: name.clone(),
                    name: l.name,
                }],
                None,
            );
            return;
        }
        if changed {
            editor.apply(
                vec![Op::PutLine {
                    road: name.clone(),
                    line: l,
                }],
                Some(&format!("line {name} {i}")),
            );
        }
    }
    if ui.small_button("+ line").clicked() {
        let n = (1..)
            .map(|k| format!("line {k}"))
            .find(|n| road.lines.iter().all(|l| &l.name != n))
            .unwrap();
        let line = PaintLine {
            name: n,
            offset: 0.0,
            width: 0.12,
            material: "paint".into(),
            ranges: vec![],
            dash: Some((3.0, 9.0)),
        };
        editor.apply(
            vec![Op::PutLine {
                road: name.clone(),
                line,
            }],
            None,
        );
    }

    // Barriers.
    ui.separator();
    ui.strong("Barriers");
    for (i, barrier) in road.barriers.iter().enumerate() {
        let mut b = barrier.clone();
        let mut changed = false;
        let mut removed = false;
        egui::CollapsingHeader::new(format!("{} ({:?})", b.name, b.side))
            .id_salt(("barrier", r, i))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    changed |= ui
                        .selectable_value(&mut b.side, Side::Left, "left")
                        .changed();
                    changed |= ui
                        .selectable_value(&mut b.side, Side::Right, "right")
                        .changed();
                });
                changed |= drag(ui, "offset from edge m", &mut b.offset, 0.1, 0.0..=500.0);
                changed |= drag(ui, "height m", &mut b.height, 0.05, 0.1..=20.0);
                changed |= drag(ui, "thickness m", &mut b.thickness, 0.05, 0.0..=5.0);
                changed |= combo(ui, ("bm", r, i), &mut b.material, &materials);
                changed |= ranges_ui(ui, &mut b.ranges, node, period);
                removed = ui.button("Remove barrier").clicked();
            });
        if removed {
            editor.apply(
                vec![Op::RemoveBarrier {
                    road: name.clone(),
                    name: b.name,
                }],
                None,
            );
            return;
        }
        if changed {
            editor.apply(
                vec![Op::PutBarrier {
                    road: name.clone(),
                    barrier: b,
                }],
                Some(&format!("barrier {name} {i}")),
            );
        }
    }
    if ui.small_button("+ barrier").clicked() {
        let n = (1..)
            .map(|k| format!("barrier {k}"))
            .find(|n| road.barriers.iter().all(|b| &b.name != n))
            .unwrap();
        let barrier = Barrier {
            name: n,
            side: Side::Left,
            offset: 10.0,
            height: 1.0,
            thickness: 0.0,
            material: "armco".into(),
            ranges: vec![],
        };
        editor.apply(
            vec![Op::PutBarrier {
                road: name.clone(),
                barrier,
            }],
            None,
        );
    }
}

/// The text field for renaming `item`, kept while it is edited.
fn rename_text<'a>(state: &'a mut UiState, item: Item, name: &str) -> &'a mut String {
    if !matches!(&state.rename, Some((i, _)) if *i == item) {
        state.rename = Some((item, name.to_string()));
    }
    &mut state.rename.as_mut().expect("just set").1
}

/// The selected nodes of a road or spline: the active one's position and handle.
fn node_ui(
    ui: &mut egui::Ui,
    editor: &mut Editor,
    name: &str,
    nodes: &[open_racing_track_project::Node],
    closed: bool,
) {
    let selected = editor.selection.nodes.len();
    let Some(n) = editor.selection.node().filter(|&n| n < nodes.len()) else {
        ui.label(format!(
            "{} nodes. Click one in the view (A: all) to edit it.",
            nodes.len()
        ));
        return;
    };
    let nd = nodes[n];
    if selected > 1 {
        ui.strong(format!("Node {n} (u = {n}), {selected} selected"));
    } else {
        ui.strong(format!("Node {n} (u = {n})"));
    }
    let mut p = nd.pos;
    let mut moved = false;
    ui.horizontal(|ui| {
        for (axis, v) in ["x", "y", "z"].iter().zip([&mut p.x, &mut p.y, &mut p.z]) {
            ui.label(*axis);
            moved |= ui.add(egui::DragValue::new(v).speed(0.5)).changed();
        }
    });
    if moved {
        editor.apply(
            vec![Op::MoveNode {
                line: name.to_string(),
                index: n,
                pos: p,
            }],
            Some(&format!("node {name} {n}")),
        );
    }
    let mut manual = nd.handle.is_some();
    let (_, auto) = open_racing_track_project::curve::handles(nodes, closed, n);
    let mut h = nd.handle.unwrap_or(auto);
    let mut hchanged = ui
        .checkbox(&mut manual, "manual handle")
        .on_hover_text("Drag the handle in the view; Alt+click it for automatic")
        .changed();
    if manual {
        ui.horizontal(|ui| {
            for (axis, v) in ["hx", "hy", "hz"]
                .iter()
                .zip([&mut h.x, &mut h.y, &mut h.z])
            {
                ui.label(*axis);
                hchanged |= ui.add(egui::DragValue::new(v).speed(0.5)).changed();
            }
        });
    }
    if hchanged {
        editor.apply(
            vec![Op::SetHandle {
                line: name.to_string(),
                index: n,
                handle: manual.then_some(h),
            }],
            Some(&format!("handle {name} {n}")),
        );
    }
    let label = if selected > 1 {
        "Remove nodes (X)"
    } else {
        "Remove node (X)"
    };
    if ui.button(label).clicked() {
        crate::viewport::delete(editor);
    }
}

fn spline_inspector(ui: &mut egui::Ui, editor: &mut Editor, state: &mut UiState) {
    let Some(i) = editor
        .selection
        .spline()
        .filter(|&i| i < editor.project.splines.len())
    else {
        ui.label("Select a spline.");
        return;
    };
    let before = editor.project.splines[i].clone();
    let name = before.name.clone();
    let (surfaces, materials) = names(&editor.project);

    let text = rename_text(state, Item::Spline(i), &name);
    let resp = ui.horizontal(|ui| {
        ui.label("Spline");
        ui.text_edit_singleline(text)
    });
    if resp.inner.lost_focus() && *text != name && !text.is_empty() {
        let spline = Spline {
            name: text.clone(),
            ..before
        };
        let nodes = editor.selection.nodes.clone();
        if editor.apply(
            vec![Op::RemoveSpline { name }, Op::PutSpline { spline }],
            None,
        ) {
            editor.selection.item = Some(Item::Spline(editor.project.splines.len() - 1));
            editor.selection.nodes = nodes;
        }
        return;
    }

    let mut sp = before.clone();
    let mut changed = ui.checkbox(&mut sp.closed, "closed loop").changed();
    changed |= ui
        .checkbox(&mut sp.drape, "follow the ground")
        .on_hover_text(
            "Lay it on the roads and terrain under its line instead of at the nodes' heights",
        )
        .changed();
    changed |= drag(ui, "resolution m", &mut sp.resolution, 0.05, 0.1..=10.0);
    ui.horizontal(|ui| {
        let band = matches!(sp.shape, Shape::Band { .. });
        let preset = |label| {
            crate::presets::PRESETS
                .iter()
                .find(|p| p.label == label)
                .map(|p| (p.shape)(&editor.project))
        };
        if ui.selectable_label(band, "band (kerb, run-off)").clicked() && !band {
            sp.shape = preset("Kerb").expect("preset");
            changed = true;
        }
        if ui
            .selectable_label(!band, "wall (barrier, fence)")
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
            changed |= drag(ui, "width m", width, 0.05, 0.0..=100.0);
            ui.horizontal(|ui| {
                ui.label("lies");
                for (a, label) in [
                    (Align::Right, "right"),
                    (Align::Center, "centred"),
                    (Align::Left, "left"),
                ] {
                    changed |= ui.selectable_value(align, a, label).changed();
                }
                ui.label("of its line");
            });
            changed |= profile_ui(ui, profile, ("spline profile", i));
            ui.horizontal(|ui| {
                ui.label("surface");
                changed |= combo(ui, ("sp surface", i), surface, &surfaces);
                ui.label("material");
                changed |= combo(ui, ("sp material", i), material, &materials);
            });
            changed |= drag(ui, "lift m", lift, 0.005, -1.0..=1.0);
        }
        Shape::Wall {
            height,
            thickness,
            material,
            collide,
        } => {
            changed |= drag(ui, "height m", height, 0.05, 0.05..=30.0);
            changed |= drag(ui, "thickness m (0: thin)", thickness, 0.05, 0.0..=10.0);
            ui.horizontal(|ui| {
                ui.label("material");
                changed |= combo(ui, ("sp material", i), material, &materials);
            });
            changed |= ui.checkbox(collide, "cars collide with it").changed();
        }
    }
    if changed {
        editor.apply(
            vec![Op::PutSpline { spline: sp }],
            Some(&format!("spline {name}")),
        );
    }
    ui.separator();
    node_ui(ui, editor, &name, &before.nodes, before.closed);
    ui.separator();
    if ui.button("Delete spline").clicked() {
        editor.selection.nodes.clear();
        crate::viewport::delete(editor);
    }
}

fn keys_len_gt_one(n: usize) -> bool {
    n > 1
}

fn profile_ui(
    ui: &mut egui::Ui,
    p: &mut Profile,
    id: impl std::hash::Hash + std::fmt::Debug,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
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
            changed |= ui
                .add(egui::DragValue::new(&mut v).speed(0.005).suffix(" m"))
                .changed();
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

fn markers_inspector(ui: &mut egui::Ui, editor: &mut Editor) {
    let p = &editor.project;
    let mut m = p.markers.clone();
    let main_period = p.road(&p.main_road).map_or(1.0, |r| r.period());
    let node = editor.selection.node();
    let mut changed = false;
    ui.heading("Race markers");
    ui.label("Places on the main road are spline parameters u (node index + fraction).");
    changed |= drag(ui, "start/finish u", &mut m.start, 0.01, 0.0..=main_period);
    ui.strong("Sector boundaries");
    let mut remove = None;
    for (i, s) in m.sectors.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.label(format!("S{}", i + 2));
            changed |= ui
                .add(egui::DragValue::new(s).speed(0.01).range(0.0..=main_period))
                .changed();
            if ui.small_button("✕").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        m.sectors.remove(i);
        changed = true;
    }
    if ui.small_button("+ sector").clicked() {
        m.sectors.push(node.map_or(main_period * 0.5, |n| n as f64));
        changed = true;
    }
    ui.separator();
    ui.strong("Grid");
    let g: &mut Grid = &mut m.grid;
    let mut count = g.count as f64;
    changed |= drag(ui, "slots", &mut count, 0.2, 0.0..=60.0);
    g.count = count as usize;
    changed |= drag(ui, "spacing m", &mut g.spacing, 0.1, 2.0..=30.0);
    changed |= drag(ui, "stagger m", &mut g.stagger, 0.05, 0.0..=10.0);
    changed |= drag(ui, "pole behind line m", &mut g.behind, 0.1, 0.0..=200.0);
    ui.horizontal(|ui| {
        ui.label("pole side");
        changed |= ui
            .selectable_value(&mut g.pole, Side::Left, "left")
            .changed();
        changed |= ui
            .selectable_value(&mut g.pole, Side::Right, "right")
            .changed();
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

    ui.separator();
    ui.strong("Pit lane");
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
    if ui.checkbox(&mut on, "has a pit lane").changed() {
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
        ui.horizontal(|ui| {
            ui.label("road");
            pchanged |= combo(ui, "pit road", &mut p.road, &others);
        });
        let mut kmh = p.speed_limit * 3.6;
        if drag(ui, "speed limit km/h", &mut kmh, 0.5, 20.0..=200.0) {
            p.speed_limit = kmh / 3.6;
            pchanged = true;
        }
        pchanged |= drag(ui, "box offset m", &mut p.box_offset, 0.1, 0.0..=30.0);
        ui.horizontal(|ui| {
            ui.label("boxes on the");
            pchanged |= ui
                .selectable_value(&mut p.box_side, Side::Left, "left")
                .changed();
            pchanged |= ui
                .selectable_value(&mut p.box_side, Side::Right, "right")
                .changed();
        });
        let period = editor.project.road(&p.road).map_or(1.0, |r| r.period());
        let mut remove = None;
        for (i, u) in p.boxes.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.label(format!("box {}", i + 1));
                pchanged |= ui
                    .add(egui::DragValue::new(u).speed(0.005).range(0.0..=period))
                    .changed();
                if ui.small_button("✕").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            p.boxes.remove(i);
            pchanged = true;
        }
        ui.horizontal(|ui| {
            if ui.small_button("+ box").clicked() {
                let last = p.boxes.last().copied().unwrap_or(period * 0.3);
                p.boxes.push((last + 0.05).min(period));
                pchanged = true;
            }
            if ui.small_button("+ 10 boxes along the lane").clicked() {
                let (a, b) = (period * 0.3, period * 0.7);
                p.boxes = (0..10).map(|i| a + (b - a) * i as f64 / 9.0).collect();
                pchanged = true;
            }
        });
    }
    if pchanged {
        editor.apply(vec![Op::SetPit { pit }], Some("pit"));
    }
}

fn terrain_inspector(ui: &mut egui::Ui, editor: &mut Editor) {
    let (surfaces, materials) = names(&editor.project);
    let mut t = editor.project.terrain.clone();
    let mut changed = false;
    ui.heading("Terrain");
    ui.label("Ground round the roads, just under them and meeting their outer edges.");
    changed |= ui.checkbox(&mut t.enabled, "enabled").changed();
    ui.horizontal(|ui| {
        changed |= combo(ui, "terrain surface", &mut t.surface, &surfaces);
        changed |= combo(ui, "terrain material", &mut t.material, &materials);
    });
    changed |= drag(ui, "margin m", &mut t.margin, 1.0, 0.0..=2000.0);
    changed |= drag(ui, "cell m", &mut t.cell, 0.5, 2.0..=50.0);
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

fn surfaces_inspector(ui: &mut egui::Ui, editor: &mut Editor, state: &mut UiState) {
    ui.heading("Surfaces");
    ui.label("What the tyres feel. Asphalt and kerb count as track.");
    let surfaces = editor.project.surfaces.clone();
    for (i, s) in surfaces.iter().enumerate() {
        let mut s = s.clone();
        let mut changed = false;
        egui::CollapsingHeader::new(&s.name)
            .id_salt(("surface", i))
            .show(ui, |ui| {
                egui::ComboBox::from_id_salt(("kind", i))
                    .selected_text(format!("{:?}", s.props.kind))
                    .show_ui(ui, |ui| {
                        for k in KINDS {
                            changed |= ui
                                .selectable_value(&mut s.props.kind, k, format!("{k:?}"))
                                .changed();
                        }
                    });
                changed |= drag(ui, "grip ×", &mut s.props.grip, 0.005, 0.05..=1.5);
                changed |= drag(ui, "rolling drag", &mut s.props.drag, 0.002, 0.0..=0.5);
                if ui.button("Remove").clicked() {
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
        ui.text_edit_singleline(&mut state.new_surface);
        if ui.button("+ surface").clicked() && !state.new_surface.is_empty() {
            let surface = NamedSurface {
                name: state.new_surface.trim().to_string(),
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
        .width(220.0)
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

fn materials_inspector(
    ui: &mut egui::Ui,
    editor: &mut Editor,
    state: &mut UiState,
    library: &Library,
) {
    ui.heading("Materials");
    ui.small("Import textures in Assets to use them here.");
    let materials = editor.project.materials.clone();
    for (i, m) in materials.iter().enumerate() {
        let mut m = m.clone();
        let mut changed = false;
        egui::CollapsingHeader::new(&m.name)
            .id_salt(("material", i))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("tint");
                    changed |= ui.color_edit_button_rgb(&mut m.color).changed();
                });
                ui.horizontal(|ui| {
                    ui.label("texture");
                    changed |= texture_picker(ui, ("tex", i), &mut m.texture, library, true);
                });
                ui.horizontal(|ui| {
                    ui.label("normal map");
                    changed |= texture_picker(ui, ("normal", i), &mut m.normal, library, false);
                });
                ui.horizontal(|ui| {
                    ui.label("alpha");
                    let mut kind = match m.alpha {
                        Alpha::Opaque => 0,
                        Alpha::Mask(_) => 1,
                        Alpha::Blend => 2,
                    };
                    let before = kind;
                    for (k, label) in ["opaque", "cut out", "blend"].iter().enumerate() {
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
                ui.horizontal(|ui| {
                    ui.label("tile m");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut m.tile[0])
                                .speed(0.05)
                                .range(0.05..=100.0),
                        )
                        .changed();
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut m.tile[1])
                                .speed(0.05)
                                .range(0.05..=100.0),
                        )
                        .changed();
                });
                ui.horizontal(|ui| {
                    ui.label("roughness");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut m.roughness)
                                .speed(0.01)
                                .range(0.0..=1.0),
                        )
                        .changed();
                    ui.label("reflectance");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut m.reflectance)
                                .speed(0.01)
                                .range(0.0..=1.0),
                        )
                        .changed();
                });
                changed |= ui.checkbox(&mut m.double_sided, "double sided").changed();
                if ui.button("Remove").clicked() {
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
        ui.text_edit_singleline(&mut state.new_material);
        if ui.button("+ material").clicked() && !state.new_material.is_empty() {
            let material = MaterialDef {
                name: state.new_material.trim().to_string(),
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

fn prop_inspector(ui: &mut egui::Ui, editor: &mut Editor, library: &Library) {
    let Some(i) = editor
        .selection
        .prop()
        .filter(|&i| i < editor.project.props.len())
    else {
        ui.label("Select a prop.");
        return;
    };
    let before = editor.project.props[i].clone();
    let mut p = before.clone();
    ui.horizontal(|ui| {
        ui.label("Prop");
        ui.strong(&p.name);
    });
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("model");
        egui::ComboBox::from_id_salt(("prop model", i))
            .selected_text(p.model.to_string_lossy())
            .width(220.0)
            .show_ui(ui, |ui| {
                for a in library.models() {
                    changed |= ui
                        .selectable_value(&mut p.model, a.path.clone(), a.path.to_string_lossy())
                        .changed();
                }
            });
    });
    ui.horizontal(|ui| {
        for (axis, v) in ["x", "y", "z"]
            .iter()
            .zip([&mut p.pos.x, &mut p.pos.y, &mut p.pos.z])
        {
            ui.label(*axis);
            changed |= ui.add(egui::DragValue::new(v).speed(0.25)).changed();
        }
    });
    let mut deg = p.yaw.to_degrees();
    if drag(ui, "turn °", &mut deg, 1.0, -360.0..=360.0) {
        p.yaw = deg.to_radians();
        changed = true;
    }
    changed |= drag(ui, "scale", &mut p.scale, 0.01, 0.01..=100.0);
    changed |= ui
        .checkbox(&mut p.drape, "stand on the ground")
        .on_hover_text("Its height comes from the road or terrain under it")
        .changed();
    changed |= ui
        .checkbox(&mut p.collide, "cars collide with it")
        .changed();
    if changed {
        editor.apply(
            vec![Op::PutProp { prop: p }],
            Some(&format!("prop {}", before.name)),
        );
    }
    ui.small("G: move · R: turn · S: scale · Shift+D: duplicate · X: delete");
    if ui.button("Delete prop").clicked() {
        crate::viewport::delete(editor);
    }
}
