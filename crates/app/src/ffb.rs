//! Force feedback: plays the simulated steering torque on the wheel that steers.
//!
//! The simulation already computes the torque the front contact patches put on the
//! steering wheel (`Telemetry::steering_torque`). Here it is scaled to the motor's
//! range and sent as one constant force, updated every frame. The driver's own
//! centring spring is switched off, so everything felt comes from the tyres,
//! including the torque dropping away as the fronts reach their grip limit and the
//! kicks of bumps and kerbs, which the road detail setting can bring out further.
//!
//! On top of it the wheel plays two vibrations, as most sims do for what the physics
//! cannot carry: the grain of the surface under the front tyres (the simulated road
//! is smooth below a few centimetres), rougher off the track, and the scrub of front
//! tyres sliding. Both grow with speed and with the load on the tyres.
//!
//! The base's peak torque, strength, maximum output, road detail, effects, damping and
//! direction are set on the settings screen and saved to `ffb.ron`, since wheels
//! differ in torque and in which way they push. Knowing the peak torque lets the
//! force be set in N·m at the rim.
//! Only Windows (DirectInput) is supported; elsewhere the plugin reports that.

#[cfg(windows)]
mod dinput;

use bevy::prelude::*;
use bevy::window::{PrimaryWindow, RawHandleWrapper, WindowCloseRequested};

use serde::{Deserialize, Serialize};

use open_racing_sim::{Car, FL, FR, GRAVITY, Surface};

use crate::bindings::{self, Bindings};
use crate::driving::{self, Mode, Simulation};
use crate::effects::{slide, smoothstep};
use crate::input::{self, InputSelection};
use crate::settings::SettingsOpen;

/// Seconds over which the force fades in after driving starts or resumes, so the
/// wheel never jerks.
const FADE_IN: f64 = 1.0;
/// Seconds between attempts to open a wheel that failed to open.
const RETRY_INTERVAL: f64 = 2.0;
/// Damping torque per steering wheel speed at 100 % damping, N·m per rad/s.
const DAMPING_FULL: f64 = 0.5;
/// Time constant smoothing the steering wheel speed for damping, s.
const RATE_SMOOTHING: f64 = 0.03;
/// Time constant separating road detail (bumps, kerbs) from the steady steering
/// torque, s: changes faster than this count as detail.
const DETAIL_SMOOTHING: f64 = 0.02;
/// Vibrations the wheel plays on top of the torque: road texture and tyre scrub.
pub const VIBRATIONS: usize = 2;
/// Speeds over which the vibrations fade in from a standstill, m/s.
const VIBRATION_SPEED: (f64, f64) = (0.5, 8.0);
/// Frequency range of the road texture, Hz.
const TEXTURE_HZ: (f64, f64) = (5.0, 80.0);
/// Tyre scrub: rim torque with both front tyres fully sliding, N·m, and its frequency
/// at the start of sliding and per m/s of sliding speed, Hz.
const SCRUB_TORQUE: f64 = 0.4;
const SCRUB_HZ: f64 = 28.0;
const SCRUB_HZ_PER_SPEED: f64 = 2.0;
const SCRUB_MAX_HZ: f64 = 60.0;
/// Frame-to-frame spread of the vibrations' strength, so the grain is not a tone.
const JITTER: f64 = 0.3;
/// Runaway guard: the wheel held at the car's lock for this long while the tyres
/// pull back towards centre with at least this torque (N·m) pauses the force until
/// the wheel leaves lock. A driver can turn a 900° wheel past the car's lock, but a
/// motor pushing the wrong way (inverted direction) slams it there again and again,
/// so this many pauses, each within the window (s) of the last, stop it for good.
const RUNAWAY_TORQUE: f64 = 1.0;
const RUNAWAY_LOCK_SHARE: f64 = 0.98;
const RUNAWAY_TIME: f64 = 0.5;
const RUNAWAY_REPEATS: u32 = 3;
const RUNAWAY_WINDOW: f64 = 10.0;
/// Seconds between checks that the device still plays the effect.
const WATCHDOG_INTERVAL: f64 = 0.5;
/// Direction test on the settings screen: torque at the rim (towards the left), N·m.
const TEST_TORQUE: f64 = 2.0;
pub const TEST_DURATION: f64 = 0.6;
const SETTINGS_FILE: &str = "ffb.ron";

