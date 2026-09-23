//! Input settings screen (Esc): assign steering, pedals and shift buttons from any
//! connected device and check the result on live values. Axes are calibrated while
//! they are assigned, so inverted pedals and any axis layout work. The simulation
//! is paused while the screen is open.

use std::collections::HashMap;
use std::fmt::Write;

use bevy::input::gamepad::GamepadInput;
use bevy::prelude::*;

use crate::bindings::{Action, Binding, Bindings, Calibration, DeviceId, Reported, Source};
use crate::input::InputSelection;

/// Smallest movement over which an axis counts as moved when it is assigned.
const MIN_TRAVEL: f32 = 0.3;
const ROTATION_STEP: f64 = 90.0;
const ROTATION_RANGE: (f64, f64) = (180.0, 1440.0);

#[derive(Resource, Default)]
pub struct SettingsOpen(pub bool);

pub fn settings_closed(open: Res<SettingsOpen>) -> bool {
    !open.0
}

#[derive(Clone, Copy, PartialEq)]
enum Row {
    Action(Action),
    Rotation,
}

const ROWS: [Row; 7] = [
    Row::Action(Action::Steer),
    Row::Rotation,
    Row::Action(Action::Throttle),
    Row::Action(Action::Brake),
    Row::Action(Action::Clutch),
    Row::Action(Action::ShiftUp),
    Row::Action(Action::ShiftDown),
];

/// An assignment in progress: the range each analog input has covered so far.
struct Listen {
    action: Action,
    /// Set on the frame the Enter that started it was pressed, which must not finish it.
    fresh: bool,
    ranges: HashMap<(Entity, GamepadInput), (f32, f32)>,
}

impl Listen {
    /// The input that moved the most, with its (min, max).
    fn best(&self) -> Option<(Entity, GamepadInput, (f32, f32))> {
        self.ranges
            .iter()
            .max_by(|a, b| (a.1.1 - a.1.0).total_cmp(&(b.1.1 - b.1.0)))
            .map(|(&(e, input), &range)| (e, input, range))
    }
}

#[derive(Resource, Default)]
struct Screen {
    row: usize,
    listen: Option<Listen>,
    message: String,
}

#[derive(Component)]
struct SettingsPanel;

pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SettingsOpen>()
            .init_resource::<Screen>()
            .add_systems(Startup, spawn)
            .add_systems(Update, (toggle, navigate.run_if(|o: Res<SettingsOpen>| o.0), listen, render).chain());
    }
}

fn spawn(mut commands: Commands) {
    commands.spawn((
        SettingsPanel,
        Text::new(""),
        TextFont::from_font_size(16.0),
        TextColor(Color::WHITE),
        BackgroundColor(Color::srgba(0.02, 0.02, 0.03, 0.96)),
        GlobalZIndex(10),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Percent(50.0),
            top: Val::Percent(50.0),
            margin: UiRect { left: Val::Px(-400.0), top: Val::Px(-190.0), ..default() },
            width: Val::Px(800.0),
            padding: UiRect::all(Val::Px(16.0)),
            ..default()
        },
        Visibility::Hidden,
    ));
}

fn toggle(keys: Res<ButtonInput<KeyCode>>, mut open: ResMut<SettingsOpen>, mut screen: ResMut<Screen>) {
    if !keys.just_pressed(KeyCode::Escape) {
        return;
    }
    if screen.listen.take().is_some() {
        screen.message = "Cancelled.".into();
    } else {
        open.0 = !open.0;
        screen.message.clear();
    }
}

fn navigate(
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<(Entity, &Gamepad, &Name)>,
    mut screen: ResMut<Screen>,
    mut bindings: ResMut<Bindings>,
) {
    if screen.listen.is_some() {
        return;
    }
    if keys.just_pressed(KeyCode::ArrowDown) {
        screen.row = (screen.row + 1) % ROWS.len();
    }
    if keys.just_pressed(KeyCode::ArrowUp) {
        screen.row = (screen.row + ROWS.len() - 1) % ROWS.len();
    }
    match ROWS[screen.row] {
        Row::Rotation => {
            let step = match (keys.just_pressed(KeyCode::ArrowLeft), keys.just_pressed(KeyCode::ArrowRight)) {
                (true, false) => -ROTATION_STEP,
                (false, true) => ROTATION_STEP,
                _ => return,
            };
            bindings.steer_rotation = (bindings.steer_rotation + step).clamp(ROTATION_RANGE.0, ROTATION_RANGE.1);
            bindings.save();
        }
        Row::Action(action) => {
            if keys.just_pressed(KeyCode::Enter) {
                let mut listen = Listen { action, fresh: true, ranges: HashMap::new() };
                track(&mut listen, &pads);
                screen.listen = Some(listen);
                screen.message.clear();
            } else if keys.just_pressed(KeyCode::Delete) || keys.just_pressed(KeyCode::Backspace) {
                bindings.set(action, None);
                bindings.save();
                screen.message = format!("{} cleared.", action.name());
            }
        }
    }
}

/// Widens each analog input's covered range by its current value.
fn track(listen: &mut Listen, pads: &Query<(Entity, &Gamepad, &Name)>) {
    for (e, pad, _) in pads {
        for &input in pad.get_analog_axes() {
            let Some(v) = pad.get(input) else { continue };
            let range = listen.ranges.entry((e, input)).or_insert((v, v));
            *range = (range.0.min(v), range.1.max(v));
        }
    }
}

