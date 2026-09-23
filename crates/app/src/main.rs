//! Bevy front-end. The simulation runs in its own fixed 1 kHz loop inside the
//! `Simulation` resource; Bevy only reads the resulting state for rendering.

mod audio;
mod camera;
mod capture;
mod driving;
mod effects;
mod hud;
mod input;
mod scene;

use std::path::PathBuf;

use bevy::prelude::*;
use bevy::window::PresentMode;
use clap::Parser;

#[derive(Parser, Resource, Clone)]
#[command(about = "open-racing: drive or watch AI agents")]
pub struct Args {
    /// Track name (assets/tracks/<name>.ron) or path.
    #[arg(long, default_value = "lakeside")]
    pub track: String,
    /// Car name (assets/cars/<name>.ron) or path.
    #[arg(long, default_value = "gt3")]
    pub car: String,
    /// Directory of a trained policy to watch (toggle with T).
    #[arg(long)]
    pub ai: Option<PathBuf>,
    /// Let the gearbox shift automatically for the human driver.
    #[arg(long)]
    pub auto_shift: bool,
}

fn main() {
    let args = Args::parse();
    let sim = driving::Simulation::new(&args).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });

    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "open-racing".into(),
            present_mode: PresentMode::AutoVsync,
            ..default()
        }),
        ..default()
    }))
    .insert_resource(ClearColor(Color::srgb(0.55, 0.72, 0.9)))
    .insert_resource(sim)
    .insert_resource(args.clone())
    .add_plugins((input::InputPlugin, driving::DrivingPlugin, scene::ScenePlugin, camera::CameraPlugin, hud::HudPlugin, capture::CapturePlugin, audio::AudioPlugin, effects::EffectsPlugin));
    driving::install_policy(&mut app, &args);
    app.run();
}
