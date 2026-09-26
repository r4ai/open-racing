//! The editor's window: a bar of workspaces at the top, and in each workspace the panels
//! its work needs; the properties on the right edit whatever is selected.

pub mod assembly;
pub mod engine;
pub mod plot;
pub mod props;
pub mod schematic;

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use open_racing_engine_sim::Quality;
use open_racing_machine_project::ops::{self, Op};
use open_racing_machine_project::part::Design;
use open_racing_machine_project::{Kind, PartRef};

use crate::jobs::Jobs;
use crate::live::Live;
use crate::state::{Editor, Workspace};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EngineTab {
    #[default]
    Dyno,
    Cycle,
    Network,
    Run,
    Sound,
}

impl EngineTab {
    pub fn named(s: &str) -> Option<Self> {
        Some(match s {
            "dyno" => Self::Dyno,
            "cycle" => Self::Cycle,
            "network" => Self::Network,
            "run" => Self::Run,
            "sound" => Self::Sound,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PropTab {
    #[default]
    Part,
    Placement,
    Setup,
    Machine,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BottomTab {
    #[default]
    Library,
    Report,
}

/// What the panels remember between frames.
#[derive(Resource)]
pub struct UiState {
    pub styled: bool,
    pub engine_tab: EngineTab,
    pub prop_tab: PropTab,
    pub bottom: BottomTab,
    pub library_kind: Option<Kind>,
    pub dyno_rpm: [f64; 3],
    pub dyno_throttle: f64,
    pub dyno_quality: Quality,
    pub trace_rpm: f64,
    pub trace_throttle: f64,
    pub run_quality: Quality,
    pub run_mics: Vec<String>,
    pub sound_preset: String,
    pub sound_mics: Vec<String>,
    pub sound_quality: Quality,
    /// The part the other workspaces edit.
    pub other_part: Option<String>,
    /// RAM preview: what to render, how finely, and whether it loops.
    pub preview_source: String,
    pub preview_quality: Quality,
    pub preview_loop: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            styled: false,
            engine_tab: EngineTab::Dyno,
            prop_tab: PropTab::Part,
            bottom: BottomTab::Library,
            library_kind: None,
            dyno_rpm: [1000.0, 7000.0, 500.0],
            dyno_throttle: 1.0,
            dyno_quality: Quality::Draft,
            trace_rpm: 6000.0,
            trace_throttle: 1.0,
            run_quality: Quality::Draft,
            run_mics: vec!["exhaust".into(), "cabin".into()],
            sound_preset: "sweep".into(),
            sound_mics: vec!["exhaust".into()],
            sound_quality: Quality::Normal,
            other_part: None,
            preview_source: "rev".into(),
            preview_quality: Quality::High,
            preview_loop: true,
        }
    }
}

/// What the panels work with.
pub struct Ctx<'a> {
    pub editor: &'a mut Editor,
    pub jobs: &'a mut Jobs,
    pub live: &'a mut Live,
    pub state: &'a mut UiState,
}

/// Where the 3D view is (the window's part the panels leave), in logical pixels.
#[derive(Resource, Default)]
pub struct ViewRect(pub Option<egui::Rect>);

fn style(ctx: &egui::Context) {
    ctx.all_styles_mut(|s| {
        let v = &mut s.visuals;
        v.selection.bg_fill = egui::Color32::from_rgb(71, 114, 179);
        v.panel_fill = egui::Color32::from_gray(43);
        v.window_fill = egui::Color32::from_gray(38);
        v.extreme_bg_color = egui::Color32::from_gray(29);
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
    });
}

pub fn system(
    mut contexts: EguiContexts,
    mut editor: ResMut<Editor>,
    mut jobs: ResMut<Jobs>,
    mut live: ResMut<Live>,
    mut state: ResMut<UiState>,
    mut rect: ResMut<ViewRect>,
    mut exit: MessageWriter<AppExit>,
) -> Result {
    let ctx = contexts.ctx_mut()?.clone();
    let mut c = Ctx {
        editor: &mut editor,
        jobs: &mut jobs,
        live: &mut live,
        state: &mut state,
    };
    let (free, quit) = draw(&ctx, &mut c);
    rect.0 = free;
    if quit {
        exit.write(AppExit::Success);
    }
    Ok(())
}

/// Draws the window; returns the free area (in the Assembly workspace) and whether to quit.
pub fn draw(ctx: &egui::Context, c: &mut Ctx) -> (Option<egui::Rect>, bool) {
    if !c.state.styled {
        style(ctx);
        c.state.styled = true;
    }
    shortcuts(ctx, c);
    // An edit restarts the running engine with it.
    if let Some((t, rev)) = c.live.target.clone()
        && rev != c.editor.revision
        && c.live.running()
    {
        if c.editor.selection.engine.as_deref() == Some(t.as_str()) {
            engine::start_live(c);
        } else {
            c.live.stop();
        }
    }
    let mut quit = false;
    let mut root = egui::Ui::new(
        ctx.clone(),
        "root".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );
    egui::Panel::top("top bar").show(&mut root, |ui| {
        ui.horizontal(|ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Write Samples into the Library").clicked() {
                    match open_racing_machine_project::samples::init(&c.editor.lib.dir) {
                        Ok(w) => {
                            c.editor.status =
                                format!("{} sample files written; they load in a moment", w.len())
                        }
                        Err(e) => c.editor.status = e,
                    }
                }
                if ui.button("Quit").clicked() {
                    quit = true;
                }
            });
            ui.menu_button("Edit", |ui| {
                if ui
                    .add_enabled(c.editor.can_undo(), egui::Button::new("Undo  Ctrl Z"))
                    .clicked()
                {
                    c.editor.undo();
                }
                if ui
                    .add_enabled(c.editor.can_redo(), egui::Button::new("Redo  Ctrl Shift Z"))
                    .clicked()
                {
                    c.editor.redo();
                }
            });
            ui.separator();
            for w in Workspace::ALL {
                ui.selectable_value(&mut c.editor.workspace, w, w.label());
            }
            ui.separator();
            ui.weak(c.editor.lib.dir.display().to_string());
        });
    });
    egui::Panel::bottom("status").show(&mut root, |ui| {
        ui.horizontal(|ui| {
            ui.label(&c.editor.status);
            if let Some(s) = &c.jobs.status {
                ui.separator();
                ui.label(s);
            }
            if !c.jobs.running.is_empty() {
                ui.separator();
                ui.spinner();
                ui.label(c.jobs.running.join(", "));
            }
        });
    });
    egui::Panel::right("properties")
        .resizable(true)
        .default_size(400.0)
        .size_range(280.0..=800.0)
        .show(&mut root, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("props scroll")
                .show(ui, |ui| properties(ui, c));
        });
    let mut free = None;
    match c.editor.workspace {
        Workspace::Assembly => {
            egui::Panel::left("outliner")
                .resizable(true)
                .default_size(300.0)
                .show(&mut root, |ui| assembly::outliner(ui, c));
            egui::Panel::bottom("bottom")
                .resizable(true)
                .default_size(220.0)
                .show(&mut root, |ui| {
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut c.state.bottom, BottomTab::Library, "Library");
                        ui.selectable_value(&mut c.state.bottom, BottomTab::Report, "Report");
                    });
                    ui.separator();
                    match c.state.bottom {
                        BottomTab::Library => assembly::library(ui, c),
                        BottomTab::Report => assembly::report(ui, c),
                    }
                });
            let r = root.available_rect_before_wrap();
            free = Some(r);
        }
        Workspace::Engine => {
            egui::Panel::left("engines")
                .resizable(true)
                .default_size(260.0)
                .show(&mut root, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("engines")
                        .show(ui, |ui| engine::targets(ui, c));
                });
            egui::CentralPanel::default().show(&mut root, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("bench")
                    .show(ui, |ui| engine::bench(ui, c));
            });
        }
        other => {
            egui::CentralPanel::default().show(&mut root, |ui| later(ui, c, other));
        }
    }
    (free, quit)
}

