//! Scripted runs: the throttle and the speed (or a free-revving load) over time, rendered
//! to microphone recordings, and written as WAV files.

use serde::{Deserialize, Serialize};

use crate::acoustics::{Acoustics, SoundSettings};
use crate::build::Build;
use crate::dsp::OUTPUT_RATE;
use crate::model::{Controls, Load, Model};
use crate::spec::EngineSpec;
use crate::table::lookup;

/// What the engine drives in a script.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum ScriptLoad {
    /// A dynamometer holding the speed of the script's `rpm` keys.
    #[default]
    Speed,
    /// Free: the engine's own inertia plus this, kg·m², and a steady torque, N·m.
    Free { inertia: f64, torque: f64 },
}

/// A run of the engine over time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Script {
    /// s.
    pub duration: f64,
    #[serde(default)]
    pub load: ScriptLoad,
    /// (s, rpm) keys; with a free load only the first sets the starting speed.
    #[serde(default)]
    pub rpm: Vec<(f64, f64)>,
    /// (s, pedal 0..1) keys, linear between.
    #[serde(default)]
    pub throttle: Vec<(f64, f64)>,
    /// Time run before recording, held at the starting speed, s.
    #[serde(default = "default_warmup")]
    pub warmup: f64,
}

fn default_warmup() -> f64 {
    0.3
}

/// Names of the built-in scripts.
pub const PRESETS: [&str; 5] = ["sweep", "idle", "blips", "rev", "overrun"];

impl Script {
    /// A built-in script for an engine: `sweep` (full throttle from idle to the limiter on
    /// a dyno), `idle`, `blips` (throttle blips from idle), `rev` (free rev into the
    /// limiter and back), `overrun` (throttle shut from the limiter down).
    pub fn preset(name: &str, e: &EngineSpec) -> Option<Self> {
        let (idle, limit) = (e.ecu.idle_rpm, e.ecu.limiter_rpm);
        let free = ScriptLoad::Free {
            inertia: 0.0,
            torque: 0.0,
        };
        Some(match name {
            "sweep" => Self {
                duration: 8.0,
                load: ScriptLoad::Speed,
                rpm: vec![(0.0, (idle + 400.0).max(1000.0)), (8.0, limit - 100.0)],
                throttle: vec![(0.0, 1.0)],
                warmup: 0.3,
            },
            "idle" => Self {
                duration: 4.0,
                load: free,
                rpm: vec![(0.0, idle)],
                throttle: vec![(0.0, 0.0)],
                warmup: 1.5,
            },
            "blips" => {
                let mut t = vec![(0.0, 0.0)];
                for k in 0..4 {
                    let s = 0.8 + k as f64 * 1.3;
                    t.extend([(s, 0.0), (s + 0.05, 1.0), (s + 0.3, 1.0), (s + 0.35, 0.0)]);
                }
                Self {
                    duration: 6.0,
                    load: free,
                    rpm: vec![(0.0, idle)],
                    throttle: t,
                    warmup: 1.5,
                }
            }
            "rev" => Self {
                duration: 6.0,
                load: free,
                rpm: vec![(0.0, idle)],
                throttle: vec![(0.0, 0.0), (0.5, 0.0), (0.6, 1.0), (3.5, 1.0), (3.6, 0.0)],
                warmup: 1.5,
            },
            "overrun" => Self {
                duration: 6.0,
                load: ScriptLoad::Speed,
                rpm: vec![(0.0, limit - 200.0), (6.0, (idle + 600.0).max(1500.0))],
                throttle: vec![(0.0, 0.0)],
                warmup: 0.3,
            },
            _ => return None,
        })
    }

    /// The controls at time `t`.
    pub fn controls(&self, t: f64) -> Controls {
        let pedal = lookup(&self.throttle, t).clamp(0.0, 1.0);
        let load = match self.load {
            ScriptLoad::Speed => Load::Speed(lookup(&self.rpm, t).max(100.0)),
            ScriptLoad::Free { inertia, torque } => Load::Inertia { inertia, torque },
        };
        Controls {
            pedal,
            ignition: true,
            starter: false,
            load,
        }
    }

    pub fn start_rpm(&self) -> f64 {
        self.rpm.first().map(|k| k.1).unwrap_or(1000.0)
    }
}

/// What a microphone heard, and how the engine ran.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Recording {
    pub rate: u32,
    pub mics: Vec<String>,
    /// Sound pressure at each microphone, Pa.
    pub channels: Vec<Vec<f32>>,
    /// Every 10 ms: (s, rpm, pedal, brake torque N·m).
    pub telemetry: Vec<(f64, f64, f64, f64)>,
}

