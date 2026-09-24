//! Bevy front-end. The simulation runs in its own fixed 1 kHz loop inside the
//! `Simulation` resource; Bevy only reads the resulting state for rendering.

mod audio;
mod bindings;
mod camera;
mod capture;
mod driving;
mod effects;
mod ffb;
mod hud;
mod input;
mod scene;
mod settings;
mod track_model;

use std::path::PathBuf;

use bevy::prelude::*;
use bevy::window::PresentMode;
use clap::Parser;

#[derive(Parser, Resource, Clone)]
#[command(about = "open-racing: drive or watch AI agents")]
pub struct Args {
    /// Track name (assets/tracks/<name>.ron) or path. Defaults to the track the `--ai`
    /// policy was trained on, otherwise lakeside.
    #[arg(long)]
    pub track: Option<String>,
    /// Car name (assets/cars/<name>.ron, or a car package in <content>/cars/) or path.
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
    let mut args = Args::parse();
    if args.track.is_none() {
        args.track = driving::policy_track(&args);
    }
    let (sim, track_model, car_model) = driving::Simulation::new(&args).unwrap_or_else(|e| {
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
    .insert_resource(track_model)
    .insert_resource(car_model)
    .insert_resource(args.clone())
    .add_plugins((
        input::InputPlugin,
        driving::DrivingPlugin,
        scene::ScenePlugin,
        camera::CameraPlugin,
        hud::HudPlugin,
        capture::CapturePlugin,
        audio::AudioPlugin,
        effects::EffectsPlugin,
        settings::SettingsPlugin,
        ffb::FfbPlugin,
    ));
    driving::install_policy(&mut app, &args);
    app.run();
}