fn shortcuts(ctx: &egui::Context, c: &mut Ctx) {
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    let (undo, redo, space) = ctx.input(|i| {
        let cmd = i.modifiers.command;
        (
            cmd && !i.modifiers.shift && i.key_pressed(egui::Key::Z),
            cmd && (i.key_pressed(egui::Key::Y)
                || i.modifiers.shift && i.key_pressed(egui::Key::Z)),
            i.key_pressed(egui::Key::Space),
        )
    });
    if undo {
        c.editor.undo();
    }
    if redo {
        c.editor.redo();
    }
    if space && c.editor.workspace == Workspace::Engine {
        if c.live.running() {
            c.live.stop();
        } else {
            engine::start_live(c);
        }
    }
}

/// Workspaces whose detailed simulation comes later: the parts' parameters, edited.
fn later(ui: &mut egui::Ui, c: &mut Ctx, w: Workspace) {
    let kinds: &[Kind] = match w {
        Workspace::Suspension => &[Kind::Suspension, Kind::Brakes, Kind::Steering],
        Workspace::Tyres => &[Kind::Wheel],
        Workspace::Aero => &[Kind::Aero, Kind::Body],
        _ => &[Kind::Interior, Kind::FuelTank, Kind::Electronics],
    };
    ui.heading(w.label());
    ui.label(match w {
        Workspace::Suspension => "Kinematics and compliance sweeps and the damper's valve model come with the suspension workbench; for now the game's geometry and rates are edited here, and the machine's setup in Assembly › Setup.",
        Workspace::Tyres => "A physical tyre model on a virtual flat-track rig, fitted to the game's tyre, comes with the tyre workbench; for now the game's tyre parameters are edited here.",
        Workspace::Aero => "Ride-height and yaw maps come with the aero workbench; for now the game's elements are edited here.",
        _ => "The cockpit's layout and the fuel and electronics parts.",
    });
    ui.separator();
    let parts: Vec<String> = c
        .editor
        .lib
        .parts
        .keys()
        .filter(|r| kinds.contains(&r.kind))
        .map(|r| r.to_string())
        .collect();
    ui.horizontal_wrapped(|ui| {
        for p in &parts {
            if ui
                .selectable_label(c.state.other_part.as_deref() == Some(p.as_str()), p)
                .clicked()
            {
                c.state.other_part = Some(p.clone());
            }
        }
    });
    ui.separator();
    if let Some(p) = c.state.other_part.clone().filter(|p| parts.contains(p)) {
        egui::ScrollArea::vertical()
            .id_salt("later")
            .show(ui, |ui| edit_target(ui, c, &p, "design", 0));
    }
}

