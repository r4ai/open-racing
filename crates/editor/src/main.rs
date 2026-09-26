//! open-racing's track editor: lay out roads as splines, shape their cross-sections,
//! kerbs, run-off and barriers, place the start line, sectors, grid and pit lane, and
//! bake the result into a track package to drive.
//!
//! The editor works on a project directory (see `open-racing-track-project`). Every
//! change is an operation, saved at once; changes others make to `project.ron` (such as
//! an agent using `open-racing-trackctl`) are loaded as they happen.

mod assets;
mod batch;
mod clipboard;
mod commands;
mod corners;
mod curve_graph;
mod edit;
mod jobs;
mod lay;
mod menus;
mod outliner;
mod overlay;
mod popups;
mod presets;
mod preview;
mod profile;
mod properties;
mod reference;
mod sidebar;
mod state;
mod theme;
mod ui;
mod viewport;

#[cfg(test)]
mod ui_tests;

use std::path::PathBuf;

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy_egui::{EguiPlugin, EguiPreUpdateSet, EguiPrimaryContextPass};
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
    /// Select this road, spline or prop and frame it (with `--screenshot`, to look at it).
    #[arg(long)]
    focus: Option<String>,
    /// View from above.
    #[arg(long)]
    top: bool,
    /// Start in edit mode on the focused road or spline, with these of its nodes
    /// selected (comma separated).
    #[arg(long, value_delimiter = ',')]
    edit: Option<Vec<usize>>,
    /// Look at this corner of the focused road (or the main road), by its number.
    #[arg(long)]
    corner: Option<usize>,
    /// Open this tab of the properties: track, markers, terrain, reference, library,
    /// object, corners, strips, lines or barriers.
    #[arg(long)]
    tab: Option<String>,
    /// How far the camera stands from what it looks at, m.
    #[arg(long)]
    distance: Option<f32>,
    /// How far above the horizon the camera looks down from, degrees (90 from above).
    #[arg(long)]
    pitch: Option<f32>,
    /// Which way round the camera stands, degrees anticlockwise from the east.
    #[arg(long)]
    yaw: Option<f32>,
    /// Hide lines, names, stretches and markers drawn over the track.
    #[arg(long)]
    clean: bool,
}

/// A corner to look at once the first build has found the corners, and a properties
/// tab to open, from the command line.
#[derive(Resource)]
pub struct StartCorner(pub usize);
#[derive(Resource)]
pub struct StartTab(pub ui::PropTab);

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

/// The camera's distance from the command line, set after it has looked at a corner.
#[derive(Resource)]
pub struct StartDistance(pub f32);

/// What the command line asks the UI to do once the track is first built.
pub type Start<'w> = (
    Option<Res<'w, StartCorner>>,
    Option<Res<'w, StartTab>>,
    Option<Res<'w, StartDistance>>,
);

fn main() {
    let args = Args::parse();
    let path = PathBuf::from(&args.project);
    let dir = if path.components().count() > 1 || path.exists() {
        path
    } else {
        open_racing_track_project::projects_dir().join(&args.project)
    };
    let mut editor = state::Editor::open(dir).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    if let Some(name) = &args.focus {
        let p = &editor.project;
        let item = p
            .road_index(name)
            .map(state::Item::Road)
            .or_else(|| {
                p.splines
                    .iter()
                    .position(|s| &s.name == name)
                    .map(state::Item::Spline)
            })
            .or_else(|| {
                p.props
                    .iter()
                    .position(|x| &x.name == name)
                    .map(state::Item::Prop)
            });
        match item {
            Some(item) => editor.selection.select(item),
            None => {
                eprintln!("no road, spline or prop named \"{name}\"");
                std::process::exit(1);
            }
        }
    }
    let edit_mode = match &args.edit {
        Some(nodes) if editor.line().is_some() => {
            let count = editor.line().map_or(0, |(_, n, _)| n.len());
            editor.selection.nodes = nodes.iter().copied().filter(|&n| n < count).collect();
            true
        }
        _ => false,
    };
    let mut orbit = viewport::Orbit::default();
    viewport::frame_selection(&editor, &mut orbit);
    if args.top {
        viewport::look(&mut orbit, viewport::ViewDir::Top);
    }
    if let Some(p) = args.pitch {
        orbit.pitch = p.to_radians().clamp(-1.5695, 1.5695);
    }
    if let Some(y) = args.yaw {
        // The orbit's yaw is round Bevy's up axis from its +X (the east), towards +Z (the
        // south).
        orbit.yaw = -y.to_radians();
    }
    let mut tool = viewport::Tool::editing(edit_mode);
    if args.clean {
        tool.overlays = viewport::Overlays {
            lines: false,
            names: false,
            indices: false,
            markers: false,
            stretches: false,
            props: false,
        };
    }

    let mut app = App::new();
    if let Some(n) = args.corner {
        app.insert_resource(StartCorner(n));
    }
    if let Some(d) = args.distance {
        app.insert_resource(StartDistance(d));
    }
    if let Some(name) = &args.tab {
        match ui::PropTab::named(name) {
            Some(tab) => {
                app.insert_resource(StartTab(tab));
            }
            None => {
                eprintln!("no properties tab \"{name}\"");
                std::process::exit(1);
            }
        }
    }
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
            resolution: (1600, 900).into(),
            ..default()
        }),
        ..default()
    }))
    .add_plugins((EguiPlugin::default(), TrackModelPlugin))
    .insert_resource(editor)
    .insert_resource(orbit)
    .init_resource::<viewport::ViewRect>()
    .insert_resource(tool)
    .init_resource::<preview::Rebuild>()
    .init_resource::<preview::Built>()
    .init_resource::<preview::Props>()
    .init_resource::<preview::SharedCache>()
    .init_resource::<assets::Library>()
    .init_resource::<jobs::Jobs>()
    .init_resource::<reference::Shown>()
    .add_systems(Startup, viewport::setup)
    .add_systems(
        PreUpdate,
        ui::keep_tab
            .after(EguiPreUpdateSet::ProcessInput)
            .before(EguiPreUpdateSet::BeginPass),
    )
    .add_systems(EguiPrimaryContextPass, ui::ui)
    .add_systems(
        Update,
        (
            state::watch_file,
            assets::watch,
            assets::dropped,
            reference::show,
            viewport::input,
            viewport::view_input,
            preview::rebuild,
            preview::props,
            preview::show_items,
            preview::rows,
            jobs::poll,
            viewport::gizmos,
            viewport::place_camera,
            auto_screenshot,
        )
            .chain(),
    )
    .run();
}
