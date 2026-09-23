//! Human input devices → physical `Controls`.
//!
//! Every device writes the same `DriverInput` resource in physical units (steering
//! wheel angle in radians, pedal travel 0..1). A steering wheel with force feedback
//! plugs in as one more system here: it writes its angle/pedals directly and reads
//! `Telemetry::steering_torque` for the FFB motor.
//!
//! With several devices connected, `InputSelection` picks which one drives: in
//! `Auto` any device that moves takes over, otherwise only the chosen one is read.
//! App requests (reset, camera, ...) always come from the keyboard.

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
/// Half the rotation range of a steering wheel controller (900° lock to lock),
/// so the in-game wheel turns 1:1 with the real one.
const WHEEL_HALF_ROTATION: f64 = 450.0 * std::f64::consts::PI / 180.0;
/// Steering wheel controllers report through the gamepad API with the wheel on the
/// left stick X axis. They are recognised by vendor (MOZA, Fanatec), since some only
/// report a generic HID name, or by these lowercase name fragments.
const WHEEL_VENDORS: &[u16] = &[0x346e, 0x0eb7];
const WHEEL_NAMES: &[&str] = &[
    "wheel", "racing", "driving force", "g25", "g27", "g29", "g920", "g923", "fanatec", "csl", "clubsport", "podium",
    "t150", "t248", "t300", "t500", "tmx", "t-gt", "moza", "simucube", "simagic",
];

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
    pub toggle_mute: bool,
}

/// Which device drives the car. Cycled with Tab (or Select on a gamepad).
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq)]
pub enum InputSelection {
    /// Whichever device was used last.
    #[default]
    Auto,
    Keyboard,
    /// A gamepad or steering wheel; falls back to `Auto` when it disconnects.
    Pad(Entity),
}

impl InputSelection {
    pub fn label(self, pads: &Query<(Entity, &Gamepad, &Name)>) -> String {
        match self {
            Self::Auto => "auto".into(),
            Self::Keyboard => "keyboard".into(),
            Self::Pad(e) => pads.get(e).map_or("auto".into(), |(_, pad, name)| {
                // Generic HID names repeat (and may not render), so add the USB ids.
                let id = |v: Option<u16>| v.map_or("?".into(), |v| format!("{v:04x}"));
                format!("{name} [{}:{}] ({})", id(pad.vendor_id()), id(pad.product_id()), pad_kind(pad, name))
            }),
        }
    }
}

fn is_wheel(pad: &Gamepad, name: &str) -> bool {
    let name = name.to_lowercase();
    pad.vendor_id().is_some_and(|v| WHEEL_VENDORS.contains(&v)) || WHEEL_NAMES.iter().any(|w| name.contains(w))
}

fn pad_kind(pad: &Gamepad, name: &str) -> &'static str {
    if is_wheel(pad, name) { "wheel" } else { "gamepad" }
}

/// Input systems; anything overriding requests must run after this set.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct InputSystemsSet;

pub struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DriverInput>()
            .init_resource::<AppRequests>()
            .init_resource::<InputSelection>()
            .add_systems(PreUpdate, (select, keyboard, gamepad).chain().in_set(InputSystemsSet).after(bevy::input::InputSystems));
    }
}

fn ramp(current: f64, pressed: bool, dt: f64) -> f64 {
    if pressed {
        (current + dt / PEDAL_APPLY_TIME).min(1.0)
    } else {
        (current - dt / PEDAL_RELEASE_TIME).max(0.0)
    }
}

