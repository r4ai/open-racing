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
//! - The valves strike their seats: the cam's closing ramp lands them at a speed that grows
//!   with the engine's, and each strike rings the head's first few structural modes.
//! - The jet leaving a mouth is turbulent. Lighthill's acoustic efficiency of a subsonic
//!   jet, η ≈ 10⁻⁴·M⁵ of its kinetic power ½ρU³A, radiated round the mouth, in a broad
//!   band peaking at a Strouhal number fD/U ≈ 0.25 (Tam's similarity spectra): it follows
//!   the pulsating velocity, so the hiss comes in bursts with the firing and rises
//!   steeply with speed. It is what fills the spectrum between the engine orders above a
//!   kilohertz or so, where the gas pulses themselves have little left.
//!
//! - A turbocharger's compressor sings (Raitor & Neise, *J. Sound Vib.* 314, 2008): a tone
//!   at the blade passing frequency (and twice it, from the splitter blades); a narrow
//!   band of tip clearance noise near half of it, the whistle that rises as the turbo
//!   spools; and a broad "whoosh" of 4–12 kHz, loudest near surge (Evans & Ward, SAE
//!   2005-01-2485). With the inducer's tip moving supersonically against the air its shock
//!   pattern adds the shaft's orders: buzz-saw. Together they take a millionth or so of the
//!   compressor's power, growing with the square of the tip Mach number. They are made at
//!   the output rate, as their frequencies reach far above what the solver's step carries.
//!
//! A microphone sums them with their distances' delays and 1/r spreading, and outside
//! hears each again off the ground (z = 0) as from its mirror image, weakened by the
//! ground's reflection coefficient: the comb of reinforcements and notches every
//! recording made over a road has. Inside the cabin the body lets the low frequencies
//! through and damps the high ones.

use std::f64::consts::PI;

use serde::{Deserialize, Serialize};

use crate::combustion::Rng;
use crate::dsp::{Biquad, DcBlocker, Delay, OUTPUT_RATE, Resampler, Svf};
use crate::model::Model;

/// A cylinder head's first structural modes: (Hz, Q, share of the strike).
const HEAD_MODES: [(f64, f64, f64); 4] = [
    (1250.0, 18.0, 0.5),
    (2600.0, 25.0, 1.0),
    (4100.0, 30.0, 0.7),
    (6300.0, 35.0, 0.4),
];

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
    /// Valve seating clicks at 1 m at 6000 rpm, Pa.
    pub valves: f64,
    /// Jet noise of the mouths, as a share of Lighthill's estimate.
    pub flow_noise: f64,
    /// Where the engine's block is, m.
    pub block: [f64; 3],
    /// Pressure reflection coefficient of the ground at z = 0 (asphalt ≈ 0.9, none: 0).
    pub ground: f64,
    /// Turbochargers' compressor noise, as a share of its estimate.
    pub turbo: f64,
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
            flow_noise: 1.0,
            block: [0.0; 3],
            ground: 0.9,
            turbo: 1.0,
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

/// A source's way to a microphone: straight, and off the ground.
struct SourcePath {
    delay: Delay,
    gain: f64,
    reflected: Option<(Delay, f64)>,
}

impl SourcePath {
    #[inline]
    fn process(&mut self, x: f64) -> f64 {
        let mut p = self.gain * self.delay.process(x);
        if let Some((d, g)) = &mut self.reflected {
            p += *g * d.process(x);
        }
        p
    }
}