#[derive(Resource, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FfbSettings {
    pub enabled: bool,
    /// The base's peak torque in N·m (with its own force-feedback gain at 100 %).
    pub wheel_torque: f64,
    /// Share of the car's steering torque reproduced at the rim, percent; 100 is 1:1.
    pub strength: f64,
    /// Largest torque ever sent to the rim, N·m.
    pub max_torque: f64,
    /// Share of the fast changes in the torque (bumps, kerbs) reproduced, percent;
    /// 100 plays them as simulated.
    pub detail: f64,
    /// Strength of the road texture and tyre scrub vibrations, percent.
    pub effects: f64,
    /// Resistance to turning the wheel quickly, percent; steadies strong bases.
    pub damping: f64,
    /// Reverses the force for wheels that push the other way.
    pub invert: bool,
}

impl Default for FfbSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            wheel_torque: 10.0,
            strength: 40.0,
            max_torque: 8.0,
            detail: 150.0,
            effects: 100.0,
            damping: 20.0,
            invert: false,
        }
    }
}

impl FfbSettings {
    pub fn load() -> Self {
        bindings::load_config(SETTINGS_FILE)
    }

    pub fn save(&self) {
        bindings::save_config(SETTINGS_FILE, self);
    }

    /// Steering torque and its fast changes (see [`DETAIL_SMOOTHING`]) in N·m and
    /// steering wheel speed in rad/s (all positive to the left) → motor force in -1..1,
    /// positive turning the wheel left.
    pub fn force(&self, torque: f64, detail: f64, rate: f64) -> f64 {
        let torque = torque + detail * (self.detail / 100.0 - 1.0);
        self.motor(torque * self.strength / 100.0 - rate * DAMPING_FULL * self.damping / 100.0)
    }

    /// Vibration amplitude at the rim in N·m → magnitude in 0..1 of the motor, scaled by
    /// the effects setting and within the maximum output.
    fn vibration(&self, rim: f64) -> f64 {
        self.motor(rim * self.effects / 100.0).abs()
    }

    /// Torque wanted at the rim in N·m → motor force in -1..1, within the maximum output.
    fn motor(&self, rim: f64) -> f64 {
        let max = self.max_torque.min(self.wheel_torque);
        rim.clamp(-max, max) / self.wheel_torque
    }
}

/// What the settings screen and HUD show about force feedback.
#[derive(Resource, Default)]
pub struct FfbStatus {
    /// The wheel in use, or why there is none.
    pub device: String,
    /// Why the force is cut while driving, if it is.
    pub cut: &'static str,
}

/// Seconds left of the direction test pulse started from the settings screen.
#[derive(Resource, Default)]
pub struct FfbTest(pub f64);

#[cfg(windows)]
type Wheel = dinput::Wheel;

/// Stand-in where no force-feedback backend exists.
#[cfg(not(windows))]
struct Wheel {
    name: String,
}

#[cfg(not(windows))]
impl Wheel {
    fn open(_hwnd: isize, _usb: Option<(u16, u16)>) -> Result<Self, String> {
        Err("force feedback is only supported on Windows".into())
    }

    fn set(&mut self, _force: f64) -> Result<(), String> {
        Ok(())
    }

    fn vibrate(&mut self, _i: usize, _magnitude: f64, _hz: f64) -> Result<(), String> {
        Ok(())
    }

    fn keep_playing(&mut self) -> Result<bool, String> {
        Ok(false)
    }
}

