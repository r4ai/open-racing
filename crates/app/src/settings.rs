//! Settings screen (Esc), with pages switched by Tab:
//! - Input: assign steering, pedals, shift buttons and H-shifter gates from any connected device and
//!   check the result on live values. Axes are calibrated while they are assigned,
//!   so inverted pedals and any axis layout work.
//! - Force feedback: the base's peak torque, strength, maximum output in N·m, road
//!   detail, effects, damping and direction, with a test push.
//! - Track: the rubber on the racing line to start from and how fast it builds up.
//! - Weather: the sky, whether it changes, the time of day and how fast it passes, the
//!   month, the temperature and the latitude, with the conditions now.
//! - Graphics: a quality preset, and the details it sets (anti-aliasing, textures,
//!   shadows, ambient occlusion, ...) tuned one by one, with motion blur and VSync.
//! - Assists: the clutch assist, auto-blip and automatic gear selection, with the
//!   gearbox and electronics the car is fitted with.
//! - Realism: whether hits damage the car and whether its engine's parts can fail, with
//!   the brakes' and the engine's temperatures now.
//!
//! The simulation is paused while the screen is open.

use std::collections::HashMap;
use std::fmt::Write;

use bevy::input::gamepad::GamepadInput;
use bevy::prelude::*;

use open_racing_sim::{GearboxKind, Sky, TrackCondition};

use crate::assists::AssistSettings;
use crate::bindings::{Action, Binding, Bindings, Calibration, DeviceId, GATES, Reported, Source};
use crate::driving::Simulation;
use crate::ffb::{self, FfbSettings, FfbStatus, FfbTest};
use crate::graphics::{GraphicsSettings, GraphicsSupport, Preset, Setting};
use crate::input::InputSelection;
use crate::realism::RealismSettings;
use crate::weather::WeatherConfig;

/// Smallest movement over which an axis counts as moved when it is assigned.
const MIN_TRAVEL: f32 = 0.3;
const ROTATION_STEP: f64 = 90.0;
const ROTATION_RANGE: (f64, f64) = (180.0, 1440.0);
/// Force feedback settings: steps and ranges of torques (N·m) and percentages.
const FFB_TORQUE_STEP: f64 = 0.5;
const FFB_WHEEL_TORQUE_RANGE: (f64, f64) = (1.0, 40.0);
const FFB_PERCENT_STEP: f64 = 5.0;
const FFB_STRENGTH_RANGE: (f64, f64) = (0.0, 200.0);
const FFB_DETAIL_RANGE: (f64, f64) = (0.0, 300.0);
const FFB_EFFECTS_RANGE: (f64, f64) = (0.0, 200.0);
const FFB_DAMPING_RANGE: (f64, f64) = (0.0, 100.0);
/// Track evolution: step and range of the grip the racing line gains per lap.
const GRIP_GAIN_STEP: f64 = 0.0005;
const GRIP_GAIN_RANGE: (f64, f64) = (0.0, 0.01);
/// Weather: steps of the time of day (h), the speeds time can pass at, and the ranges of
/// the temperature offset (K) and latitude (degrees).
const HOUR_STEP: f64 = 0.5;
const TIME_SCALES: [f64; 7] = [1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0];
const TEMPERATURE_RANGE: (f64, f64) = (-15.0, 15.0);
const LATITUDE_STEP: f64 = 5.0;
const LATITUDE_RANGE: (f64, f64) = (-60.0, 60.0);
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

#[derive(Resource, Default)]
pub struct SettingsOpen(pub bool);

pub fn settings_closed(open: Res<SettingsOpen>) -> bool {
    !open.0
}

#[derive(Clone, Copy, PartialEq, Default)]
enum Page {
    #[default]
    Input,
    ForceFeedback,
    Track,
    Weather,
    Graphics,
    Assists,
    Realism,
}

/// The pages in the order Tab steps through them, with their names.
const PAGES: [(Page, &str); 7] = [
    (Page::Input, "input"),
    (Page::ForceFeedback, "force feedback"),
    (Page::Track, "track"),
    (Page::Weather, "weather"),
    (Page::Graphics, "graphics"),
    (Page::Assists, "assists"),
    (Page::Realism, "realism"),
];

#[derive(Clone, Copy, PartialEq)]
enum RealismRow {
    Damage,
    Failures,
}

const REALISM_ROWS: [RealismRow; 2] = [RealismRow::Damage, RealismRow::Failures];

#[derive(Clone, Copy, PartialEq)]
enum AssistRow {
    Clutch,
    Blip,
    AutoShift,
}

const ASSIST_ROWS: [AssistRow; 3] = [AssistRow::Clutch, AssistRow::Blip, AssistRow::AutoShift];