/// Edits a part or machine (or one field of it, by path) from its JSON form.
pub fn edit_target(ui: &mut egui::Ui, c: &mut Ctx, target: &str, path: &str, depth: usize) {
    let v = match target.strip_prefix("machine/") {
        Some(m) => c
            .editor
            .lib
            .machines
            .get(m)
            .and_then(|x| serde_json::to_value(x).ok()),
        None => PartRef::parse(target)
            .ok()
            .and_then(|r| c.editor.lib.parts.get(&r))
            .and_then(|p| serde_json::to_value(p).ok()),
    };
    let Some(v) = v else {
        ui.weak(format!("no {target}"));
        return;
    };
    let sub = if path.is_empty() {
        Ok(&v)
    } else {
        ops::get_path(&v, path)
    };
    let Ok(sub) = sub else {
        ui.weak(format!("{target} has no {path}"));
        return;
    };
    let mut changes = Vec::new();
    let key = path
        .rsplit(['.', '['])
        .next()
        .unwrap_or("")
        .trim_end_matches(']');
    props::value(ui, key, path, sub, depth, &mut changes);
    if !changes.is_empty() {
        let key = format!("{target} {}", changes[0].0);
        let list = changes
            .into_iter()
            .map(|(path, value)| Op::Set {
                target: target.to_string(),
                path,
                value,
            })
            .collect();
        c.editor.apply(list, Some(&key));
    }
}