/// Grain of a surface: rim torque with both front tyres on it at their static load and
/// speed, N·m, and the distance between its bumps, m.
fn texture(surface: Surface) -> (f64, f64) {
    match surface {
        Surface::Asphalt => (0.07, 0.4),
        Surface::Kerb | Surface::Runoff => (0.1, 0.35),
        Surface::Turf => (0.26, 0.45),
        Surface::Grass => (0.45, 0.6),
        Surface::Dirt => (0.5, 0.5),
        Surface::Gravel => (0.75, 0.3),
    }
}

/// Road texture and tyre scrub under the front tyres: (rim torque amplitude in N·m,
/// frequency in Hz) of each.
fn vibrations(car: &Car) -> [(f64, f64); VIBRATIONS] {
    let p = &car.model.params;
    let static_load = 0.5 * p.mass * GRAVITY * p.front_weight;
    let speed = car.speed();
    let moving = smoothstep(VIBRATION_SPEED.0, VIBRATION_SPEED.1, speed);
    let (mut grain, mut wavelength, mut roughest) = (0.0, 1.0, 0.0);
    let (mut scrub, mut slide_speed) = (0.0, 0.0);
    for i in [FL, FR] {
        let w = &car.telemetry.wheels[i];
        let load = (w.load / static_load).min(1.5);
        let (amplitude, length) = texture(w.surface);
        if amplitude * load > roughest {
            (roughest, wavelength) = (amplitude * load, length);
        }
        grain += 0.5 * amplitude * load;
        if w.surface.paved() {
            scrub += 0.5 * SCRUB_TORQUE * slide(w) * load;
        }
        slide_speed += 0.5 * w.slide_speed;
    }
    [
        (
            grain * moving,
            (speed / wavelength).clamp(TEXTURE_HZ.0, TEXTURE_HZ.1),
        ),
        (
            scrub * moving,
            (SCRUB_HZ + SCRUB_HZ_PER_SPEED * slide_speed).min(SCRUB_MAX_HZ),
        ),
    ]
}

/// The opened wheel. DirectInput objects belong to the thread that made them, so
/// this lives on the main thread.
#[derive(Default)]
struct Ffb {
    wheel: Option<Wheel>,
    /// USB ids of the device steering now (`Some(None)` when they are unknown).
    target: Option<Option<(u16, u16)>>,
    retry_at: f64,
    fade: f64,
    /// Steering wheel angle last frame and smoothed speed, for damping.
    angle: f64,
    rate: f64,
    /// Steering torque without its fast changes, N·m.
    steady: f64,
    /// State of the random spread of the vibrations.
    noise: u32,
    next_watchdog: f64,
    /// Runaway guard: since when the wheel has been pinned at lock, whether the force
    /// is paused for it, the pauses in a row and when the last began.
    pinned_since: Option<f64>,
    paused: bool,
    pauses: u32,
    last_pause: f64,
    /// Stopped by the runaway guard until the settings screen is opened.
    tripped: bool,
    /// The app is closing: the wheel is released and never reopened.
    closed: bool,
}

pub struct FfbPlugin;

impl Plugin for FfbPlugin {
    fn build(&self, app: &mut App) {
        app.init_non_send::<Ffb>()
            .insert_resource(FfbSettings::load())
            .init_resource::<FfbStatus>()
            .init_resource::<FfbTest>()
            .add_systems(
                Update,
                (update, log_status).chain().after(driving::step_simulation),
            )
            .add_systems(Last, release_on_exit);
    }
}

/// USB ids of the steering device, if a wheel (or pad) steers the car.
fn steering_device(
    selection: InputSelection,
    bindings: &Bindings,
    pads: &Query<(Entity, &Gamepad, &Name)>,
) -> Option<Option<(u16, u16)>> {
    let ids = |pad: &Gamepad| pad.vendor_id().zip(pad.product_id());
    match selection {
        InputSelection::Keyboard => None,
        InputSelection::Auto => pads
            .iter()
            .find(|(_, pad, name)| input::is_wheel(pad, name))
            .map(|(_, pad, _)| ids(pad)),
        InputSelection::Pad(e) => pads.get(e).ok().map(|(_, pad, _)| ids(pad)),
        InputSelection::Custom => bindings
            .steer
            .as_ref()
            .map(|b| b.device.vendor.zip(b.device.product)),
    }
}

