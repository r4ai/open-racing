//! Human input devices → physical `Controls`.
//!
//! Every device writes the same `DriverInput` resource in physical units (steering
//! wheel angle in radians, pedal travel 0..1). A steering wheel with force feedback
//! plugs in as one more system here: it writes its angle/pedals directly and reads
//! `Telemetry::steering_torque` for the FFB motor.

use bevy::prelude::*;
use open_racing_sim::{Controls, Shift};

use crate::driving::Simulation;

/// Keyboard steering: maximum steering wheel angle and how fast it is reached.
/// A keyboard is binary, so it needs *some* ramp; the physics is untouched.
const KEY_STEER_MAX: f64 = 2.4; // rad at the steering wheel (~140°)
const KEY_STEER_RATE: f64 = 3.5; // rad/s towards the target
const KEY_STEER_RETURN: f64 = 6.0; // rad/s back to centre
const PEDAL_APPLY_TIME: f64 = 0.12; // s from 0 to full
const PEDAL_RELEASE_TIME: f64 = 0.08; // s from full to 0
/// Share of full lock reached at full stick deflection.
const STICK_STEER_FRACTION: f64 = 0.5;

#[derive(Resource, Default)]
pub struct DriverInput {
    pub controls: Controls,
    /// Name of the device that produced the last input, for the HUD.
    pub device: &'static str,
}

/// One-shot requests from the keyboard for the app (not the car).
#[derive(Resource, Default)]
pub struct AppRequests {
    pub reset_car: bool,
    pub toggle_ai: bool,
    pub toggle_replay: bool,
    pub cycle_camera: bool,
    pub restart_engine: bool,
    pub toggle_help: bool,
}

/// Input systems; anything overriding requests must run after this set.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct InputSystemsSet;

pub struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DriverInput>()
            .init_resource::<AppRequests>()
            .add_systems(PreUpdate, (keyboard, gamepad).chain().in_set(InputSystemsSet).after(bevy::input::InputSystems));
    }
}

fn ramp(current: f64, pressed: bool, dt: f64) -> f64 {
    if pressed {
        (current + dt / PEDAL_APPLY_TIME).min(1.0)
    } else {
        (current - dt / PEDAL_RELEASE_TIME).max(0.0)
    }
}

fn keyboard(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    sim: Res<Simulation>,
    mut input: ResMut<DriverInput>,
    mut requests: ResMut<AppRequests>,
) {
    let dt = time.delta_secs_f64();
    let any = |codes: &[KeyCode]| codes.iter().any(|&k| keys.pressed(k));
    let left = any(&[KeyCode::KeyA, KeyCode::ArrowLeft]);
    let right = any(&[KeyCode::KeyD, KeyCode::ArrowRight]);
    let throttle = any(&[KeyCode::KeyW, KeyCode::ArrowUp]);
    let brake = any(&[KeyCode::KeyS, KeyCode::ArrowDown]);

    let c = &mut input.controls;
    let lock = sim.car.model.params.steering.lock;
    let target = match (left, right) {
        (true, false) => KEY_STEER_MAX.min(lock),
        (false, true) => -KEY_STEER_MAX.min(lock),
        _ => 0.0,
    };
    let rate = if target == 0.0 || target.signum() != c.steer_wheel_angle.signum() { KEY_STEER_RETURN } else { KEY_STEER_RATE };
    c.steer_wheel_angle += (target - c.steer_wheel_angle).clamp(-rate * dt, rate * dt);
    c.throttle = ramp(c.throttle, throttle, dt);
    c.brake = ramp(c.brake, brake, dt);
    c.clutch = if keys.pressed(KeyCode::KeyC) { 1.0 } else { 0.0 };
    c.shift = if keys.just_pressed(KeyCode::KeyE) || keys.just_pressed(KeyCode::ShiftLeft) {
        Shift::Up
    } else if keys.just_pressed(KeyCode::KeyQ) || keys.just_pressed(KeyCode::ControlLeft) {
        Shift::Down
    } else {
        Shift::None
    };
    if left || right || throttle || brake || c.shift != Shift::None {
        input.device = "keyboard";
    }

    requests.reset_car = keys.just_pressed(KeyCode::Backspace);
    requests.toggle_ai = keys.just_pressed(KeyCode::KeyT);
    requests.toggle_replay = keys.just_pressed(KeyCode::KeyP);
    requests.cycle_camera = keys.just_pressed(KeyCode::KeyV);
    requests.restart_engine = keys.just_pressed(KeyCode::KeyI);
    requests.toggle_help = keys.just_pressed(KeyCode::KeyH);
}

fn gamepad(gamepads: Query<&Gamepad>, sim: Res<Simulation>, mut input: ResMut<DriverInput>) {
    let Some(pad) = gamepads.iter().next() else { return };
    let stick = pad.left_stick().x as f64;
    let throttle = pad.get(GamepadButton::RightTrigger2).unwrap_or(0.0) as f64;
    let brake = pad.get(GamepadButton::LeftTrigger2).unwrap_or(0.0) as f64;
    let active = stick.abs() > 0.05 || throttle > 0.02 || brake > 0.02;
    let up = pad.just_pressed(GamepadButton::RightTrigger) || pad.just_pressed(GamepadButton::East);
    let down = pad.just_pressed(GamepadButton::LeftTrigger) || pad.just_pressed(GamepadButton::West);
    if !(active || up || down) {
        return;
    }
    let lock = sim.car.model.params.steering.lock;
    let c = &mut input.controls;
    c.steer_wheel_angle = -stick * lock * STICK_STEER_FRACTION;
    c.throttle = throttle;
    c.brake = brake;
    if up {
        c.shift = Shift::Up;
    } else if down {
        c.shift = Shift::Down;
    }
    input.device = "gamepad";
}