/// Runs a script and records it.
pub fn render(
    b: &Build,
    script: &Script,
    sound: Option<SoundSettings>,
) -> Result<Recording, String> {
    let (mut m, _) = b.build()?;
    let sound = sound.unwrap_or_else(|| SoundSettings::near(&m, "tail"));
    record(&mut m, script, sound)
}

/// Runs a script on a model and records it.
pub fn record(m: &mut Model, script: &Script, sound: SoundSettings) -> Result<Recording, String> {
    record_streaming(m, script, sound, |_, _| true)
}

/// As `record`, handing each new stretch of 48 kHz samples (per microphone, Pa) and the
/// share of the script done to `progress` as it goes (for a preview that plays while it
/// renders); `progress` returning false stops the run there.
pub fn record_streaming(
    m: &mut Model,
    script: &Script,
    sound: SoundSettings,
    mut progress: impl FnMut(&[Vec<f32>], f64) -> bool,
) -> Result<Recording, String> {
    if script.duration.is_nan() || script.duration <= 0.0 {
        return Err("the script needs a duration".into());
    }
    let mics: Vec<String> = sound.mics.iter().map(|m| m.name.clone()).collect();
    // Warm up held at the starting speed.
    m.set_crank(0.0, script.start_rpm());
    let hold = Controls {
        load: Load::Speed(script.start_rpm()),
        ..script.controls(0.0)
    };
    let t0 = m.time;
    while m.time - t0 < script.warmup {
        m.step(&hold);
    }
    let mut ac = Acoustics::new(m, sound);
    let mut channels = vec![Vec::new(); ac.mic_count()];
    let mut telemetry = Vec::new();
    let start = m.time;
    let mut next_tel = 0.0;
    let mut buf = vec![Vec::new(); ac.mic_count()];
    let mut sent = 0;
    while m.time - start < script.duration {
        let t = m.time - start;
        let c = script.controls(t);
        m.step(&c);
        for b in &mut buf {
            b.clear();
        }
        ac.process(m, &mut buf);
        for (ch, b) in channels.iter_mut().zip(&buf) {
            ch.extend(b.iter().map(|&v| v as f32));
        }
        if t >= next_tel {
            telemetry.push((t, m.rpm(), c.pedal, m.gas_torque - m.friction_torque));
            next_tel += 0.01;
        }
        // Every 20 ms of sound.
        if !channels.is_empty() && channels[0].len() >= sent + 960 {
            let new: Vec<Vec<f32>> = channels.iter().map(|c| c[sent..].to_vec()).collect();
            sent = channels[0].len();
            if !progress(&new, t / script.duration) {
                break;
            }
        }
    }
    let new: Vec<Vec<f32>> = channels
        .iter()
        .map(|c| c[sent.min(c.len())..].to_vec())
        .collect();
    progress(&new, 1.0);
    Ok(Recording {
        rate: OUTPUT_RATE,
        mics,
        channels,
        telemetry,
    })
}

impl Recording {
    /// Largest absolute pressure, Pa.
    pub fn peak(&self) -> f32 {
        self.channels
            .iter()
            .flat_map(|c| c.iter())
            .fold(0.0f32, |m, v| m.max(v.abs()))
    }

    /// Writes the recording as a WAV file: one channel per microphone (two microphones make
    /// a stereo file), 32-bit float, scaled so its peak is at `peak_dbfs` (or, with
    /// `full_scale` Pa, at a fixed level: that pressure is 0 dBFS).
    pub fn write_wav(
        &self,
        path: &std::path::Path,
        peak_dbfs: f64,
        full_scale: Option<f64>,
    ) -> std::io::Result<()> {
        let gain = match full_scale {
            Some(fs) => 1.0 / fs as f32,
            None => 10f32.powf(peak_dbfs as f32 / 20.0) / self.peak().max(1e-9),
        };
        let frames = self.channels.iter().map(|c| c.len()).min().unwrap_or(0);
        let ch = self.channels.len().max(1) as u16;
        let mut data = Vec::with_capacity(frames * ch as usize * 4);
        for i in 0..frames {
            for c in &self.channels {
                data.extend_from_slice(&(c[i] * gain).clamp(-1.0, 1.0).to_le_bytes());
            }
        }
        std::fs::write(path, wav_bytes(self.rate, ch, &data))
    }
}

/// A WAV file of 32-bit float samples (`data` interleaved, little endian).
pub fn wav_bytes(rate: u32, channels: u16, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(44 + data.len());
    let block = channels * 4;
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * block as u32).to_le_bytes());
    out.extend_from_slice(&block.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    out
}
