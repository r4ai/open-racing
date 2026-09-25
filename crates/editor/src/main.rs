//! open-racing's track editor: lay out roads as splines, shape their cross-sections,
//! kerbs, run-off and barriers, place the start line, sectors, grid and pit lane, and
//! bake the result into a track package to drive.
//!
//! The editor works on a project directory (see `open-racing-track-project`). Every
//! change is an operation, saved at once; changes others make to `project.ron` (such as
//! an agent using `open-racing-trackctl`) are loaded as they happen.

mod jobs;
mod preview;
mod state;
mod ui;
mod viewport;

use std::path::PathBuf;

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy_egui::input::egui_wants_any_pointer_input;
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};
use clap::Parser;
use open_racing_track_render::TrackModelPlugin;

#[derive(Parser)]
#[command(name = "open-racing-editor", about = "Track editor for open-racing")]
struct Args {
    /// Project directory, or a name under <content>/track-src/. Created if missing.
    #[arg(default_value = "untitled")]
    project: String,
    /// Save a picture of the window here once the track is built, then quit. Lets
    /// scripts and agents see the 3D view.
    #[arg(long)]
    screenshot: Option<PathBuf>,
}

/// Where `--screenshot` saves, and the frames left before it is taken or the app quits.
#[derive(Resource)]
struct AutoScreenshot {
    path: PathBuf,
    wait: u32,
    taken: bool,
}

fn auto_screenshot(
    mut commands: Commands,
    shot: Option<ResMut<AutoScreenshot>>,
    built: Res<preview::Built>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(mut shot) = shot else { return };
    // Wait for the first build, then for its meshes and textures to reach the GPU.
    if built.count == 0 {
        return;
    }
    if shot.wait > 0 {
        shot.wait -= 1;
        return;
    }
    if !shot.taken {
        shot.taken = true;
        shot.wait = 30;
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(shot.path.clone()));
    } else {
        exit.write(AppExit::Success);
    }
}

fn main() {
    let args = Args::parse();
    let path = PathBuf::from(&args.project);
    let dir = if path.components().count() > 1 || path.exists() {
        path
    } else {
        open_racing_track_project::projects_dir().join(&args.project)
    };
    let editor = state::Editor::open(dir).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });

    let mut app = App::new();
    if let Some(path) = args.screenshot {
        app.insert_resource(AutoScreenshot {
            path,
            wait: 60,
            taken: false,
        });
    }
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "open-racing track editor".into(),
            ..default()
        }),
        ..default()
    }))
    .add_plugins((EguiPlugin::default(), TrackModelPlugin))
    .insert_resource(editor)
    .init_resource::<viewport::Orbit>()
    .init_resource::<viewport::ViewRect>()
    .init_resource::<viewport::Drag>()
    .init_resource::<preview::Rebuild>()
    .init_resource::<preview::Built>()
    .init_resource::<jobs::Jobs>()
    .add_systems(Startup, (viewport::setup, frame_on_start))
    .add_systems(EguiPrimaryContextPass, ui::ui)
    .add_systems(
        Update,
        (
            state::watch_file,
            viewport::input.run_if(not(egui_wants_any_pointer_input)),
            preview::rebuild,
            jobs::poll,
            viewport::gizmos,
            viewport::place_camera,
            auto_screenshot,
        )
            .chain(),
    )
    .run();
}

fn frame_on_start(editor: Res<state::Editor>, mut orbit: ResMut<viewport::Orbit>) {
    viewport::frame_selection(&editor, &mut orbit);
}
