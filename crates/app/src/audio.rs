//! Procedural car sound: engine, tyre squeal, road and wind noise, kerbs, grass,
//! gear shifts and suspension thumps, all synthesised from the simulation state.
//!
//! The ECS writes target `Knobs` once per frame into a shared mutex; the audio
//! thread picks them up every `BLOCK` samples (without ever blocking) and smooths
//! them, so no audio assets are needed and the sound follows the physics continuously.

use std::f32::consts::TAU;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bevy::audio::{AddAudioSource, ChannelCount, Decodable, SampleRate, Source};
use bevy::prelude::*;
use open_racing_sim::{GRAVITY, Surface};

use crate::driving::{self, Simulation};
use crate::input::AppRequests;

const SAMPLE_RATE: u32 = 44_100;
const SR: f32 = SAMPLE_RATE as f32;
/// Samples between knob updates and filter coefficient recalculation.
const BLOCK: usize = 32;
/// Relative strength of each of the eight firing pulses of a V8 cycle; the
/// imbalance gives the characteristic crank-rate burble.
const CYLINDERS: [f32; 8] = [1.0, 0.82, 0.95, 0.78, 0.97, 0.88, 0.8, 0.92];
/// Rev-limiter fuel cut rate heard as the "brrap", Hz.
const LIMITER_HZ: f32 = 18.0;
/// Distance between kerb ridges, m.
const KERB_PITCH: f32 = 0.9;

/// Targets for the synthesiser. Everything is 0..1 except `rpm` and `speed` (m/s).
#[derive(Clone, Copy, Default)]
struct Knobs {
    rpm: f32,
    load: f32,
    limiter: f32,
    running: f32,
    skid: f32,
    speed: f32,
    kerb: f32,
    grass: f32,
    master: f32,
    /// One-shot events, consumed by the audio thread.
    shift: bool,
    bump: f32,
}

#[derive(Asset, TypePath)]
struct CarSynth {
    knobs: Arc<Mutex<Knobs>>,
}

impl Decodable for CarSynth {
    type Decoder = CarSynthDecoder;

    fn decoder(&self) -> Self::Decoder {
        CarSynthDecoder::new(self.knobs.clone())
    }
}

/// Topology-preserving state variable filter (Zavalishin); gives low, band and
/// high pass at once and stays stable under fast cutoff changes.
#[derive(Default)]
struct Svf {
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    ic1: f32,
    ic2: f32,
}