/// A turbocharger's compressor as a source.
struct TurboVoice {
    /// Its shaft's angle, rad.
    phase: f64,
    tcn: Svf,
    whoosh: Svf,
    /// Amplitude and phase of each shaft order in the buzz-saw noise.
    buzz: Vec<(f64, f64)>,
    /// Its way to each microphone, at the output rate.
    paths: Vec<SourcePath>,
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
    /// The head's modes the valves ring: filter, and the impulse that starts it ringing
    /// at 1 Pa.
    valve_ring: Vec<(Biquad, f64)>,
    jet: Vec<Svf>,
    turbos: Vec<TurboVoice>,
    rho_air: f64,
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
        let path_at = |mic: &Mic, pos: [f64; 3], k: f64, rate: f64| {
            let ground = if mic.cabin { 0.0 } else { settings.ground };
            let r = dist(pos, mic.position);
            let image = [pos[0], pos[1], -pos[2]];
            let ri = dist(image, mic.position);
            SourcePath {
                delay: Delay::new(r / C_AIR * rate),
                gain: k / r,
                reflected: (ground != 0.0)
                    .then(|| (Delay::new(ri / C_AIR * rate), ground * k / ri)),
            }
        };
        let mut seed = Rng::new(0x7ab0);
        let turbos = model
            .turbos
            .iter()
            .map(|t| TurboVoice {
                phase: 0.0,
                tcn: Svf::default(),
                whoosh: Svf::default(),
                buzz: (0..t.compressor.as_ref().map_or(1, |c| c.blades.max(1)))
                    .map(|_| (seed.uniform(), std::f64::consts::TAU * seed.uniform()))
                    .collect(),
                paths: settings
                    .mics
                    .iter()
                    .map(|mic| {
                        // Through the body, the cabin hears little of a whistle.
                        let k = if mic.cabin { 0.1 } else { 1.0 };
                        path_at(mic, t.position, k, OUTPUT_RATE as f64)
                    })
                    .collect(),
            })
            .collect();
        let mics = settings
            .mics
            .iter()
            .map(|mic| {
                let path = |pos: [f64; 3], k: f64| path_at(mic, pos, k, rate);
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
            valve_ring: HEAD_MODES
                .iter()
                .filter(|m| m.0 < 0.45 * rate)
                .map(|&(f, q, w)| (Biquad::bandpass(f, rate, q), w * q * rate / (PI * f)))
                .collect(),
            jet: vec![Svf::default(); model.mouths.len()],
            turbos,
            rho_air: rho,
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
        let strike = seated * self.settings.valves * model.rpm().abs() / 6000.0;
        for (f, k) in &mut self.valve_ring {
            block += f.process(strike * *k);
        }
        // Mouths.
        let mut mouth = [0.0f64; 16];
        let n = model.mouths.len().min(16);
        let four_pi = 4.0 * std::f64::consts::PI;
        for (k, m) in model.mouths.iter().take(n).enumerate() {
            let dq = (m.flow - m.prev_flow) / dt;
            // Drawn in, the air is not a jet: a sink flow, far quieter.
            let (u, share) = if m.velocity >= 0.0 {
                (m.velocity, 1.0)
            } else {
                (-m.velocity, 0.1)
            };
            let power = share
                * self.settings.flow_noise
                * 1e-4
                * (u / C_AIR).powi(5)
                * 0.5
                * m.density
                * u.powi(3)
                * m.area;
            // Pressure at 1 m of that power spread over a sphere.
            let p1 = (power * self.rho_air * C_AIR / four_pi).sqrt();
            let d = (4.0 * m.area / std::f64::consts::PI).sqrt();
            let (f, q) = ((0.25 * u / d).max(20.0), 0.6);
            let bandwidth = std::f64::consts::PI * f / (2.0 * q);
            let white = (self.rng.uniform() - 0.5) * 12f64.sqrt();
            let x = white * p1 * (0.5 * self.rate / bandwidth).sqrt();
            let jet = self.jet[k].bandpass(x, f, q, self.rate);
            mouth[k] = dq + jet * four_pi / self.rho_air;
        }
        let mut fresh = 0;
        for (i, mic) in self.mics.iter_mut().enumerate() {
            let mut p = 0.0;
            for (k, path) in mic.mouths.iter_mut().enumerate().take(n) {
                p += path.process(mouth[k]);
            }
            p += mic.block.process(block);
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
            fresh = self.scratch.len();
        }
        self.turbo_sound(model, out, fresh);
    }

    /// Adds the turbochargers' compressor noise to the last `fresh` samples of each
    /// microphone's output.
    fn turbo_sound(&mut self, model: &Model, out: &mut [Vec<f64>], fresh: usize) {
        if fresh == 0 || self.settings.turbo == 0.0 {
            return;
        }
        let rate = OUTPUT_RATE as f64;
        let nyquist = 0.45 * rate;
        let tau = std::f64::consts::TAU;
        for (t, v) in model.turbos.iter().zip(&mut self.turbos) {
            let Some(c) = &t.compressor else {
                continue;
            };
            let inlet = &model.lumps[t.compressor_ports[0]];
            let a = (1.4 * crate::gas::R_AIR * inlet.t).sqrt();
            let shaft = t.omega / tau;
            // Relative Mach number at the inducer's tip.
            let rho = inlet.mass / inlet.volume;
            let inducer = PI * 0.25 * c.inducer * c.inducer;
            let axial = t.compressor_flow.abs() / (rho * inducer);
            let tip = t.omega * 0.5 * c.inducer;
            let m_rel = (tip * tip + axial * axial).sqrt() / a;
            let m_wheel = t.omega * 0.5 * c.wheel / a;
            // Sound power, W, and the pressure at 1 m of it spread over a sphere.
            let power = self.settings.turbo * 1e-6 * t.compressor_power.abs() * m_wheel * m_wheel;
            let p1 = (power * self.rho_air * C_AIR / (4.0 * PI)).sqrt();
            // Near surge the flow separates off the blades: the whoosh grows.
            let phi = t.flow_coefficient;
            let surge = if phi > 0.0 {
                (c.surge_flow / phi).powi(2).min(4.0)
            } else {
                4.0
            };
            let z = c.blades as f64;
            let bpf = z * shaft;
            let (tone, tcn, whoosh) = (0.35 * p1, 0.35 * p1, 0.3 * p1 * surge.sqrt());
            let buzz = ((m_rel - 1.0) * 3.0).clamp(0.0, 1.0) * 0.5 * p1;
            let mut src = [0.0f64; 8];
            let n = fresh.min(src.len());
            for s in src.iter_mut().take(n) {
                v.phase = (v.phase + tau * shaft / rate) % (tau * 64.0);
                let mut x = 0.0;
                for (k, amp) in [(z, 1.0), (2.0 * z, 0.5)] {
                    if k * shaft < nyquist {
                        x += tone * std::f64::consts::SQRT_2 * amp * (k * v.phase).sin();
                    }
                }
                if buzz > 0.0 {
                    for (o, &(amp, ph)) in v.buzz.iter().enumerate() {
                        let k = (o + 1) as f64;
                        if k * shaft < nyquist {
                            x += buzz * amp * (k * v.phase + ph).sin();
                        }
                    }
                }
                let white = (self.rng.uniform() - 0.5) * 12f64.sqrt();
                let (f, q) = ((0.55 * bpf).clamp(50.0, nyquist), 6.0);
                let band = PI * f / (2.0 * q);
                x += v
                    .tcn
                    .bandpass(white * tcn * (0.5 * rate / band).sqrt(), f, q, rate);
                let white = (self.rng.uniform() - 0.5) * 12f64.sqrt();
                let (f, q) = (7000.0, 0.8);
                let band = PI * f / (2.0 * q);
                x += v
                    .whoosh
                    .bandpass(white * whoosh * (0.5 * rate / band).sqrt(), f, q, rate);
                *s = x;
            }
            for (i, path) in v.paths.iter_mut().enumerate() {
                let len = out[i].len();
                for (k, &x) in src.iter().take(n).enumerate() {
                    out[i][len - fresh + k] += path.process(x);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::Build;
    use crate::samples;

    /// Outside, a click from a tailpipe arrives twice: straight, and off the ground from
    /// its image below it, later by the paths' difference and weaker by their ratio and
    /// the ground's reflection.
    #[test]
    fn microphones_hear_the_ground() {
        let (e, i, x) = (samples::i4(), samples::i4_intake(), samples::i4_exhaust());
        let (m, _) = Build::new(&e)
            .system("intake", &i)
            .system("exhaust", &x)
            .build()
            .unwrap();
        let tail = m.mouths.iter().find(|m| m.name.contains("tail")).unwrap();
        let s = tail.position;
        let mic = [s[0] - 2.0, s[1], 1.0];
        let settings = SoundSettings {
            mics: vec![Mic {
                name: "m".into(),
                position: mic,
                cabin: false,
            }],
            ..Default::default()
        };
        let mut ac = Acoustics::new(&m, settings);
        let k = m
            .mouths
            .iter()
            .position(|m| m.name.contains("tail"))
            .unwrap();
        let path = &mut ac.mics[0].mouths[k];
        let heard: Vec<f64> = (0..400)
            .map(|i| path.process(if i == 0 { 1.0 } else { 0.0 }))
            .collect();
        let rate = m.quality.rate() as f64;
        let (r, ri) = (dist(s, mic), dist([s[0], s[1], -s[2]], mic));
        // Each arrival lands on the two samples round its fractional delay.
        let arrival = |t: f64| -> f64 {
            let i = (t * rate).floor() as usize;
            heard[i] + heard[i + 1]
        };
        let (direct, bounced) = (arrival(r / C_AIR), arrival(ri / C_AIR));
        assert!((direct - path.gain).abs() < 1e-12, "{direct}");
        assert!((bounced / direct - 0.9 * r / ri).abs() < 1e-9, "{bounced}");
        let total: f64 = heard.iter().sum();
        assert!((total - direct - bounced).abs() < 1e-12);
    }
}
