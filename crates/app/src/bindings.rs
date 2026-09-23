//! Custom control bindings: assigned on the settings screen, saved to disk and
//! read when the input selection is `Custom`.
//!
//! A binding names a device by its name and USB ids (entities change between runs)
//! and one of its inputs, calibrated when it was assigned. Steering and pedals may
//! come from different devices, as with a wheel base and separate USB pedals.

use std::collections::HashSet;
use std::path::PathBuf;

use bevy::input::gamepad::{
    AxisSettings, ButtonAxisSettings, GamepadAxisChangedEvent, GamepadButtonChangedEvent, GamepadConnectionEvent, GamepadInput, GamepadSettings,
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Pedal travel ignored near rest, so sensor noise does not creep the throttle open.
const PEDAL_DEADZONE: f32 = 0.02;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Steer,
    Throttle,
    Brake,
    Clutch,
    ShiftUp,
    ShiftDown,
}

impl Action {
    pub const ALL: [Action; 6] = [Action::Steer, Action::Throttle, Action::Brake, Action::Clutch, Action::ShiftUp, Action::ShiftDown];

    pub fn name(self) -> &'static str {
        match self {
            Action::Steer => "Steering",
            Action::Throttle => "Throttle",
            Action::Brake => "Brake",
            Action::Clutch => "Clutch",
            Action::ShiftUp => "Shift up",
            Action::ShiftDown => "Shift down",
        }
    }

    pub fn is_button(self) -> bool {
        matches!(self, Action::ShiftUp | Action::ShiftDown)
    }
}

/// `GamepadInput` with serde support.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Source {
    Axis(GamepadAxis),
    Button(GamepadButton),
}

impl From<GamepadInput> for Source {
    fn from(input: GamepadInput) -> Self {
        match input {
            GamepadInput::Axis(a) => Source::Axis(a),
            GamepadInput::Button(b) => Source::Button(b),
        }
    }
}

impl From<Source> for GamepadInput {
    fn from(source: Source) -> Self {
        match source {
            Source::Axis(a) => GamepadInput::Axis(a),
            Source::Button(b) => GamepadInput::Button(b),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceId {
    pub name: String,
    pub vendor: Option<u16>,
    pub product: Option<u16>,
}

impl DeviceId {
    pub fn of(pad: &Gamepad, name: &Name) -> Self {
        Self { name: name.as_str().to_owned(), vendor: pad.vendor_id(), product: pad.product_id() }
    }

    fn matches(&self, pad: &Gamepad, name: &Name) -> bool {
        self.vendor == pad.vendor_id() && self.product == pad.product_id() && self.name == name.as_str()
    }

    /// The first connected pad that is this device.
    pub fn find<'a>(&self, pads: &'a Query<(Entity, &Gamepad, &Name)>) -> Option<(Entity, &'a Gamepad)> {
        pads.iter().find(|(_, pad, name)| self.matches(pad, name)).map(|(e, pad, _)| (e, pad))
    }

    /// Name plus USB ids. Generic HID names repeat and may be localised into glyphs
    /// the HUD font lacks, so those are replaced.
    pub fn label(&self) -> String {
        let id = |v: Option<u16>| v.map_or("?".into(), |v| format!("{v:04x}"));
        let name = if self.name.is_ascii() { self.name.as_str() } else { "HID device" };
        format!("{name} [{}:{}]", id(self.vendor), id(self.product))
    }
}

/// How a raw input value becomes the action's value.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Calibration {
    /// Centred axis; `sign` makes turning right positive.
    Steer { centre: f32, sign: f32 },
    /// Pedal travel from `rest` to `full`.
    Pedal { rest: f32, full: f32 },
    Button,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Binding {
    pub device: DeviceId,
    pub source: Source,
    pub calibration: Calibration,
}

impl Binding {
    /// Steering in -1..1 of half the rotation range (right positive), pedals in 0..1.
    /// An input that has not `reported` yet reads as at rest.
    pub fn value(&self, pad: &Gamepad, reported: bool) -> f32 {
        let raw = if reported { pad.get(GamepadInput::from(self.source)) } else { None };
        match self.calibration {
            Calibration::Steer { centre, sign } => raw.map_or(0.0, |v| ((v - centre) * sign).clamp(-1.0, 1.0)),
            Calibration::Pedal { rest, full } => {
                let travel = raw.map_or(0.0, |v| ((v - rest) / (full - rest)).clamp(0.0, 1.0));
                ((travel - PEDAL_DEADZONE) / (1.0 - PEDAL_DEADZONE)).max(0.0)
            }
            Calibration::Button => match self.source {
                Source::Button(b) if pad.pressed(b) => 1.0,
                _ => 0.0,
            },
        }
    }

    pub fn just_pressed(&self, pad: &Gamepad) -> bool {
        matches!(self.source, Source::Button(b) if pad.just_pressed(b))
    }

    pub fn label(&self) -> String {
        let input = match self.source {
            Source::Axis(a) => format!("{a:?}"),
            Source::Button(b) => format!("{b:?}"),
        };
        format!("{} {input}", self.device.label())
    }
}

#[derive(Resource, Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Bindings {
    /// Lock-to-lock rotation of the steering controller in degrees; the in-game
    /// wheel turns 1:1 with it up to the car's own lock.
    pub steer_rotation: f64,
    pub steer: Option<Binding>,
    pub throttle: Option<Binding>,
    pub brake: Option<Binding>,
    pub clutch: Option<Binding>,
    pub shift_up: Option<Binding>,
    pub shift_down: Option<Binding>,
}

impl Default for Bindings {
    fn default() -> Self {
        Self { steer_rotation: 900.0, steer: None, throttle: None, brake: None, clutch: None, shift_up: None, shift_down: None }
    }
}

impl Bindings {
    pub fn get(&self, action: Action) -> Option<&Binding> {
        self.slot(action).as_ref()
    }