#[derive(Clone, Copy, PartialEq)]
enum WeatherRow {
    Sky,
    Changes,
    Time,
    Speed,
    Month,
    Temperature,
    Latitude,
    NewClouds,
}

const WEATHER_ROWS: [WeatherRow; 8] = [
    WeatherRow::Sky,
    WeatherRow::Changes,
    WeatherRow::Time,
    WeatherRow::Speed,
    WeatherRow::Month,
    WeatherRow::Temperature,
    WeatherRow::Latitude,
    WeatherRow::NewClouds,
];

#[derive(Clone, Copy, PartialEq)]
enum TrackRow {
    Condition,
    Gain,
    Restart,
}

const TRACK_ROWS: [TrackRow; 3] = [TrackRow::Condition, TrackRow::Gain, TrackRow::Restart];

#[derive(Clone, Copy, PartialEq)]
enum FfbRow {
    Enabled,
    WheelTorque,
    Strength,
    MaxTorque,
    Detail,
    Effects,
    Damping,
    Direction,
    Test,
}

const FFB_ROWS: [FfbRow; 9] = [
    FfbRow::Enabled,
    FfbRow::WheelTorque,
    FfbRow::Strength,
    FfbRow::MaxTorque,
    FfbRow::Detail,
    FfbRow::Effects,
    FfbRow::Damping,
    FfbRow::Direction,
    FfbRow::Test,
];

#[derive(Clone, Copy, PartialEq)]
enum Row {
    Action(Action),
    Rotation,
}

const ROWS: [Row; 15] = [
    Row::Action(Action::Steer),
    Row::Rotation,
    Row::Action(Action::Throttle),
    Row::Action(Action::Brake),
    Row::Action(Action::Clutch),
    Row::Action(Action::ShiftUp),
    Row::Action(Action::ShiftDown),
    Row::Action(Action::Gate(GATES[0])),
    Row::Action(Action::Gate(GATES[1])),
    Row::Action(Action::Gate(GATES[2])),
    Row::Action(Action::Gate(GATES[3])),
    Row::Action(Action::Gate(GATES[4])),
    Row::Action(Action::Gate(GATES[5])),
    Row::Action(Action::Gate(GATES[6])),
    Row::Action(Action::Gate(GATES[7])),
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
    page: Page,
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
            .add_systems(
                Update,
                (
                    toggle,
                    (
                        switch_page,
                        navigate,
                        navigate_ffb,
                        navigate_track,
                        navigate_weather,
                        navigate_graphics,
                        navigate_assists,
                        navigate_realism,
                    )
                        .chain()
                        .run_if(|o: Res<SettingsOpen>| o.0),
                    listen,
                    render,
                )
                    .chain(),
            );
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
            margin: UiRect {
                left: Val::Px(-400.0),
                top: Val::Px(-190.0),
                ..default()
            },
            width: Val::Px(800.0),
            padding: UiRect::all(Val::Px(16.0)),
            ..default()
        },
        Visibility::Hidden,
    ));
}