fn properties(ui: &mut egui::Ui, c: &mut Ctx) {
    match c.editor.workspace {
        Workspace::Assembly => {
            ui.horizontal(|ui| {
                for (t, l) in [
                    (PropTab::Part, "Part"),
                    (PropTab::Placement, "Placement"),
                    (PropTab::Setup, "Setup"),
                    (PropTab::Machine, "Machine"),
                ] {
                    ui.selectable_value(&mut c.state.prop_tab, t, l);
                }
            });
            ui.separator();
            let m = c.editor.selection.machine.clone();
            match c.state.prop_tab {
                PropTab::Part => match c.editor.selection.part.clone() {
                    Some(p) => {
                        ui.heading(&p);
                        if let Ok(part) = c.editor.lib.part(&p) {
                            if !part.description.is_empty() {
                                ui.label(&part.description);
                            }
                            let s = open_racing_machine_project::inspect::part(
                                &c.editor.lib,
                                &PartRef::parse(&p).expect("valid"),
                                part,
                            );
                            ui.weak(format!(
                                "{:.1} kg, used by {}",
                                s.mass,
                                if s.used_by.is_empty() {
                                    "nothing".into()
                                } else {
                                    s.used_by.join(", ")
                                }
                            ));
                        }
                        ui.separator();
                        edit_target(ui, c, &p, "", 0);
                    }
                    None => {
                        ui.weak("Pick a part in the outliner, the view or the library.");
                    }
                },
                PropTab::Placement => match (m, c.editor.selection.placed.clone()) {
                    (Some(m), Some(p)) => {
                        ui.heading(&p);
                        ui.weak("Attach: the part it sits on (to), that part's mount (at) and its own mount; at and rotation offset it.");
                        edit_target(ui, c, &format!("machine/{m}"), &format!("parts[{p}]"), 0);
                    }
                    _ => {
                        ui.weak("Pick a part of the machine.");
                    }
                },
                PropTab::Setup => {
                    if let Some(m) = m {
                        edit_target(ui, c, &format!("machine/{m}"), "setup", 0);
                    }
                }
                PropTab::Machine => {
                    if let Some(m) = m {
                        edit_target(ui, c, &format!("machine/{m}"), "", 0);
                    }
                }
            }
        }
        Workspace::Engine => {
            let target = c.editor.selection.engine.clone().unwrap_or_default();
            // The engine part: itself, or the machine's.
            let engine_part = match target.strip_prefix("machine/") {
                Some(m) => c.editor.lib.machines.get(m).and_then(|x| {
                    x.parts
                        .iter()
                        .find(|p| PartRef::parse(&p.part).is_ok_and(|r| r.kind == Kind::Engine))
                        .map(|p| p.part.clone())
                }),
                None => Some(target.clone()),
            };
            if let Some((part, name)) = c.editor.selection.element.clone() {
                ui.horizontal(|ui| {
                    ui.heading(format!("{part} › {name}"));
                    if ui
                        .small_button("✕")
                        .on_hover_text("Back to the engine")
                        .clicked()
                    {
                        c.editor.selection.element = None;
                    }
                });
                let design = match c.editor.lib.part(&part).map(|p| &p.design) {
                    Ok(Design::Intake(i)) => Some(("Intake", &i.network)),
                    Ok(Design::Exhaust(x)) => Some(("Exhaust", &x.network)),
                    _ => None,
                };
                if let Some((kind, net)) = design {
                    let list = if net.pipes.iter().any(|p| p.name == name) {
                        "pipes"
                    } else {
                        "volumes"
                    };
                    let path = format!("design.{kind}.network.{list}[{name}]");
                    edit_target(ui, c, &part, &path, 0);
                }
                return;
            }
            if let Some(p) = engine_part {
                ui.heading(&p);
                edit_target(ui, c, &p, "design.Engine.spec", 0);
                egui::CollapsingHeader::new("bench, cooling and the physical part").show(
                    ui,
                    |ui| {
                        edit_target(ui, c, &p, "design.Engine.bench", 1);
                        edit_target(ui, c, &p, "design.Engine.cooling", 1);
                        edit_target(ui, c, &p, "physical", 1);
                    },
                );
            }
        }
        _ => {
            ui.weak("The parameters are in the middle.");
        }
    }
}