    pub fn set(&mut self, action: Action, binding: Option<Binding>) {
        *self.slot_mut(action) = binding;
    }

    pub fn any(&self) -> bool {
        Action::ALL.iter().any(|&a| self.get(a).is_some())
    }

    fn slot(&self, action: Action) -> &Option<Binding> {
        match action {
            Action::Steer => &self.steer,
            Action::Throttle => &self.throttle,
            Action::Brake => &self.brake,
            Action::Clutch => &self.clutch,
            Action::ShiftUp => &self.shift_up,
            Action::ShiftDown => &self.shift_down,
        }
    }

    fn slot_mut(&mut self, action: Action) -> &mut Option<Binding> {
        match action {
            Action::Steer => &mut self.steer,
            Action::Throttle => &mut self.throttle,
            Action::Brake => &mut self.brake,
            Action::Clutch => &mut self.clutch,
            Action::ShiftUp => &mut self.shift_up,
            Action::ShiftDown => &mut self.shift_down,
        }
    }

    /// The action's value from its bound device, 0 when unbound or disconnected.
    pub fn value(&self, action: Action, pads: &Query<(Entity, &Gamepad, &Name)>, reported: &Reported) -> f32 {
        self.get(action)
            .and_then(|b| b.device.find(pads).map(|(e, pad)| b.value(pad, reported.0.contains(&(e, b.source)))))
            .unwrap_or(0.0)
    }

    pub fn just_pressed(&self, action: Action, pads: &Query<(Entity, &Gamepad, &Name)>) -> bool {
        self.get(action).is_some_and(|b| b.device.find(pads).is_some_and(|(_, pad)| b.just_pressed(pad)))
    }

