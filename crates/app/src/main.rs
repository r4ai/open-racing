//! Bevy front-end. The simulation runs in its own fixed 1 kHz loop inside the
//! `Simulation` resource; Bevy only reads the resulting state for rendering.

mod audio;
mod bindings;
mod camera;
mod capture;
mod clouds;
mod driving;
mod effects;
mod ffb;
mod graphics;
mod hud;
mod input;
mod scene;
mod settings;
mod track_model;
mod tyre_dirt;
mod vr;
mod weather;

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
    /// Track evolution: grip on the racing line at the start (dusty, green, fast,
    /// optimum, or e.g. 0.97). Off the line the asphalt stays dirtier. Also set in the
    /// settings screen (Esc, Tab to "track").
    #[arg(long, default_value = "optimum", value_parser = open_racing_sim::parse_grip)]
    pub track_grip: f64,
    /// Grip the racing line gains per lap driven, as rubber is laid down.
    #[arg(long, default_value_t = open_racing_sim::TrackEvolution::DEFAULT_GAIN_PER_LAP)]
    pub grip_gain: f64,
    /// Drive in VR through OpenXR (SteamVR, Quest Link, ...), seated in the cockpit.
    /// R recenters the view.
    #[arg(long)]
    pub vr: bool,
    /// Weather at the start: clear, fair, partly-cloudy, cloudy or overcast. Also set in
    /// the settings screen (Esc, Tab to "weather"), which keeps it for the next run.
    #[arg(long, value_parser = weather::parse_sky)]
    pub weather: Option<String>,
    /// Time of day at the start, e.g. 14:30.
    #[arg(long, value_parser = weather::parse_time)]
    pub time: Option<f64>,
}

fn main() {
    let mut args = Args::parse();
    if args.track.is_none() {
        args.track = driving::policy_track(&args);
    }
    let weather = weather::WeatherConfig::load(&args);
    let (sim, track_model, car_model) =
        driving::Simulation::new(&args, weather.0).unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(1);
        });

    let plugins = DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "open-racing".into(),
            resolution: if std::env::var_os("OPEN_RACING_CAPTURE_QUALITY").is_some() {
                bevy::window::WindowResolution::new(2560, 1440).with_scale_factor_override(1.0)
            } else {
                default()
            },
            // In VR the headset paces the frames; the window must not hold them back.
            present_mode: if args.vr {
                PresentMode::AutoNoVsync
            } else {
                PresentMode::AutoVsync
            },
            ..default()
        }),
        ..default()
    });
    let mut app = App::new();
    if args.vr {
        app.add_plugins((vr::plugins(plugins), vr::VrPlugin));
    } else {
        app.add_plugins(plugins);
    }
    app.insert_resource(ClearColor(Color::srgb(0.55, 0.72, 0.9)))
        .insert_resource(sim)
        .insert_resource(weather)
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
            graphics::GraphicsPlugin,
            tyre_dirt::TyreDirtPlugin,
            weather::WeatherPlugin,
        ));
    driving::install_policy(&mut app, &args);
    app.run();
}
