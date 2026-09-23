//! Text HUD: speed, gear, rpm, laps, inputs, tyres and force feedback torque.

use std::fmt::Write;

use bevy::prelude::*;

use crate::camera::CameraMode;
use crate::driving::{Mode, Simulation};
use crate::input::{AppRequests, DriverInput};

#[derive(Component)]
struct HudText;

#[derive(Component)]
struct HelpText;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn).add_systems(Update, (update, toggle_help));
    }
}

const HELP: &str = "\
W/S or Up/Down   throttle / brake
A/D or Left/Right steer
E / Q            shift up / down (also L-Shift / L-Ctrl)
C (hold)         clutch
I                restart stalled engine
Backspace        reset car to track
V                cycle camera
P                replay since last reset / stop replay
T                toggle AI driver (with --ai)
H                hide this help
Gamepad: left stick steer, RT/LT throttle/brake, RB/LB or B/X shift";

fn spawn(mut commands: Commands) {
    let panel = |top: bool| Node {
        position_type: PositionType::Absolute,
        left: Val::Px(12.0),
        top: if top { Val::Px(10.0) } else { Val::Auto },
        bottom: if top { Val::Auto } else { Val::Px(10.0) },
        padding: UiRect::all(Val::Px(8.0)),
        ..default()
    };
    commands.spawn((
        HudText,
        Text::new(""),
        TextFont::from_font_size(15.0),
        TextColor(Color::WHITE),
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
        panel(true),
    ));
    commands.spawn((
        HelpText,
        Text::new(HELP),
        TextFont::from_font_size(13.0),
        TextColor(Color::srgb(0.9, 0.9, 0.9)),
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
        panel(false),
    ));
}

fn toggle_help(requests: Res<AppRequests>, mut help: Query<&mut Visibility, With<HelpText>>) {
    if requests.toggle_help {
        for mut v in &mut help {
            *v = if *v == Visibility::Hidden { Visibility::Inherited } else { Visibility::Hidden };
        }
    }
}

fn bar(x: f64) -> String {
    let n = (x.clamp(0.0, 1.0) * 10.0).round() as usize;
    format!("{}{}", "#".repeat(n), ".".repeat(10 - n))
}

fn time(t: Option<f64>) -> String {
    t.map_or("--:--.---".into(), |t| format!("{}:{:06.3}", (t / 60.0) as u32, t % 60.0))
}

fn update(sim: Res<Simulation>, input: Res<DriverInput>, camera: Res<CameraMode>, diagnostics: Res<Time>, mut hud: Query<&mut Text, With<HudText>>) {
    let Ok(mut text) = hud.single_mut() else { return };
    let car = &sim.car;
    let st = &car.state;
    let dt = &st.drivetrain;
    let c = &sim.controls;
    let mode = match sim.mode {
        Mode::Human => format!("DRIVER ({})", if input.device.is_empty() { "keyboard" } else { input.device }),
        Mode::Ai => "AI".into(),
        Mode::Replay => "REPLAY".into(),
    };
    let gear = match (dt.gear, dt.shift_timer > 0.0) {
        (_, true) => "-".into(),
        (-1, _) => "R".into(),
        (0, _) => "N".into(),
        (g, _) => g.to_string(),
    };

    let mut s = String::new();
    let _ = writeln!(s, "{mode}   camera: {}   {:.0} fps", camera.name(), 1.0 / diagnostics.delta_secs().max(1e-3));
    let _ = writeln!(
        s,
        "{:5.0} km/h   gear {gear}   {:5.0} rpm{}",
        car.speed() * 3.6,
        dt.rpm(),
        if dt.stalled { "   ENGINE STALLED (I)" } else { "" }
    );
    let lap = &sim.lap;
    let _ = writeln!(s, "lap {}   now {}   last {}   best {}", lap.laps + 1, time(lap.current_lap), time(lap.last_lap), time(lap.best_lap));
    let _ = writeln!(
        s,
        "throttle {}  brake {}  steer {:+5.0} deg",
        bar(c.throttle),
        bar(c.brake),
        c.steer_wheel_angle.to_degrees()
    );
    let _ = writeln!(s, "tyre   load N   slip deg   slip %");
    for (name, w) in ["FL", "FR", "RL", "RR"].iter().zip(&car.telemetry.wheels) {
        let _ = writeln!(s, "{name}   {:6.0}   {:8.1}   {:6.1}", w.load, w.slip_angle.to_degrees(), w.slip_ratio * 100.0);
    }
    let a = car.telemetry.acceleration;
    let _ = write!(
        s,
        "g long {:+.2}  lat {:+.2}   FFB {:+5.1} Nm   downforce {:.0} N",
        a.x / 9.81,
        a.y / 9.81,
        car.telemetry.steering_torque,
        car.telemetry.downforce[0] + car.telemetry.downforce[1]
    );
    text.0 = s;
}