    /// `%APPDATA%/open-racing/input.ron`, or `~/.config/open-racing/input.ron`.
    fn path() -> Option<PathBuf> {
        let base = std::env::var_os("APPDATA")
            .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("open-racing").join("input.ron"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::path() else { return Self::default() };
        match std::fs::read_to_string(&path) {
            Ok(text) => ron::from_str(&text).unwrap_or_else(|e| {
                warn!("ignoring {}: {e}", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else { return };
        let result = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|_| {
                let text = ron::ser::to_string_pretty(self, Default::default()).map_err(std::io::Error::other)?;
                std::fs::write(&path, text)
            });
        if let Err(e) = result {
            warn!("could not save {}: {e}", path.display());
        }
    }
}

/// Pad inputs that have reported a value since their device connected. Until then
/// Bevy reads 0, which for a pedal resting at -1 would be half travel; many devices
/// only report an input once it moves.
#[derive(Resource, Default)]
pub struct Reported(HashSet<(Entity, Source)>);

pub fn track_reported(
    mut connections: MessageReader<GamepadConnectionEvent>,
    mut axes: MessageReader<GamepadAxisChangedEvent>,
    mut buttons: MessageReader<GamepadButtonChangedEvent>,
    mut reported: ResMut<Reported>,
) {
    for c in connections.read() {
        reported.0.retain(|&(e, _)| e != c.gamepad);
    }
    reported.0.extend(axes.read().map(|a| (a.entity, Source::Axis(a.axis))));
    reported.0.extend(buttons.read().map(|b| (b.entity, Source::Button(b.button))));
}

/// Bound inputs bypass Bevy's default 5 % deadzone and 1 % change threshold: a
/// 900° wheel would otherwise move in 9° steps and lose the start of pedal travel.
pub fn raw_filters(bindings: Res<Bindings>, mut pads: Query<(&Gamepad, &Name, &mut GamepadSettings)>) {
    let raw_axis = AxisSettings::new(-1.0, 0.0, 0.0, 1.0, 0.0).expect("valid axis settings");
    for (pad, name, mut settings) in &mut pads {
        for binding in Action::ALL.iter().filter_map(|&a| bindings.get(a)).filter(|b| b.device.matches(pad, name)) {
            match binding.source {
                Source::Axis(axis) => {
                    if settings.axis_settings.get(&axis) != Some(&raw_axis) {
                        settings.axis_settings.insert(axis, raw_axis.clone());
                    }
                }
                Source::Button(button) => {
                    if settings.button_axis_settings.get(&button).is_none_or(|s| s.low != 0.0 || s.high != 1.0 || s.threshold != 0.0) {
                        settings.button_axis_settings.insert(button, ButtonAxisSettings { high: 1.0, low: 0.0, threshold: 0.0 });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(source: Source, calibration: Calibration) -> Binding {
        Binding { device: DeviceId { name: "pad".into(), vendor: None, product: None }, source, calibration }
    }

    fn pad_with(input: impl Into<GamepadInput>, v: f32) -> Gamepad {
        let mut pad = Gamepad::default();
        pad.analog_mut().set(input, v);
        pad
    }

    #[test]
    fn inverted_pedal_reads_zero_at_rest_and_one_when_pressed() {
        let b = binding(Source::Axis(GamepadAxis::LeftStickY), Calibration::Pedal { rest: 1.0, full: -1.0 });
        assert_eq!(b.value(&pad_with(GamepadAxis::LeftStickY, 1.0), true), 0.0);
        assert_eq!(b.value(&pad_with(GamepadAxis::LeftStickY, -1.0), true), 1.0);
        assert!((b.value(&pad_with(GamepadAxis::LeftStickY, 0.0), true) - 0.5).abs() < 0.02);
    }

    #[test]
    fn unreported_input_reads_as_at_rest() {
        // Bevy reads 0 before the first report, which is half travel for this pedal.
        let b = binding(Source::Axis(GamepadAxis::RightZ), Calibration::Pedal { rest: -1.0, full: 1.0 });
        assert_eq!(b.value(&Gamepad::default(), false), 0.0);
    }

    #[test]
    fn steering_sign_makes_right_positive() {
        let b = binding(Source::Axis(GamepadAxis::LeftStickX), Calibration::Steer { centre: 0.0, sign: -1.0 });
        assert_eq!(b.value(&pad_with(GamepadAxis::LeftStickX, -0.5), true), 0.5);
    }

    #[test]
    fn bindings_round_trip_through_ron() {
        let mut bindings = Bindings::default();
        bindings.set(Action::Brake, Some(binding(Source::Button(GamepadButton::LeftTrigger2), Calibration::Pedal { rest: 0.0, full: 1.0 })));
        let text = ron::ser::to_string_pretty(&bindings, Default::default()).unwrap();
        let back: Bindings = ron::from_str(&text).unwrap();
        assert_eq!(back.get(Action::Brake), bindings.get(Action::Brake));
        assert!(back.get(Action::Steer).is_none());
    }
}