fn listen(
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<(Entity, &Gamepad, &Name)>,
    mut screen: ResMut<Screen>,
    mut bindings: ResMut<Bindings>,
    mut selection: ResMut<InputSelection>,
) {
    let Some(listen) = &mut screen.listen else { return };
    if std::mem::take(&mut listen.fresh) {
        return;
    }
    let action = listen.action;
    let found = if action.is_button() {
        pads.iter().find_map(|(e, pad, _)| pad.get_just_pressed().next().map(|&b| (e, Source::Button(b), Calibration::Button)))
    } else {
        track(listen, &pads);
        if !keys.just_pressed(KeyCode::Enter) {
            return;
        }
        match listen.best() {
            Some((e, input, (min, max))) if max - min >= MIN_TRAVEL => {
                // The input is back at rest (pedal) or centred (wheel) when Enter is pressed.
                let now = pads.get(e).ok().and_then(|(_, pad, _)| pad.get(input)).unwrap_or(0.0);
                let far = if (max - now).abs() > (min - now).abs() { max } else { min };
                let calibration = match action {
                    Action::Steer => Calibration::Steer { centre: now, sign: (far - now).signum() },
                    _ => Calibration::Pedal { rest: now, full: far },
                };
                Some((e, input.into(), calibration))
            }
            _ => {
                screen.listen = None;
                screen.message = "No movement detected; nothing assigned.".into();
                return;
            }
        }
    };
    let Some((e, source, calibration)) = found else { return };
    let Ok((_, pad, name)) = pads.get(e) else { return };
    let binding = Binding { device: DeviceId::of(pad, name), source, calibration };
    screen.message = format!("{} -> {}", action.name(), binding.label());
    bindings.set(action, Some(binding));
    bindings.save();
    *selection = InputSelection::Custom;
    screen.listen = None;
}

fn instructions(action: Action) -> &'static str {
    match action {
        Action::Steer => "Turn the wheel (or stick) fully to the RIGHT, bring it back to centre, then press Enter.",
        Action::ShiftUp | Action::ShiftDown => "Press the button or paddle to use.",
        _ => "Press the pedal (or trigger) fully, release it, then press Enter.",
    }
}

fn bar(x: f32) -> String {
    let n = (x.clamp(0.0, 1.0) * 10.0).round() as usize;
    format!("{}{}", "#".repeat(n), ".".repeat(10 - n))
}

fn render(
    open: Res<SettingsOpen>,
    screen: Res<Screen>,
    bindings: Res<Bindings>,
    reported: Res<Reported>,
    selection: Res<InputSelection>,
    pads: Query<(Entity, &Gamepad, &Name)>,
    mut panel: Query<(&mut Text, &mut Visibility), With<SettingsPanel>>,
) {
    let Ok((mut text, mut visibility)) = panel.single_mut() else { return };
    let shown = if open.0 { Visibility::Inherited } else { Visibility::Hidden };
    visibility.set_if_neq(shown);
    if !open.0 {
        return;
    }
    let mut s = String::new();
    let _ = writeln!(s, "INPUT SETTINGS  (simulation paused)                     Esc close\n");
    for (i, row) in ROWS.iter().enumerate() {
        let cursor = if i == screen.row { ">" } else { " " };
        let line = match *row {
            Row::Rotation => format!("{:<11} {:.0} deg lock to lock   (Left/Right)", "Rotation", bindings.steer_rotation),
            Row::Action(action) => {
                let assigned = bindings.get(action).map_or("-".into(), |b| b.label());
                let live = match action {
                    _ if bindings.get(action).is_none() => String::new(),
                    Action::Steer => format!("{:+5.0} deg", bindings.value(action, &pads, &reported) as f64 * bindings.steer_rotation / 2.0),
                    Action::ShiftUp | Action::ShiftDown => (if bindings.value(action, &pads, &reported) > 0.5 { "pressed" } else { "" }).into(),
                    _ => bar(bindings.value(action, &pads, &reported)),
                };
                format!("{:<11} {assigned:<46} {live}", action.name())
            }
        };
        let _ = writeln!(s, "{cursor} {line}");
    }
    let _ = writeln!(s);
    match &screen.listen {
        Some(listen) => {
            let _ = writeln!(s, "Assigning {}: {}", listen.action.name(), instructions(listen.action));
            if let Some((e, input, (min, max))) = listen.best().filter(|_| !listen.action.is_button())
                && let Ok((_, pad, name)) = pads.get(e)
                && max - min >= MIN_TRAVEL
            {
                let _ = writeln!(s, "Detected: {} {:?}", DeviceId::of(pad, name).label(), Source::from(input));
            }
            let _ = writeln!(s, "Esc cancel");
        }
        None => {
            if !screen.message.is_empty() {
                let _ = writeln!(s, "{}", screen.message);
            }
            let _ = writeln!(s, "Up/Down select   Enter assign   Delete clear");
            let _ = writeln!(
                s,
                "{}",
                if *selection == InputSelection::Custom {
                    "Driving with these bindings (input: custom)."
                } else {
                    "Not in use: press Tab outside this screen to select \"custom\"."
                }
            );
        }
    }
    text.0 = s;
}
