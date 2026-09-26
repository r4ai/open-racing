//! Every workspace, tab and selection drawn headless over the sample library: nothing
//! panics, and the panels' edits go through as operations.

use bevy_egui::egui;

use crate::jobs::Jobs;
use crate::live::Live;
use crate::state::{Editor, Workspace};
use crate::ui::{self, BottomTab, Ctx, EngineTab, PropTab, UiState};

fn editor() -> Editor {
    let mut e = Editor::new(open_racing_machine_project::samples::library("ui-test"));
    e.save = false;
    e
}

fn frame(ctx: &egui::Context, c: &mut Ctx) {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1600.0, 950.0),
        )),
        ..Default::default()
    };
    let mut out = ctx.run_ui(input, |ui| {
        let _ = ui::draw(ui.ctx(), c);
    });
    out.textures_delta.clear();
}

#[test]
fn every_workspace_and_tab_draws() {
    let ctx = egui::Context::default();
    let mut editor = editor();
    let mut jobs = Jobs::default();
    let mut live = Live::default();
    let mut state = UiState::default();
    for w in Workspace::ALL {
        editor.workspace = w;
        for t in [
            EngineTab::Dyno,
            EngineTab::Cycle,
            EngineTab::Network,
            EngineTab::Run,
            EngineTab::Sound,
        ] {
            state.engine_tab = t;
            for p in [
                PropTab::Part,
                PropTab::Placement,
                PropTab::Setup,
                PropTab::Machine,
            ] {
                state.prop_tab = p;
                for b in [BottomTab::Library, BottomTab::Report] {
                    state.bottom = b;
                    let mut c = Ctx {
                        editor: &mut editor,
                        jobs: &mut jobs,
                        live: &mut live,
                        state: &mut state,
                    };
                    frame(&ctx, &mut c);
                }
            }
        }
    }
    // With selections: a placed part, an engine part on its bench, a pipe.
    editor.workspace = Workspace::Assembly;
    editor.selection.placed = Some("engine".into());
    editor.selection.part = Some("engine/v8_4l_flatplane".into());
    let mut c = Ctx {
        editor: &mut editor,
        jobs: &mut jobs,
        live: &mut live,
        state: &mut state,
    };
    frame(&ctx, &mut c);
    editor.workspace = Workspace::Engine;
    editor.selection.engine = Some("engine/i4_2l_na".into());
    editor.selection.element = Some(("exhaust/i4_road".into(), "primary.1".into()));
    state.engine_tab = EngineTab::Network;
    let mut c = Ctx {
        editor: &mut editor,
        jobs: &mut jobs,
        live: &mut live,
        state: &mut state,
    };
    frame(&ctx, &mut c);
}

#[test]
fn a_dyno_result_draws() {
    let ctx = egui::Context::default();
    let mut editor = editor();
    let mut jobs = Jobs::default();
    let mut live = Live::default();
    let mut state = UiState::default();
    editor.workspace = Workspace::Engine;
    editor.selection.engine = Some("engine/i4_2l_na".into());
    let b = open_racing_machine_project::engines::for_target(
        &editor.lib,
        "engine/i4_2l_na",
        open_racing_engine_sim::Quality::Draft,
    )
    .unwrap();
    let run = open_racing_engine_sim::dyno::sweep(&b, &[3000.0, 5000.0], 1.0).unwrap();
    jobs.dyno = Some(("engine/i4_2l_na".into(), run));
    let t = open_racing_engine_sim::trace::cycle(&b, 4000.0, 1.0, 0, &[]).unwrap();
    assert!(t.deg.len() > 600, "a whole cycle: {}", t.deg.len());
    jobs.trace = Some(("engine/i4_2l_na".into(), t));
    for tab in [EngineTab::Dyno, EngineTab::Cycle] {
        state.engine_tab = tab;
        let mut c = Ctx {
            editor: &mut editor,
            jobs: &mut jobs,
            live: &mut live,
            state: &mut state,
        };
        frame(&ctx, &mut c);
    }
}

#[test]
fn picking_finds_the_engine() {
    let mut editor = editor();
    editor.selection.machine = Some("gt3_v8".into());
    let m = editor.lib.machines.get("gt3_v8").unwrap();
    let asm = open_racing_machine_project::assembly::assemble(&editor.lib, m).unwrap();
    let mut shown = crate::view::Shown::default();
    shown.assembly = Some(asm);
    // Down from above: something is hit.
    let hit = crate::view::pick(
        &editor,
        &shown,
        glam::DVec3::new(-3.1, 0.45, 3.0),
        glam::DVec3::new(0.0, 0.0, -1.0),
    );
    assert!(hit.is_some());
    let wheel = crate::view::pick(
        &editor,
        &shown,
        glam::DVec3::new(0.0, 3.0, 0.2),
        glam::DVec3::new(0.0, -1.0, 0.0),
    );
    assert_eq!(wheel.as_deref(), Some("front wheels"));
}

#[test]
fn a_ram_preview_renders_and_plays() {
    use std::sync::atomic::Ordering::Relaxed;
    let mut editor = editor();
    let mut jobs = Jobs::default();
    let mut live = Live::default();
    let mut state = UiState::default();
    editor.selection.engine = Some("engine/i4_2l_na".into());
    state.preview_source = "blips".into();
    state.preview_quality = open_racing_engine_sim::Quality::Draft;
    let mut c = Ctx {
        editor: &mut editor,
        jobs: &mut jobs,
        live: &mut live,
        state: &mut state,
    };
    crate::ui::engine::start_preview(&mut c, None);
    let p = live.preview.clone().expect("started");
    let t0 = std::time::Instant::now();
    while !p.done.load(Relaxed) && t0.elapsed().as_secs() < 120 {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(p.done.load(Relaxed));
    assert!(
        p.rendered.load(Relaxed) + 2000 >= p.total,
        "{} of {}",
        p.rendered.load(Relaxed),
        p.total
    );
    assert!(p.playing.load(Relaxed), "plays once cached");
    // And the panel draws it.
    editor.workspace = Workspace::Engine;
    state.engine_tab = EngineTab::Run;
    let ctx = egui::Context::default();
    let mut c = Ctx {
        editor: &mut editor,
        jobs: &mut jobs,
        live: &mut live,
        state: &mut state,
    };
    frame(&ctx, &mut c);
}
