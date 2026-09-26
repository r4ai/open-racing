//! The engine's sound, from the simulation.
//!
//! - Each mouth open to the air (tailpipes, intake snorkels, open ports) radiates as a
//!   monopole: p(r, t) = ρ₀/(4πr) · dQ/dt(t − r/c), Q the volume flow leaving it (Davies,
//!   *J. Sound Vib.* 1988; Munjal, *Acoustics of Ducts and Mufflers*). The pulses the
//!   cylinders send down the pipes, shaped by their reflections and the silencers, are
//!   what is heard.
//! - The block radiates the combustion: each cylinder's rate of pressure rise through the
//!   structure's attenuation, a high pass (Austen & Priede's structure attenuation curve,
//!   simplified).
//! - The valves strike their seats: a click ringing at the head's resonance.
//! - The jet leaving a mouth is turbulent: noise growing with its velocity (Lighthill's
//!   scaling, semi-empirical).
//!
//! A microphone sums them with their distances' delays and 1/r spreading; inside the
//! cabin the body lets the low frequencies through and damps the high ones.

use serde::{Deserialize, Serialize};

use crate::combustion::Rng;
use crate::dsp::{Biquad, DcBlocker, Delay, Resampler};
use crate::model::Model;

/// Speed of sound in the air, m/s.
const C_AIR: f64 = 343.0;

/// A microphone.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mic {
    pub name: String,
    /// Where it is, m, in the frame of the mouths' positions (the machine's, or the
    /// engine's on a bench).
    pub position: [f64; 3],
    /// Inside the cabin: through the body's transmission.
    #[serde(default)]
    pub cabin: bool,
}

/// Levels of the sound's sources, and the microphones.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SoundSettings {
    pub mics: Vec<Mic>,
    /// Radiated pressure at 1 m per Pa/s of cylinder pressure rise.
    pub combustion: f64,
    /// Valve seating clicks at 1 m, Pa.
    pub valves: f64,
    /// Jet noise of the mouths at 1 m, Pa at 100 m/s.
    pub flow_noise: f64,
    /// Where the engine's block is, m.
    pub block: [f64; 3],
}

impl Default for SoundSettings {
    fn default() -> Self {
        Self {
            mics: vec![Mic {
                name: "exhaust".into(),
                position: [-5.0, 1.0, 0.5],
                cabin: false,
            }],
            combustion: 2e-11,
            valves: 0.02,
            flow_noise: 0.05,
            block: [0.0; 3],
        }
    }
}

impl SoundSettings {
    /// A microphone 1 m from each mouth of the model (the nearest), for a bench.
    pub fn near(model: &Model, mouth: &str) -> Self {
        let m = model
            .mouths
            .iter()
            .find(|m| m.name.contains(mouth))
            .or(model.mouths.first());
        let p = m.map(|m| m.position).unwrap_or([0.0; 3]);
        Self {
            mics: vec![Mic {
                name: mouth.into(),
                position: [p[0] - 0.7, p[1] + 0.7, p[2]],
                cabin: false,
            }],
            ..Default::default()
        }
    }
}

struct SourcePath {
    delay: Delay,
    gain: f64,
}

struct MicState {
    mouths: Vec<SourcePath>,
    block: SourcePath,
    cabin: Option<[Biquad; 3]>,
    dc: DcBlocker,
    out: Resampler,
}

/// Turns a model's steps into microphone signals at 48 kHz.
pub struct Acoustics {
    settings: SoundSettings,
    rate: f64,
    mics: Vec<MicState>,
    structure: [Biquad; 2],
    valve_ring: Biquad,
    jet: Vec<Biquad>,
    rng: Rng,
    /// Scratch for one input sample's output.
    scratch: Vec<f64>,
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2))
        .sqrt()
        .max(0.2)
}

impl Acoustics {
    pub fn new(model: &Model, settings: SoundSettings) -> Self {
        let rate = model.quality.rate() as f64;
        let rho = model.ambient.pressure / (crate::gas::R_AIR * model.ambient.temperature);
        let mics = settings
            .mics
            .iter()
            .map(|mic| {
                let path = |pos: [f64; 3], k: f64| {
                    let r = dist(pos, mic.position);
                    SourcePath {
                        delay: Delay::new(r / C_AIR * rate),
                        gain: k / r,
                    }
                };
                MicState {
                    mouths: model
                        .mouths
                        .iter()
                        .map(|m| path(m.position, rho / (4.0 * std::f64::consts::PI)))
                        .collect(),
                    block: path(settings.block, 1.0),
                    cabin: mic.cabin.then(|| {
                        [
                            Biquad::lowpass(700.0, rate, 0.8),
                            Biquad::peak(90.0, rate, 1.5, 6.0),
                            Biquad::lowpass(2500.0, rate, 0.707),
                        ]
                    }),
                    dc: DcBlocker::new(8.0, rate),
                    out: Resampler::new(model.quality.rate()),
                }
            })
            .collect();
        Self {
            rate,
            mics,
            structure: [
                Biquad::highpass(800.0, rate, 0.707),
                Biquad::lowpass(6000.0, rate, 0.707),
            ],
            valve_ring: Biquad::bandpass(3200.0, rate, 12.0),
            jet: model
                .mouths
                .iter()
                .map(|_| Biquad::lowpass(3500.0, rate, 0.707))
                .collect(),
            rng: Rng::new(0x5eed),
            scratch: Vec::with_capacity(4),
            settings,
        }
    }

    pub fn mic_count(&self) -> usize {
        self.mics.len()
    }

    /// Takes the sources of the model's last step; appends each microphone's completed
    /// 48 kHz samples (Pa) to `out[mic]`.
    pub fn process(&mut self, model: &Model, out: &mut [Vec<f64>]) {
        let dt = 1.0 / self.rate;
        // Combustion through the structure.
        let dpdt: f64 = model.cylinders.iter().map(|c| c.dpdt).sum();
        let mut block = dpdt * self.settings.combustion;
        for f in &mut self.structure {
            block = f.process(block);
        }
        // Valve seating.
        let seated = model.cylinders.iter().filter(|c| c.seated > 0.0).count() as f64;
        let impulse = if seated > 0.0 {
            seated * self.settings.valves * self.rate / 48_000.0 * 30.0
        } else {
            0.0
        };
        block += self.valve_ring.process(impulse);
        // Mouths.
        let mut mouth = [0.0f64; 16];
        let n = model.mouths.len().min(16);
        for (k, m) in model.mouths.iter().take(n).enumerate() {
            let dq = (m.flow - m.prev_flow) / dt;
            let v = m.velocity.abs();
            let noise =
                (self.rng.uniform() - 0.5) * 3.46 * self.settings.flow_noise * (v / 100.0).powi(3);
            let jet = self.jet[k].process(noise);
            mouth[k] = dq + jet * (4.0 * std::f64::consts::PI) / 1.2;
        }
        for (i, mic) in self.mics.iter_mut().enumerate() {
            let mut p = 0.0;
            for (k, path) in mic.mouths.iter_mut().enumerate().take(n) {
                p += path.gain * path.delay.process(mouth[k]);
            }
            p += mic.block.gain * mic.block.delay.process(block);
            if let Some(f) = &mut mic.cabin {
                for b in f.iter_mut() {
                    p = b.process(p);
                }
                p *= 0.3;
            }
            let p = mic.dc.process(p);
            self.scratch.clear();
            mic.out.process(p, &mut self.scratch);
            out[i].extend_from_slice(&self.scratch);
        }
    }
}