/// Cycles Auto → Keyboard → each connected pad → Auto.
fn select(keys: Res<ButtonInput<KeyCode>>, pads: Query<(Entity, &Gamepad, &Name)>, mut selection: ResMut<InputSelection>) {
    if let InputSelection::Pad(e) = *selection
        && !pads.contains(e)
    {
        *selection = InputSelection::Auto;
    }
    if !(keys.just_pressed(KeyCode::Tab) || pads.iter().any(|(_, pad, _)| pad.just_pressed(GamepadButton::Select))) {
        return;
    }
    let mut order: Vec<Entity> = pads.iter().map(|(e, ..)| e).collect();
    order.sort();
    let next = match *selection {
        InputSelection::Auto => {
            *selection = InputSelection::Keyboard;
            return;
        }
        InputSelection::Keyboard => order.first(),
        InputSelection::Pad(cur) => order.iter().skip_while(|&&e| e != cur).nth(1),
    };
    *selection = next.map_or(InputSelection::Auto, |&e| InputSelection::Pad(e));
}

fn keyboard(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    sim: Res<Simulation>,
    selection: Res<InputSelection>,
    mut input: ResMut<DriverInput>,
    mut requests: ResMut<AppRequests>,
) {
    requests.reset_car = keys.just_pressed(KeyCode::Backspace);
    requests.toggle_ai = keys.just_pressed(KeyCode::KeyT);
    requests.toggle_replay = keys.just_pressed(KeyCode::KeyP);
    requests.cycle_camera = keys.just_pressed(KeyCode::KeyV);
    requests.restart_engine = keys.just_pressed(KeyCode::KeyI);
    requests.toggle_help = keys.just_pressed(KeyCode::KeyH);
    requests.toggle_mute = keys.just_pressed(KeyCode::KeyM);
    if matches!(*selection, InputSelection::Pad(_)) {
        return;
    }

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
    if left || right || throttle || brake || c.shift != Shift::None || *selection == InputSelection::Keyboard {
        input.device = "keyboard";
    }
}


/// One frame of pad input, in the pad's own units.
struct PadReading {
    stick: f64,
    throttle: f64,
    brake: f64,
    up: bool,
    down: bool,
}

impl PadReading {
    fn new(pad: &Gamepad) -> Self {
        Self {
            stick: pad.left_stick().x as f64,
            throttle: pad.get(GamepadButton::RightTrigger2).unwrap_or(0.0) as f64,
            brake: pad.get(GamepadButton::LeftTrigger2).unwrap_or(0.0) as f64,
            up: pad.just_pressed(GamepadButton::RightTrigger) || pad.just_pressed(GamepadButton::East),
            down: pad.just_pressed(GamepadButton::LeftTrigger) || pad.just_pressed(GamepadButton::West),
        }
    }

    fn active(&self) -> bool {
        self.stick.abs() > 0.05 || self.throttle > 0.02 || self.brake > 0.02 || self.up || self.down
    }
}

fn gamepad(pads: Query<(Entity, &Gamepad, &Name)>, sim: Res<Simulation>, selection: Res<InputSelection>, mut input: ResMut<DriverInput>) {
    // In Auto an idle pad must not overwrite the keyboard; a chosen pad always drives.
    let (pad, name, r) = match *selection {
        InputSelection::Keyboard => return,
        InputSelection::Auto => {
            let Some(found) = pads.iter().map(|(_, pad, name)| (pad, name, PadReading::new(pad))).find(|(.., r)| r.active()) else { return };
            found
        }
        InputSelection::Pad(e) => {
            let Ok((_, pad, name)) = pads.get(e) else { return };
            input.controls.clutch = 0.0;
            (pad, name, PadReading::new(pad))
        }
    };
    let lock = sim.car.model.params.steering.lock;
    let wheel = is_wheel(pad, name);
    let c = &mut input.controls;
    c.steer_wheel_angle =
        if wheel { (-r.stick * WHEEL_HALF_ROTATION).clamp(-lock, lock) } else { -r.stick * lock * STICK_STEER_FRACTION };
    c.throttle = r.throttle;
    c.brake = r.brake;
    if r.up {
        c.shift = Shift::Up;
    } else if r.down {
        c.shift = Shift::Down;
    }
    input.device = pad_kind(pad, name);
}