#[allow(clippy::too_many_arguments)]
fn update(
    time: Res<Time>,
    sim: Res<Simulation>,
    bindings: Res<Bindings>,
    ffb_settings: Res<FfbSettings>,
    selection: Res<InputSelection>,
    settings: Res<SettingsOpen>,
    pads: Query<(Entity, &Gamepad, &Name)>,
    window: Query<&RawHandleWrapper, With<PrimaryWindow>>,
    mut ffb: NonSendMut<Ffb>,
    mut status: ResMut<FfbStatus>,
    mut test: ResMut<FfbTest>,
) {
    if ffb.closed {
        return;
    }
    let target = if ffb_settings.enabled {
        steering_device(*selection, &bindings, &pads)
    } else {
        None
    };
    if target != ffb.target {
        ffb.wheel = None;
        ffb.target = target;
        ffb.retry_at = 0.0;
    }
    let Some(usb) = target else {
        status.device = if ffb_settings.enabled {
            "no wheel selected"
        } else {
            "off"
        }
        .into();
        status.cut = "";
        return;
    };
    let now = time.elapsed_secs_f64();
    if ffb.wheel.is_none() && now >= ffb.retry_at {
        let hwnd = window
            .single()
            .ok()
            .and_then(|w| match w.get_window_handle() {
                raw_window_handle::RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
                _ => None,
            });
        let opened = hwnd
            .ok_or_else(|| "no window".to_string())
            .and_then(|hwnd| Wheel::open(hwnd, usb));
        match opened {
            Ok(wheel) => {
                status.device = wheel.name.clone();
                ffb.wheel = Some(wheel);
                ffb.fade = 0.0;
            }
            Err(e) => {
                status.device = e;
                ffb.retry_at = now + RETRY_INTERVAL;
            }
        }
    }
    if now >= ffb.next_watchdog {
        ffb.next_watchdog = now + WATCHDOG_INTERVAL;
        match ffb.wheel.as_mut().map(Wheel::keep_playing) {
            Some(Ok(true)) => {
                warn!("force feedback: the device had stopped the effect; restarted it")
            }
            Some(Err(e)) => {
                status.device = e;
                ffb.wheel = None;
                ffb.retry_at = now + RETRY_INTERVAL;
            }
            _ => {}
        }
    }

    let dt = time.delta_secs_f64();
    let angle = sim.controls.steer_wheel_angle;
    if dt > 0.0 {
        let raw = (angle - ffb.angle) / dt;
        ffb.rate += (raw - ffb.rate) * (1.0 - (-dt / RATE_SMOOTHING).exp());
    }
    ffb.angle = angle;
    let torque = sim.ffb_torque;
    ffb.steady += (torque - ffb.steady) * (1.0 - (-dt / DETAIL_SMOOTHING).exp());
    if settings.0 {
        ffb.tripped = false;
        ffb.pauses = 0;
    }
    guard_runaway(
        &mut ffb,
        torque,
        angle,
        sim.car.model.params.steering.lock,
        now,
    );
    status.cut = if ffb.tripped {
        "stopped: wheel driven into lock repeatedly; check direction (Esc)"
    } else if ffb.paused {
        "paused at steering lock"
    } else {
        ""
    };
    let mut shake = [(0.0, 1.0); VIBRATIONS];
    let left = if test.0 > 0.0 {
        test.0 -= dt;
        ffb_settings.motor(TEST_TORQUE)
    } else if !settings.0 && !ffb.tripped && !ffb.paused && sim.mode == Mode::Human {
        ffb.fade = (ffb.fade + dt / FADE_IN).min(1.0);
        for ((amplitude, hz), s) in vibrations(&sim.car).into_iter().zip(&mut shake) {
            let spread = 1.0 - JITTER * next_noise(&mut ffb.noise);
            *s = (ffb_settings.vibration(amplitude * spread) * ffb.fade, hz);
        }
        ffb_settings.force(torque, torque - ffb.steady, ffb.rate) * ffb.fade
    } else {
        ffb.fade = 0.0;
        0.0
    };
    // A positive DirectInput force turns a MOZA R12 left; other bases may differ.
    let out = if ffb_settings.invert { -left } else { left };
    let played = ffb.wheel.as_mut().map(|wheel| {
        wheel.set(out)?;
        for (i, (magnitude, hz)) in shake.into_iter().enumerate() {
            wheel.vibrate(i, magnitude, hz)?;
        }
        Ok::<_, String>(())
    });
    if let Some(Err(e)) = played {
        status.device = e;
        ffb.wheel = None;
        ffb.retry_at = now + RETRY_INTERVAL;
    }
}

