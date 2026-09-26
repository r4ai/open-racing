//! open-racing's machine editor: assemble cars from parts, develop engines on a
//! simulated dyno and hear them run, and bake machines for the game.
//!
//! It edits a library directory (see `open-racing-machine-project`). Every change is an
//! operation, saved at once; changes others make to the files (an agent using
//! `open-racing-machinectl`) are loaded as they happen.

mod jobs;
mod live;
mod state;
mod ui;
mod view;

#[cfg(test)]
mod ui_tests;

use std::path::PathBuf;

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};
use clap::Parser;

#[derive(Parser)]
#[command(
    name = "open-racing-machine-editor",
    about = "Machine editor for open-racing"
)]
struct Args {
    /// What to open: `machine/<name>`, `engine/<name>` (in the Engine workspace), or a
    /// library directory.
    target: Option<String>,
    /// Library directory (default: <content>/machine-src; the samples are written there
    /// when it does not exist).
    #[arg(long)]
    dir: Option<PathBuf>,
    /// Start in this workspace: assembly, engine, suspension, tyres, aero or interior.
    #[arg(long)]
    workspace: Option<String>,
    /// Start on this tab of the engine workbench: dyno, cycle, network, run or sound.
    #[arg(long)]
    tab: Option<String>,
    /// Select this placed part of the machine.
    #[arg(long)]
    select: Option<String>,
    /// Run the dyno at the start (the Dyno tab shows it).
    #[arg(long)]
    dyno: bool,
    /// Start the engine running at the start (the Run tab shows it).
    #[arg(long)]
    run: bool,
    /// Render a RAM preview of this script at the start (sweep, idle, blips, rev,
    /// overrun), at High quality, and play it.
    #[arg(long)]
    preview: Option<String>,
    /// Save a picture of the window here once ready, then quit: lets scripts and agents
    /// see the editor.
    #[arg(long)]
    screenshot: Option<PathBuf>,
}

#[derive(Resource)]
struct AutoScreenshot {
    path: PathBuf,
    wait: u32,
    taken: bool,
}

#[derive(Resource)]
struct StartJobs {
    dyno: bool,
    run: bool,
    preview: Option<String>,
}

fn start_jobs(
    mut commands: Commands,
    start: Option<Res<StartJobs>>,
    mut editor: ResMut<state::Editor>,
    mut jobs: ResMut<jobs::Jobs>,
    mut live: ResMut<live::Live>,
    mut ui: ResMut<ui::UiState>,
) {
    let Some(s) = start else { return };
    let mut c = ui::Ctx {
        editor: &mut editor,
        jobs: &mut jobs,
        live: &mut live,
        state: &mut ui,
    };
    if s.dyno
        && let Ok(b) = ui::engine::build(&c, c.state.dyno_quality)
    {
        let e = &b.engine.ecu;
        let rpms = open_racing_engine_sim::dyno::speeds(
            (e.idle_rpm - 200.0).max(800.0),
            e.limiter_rpm,
            500.0,
        );
        c.state.dyno_rpm = [rpms[0], e.limiter_rpm, 500.0];
        let t = c.editor.selection.engine.clone().unwrap_or_default();
        c.jobs.dyno(t, b, rpms, 1.0);
    }
    if s.run {
        ui::engine::start_live(&mut c);
    }
    if let Some(p) = &s.preview {
        c.state.preview_source = p.clone();
        ui::engine::start_preview(&mut c, None);
    }
    commands.remove_resource::<StartJobs>();
}

fn auto_screenshot(
    mut commands: Commands,
    shot: Option<ResMut<AutoScreenshot>>,
    jobs: Res<jobs::Jobs>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(mut shot) = shot else { return };
    if !jobs.running.is_empty() {
        return;
    }
    if shot.wait > 0 {
        shot.wait -= 1;
        return;
    }
    if !shot.taken {
        shot.taken = true;
        shot.wait = 20;
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(shot.path.clone()));
    } else {
        exit.write(AppExit::Success);
    }
}

fn main() {
    let args = Args::parse();
    let mut target = args.target.clone();
    let dir = match (&args.dir, &target) {
        (Some(d), _) => d.clone(),
        (None, Some(t)) if !t.contains('/') || PathBuf::from(t).is_dir() => {
            let d = PathBuf::from(t);
            target = None;
            d
        }
        _ => open_racing_machine_project::library_dir(),
    };
    if !dir.exists() {
        match open_racing_machine_project::samples::init(&dir) {
            Ok(w) => eprintln!(
                "{}: a new library with {} sample files",
                dir.display(),
                w.len()
            ),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    }
    let mut editor = state::Editor::open(dir).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    let mut ui_state = ui::UiState::default();
    if let Some(t) = &target {
        if let Some(m) = t.strip_prefix("machine/") {
            if !editor.lib.machines.contains_key(m) {
                eprintln!("no machine \"{m}\"");
                std::process::exit(1);
            }
            editor.selection.machine = Some(m.into());
            editor.selection.engine = Some(t.clone());
        } else if editor.lib.part(t).is_ok() {
            if t.starts_with("engine/") {
                editor.selection.engine = Some(t.clone());
                editor.workspace = state::Workspace::Engine;
            }
            editor.selection.part = Some(t.clone());
        } else {
            eprintln!("nothing called {t} in the library");
            std::process::exit(1);
        }
    }
    if let Some(w) = &args.workspace {
        editor.workspace = state::Workspace::named(w).unwrap_or_else(|| {
            eprintln!("no workspace \"{w}\"");
            std::process::exit(1);
        });
    }
    if let Some(t) = &args.tab {
        ui_state.engine_tab = ui::EngineTab::named(t).unwrap_or_else(|| {
            eprintln!("no tab \"{t}\"");
            std::process::exit(1);
        });
    }
    if let Some(s) = &args.select {
        let found = editor
            .selection
            .machine
            .as_ref()
            .and_then(|m| editor.lib.machines.get(m))
            .and_then(|m| m.placed(s))
            .map(|p| p.part.clone());
        match found {
            Some(p) => {
                editor.selection.placed = Some(s.clone());
                editor.selection.part = Some(p);
            }
            None => {
                eprintln!("the machine has no part \"{s}\"");
                std::process::exit(1);
            }
        }
    }
    if args.run || args.preview.is_some() {
        ui_state.engine_tab = ui::EngineTab::Run;
    }

    let mut app = App::new();
    if let Some(path) = args.screenshot {
        app.insert_resource(AutoScreenshot {
            path,
            wait: if args.run || args.preview.is_some() {
                150
            } else {
                40
            },
            taken: false,
        });
    }
    app.insert_resource(StartJobs {
        dyno: args.dyno,
        run: args.run,
        preview: args.preview.clone(),
    });
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "open-racing machine editor".into(),
            resolution: (1600, 950).into(),
            ..default()
        }),
        ..default()
    }))
    .add_plugins((EguiPlugin::default(), live::LivePlugin))
    .insert_resource(editor)
    .insert_resource(ui_state)
    .init_resource::<jobs::Jobs>()
    .init_resource::<ui::ViewRect>()
    .init_resource::<view::Orbit>()
    .init_resource::<view::Shown>()
    .add_systems(Startup, view::setup)
    .add_systems(EguiPrimaryContextPass, ui::system)
    .add_systems(
        Update,
        (
            state::watch_files,
            jobs::poll,
            start_jobs,
            view::rebuild,
            view::input,
            view::place_camera,
            view::gizmos,
            auto_screenshot,
        )
            .chain(),
    )
    .run();
}