impl Svf {
    fn set(&mut self, hz: f32, q: f32) {
        let g = (std::f32::consts::PI * hz.clamp(10.0, SR * 0.45) / SR).tan();
        self.k = 1.0 / q;
        self.a1 = 1.0 / (1.0 + g * (g + self.k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }

    /// Returns (low, band, high).
    fn process(&mut self, x: f32) -> (f32, f32, f32) {
        let v3 = x - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        (v2, v1, x - self.k * v1 - v2)
    }
}

/// Decaying one-shot envelope with its own oscillator phase.
struct Hit {
    env: f32,
    phase: f32,
    decay: f32,
    hz: f32,
}

impl Hit {
    fn new(hz: f32, seconds: f32) -> Self {
        Self { env: 0.0, phase: 0.0, decay: (-1.0 / (SR * seconds)).exp(), hz }
    }

    fn trigger(&mut self, amplitude: f32) {
        self.env = self.env.max(amplitude);
        self.phase = 0.0;
    }

    fn next(&mut self, noise: f32, noise_mix: f32) -> f32 {
        self.env *= self.decay;
        self.phase = (self.phase + self.hz / SR).fract();
        self.env * ((TAU * self.phase).sin() + noise * noise_mix)
    }
}

struct CarSynthDecoder {
    shared: Arc<Mutex<Knobs>>,
    target: Knobs,
    /// Smoothed knobs actually used for synthesis.
    k: Knobs,
    until_block: usize,
    rng: u32,

    engine_phase: f32,
    cylinder: usize,
    fire: f32,
    engine_lp: Svf,
    dc_in: f32,
    dc_out: f32,
    intake: Svf,
    limiter_phase: f32,

    squeal: Svf,
    squeal_phase: f32,
    squeal_hz: f32,
    vibrato_phase: f32,

    road: Svf,
    wind: Svf,
    kerb_phase: f32,
    kerb_lp: Svf,
    grass_hp: Svf,

    clunk: Hit,
    thump: Hit,
}

impl CarSynthDecoder {
    fn new(shared: Arc<Mutex<Knobs>>) -> Self {
        Self {
            shared,
            target: Knobs::default(),
            k: Knobs::default(),
            until_block: 0,
            rng: 0x9E37_79B9,
            engine_phase: 0.0,
            cylinder: 0,
            fire: 1.0,
            engine_lp: Svf::default(),
            dc_in: 0.0,
            dc_out: 0.0,
            intake: Svf::default(),
            limiter_phase: 0.0,
            squeal: Svf::default(),
            squeal_phase: 0.0,
            squeal_hz: 800.0,
            vibrato_phase: 0.0,
            road: Svf::default(),
            wind: Svf::default(),
            kerb_phase: 0.0,
            kerb_lp: Svf::default(),
            grass_hp: Svf::default(),
            clunk: Hit::new(75.0, 0.045),
            thump: Hit::new(45.0, 0.1),
        }
    }

    /// White noise in [-1, 1) (xorshift32).
    fn noise(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng as f32 / u32::MAX as f32 * 2.0 - 1.0
    }

    /// Reads new targets and events, advances the smoothing and retunes the filters.
    fn block(&mut self) {
        if let Ok(mut shared) = self.shared.try_lock() {
            self.target = *shared;
            if std::mem::take(&mut shared.shift) {
                self.clunk.trigger(0.7);
            }
            let bump = std::mem::take(&mut shared.bump);
            if bump > 0.0 {
                self.thump.trigger(bump);
            }
        }
        let dt = BLOCK as f32 / SR;
        let fast = 1.0 - (-dt / 0.008).exp();
        let slow = 1.0 - (-dt / 0.04).exp();
        let (t, k) = (&self.target, &mut self.k);
        k.rpm += (t.rpm - k.rpm) * fast;
        k.load += (t.load - k.load) * slow;
        k.limiter += (t.limiter - k.limiter) * fast;
        k.running += (t.running - k.running) * slow;
        k.skid += (t.skid - k.skid) * slow;
        k.speed += (t.speed - k.speed) * slow;
        k.kerb += (t.kerb - k.kerb) * fast;
        k.grass += (t.grass - k.grass) * slow;
        k.master += (t.master - k.master) * slow;

        let firing = self.k.rpm / 15.0;
        self.engine_lp.set(firing * (3.0 + 9.0 * self.k.load) + 200.0, 0.8);
        self.intake.set(firing * 2.0 + 150.0, 1.5);

        self.vibrato_phase = (self.vibrato_phase + 6.5 * dt).fract();
        self.squeal_hz = 720.0 + 380.0 * self.k.skid + 30.0 * (TAU * self.vibrato_phase).sin();
        self.squeal.set(self.squeal_hz, 14.0);
        self.road.set(220.0 + 3.0 * self.k.speed, 0.7);
        self.wind.set(500.0 + 6.0 * self.k.speed, 0.6);
        self.kerb_lp.set(180.0, 0.9);
        self.grass_hp.set(1400.0, 0.7);
    }

    fn engine(&mut self, noise: f32) -> f32 {
        let k = self.k;
        // V8: four firings per crank revolution.
        self.engine_phase += k.rpm / 15.0 / SR;
        if self.engine_phase >= 1.0 {
            self.engine_phase -= 1.0;
            self.cylinder = (self.cylinder + 1) % CYLINDERS.len();
            self.fire = CYLINDERS[self.cylinder] * (1.0 + 0.12 * noise);
        }
        let pulse = self.fire * (-self.engine_phase * 7.0).exp() + noise * 0.04 * k.load;
        let (low, ..) = self.engine_lp.process(pulse);
        // DC blocker: the pulse train has a large mean.
        self.dc_out = low - self.dc_in + 0.995 * self.dc_out;
        self.dc_in = low;

        self.limiter_phase = (self.limiter_phase + LIMITER_HZ / SR).fract();
        let cut = if self.limiter_phase < 0.5 { 0.0 } else { 0.8 };
        let gain = k.running * (0.35 + 0.65 * k.load) * (1.0 - k.limiter * cut);
        let (_, intake, _) = self.intake.process(noise);
        gain * (self.dc_out * 1.6 + intake * 0.5 * k.load)
    }

    fn tyres(&mut self, noise: f32) -> f32 {
        let k = self.k;
        self.squeal_phase = (self.squeal_phase + self.squeal_hz / SR).fract();
        let (_, band, _) = self.squeal.process(noise);
        let squeal = ((TAU * self.squeal_phase).sin() * 0.5 + band * 3.0) * k.skid * 0.2;

        let (road, ..) = self.road.process(noise);
        let (_, wind, _) = self.wind.process(noise);
        let rolling = road * (k.speed / 70.0).min(1.0) * 0.5 + wind * (k.speed / 90.0).powi(2).min(1.0) * 0.35;

        self.kerb_phase = (self.kerb_phase + k.speed / KERB_PITCH / SR).fract();
        let ridge = if self.kerb_phase < 0.5 { 1.0 } else { -1.0 };
        let (kerb, ..) = self.kerb_lp.process(ridge * 0.8 + noise * 0.4);

        let crackle = if self.noise() > 0.97 - 0.05 * (k.speed / 30.0).min(1.0) { noise * 2.0 } else { 0.0 };
        let (.., gravel) = self.grass_hp.process(crackle + noise * 0.15);
        let grass = (gravel + road * 2.0) * k.grass * 0.35;

        squeal + rolling + kerb * k.kerb * 0.6 + grass
    }
}

impl Iterator for CarSynthDecoder {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.until_block == 0 {
            self.block();
            self.until_block = BLOCK;
        }
        self.until_block -= 1;
        let noise = self.noise();
        let mix = self.engine(noise) + self.tyres(noise) + self.clunk.next(noise, 0.4) + self.thump.next(noise, 0.2);
        Some(mix.tanh() * self.k.master * 0.8)
    }
}

impl Source for CarSynthDecoder {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        ChannelCount::new(1).unwrap()
    }