/// Uniform random number in 0..1 (xorshift).
fn next_noise(state: &mut u32) -> f64 {
    let mut x = if *state == 0 { 0x9e37_79b9 } else { *state };
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x as f64 / u32::MAX as f64
}

/// With the direction inverted, the aligning torque drives the wheel into lock
/// instead of back to centre and holds it there. Pauses the force while the wheel is
/// held at lock against the tyres' pull, and stops it after repeated pauses.
fn guard_runaway(ffb: &mut Ffb, torque: f64, angle: f64, lock: f64, now: f64) {
    let pinned =
        -torque * angle.signum() > RUNAWAY_TORQUE && angle.abs() >= RUNAWAY_LOCK_SHARE * lock;
    match ffb.pinned_since {
        _ if !pinned => {
            ffb.pinned_since = None;
            ffb.paused = false;
        }
        None => ffb.pinned_since = Some(now),
        Some(since) if !ffb.paused && now - since >= RUNAWAY_TIME => {
            ffb.paused = true;
            ffb.pauses = if now - ffb.last_pause < RUNAWAY_WINDOW {
                ffb.pauses + 1
            } else {
                1
            };
            ffb.last_pause = now;
            ffb.tripped |= ffb.pauses >= RUNAWAY_REPEATS;
        }
        Some(_) => {}
    }
}

/// Releases the wheel as soon as the app is asked to close, before the window and
/// the world go away.
fn release_on_exit(
    mut close: MessageReader<WindowCloseRequested>,
    mut exit: MessageReader<AppExit>,
    mut ffb: NonSendMut<Ffb>,
) {
    if close.read().count() + exit.read().count() > 0 {
        ffb.wheel = None;
        ffb.closed = true;
    }
}