fn toggle(
    keys: Res<ButtonInput<KeyCode>>,
    mut open: ResMut<SettingsOpen>,
    mut screen: ResMut<Screen>,
) {
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

fn switch_page(keys: Res<ButtonInput<KeyCode>>, mut screen: ResMut<Screen>) {
    if screen.listen.is_none() && keys.just_pressed(KeyCode::Tab) {
        let i = PAGES.iter().position(|p| p.0 == screen.page).unwrap_or(0);
        screen.page = PAGES[(i + 1) % PAGES.len()].0;
        screen.row = 0;
        screen.message.clear();
    }
}

/// Moves the cursor over `rows` rows with Up/Down.
fn move_cursor(keys: &ButtonInput<KeyCode>, row: &mut usize, rows: usize) {
    if keys.just_pressed(KeyCode::ArrowDown) {
        *row = (*row + 1) % rows;
    }
    if keys.just_pressed(KeyCode::ArrowUp) {
        *row = (*row + rows - 1) % rows;
    }
}

fn navigate_ffb(
    keys: Res<ButtonInput<KeyCode>>,
    mut screen: ResMut<Screen>,
    mut settings: ResMut<FfbSettings>,
    mut test: ResMut<FfbTest>,
) {
    if screen.page != Page::ForceFeedback {
        return;
    }
    move_cursor(&keys, &mut screen.row, FFB_ROWS.len());
    let (left, right, enter) = (
        keys.just_pressed(KeyCode::ArrowLeft),
        keys.just_pressed(KeyCode::ArrowRight),
        keys.just_pressed(KeyCode::Enter),
    );
    let step = |value: &mut f64, step: f64, (min, max): (f64, f64)| match (left, right) {
        (true, false) => *value = (*value - step).max(min),
        (false, true) => *value = (*value + step).min(max),
        _ => {}
    };
    let before = settings.clone();
    let s = &mut *settings;
    match FFB_ROWS[screen.row] {
        FfbRow::Enabled if left || right || enter => s.enabled = !s.enabled,
        FfbRow::WheelTorque => {
            step(&mut s.wheel_torque, FFB_TORQUE_STEP, FFB_WHEEL_TORQUE_RANGE);
            s.max_torque = s.max_torque.min(s.wheel_torque);
        }
        FfbRow::Strength => step(&mut s.strength, FFB_PERCENT_STEP, FFB_STRENGTH_RANGE),
        FfbRow::MaxTorque => step(
            &mut s.max_torque,
            FFB_TORQUE_STEP,
            (FFB_TORQUE_STEP, s.wheel_torque),
        ),
        FfbRow::Detail => step(&mut s.detail, FFB_PERCENT_STEP, FFB_DETAIL_RANGE),
        FfbRow::Effects => step(&mut s.effects, FFB_PERCENT_STEP, FFB_EFFECTS_RANGE),
        FfbRow::Damping => step(&mut s.damping, FFB_PERCENT_STEP, FFB_DAMPING_RANGE),
        FfbRow::Direction if left || right || enter => {
            s.invert = !s.invert;
            test.0 = ffb::TEST_DURATION;
        }
        FfbRow::Test if enter => test.0 = ffb::TEST_DURATION,
        _ => {}
    }
    if *settings != before {
        settings.save();
    }
}

fn navigate_assists(
    keys: Res<ButtonInput<KeyCode>>,
    mut screen: ResMut<Screen>,
    mut settings: ResMut<AssistSettings>,
) {
    if screen.page != Page::Assists {
        return;
    }
    move_cursor(&keys, &mut screen.row, ASSIST_ROWS.len());
    if !(keys.just_pressed(KeyCode::ArrowLeft)
        || keys.just_pressed(KeyCode::ArrowRight)
        || keys.just_pressed(KeyCode::Enter))
    {
        return;
    }
    let s = &mut *settings;
    let flag = match ASSIST_ROWS[screen.row] {
        AssistRow::Clutch => &mut s.clutch,
        AssistRow::Blip => &mut s.blip,
        AssistRow::AutoShift => &mut s.auto_shift,
    };
    *flag = !*flag;
    settings.save();
}

fn navigate_realism(
    keys: Res<ButtonInput<KeyCode>>,
    mut screen: ResMut<Screen>,
    mut settings: ResMut<RealismSettings>,
    mut sim: ResMut<Simulation>,
) {
    if screen.page != Page::Realism {
        return;
    }
    move_cursor(&keys, &mut screen.row, REALISM_ROWS.len());
    if !(keys.just_pressed(KeyCode::ArrowLeft)
        || keys.just_pressed(KeyCode::ArrowRight)
        || keys.just_pressed(KeyCode::Enter))
    {
        return;
    }
    let r = &mut settings.0;
    let flag = match REALISM_ROWS[screen.row] {
        RealismRow::Damage => &mut r.damage,
        RealismRow::Failures => &mut r.failures,
    };
    *flag = !*flag;
    sim.set_realism(settings.0);
    settings.save();
    screen.message = "Applies from now on; a reset (Backspace) repairs the car.".into();
}

fn navigate_graphics(
    keys: Res<ButtonInput<KeyCode>>,
    support: Res<GraphicsSupport>,
    mut screen: ResMut<Screen>,
    mut settings: ResMut<GraphicsSettings>,
) {
    if screen.page != Page::Graphics {
        return;
    }
    // The preset, then each setting.
    move_cursor(&keys, &mut screen.row, 1 + Setting::ALL.len());
    let (left, right, enter) = (
        keys.just_pressed(KeyCode::ArrowLeft),
        keys.just_pressed(KeyCode::ArrowRight),
        keys.just_pressed(KeyCode::Enter),
    );
    let mut s = *settings;
    match screen.row.checked_sub(1).map(|i| Setting::ALL[i]) {
        None if left != right => {
            // Custom settings return to the default preset.
            let preset = s.preset().map_or(Preset::High, |p| {
                let i = Preset::ALL.iter().position(|&x| x == p).unwrap_or(0);
                Preset::ALL[if left {
                    i.saturating_sub(1)
                } else {
                    (i + 1).min(Preset::ALL.len() - 1)
                }]
            });
            s = preset.apply(s);
        }
        Some(setting) if left != right || (enter && setting.is_toggle()) => {
            s.step(setting, right, *support);
        }
        _ => return,
    }
    if s != *settings {
        *settings = s;
        settings.save();
    }
}

fn navigate_track(
    keys: Res<ButtonInput<KeyCode>>,
    mut screen: ResMut<Screen>,
    mut sim: ResMut<Simulation>,
) {
    if screen.page != Page::Track {
        return;
    }
    move_cursor(&keys, &mut screen.row, TRACK_ROWS.len());
    let (left, right, enter) = (
        keys.just_pressed(KeyCode::ArrowLeft),
        keys.just_pressed(KeyCode::ArrowRight),
        keys.just_pressed(KeyCode::Enter),
    );
    let grip = sim.track_grip;
    match TRACK_ROWS[screen.row] {
        // To the next named condition below or above the current level.
        TrackRow::Condition if left => {
            if let Some(c) = TrackCondition::ALL
                .into_iter()
                .rev()
                .find(|c| c.grip() < grip - 1e-6)
            {
                sim.track_grip = c.grip();
            }
        }
        TrackRow::Condition if right => {
            if let Some(c) = TrackCondition::ALL
                .into_iter()
                .find(|c| c.grip() > grip + 1e-6)
            {
                sim.track_grip = c.grip();
            }
        }
        TrackRow::Gain if left || right => {
            let step = if left {
                -GRIP_GAIN_STEP
            } else {
                GRIP_GAIN_STEP
            };
            sim.grip_gain = (sim.grip_gain + step).clamp(GRIP_GAIN_RANGE.0, GRIP_GAIN_RANGE.1);
        }
        TrackRow::Restart if enter => {}
        _ => return,
    }
    sim.restart_track();
    screen.message = "Track restarted.".into();
}

fn navigate_weather(
    keys: Res<ButtonInput<KeyCode>>,
    mut screen: ResMut<Screen>,
    mut config: ResMut<WeatherConfig>,
    mut sim: ResMut<Simulation>,
) {
    if screen.page != Page::Weather {
        return;
    }
    move_cursor(&keys, &mut screen.row, WEATHER_ROWS.len());
    let (left, right, enter) = (
        keys.just_pressed(KeyCode::ArrowLeft),
        keys.just_pressed(KeyCode::ArrowRight),
        keys.just_pressed(KeyCode::Enter),
    );
    let dir: isize = match (left, right) {
        (true, false) => -1,
        (false, true) => 1,
        _ => 0,
    };
    let mut s = config.0;
    let index = |i: usize, n: usize| (i as isize + dir).clamp(0, n as isize - 1) as usize;
    match WEATHER_ROWS[screen.row] {
        WeatherRow::Sky if dir != 0 => {
            let i = Sky::ALL.iter().position(|&k| k == s.sky).unwrap_or(0);
            s.sky = Sky::ALL[index(i, Sky::ALL.len())];
        }
        WeatherRow::Changes if dir != 0 || enter => s.dynamic = !s.dynamic,
        WeatherRow::Time if dir != 0 => {
            s.hour = (s.hour + HOUR_STEP * dir as f64).rem_euclid(24.0);
        }
        WeatherRow::Speed if dir != 0 => {
            let i = TIME_SCALES
                .iter()
                .position(|&x| x >= s.time_scale)
                .unwrap_or(0);
            s.time_scale = TIME_SCALES[index(i, TIME_SCALES.len())];
            // Only the pace changes; the weather carries on.
            config.0 = s;
            config.save();
            sim.weather.settings.time_scale = s.time_scale;
            return;
        }
        WeatherRow::Month if dir != 0 => {
            s.month = ((s.month as isize - 1 + dir).rem_euclid(12) + 1) as u32;
        }
        WeatherRow::Temperature if dir != 0 => {
            s.temperature_offset =
                (s.temperature_offset + dir as f64).clamp(TEMPERATURE_RANGE.0, TEMPERATURE_RANGE.1);
        }
        WeatherRow::Latitude if dir != 0 => {
            s.latitude =
                (s.latitude + LATITUDE_STEP * dir as f64).clamp(LATITUDE_RANGE.0, LATITUDE_RANGE.1);
        }
        WeatherRow::NewClouds if enter => s.seed = s.seed.wrapping_add(0x9E37_79B9),
        _ => return,
    }
    config.0 = s;
    config.save();
    sim.restart_weather(s);
    screen.message = "Weather restarted.".into();
}

fn navigate(
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<(Entity, &Gamepad, &Name)>,
    mut screen: ResMut<Screen>,
    mut bindings: ResMut<Bindings>,
) {
    if screen.listen.is_some() || screen.page != Page::Input {
        return;
    }
    move_cursor(&keys, &mut screen.row, ROWS.len());
    match ROWS[screen.row] {
        Row::Rotation => {
            let step = match (
                keys.just_pressed(KeyCode::ArrowLeft),
                keys.just_pressed(KeyCode::ArrowRight),
            ) {
                (true, false) => -ROTATION_STEP,
                (false, true) => ROTATION_STEP,
                _ => return,
            };
            bindings.steer_rotation =
                (bindings.steer_rotation + step).clamp(ROTATION_RANGE.0, ROTATION_RANGE.1);
            bindings.save();
        }
        Row::Action(action) => {
            if keys.just_pressed(KeyCode::Enter) {
                let mut listen = Listen {
                    action,
                    fresh: true,
                    ranges: HashMap::new(),
                };
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
    let Some(listen) = &mut screen.listen else {
        return;
    };
    if std::mem::take(&mut listen.fresh) {
        return;
    }
    let action = listen.action;
    let found = if action.is_button() {
        pads.iter().find_map(|(e, pad, _)| {
            pad.get_just_pressed()
                .next()
                .map(|&b| (e, Source::Button(b), Calibration::Button))
        })
    } else {
        track(listen, &pads);
        if !keys.just_pressed(KeyCode::Enter) {
            return;
        }
        match listen.best() {
            Some((e, input, (min, max))) if max - min >= MIN_TRAVEL => {
                // The input is back at rest (pedal) or centred (wheel) when Enter is pressed.
                let now = pads
                    .get(e)
                    .ok()
                    .and_then(|(_, pad, _)| pad.get(input))
                    .unwrap_or(0.0);
                let far = if (max - now).abs() > (min - now).abs() {
                    max
                } else {
                    min
                };
                let calibration = match action {
                    Action::Steer => Calibration::Steer {
                        centre: now,
                        sign: (far - now).signum(),
                    },
                    _ => Calibration::Pedal {
                        rest: now,
                        full: far,
                    },
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
    let Some((e, source, calibration)) = found else {
        return;
    };
    let Ok((_, pad, name)) = pads.get(e) else {
        return;
    };
    let binding = Binding {
        device: DeviceId::of(pad, name),
        source,
        calibration,
    };
    screen.message = format!("{} -> {}", action.name(), binding.label());
    bindings.set(action, Some(binding));
    bindings.save();
    *selection = InputSelection::Custom;
    screen.listen = None;
}

fn instructions(action: Action) -> &'static str {
    match action {
        Action::Steer => {
            "Turn the wheel (or stick) fully to the RIGHT, bring it back to centre, then press Enter."
        }
        Action::ShiftUp | Action::ShiftDown => "Press the button or paddle to use.",
        Action::Gate(_) => "Put the shifter into this gear.",
        _ => "Press the pedal (or trigger) fully, release it, then press Enter.",
    }
}

fn bar(x: f32) -> String {
    let n = (x.clamp(0.0, 1.0) * 10.0).round() as usize;
    format!("{}{}", "#".repeat(n), ".".repeat(10 - n))
}

#[allow(clippy::too_many_arguments)]
fn render(
    open: Res<SettingsOpen>,
    screen: Res<Screen>,
    bindings: Res<Bindings>,
    reported: Res<Reported>,
    selection: Res<InputSelection>,
    ffb_settings: Res<FfbSettings>,
    ffb_status: Res<FfbStatus>,
    graphics: Res<GraphicsSettings>,
    graphics_support: Res<GraphicsSupport>,
    weather: Res<WeatherConfig>,
    assists: Res<AssistSettings>,
    realism: Res<RealismSettings>,
    sim: Res<Simulation>,
    pads: Query<(Entity, &Gamepad, &Name)>,
    mut panel: Query<(&mut Text, &mut Visibility), With<SettingsPanel>>,
) {
    let Ok((mut text, mut visibility)) = panel.single_mut() else {
        return;
    };
    let shown = if open.0 {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    visibility.set_if_neq(shown);
    if !open.0 {
        return;
    }
    let tabs = PAGES
        .iter()
        .map(|&(page, name)| {
            if page == screen.page {
                format!("[{}]", name.to_uppercase())
            } else {
                format!(" {name} ")
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let mut s = format!("{tabs}   (Tab page, paused)   Esc close\n\n");
    match screen.page {
        Page::Input => render_input(&mut s, &screen, &bindings, &reported, *selection, &pads),
        Page::ForceFeedback => render_ffb(&mut s, &screen, &ffb_settings, &ffb_status),
        Page::Track => render_track(&mut s, &screen, &sim),
        Page::Weather => render_weather(&mut s, &screen, &weather, &sim),
        Page::Graphics => render_graphics(&mut s, &screen, &graphics, *graphics_support),
        Page::Assists => render_assists(&mut s, &screen, &assists, &sim),
        Page::Realism => render_realism(&mut s, &screen, &realism, &sim),
    }
    text.0 = s;
}

fn render_graphics(
    s: &mut String,
    screen: &Screen,
    settings: &GraphicsSettings,
    support: GraphicsSupport,
) {
    let cursor = |row: usize| if row == screen.row { ">" } else { " " };
    let _ = writeln!(
        s,
        "{} {:<16} {:<12} sets all but motion blur and VSync\n\n  Details",
        cursor(0),
        "Quality",
        settings.preset().map_or("custom", Preset::name)
    );
    for (i, &setting) in Setting::ALL.iter().enumerate() {
        let _ = writeln!(
            s,
            "{} {:<16} {:<12} {}",
            cursor(i + 1),
            setting.name(),
            settings.value(setting, support),
            setting.hint()
        );
    }
    let _ = writeln!(
        s,
        "\nLeft/Right change, Enter on/off. Lower settings run faster."
    );
}

fn render_assists(s: &mut String, screen: &Screen, settings: &AssistSettings, sim: &Simulation) {
    let on_off = |on: bool| if on { "on" } else { "off" };
    for (i, row) in ASSIST_ROWS.iter().enumerate() {
        let cursor = if i == screen.row { ">" } else { " " };
        let (name, on, hint) = match row {
            AssistRow::Clutch => (
                "Clutch",
                settings.clutch,
                "pulls away, saves stalls and works H-pattern shifts",
            ),
            AssistRow::Blip => (
                "Auto-blip",
                settings.blip,
                "matches revs on downshifts (heel-and-toe)",
            ),
            AssistRow::AutoShift => ("Auto shift", settings.auto_shift, "picks the gears"),
        };
        let _ = writeln!(s, "{cursor} {name:<12} {:<5} {hint}", on_off(on));
    }
    let p = &sim.car.model.params;
    let gearbox = match p.gearbox.kind {
        GearboxKind::HPattern { .. } => "H-pattern manual with a clutch pedal",
        GearboxKind::Sequential { .. } => "sequential, paddle shift",
        GearboxKind::DualClutch { .. } => "dual clutch (works its own clutches)",
    };
    let e = &p.electronics;
    let mut fitted = Vec::new();
    if e.anti_stall.is_some() {
        fitted.push("anti-stall");
    }
    if e.auto_blip {
        fitted.push("auto-blip");
    }
    if e.ignition_cut {
        fitted.push("ignition cut");
    }
    if e.downshift_protection_rpm.is_some() {
        fitted.push("downshift protection");
    }
    let _ = writeln!(s, "\n{}: {gearbox}", p.name);
    let _ = writeln!(
        s,
        "Fitted: {}",
        if fitted.is_empty() {
            "no gearbox electronics".to_string()
        } else {
            fitted.join(", ")
        }
    );
    let _ = writeln!(
        s,
        "\nLeft/Right or Enter toggles. The aids work the pedals as a driver would; what the"
    );
    let _ = writeln!(s, "car is fitted with works regardless.");
}

fn render_realism(s: &mut String, screen: &Screen, settings: &RealismSettings, sim: &Simulation) {
    let on_off = |on: bool| if on { "on" } else { "off" };
    for (i, row) in REALISM_ROWS.iter().enumerate() {
        let cursor = if i == screen.row { ">" } else { " " };
        let (name, on, hint) = match row {
            RealismRow::Damage => (
                "Damage",
                settings.0.damage,
                "hits into walls cost downforce and crush the radiator",
            ),
            RealismRow::Failures => (
                "Failures",
                settings.0.failures,
                "overheated or over-revved engine parts wear and break",
            ),
        };
        let _ = writeln!(s, "{cursor} {name:<12} {:<5} {hint}", on_off(on));
    }
    let _ = writeln!(
        s,
        "\nEither way the brakes and the engine heat and cool: cold or overheated pads bite\n\
         less, boiling fluid takes the pedal, hot rims warm the tyres, and the engine's\n\
         power and friction follow its temperatures.\n"
    );
    let st = &sim.car.state;
    let _ = writeln!(
        s,
        "discs C {}   fluid boils at {:.0} C",
        st.wheels
            .iter()
            .map(|w| format!("{:4.0}", w.brake.disc))
            .collect::<Vec<_>>()
            .join(" "),
        sim.car.model.params.brakes.fluid_boiling_point
    );
    let (h, e) = (&st.drivetrain.engine.heat, &sim.car.model.engine.thermal);
    let _ = writeln!(
        s,
        "coolant {:.0} C (thermostat {:.0}, boils {:.0})   oil {:.0} C   over-rev {:.0} rpm",
        h.coolant, e.thermostat, e.boiling_point, h.oil, e.over_rev_rpm
    );
    let _ = writeln!(s, "\nLeft/Right or Enter toggles.");
    if !screen.message.is_empty() {
        let _ = writeln!(s, "{}", screen.message);
    }
}

/// "14:05" for 14.08 h.
pub fn clock(hour: f64) -> String {
    let minutes = (hour.rem_euclid(24.0) * 60.0).round() as u32 % (24 * 60);
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// Compass point of a direction in degrees.
pub fn compass(degrees: f64) -> &'static str {
    const POINTS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    POINTS[((degrees.rem_euclid(360.0) + 22.5) / 45.0) as usize % 8]
}

fn render_weather(s: &mut String, screen: &Screen, config: &WeatherConfig, sim: &Simulation) {
    let c = &config.0;
    for (i, row) in WEATHER_ROWS.iter().enumerate() {
        let cursor = if i == screen.row { ">" } else { " " };
        let (name, value, hint) = match row {
            WeatherRow::Sky => ("Weather", c.sky.name().to_string(), "(Left/Right)"),
            WeatherRow::Changes => (
                "Changes",
                if c.dynamic { "on" } else { "off" }.to_string(),
                "(Enter; the sky drifts between states by itself)",
            ),
            WeatherRow::Time => ("Time", clock(c.hour), "(Left/Right; time at the start)"),
            WeatherRow::Speed => (
                "Time speed",
                format!("{}x", c.time_scale),
                "(Left/Right; how fast the day passes)",
            ),
            WeatherRow::Month => (
                "Month",
                MONTHS[c.month.clamp(1, 12) as usize - 1].to_string(),
                "(Left/Right; season and path of the sun)",
            ),
            WeatherRow::Temperature => (
                "Temperature",
                format!("{:+.0} K", c.temperature_offset),
                "(Left/Right; warmer or colder than usual)",
            ),
            WeatherRow::Latitude => (
                "Latitude",
                format!("{:.0} deg", c.latitude),
                "(Left/Right; of the circuit)",
            ),
            WeatherRow::NewClouds => ("New clouds", String::new(), "Enter: other clouds"),
        };
        let _ = writeln!(s, "{cursor} {name:<12} {value:<14} {hint}");
    }
    let w = &sim.weather;
    let (wind, from) = w.wind();
    let _ = writeln!(
        s,
        "\nNow {}   cloud {:.0} %, heading for {}",
        clock(w.hour()),
        w.cloud_cover() * 100.0,
        w.regime().name(),
    );
    let _ = writeln!(
        s,
        "Air {:.1} C, humidity {:.0} %, {:.0} hPa, density {:.3} kg/m3, wind {:.1} m/s from {}",
        w.air_temperature(),
        w.relative_humidity() * 100.0,
        w.pressure(),
        w.air_density(),
        wind,
        compass(from)
    );
    if let Some((mean, lo, hi)) = w.road().map(|r| r.stats()) {
        let _ = writeln!(
            s,
            "Road {mean:.1} C on average, {lo:.1} C in the coolest shade, {hi:.1} C in the sun"
        );
    }
    let _ = writeln!(s);
    if !screen.message.is_empty() {
        let _ = writeln!(s, "{}", screen.message);
    }
    let _ = writeln!(
        s,
        "Sun and cloud heat the road where they reach it; hot air thins, costing downforce"
    );
    let _ = writeln!(
        s,
        "and power. Changing a setting other than the time speed restarts the weather."
    );
}

fn render_track(s: &mut String, screen: &Screen, sim: &Simulation) {
    let (on_line, off_line) = sim.evolution.start_grip();
    for (i, row) in TRACK_ROWS.iter().enumerate() {
        let cursor = if i == screen.row { ">" } else { " " };
        let line = match row {
            TrackRow::Condition => format!(
                "{:<12} {:<8} racing line {:.0} %, off line {:.0} %   (Left/Right)",
                "Condition",
                TrackCondition::nearest(on_line).name(),
                on_line * 100.0,
                off_line * 100.0
            ),
            TrackRow::Gain => format!(
                "{:<12} +{:.2} % grip per lap   (Left/Right; rubber laid by the tyres)",
                "Rubber",
                sim.grip_gain * 100.0
            ),
            TrackRow::Restart => format!(
                "{:<12} Enter: clear the rubber and dirt the car left",
                "Restart"
            ),
        };
        let _ = writeln!(s, "{cursor} {line}");
    }
    let _ = writeln!(s);
    if !screen.message.is_empty() {
        let _ = writeln!(
            s,
            "{}
",
            screen.message
        );
    }
    let _ = writeln!(
        s,
        "Rubber builds up where the tyres roll, so the racing line grips best. Tyres that"
    );
    let _ = writeln!(
        s,
        "leave the track pick up dirt, lose grip, and drop it on the road where they rejoin."
    );
    let _ = writeln!(s, "Changing a setting restarts the track.");
}

fn render_ffb(s: &mut String, screen: &Screen, settings: &FfbSettings, status: &FfbStatus) {
    for (i, row) in FFB_ROWS.iter().enumerate() {
        let cursor = if i == screen.row { ">" } else { " " };
        let line = match row {
            FfbRow::Enabled => format!(
                "{:<12} {}   (Enter)",
                "FFB",
                if settings.enabled { "on" } else { "off" }
            ),
            FfbRow::WheelTorque => {
                format!(
                    "{:<12} {:.1} Nm   (Left/Right; the base's peak, with its own FFB gain at 100 %)",
                    "Wheel torque", settings.wheel_torque
                )
            }
            FfbRow::Strength => format!(
                "{:<12} {:.0} %   (Left/Right; share of the car's steering torque, 100 % = 1:1)",
                "Strength", settings.strength
            ),
            FfbRow::MaxTorque => format!(
                "{:<12} {:.1} Nm   (Left/Right; the most torque ever sent)",
                "Max output", settings.max_torque
            ),
            FfbRow::Detail => format!(
                "{:<12} {:.0} %   (Left/Right; bumps and kerbs, 100 % = as simulated)",
                "Road detail", settings.detail
            ),
            FfbRow::Effects => format!(
                "{:<12} {:.0} %   (Left/Right; road texture and tyre scrub vibration)",
                "Effects", settings.effects
            ),
            FfbRow::Damping => format!(
                "{:<12} {:.0} %   (Left/Right; steadies strong bases)",
                "Damping", settings.damping
            ),
            FfbRow::Direction => format!(
                "{:<12} {}   (Enter flips it and tests)",
                "Direction",
                if settings.invert {
                    "inverted"
                } else {
                    "normal"
                }
            ),
            FfbRow::Test => format!(
                "{:<12} Enter: a short push that must turn the wheel LEFT",
                "Test"
            ),
        };
        let _ = writeln!(s, "{cursor} {line}");
    }
    let _ = writeln!(s, "\nDevice: {}\n", status.device);
    let _ = writeln!(
        s,
        "Hold the wheel when testing. If the test turns it right, flip the direction,"
    );
    let _ = writeln!(s, "or the force drives the wheel into lock.");
}

fn render_input(
    s: &mut String,
    screen: &Screen,
    bindings: &Bindings,
    reported: &Reported,
    selection: InputSelection,
    pads: &Query<(Entity, &Gamepad, &Name)>,
) {
    for (i, row) in ROWS.iter().enumerate() {
        let cursor = if i == screen.row { ">" } else { " " };
        let line = match *row {
            Row::Rotation => format!(
                "{:<11} {:.0} deg lock to lock   (Left/Right)",
                "Rotation", bindings.steer_rotation
            ),
            Row::Action(action) => {
                let assigned = bindings.get(action).map_or("-".into(), |b| b.label());
                let live = match action {
                    _ if bindings.get(action).is_none() => String::new(),
                    Action::Steer => format!(
                        "{:+5.0} deg",
                        bindings.value(action, pads, reported) as f64 * bindings.steer_rotation
                            / 2.0
                    ),
                    Action::ShiftUp | Action::ShiftDown | Action::Gate(_) => {
                        (if bindings.value(action, pads, reported) > 0.5 {
                            "pressed"
                        } else {
                            ""
                        })
                        .into()
                    }
                    _ => bar(bindings.value(action, pads, reported)),
                };
                format!("{:<11} {assigned:<46} {live}", action.name())
            }
        };
        let _ = writeln!(s, "{cursor} {line}");
    }
    let _ = writeln!(s);
    match &screen.listen {
        Some(listen) => {
            let _ = writeln!(
                s,
                "Assigning {}: {}",
                listen.action.name(),
                instructions(listen.action)
            );
            if let Some((e, input, (min, max))) =
                listen.best().filter(|_| !listen.action.is_button())
                && let Ok((_, pad, name)) = pads.get(e)
                && max - min >= MIN_TRAVEL
            {
                let _ = writeln!(
                    s,
                    "Detected: {} {:?}",
                    DeviceId::of(pad, name).label(),
                    Source::from(input)
                );
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
                if selection == InputSelection::Custom {
                    "Driving with these bindings (input: custom)."
                } else {
                    "Not in use: press Tab outside this screen to select \"custom\"."
                }
            );
        }
    }
}