    fn sample_rate(&self) -> SampleRate {
        SampleRate::new(SAMPLE_RATE).unwrap()
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// ECS side of the synthesiser: the shared knobs and what is needed to detect events.
#[derive(Resource)]
struct CarSound {
    knobs: Arc<Mutex<Knobs>>,
    muted: bool,
    last_gear: i32,
    last_suspension: [f64; 4],
}

pub struct AudioPlugin;

impl Plugin for AudioPlugin {
    fn build(&self, app: &mut App) {
        app.add_audio_source::<CarSynth>()
            .add_systems(Startup, spawn)
            .add_systems(Update, update.after(driving::step_simulation));
    }
}

fn spawn(mut commands: Commands, mut synths: ResMut<Assets<CarSynth>>, sim: Res<Simulation>) {
    let knobs = Arc::new(Mutex::new(Knobs::default()));
    commands.spawn(AudioPlayer(synths.add(CarSynth { knobs: knobs.clone() })));
    commands.insert_resource(CarSound {
        knobs,
        muted: false,
        last_gear: sim.car.state.drivetrain.gear,
        last_suspension: [0.0; 4],
    });
}

fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn update(sim: Res<Simulation>, requests: Res<AppRequests>, mut sound: ResMut<CarSound>) {
    if requests.toggle_mute {
        sound.muted = !sound.muted;
    }
    let car = &sim.car;
    let params = &car.model.params;
    let drivetrain = &car.state.drivetrain;
    let speed = car.speed();
    let static_load = params.mass * GRAVITY / 4.0;
    let moving = smoothstep(0.3, 3.0, speed);

    let (mut skid, mut kerb, mut grass, mut bump) = (0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64);
    for (w, last) in car.telemetry.wheels.iter().zip(&mut sound.last_suspension) {
        // A sudden change in suspension force within one frame is a hit.
        bump = bump.max((w.suspension_force - *last).abs() / static_load - 0.8);
        *last = w.suspension_force;
        if w.load <= 0.0 {
            continue;
        }
        match w.surface {
            Surface::Asphalt => {}
            Surface::Kerb => kerb += 0.5,
            Surface::Grass => grass += 0.25,
        }
        if w.surface != Surface::Grass {
            let slide = smoothstep(0.06, 0.15, w.slip_angle.abs()).max(smoothstep(0.1, 0.3, w.slip_ratio.abs()));
            skid = skid.max(slide * (w.load / static_load).min(1.5));
        }
    }

    let rpm = drivetrain.rpm();
    let shifted = drivetrain.gear != sound.last_gear;
    sound.last_gear = drivetrain.gear;
    let master = if sound.muted { 0.0 } else { 1.0 };
    let mut knobs = sound.knobs.lock().unwrap();
    *knobs = Knobs {
        rpm: rpm as f32,
        load: sim.controls.throttle as f32,
        limiter: if rpm >= params.engine.limiter_rpm - 100.0 { 1.0 } else { 0.0 },
        running: if drivetrain.stalled { 0.0 } else { 1.0 },
        skid: (skid * moving).min(1.0) as f32,
        speed: speed as f32,
        kerb: (kerb.min(1.0) * moving) as f32,
        grass: (grass * moving) as f32,
        master,
        shift: knobs.shift || shifted,
        bump: knobs.bump.max((bump * moving).min(1.0) as f32),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders one second with the given knobs, returning (rms, peak).
    fn render(knobs: Knobs) -> (f32, f32) {
        let shared = Arc::new(Mutex::new(Knobs { master: 1.0, ..knobs }));
        let mut synth = CarSynthDecoder::new(shared);
        let samples: Vec<f32> = synth.by_ref().take(SAMPLE_RATE as usize).collect();
        let tail = &samples[SAMPLE_RATE as usize / 2..];
        let rms = (tail.iter().map(|x| x * x).sum::<f32>() / tail.len() as f32).sqrt();
        (rms, tail.iter().fold(0.0, |m, x| m.max(x.abs())))
    }

    #[test]
    fn levels_are_bounded_and_respond_to_state() {
        let idle = Knobs { rpm: 1500.0, running: 1.0, ..Knobs::default() };
        let full = Knobs { rpm: 8000.0, load: 1.0, running: 1.0, speed: 60.0, ..Knobs::default() };
        let sliding = Knobs { skid: 1.0, speed: 30.0, ..Knobs::default() };
        let stalled = Knobs::default();
        let all = Knobs { rpm: 9250.0, load: 1.0, limiter: 1.0, running: 1.0, skid: 1.0, speed: 80.0, kerb: 1.0, grass: 1.0, ..Knobs::default() };
        for (name, k) in [("idle", idle), ("full", full), ("sliding", sliding), ("stalled", stalled), ("all", all)] {
            let (rms, peak) = render(k);
            println!("{name:8} rms {rms:.3} peak {peak:.3}");
            assert!(rms.is_finite() && peak <= 0.8 + 1e-6, "{name}");
        }
        assert!(render(idle).0 > 0.02);
        assert!(render(full).0 > render(idle).0);
        assert!(render(sliding).0 > 0.02);
        assert!(render(stalled).0 < 1e-3);
    }
}