fn log_status(status: Res<FfbStatus>, mut last: Local<(String, &'static str)>) {
    if status.device != last.0 {
        info!("force feedback: {}", status.device);
        last.0.clone_from(&status.device);
    }
    if status.cut != last.1 {
        info!(
            "force feedback: {}",
            if status.cut.is_empty() {
                "resumed"
            } else {
                status.cut
            }
        );
        last.1 = status.cut;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn torque_is_reproduced_at_the_rim_up_to_the_maximum() {
        let s = FfbSettings {
            wheel_torque: 12.0,
            strength: 100.0,
            max_torque: 12.0,
            damping: 0.0,
            ..default()
        };
        assert_eq!(s.force(6.0, 0.0, 0.0), 0.5);
        assert_eq!(
            FfbSettings {
                strength: 50.0,
                ..s.clone()
            }
            .force(-6.0, 0.0, 0.0),
            -0.25
        );
        assert_eq!(
            FfbSettings {
                max_torque: 3.0,
                ..s.clone()
            }
            .force(40.0, 0.0, 0.0),
            0.25
        );
        // A maximum above the base's own peak cannot exceed full force.
        assert_eq!(
            FfbSettings {
                max_torque: 30.0,
                ..s.clone()
            }
            .force(40.0, 0.0, 0.0),
            1.0
        );
        assert_eq!(
            FfbSettings { strength: 0.0, ..s }.force(10.0, 0.0, 0.0),
            0.0
        );
    }

    #[test]
    fn a_wheel_turned_past_lock_pauses_then_resumes() {
        let mut ffb = Ffb::default();
        let lock = 4.7;
        // Held at lock against a centring torque.
        guard_runaway(&mut ffb, -3.0, lock, lock, 100.0);
        guard_runaway(&mut ffb, -3.0, lock, lock, 100.6);
        assert!(ffb.paused && !ffb.tripped);
        guard_runaway(&mut ffb, -3.0, lock - 0.5, lock, 101.0);
        assert!(!ffb.paused);
    }

    #[test]
    fn repeated_runaways_stop_force_feedback() {
        let mut ffb = Ffb::default();
        let lock = 4.7;
        for i in 0..RUNAWAY_REPEATS {
            let t = 100.0 + i as f64 * 2.0;
            guard_runaway(&mut ffb, -3.0, lock, lock, t);
            guard_runaway(&mut ffb, -3.0, lock, lock, t + 0.6);
            guard_runaway(&mut ffb, -3.0, 0.0, lock, t + 1.0);
        }
        assert!(ffb.tripped);
    }

    #[test]
    fn road_detail_scales_only_the_fast_changes() {
        let s = FfbSettings {
            wheel_torque: 10.0,
            strength: 100.0,
            max_torque: 10.0,
            damping: 0.0,
            detail: 200.0,
            ..default()
        };
        // 2 N·m steady plus a 1 N·m kick: the kick is doubled, the rest kept.
        assert!((s.force(3.0, 1.0, 0.0) - 0.4).abs() < 1e-12);
        assert!((FfbSettings { detail: 0.0, ..s }.force(3.0, 1.0, 0.0) - 0.2).abs() < 1e-12);
    }

    /// The GT3 rolling at `speed` with both front tyres on `surface` at their static
    /// load, sliding at `slip_angle`.
    fn car_on(surface: Surface, speed: f64, slip_angle: f64) -> Car {
        let model = std::sync::Arc::new(open_racing_sim::CarModel::gt3());
        let track = open_racing_sim::Track::default_circuit();
        let mut car = Car::new(model, &track, 0.0, 0.0, speed, 3);
        let p = &car.model.params;
        let load = 0.5 * p.mass * GRAVITY * p.front_weight;
        for i in [FL, FR] {
            let w = &mut car.telemetry.wheels[i];
            (w.surface, w.load, w.slip_angle) = (surface, load, slip_angle);
        }
        car
    }

    #[test]
    fn vibrations_follow_speed_surface_and_sliding() {
        let [grain, scrub] = vibrations(&car_on(Surface::Asphalt, 30.0, 0.0));
        assert!(grain.0 > 0.0 && scrub.0 == 0.0);
        assert!(vibrations(&car_on(Surface::Asphalt, 0.0, 0.0))[0].0 == 0.0);
        let [gravel, _] = vibrations(&car_on(Surface::Gravel, 30.0, 0.0));
        assert!(gravel.0 > 5.0 * grain.0);
        let slow = vibrations(&car_on(Surface::Asphalt, 10.0, 0.0))[0];
        assert!(
            slow.1 < grain.1,
            "texture at {} Hz slow, {} Hz fast",
            slow.1,
            grain.1
        );
        let [_, sliding] = vibrations(&car_on(Surface::Asphalt, 30.0, 0.2));
        assert!(sliding.0 > 0.0);
    }

    #[test]
    fn damping_resists_the_wheel_turning() {
        let s = FfbSettings {
            damping: 50.0,
            ..default()
        };
        assert!(s.force(0.0, 0.0, 2.0) < 0.0);
        assert!(s.force(0.0, 0.0, -2.0) > 0.0);
    }
}
