//! The engine run live: a thread steps the model and its sound, and writes 48 kHz
//! stereo frames into a lock-free ring buffer that an audio output reads. The controls
//! are atomics the UI sets; the telemetry is a snapshot the thread publishes.
//!
//! The thread keeps the ring about 50 ms ahead and sleeps when it is full; the audio side
//! never waits on it. When the model cannot keep up (`realtime_factor` below 1), use a
//! lower quality.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::acoustics::{Acoustics, SoundSettings};
use crate::build::Build;
use crate::model::{Controls, Load};

/// Frames the thread produces at a time (≈ 5 ms).
const BLOCK: usize = 256;
/// Frames of the ring buffer (≈ 170 ms), and how far ahead the thread keeps it.
const RING: usize = 8192;
const AHEAD: usize = 2400;

/// An `f64` in an atomic.
#[derive(Debug, Default)]
pub struct AtomicF64(AtomicU64);

impl AtomicF64 {
    pub fn new(v: f64) -> Self {
        Self(AtomicU64::new(v.to_bits()))
    }
    pub fn get(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::Relaxed))
    }
    pub fn set(&self, v: f64) {
        self.0.store(v.to_bits(), Ordering::Relaxed)
    }
}

/// What the UI sets while the engine runs.
#[derive(Debug, Default)]
pub struct LiveControls {
    /// 0..1.
    pub pedal: AtomicF64,
    /// Held at this speed when above zero (a dyno), else free, rpm.
    pub hold_rpm: AtomicF64,
    /// Inertia added when free (a flywheel, or the car in a gear), kg·m².
    pub load_inertia: AtomicF64,
    /// Steady torque the load takes when free, N·m.
    pub load_torque: AtomicF64,
    pub ignition: AtomicBool,
    pub starter: AtomicBool,
    /// Sound pressure (Pa) at full scale.
    pub full_scale: AtomicF64,
    pub stop: AtomicBool,
    /// Frames the audio side wanted and did not get.
    pub underruns: AtomicU64,
}

/// A snapshot of the running engine.
#[derive(Clone, Debug, Default)]
pub struct Telemetry {
    pub time: f64,
    pub rpm: f64,
    pub pedal: f64,
    pub throttle: f64,
    /// Brake torque, N·m.
    pub torque: f64,
    /// Intake manifold (plenum) pressure, Pa.
    pub manifold_pressure: f64,
    /// Simulated seconds per wall-clock second, over the last half second.
    pub realtime_factor: f64,
    /// Cylinder 1 over its last cycle: (degrees from the firing TDC, Pa, m³).
    pub cylinder: Vec<(f64, f64, f64)>,
    /// The last output samples of the first microphone, Pa.
    pub scope: Vec<f32>,
    pub split_steps: u64,
}

/// A running engine.
pub struct Realtime {
    pub controls: Arc<LiveControls>,
    telemetry: Arc<Mutex<Telemetry>>,
    thread: Option<JoinHandle<()>>,
}

/// The audio side of a running engine: interleaved stereo frames at 48 kHz.
pub struct AudioOut {
    consumer: rtrb::Consumer<f32>,
    controls: Arc<LiveControls>,
}

impl AudioOut {
    /// Fills `out` (interleaved stereo); silence where the engine fell behind.
    pub fn fill(&mut self, out: &mut [f32]) {
        let n = self.consumer.slots().min(out.len());
        if let Ok(chunk) = self.consumer.read_chunk(n) {
            let (a, b) = chunk.as_slices();
            out[..a.len()].copy_from_slice(a);
            out[a.len()..a.len() + b.len()].copy_from_slice(b);
            chunk.commit_all();
        }
        if n < out.len() {
            out[n..].fill(0.0);
            self.controls
                .underruns
                .fetch_add(((out.len() - n) / 2) as u64, Ordering::Relaxed);
        }
    }

    /// One sample (the next of the interleaved stream), or silence.
    pub fn next_sample(&mut self) -> f32 {
        match self.consumer.pop() {
            Ok(v) => v,
            Err(_) => {
                self.controls.underruns.fetch_add(1, Ordering::Relaxed);
                0.0
            }
        }
    }
}

impl Realtime {
    /// Builds the engine and starts it at idle. The first one or two microphones of `sound`
    /// make the left and right channels.
    pub fn start(build: Build, sound: SoundSettings) -> Result<(Self, AudioOut), String> {
        let (mut m, _) = build.build()?;
        let controls = Arc::new(LiveControls::default());
        controls.ignition.store(true, Ordering::Relaxed);
        controls.full_scale.set(20.0);
        let telemetry = Arc::new(Mutex::new(Telemetry::default()));
        let (mut producer, consumer) = rtrb::RingBuffer::<f32>::new(RING * 2);
        let idle = build.engine.ecu.idle_rpm;
        // Settle at idle before the sound starts.
        m.set_crank(0.0, idle);
        let hold = Controls {
            load: Load::Speed(idle),
            ..Default::default()
        };
        while m.time < 0.4 {
            m.step(&hold);
        }
        let manifold = m
            .lumps
            .iter()
            .position(|l| l.name.ends_with("plenum"))
            .or_else(|| m.lumps.iter().position(|l| !l.cylinder));
        let (c2, t2) = (controls.clone(), telemetry.clone());
        let thread = std::thread::Builder::new()
            .name("engine".into())
            .spawn(move || {
                let mut ac = Acoustics::new(&m, sound);
                let mics = ac.mic_count().max(1);
                let mut bufs = vec![Vec::new(); mics];
                let mut pending: Vec<Vec<f64>> = vec![Vec::new(); mics];
                let mut cycle: Vec<(f64, f64, f64)> = Vec::new();
                let mut last_deg = 0.0;
                let mut scope: Vec<f32> = Vec::new();
                let (mut wall0, mut sim0) = (Instant::now(), m.time);
                let mut factor = 1.0;
                let mut next_tel = m.time;
                while !c2.stop.load(Ordering::Relaxed) {
                    let queued = RING * 2 - producer.slots();
                    if queued / 2 >= AHEAD {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    let hold = c2.hold_rpm.get();
                    let c = Controls {
                        pedal: c2.pedal.get(),
                        ignition: c2.ignition.load(Ordering::Relaxed),
                        starter: c2.starter.load(Ordering::Relaxed),
                        load: if hold > 0.0 {
                            Load::Speed(hold)
                        } else {
                            Load::Inertia {
                                inertia: c2.load_inertia.get(),
                                torque: c2.load_torque.get(),
                            }
                        },
                    };
                    while pending[0].len() < BLOCK {
                        m.step(&c);
                        for b in &mut bufs {
                            b.clear();
                        }
                        ac.process(&m, &mut bufs);
                        for (p, b) in pending.iter_mut().zip(&bufs) {
                            p.extend_from_slice(b);
                        }
                        let deg = m.cycle_deg(0);
                        if deg < last_deg && !cycle.is_empty() {
                            if let Ok(mut t) = t2.try_lock() {
                                t.cylinder = std::mem::take(&mut cycle);
                            }
                            cycle.clear();
                        }
                        last_deg = deg;
                        let g = m.cylinders[0].gas;
                        cycle.push((deg, m.lumps[g].p, m.lumps[g].volume));
                    }
                    let fs = c2.full_scale.get().max(1e-3) as f32;
                    let frames = pending[0].len();
                    if let Ok(mut chunk) = producer.write_chunk(frames * 2) {
                        let (a, b) = chunk.as_mut_slices();
                        for (k, slot) in a.iter_mut().chain(b.iter_mut()).enumerate() {
                            let (i, ch) = (k / 2, k % 2);
                            let v = pending[ch.min(mics - 1)][i] as f32 / fs;
                            *slot = v.tanh();
                        }
                        chunk.commit_all();
                    }
                    scope.extend(pending[0].iter().map(|&v| v as f32));
                    if scope.len() > 4096 {
                        scope.drain(..scope.len() - 4096);
                    }
                    for p in &mut pending {
                        p.clear();
                    }
                    let wall = wall0.elapsed().as_secs_f64();
                    if wall > 0.5 {
                        factor = (m.time - sim0) / wall;
                        wall0 = Instant::now();
                        sim0 = m.time;
                    }
                    if m.time >= next_tel {
                        next_tel = m.time + 1.0 / 60.0;
                        if let Ok(mut t) = t2.try_lock() {
                            t.time = m.time;
                            t.rpm = m.rpm();
                            t.pedal = c.pedal;
                            t.throttle = m.throttle;
                            t.torque = m.gas_torque - m.friction_torque;
                            t.manifold_pressure = manifold.map(|i| m.lumps[i].p).unwrap_or(0.0);
                            t.realtime_factor = factor;
                            t.scope.clone_from(&scope);
                            t.split_steps = m.split_steps;
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok((
            Self {
                controls: controls.clone(),
                telemetry,
                thread: Some(thread),
            },
            AudioOut { consumer, controls },
        ))
    }

    pub fn telemetry(&self) -> Telemetry {
        self.telemetry.lock().map(|t| t.clone()).unwrap_or_default()
    }

    pub fn stop(&mut self) {
        self.controls.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Realtime {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Quality;
    use crate::samples;

    /// The engine runs, revs up on the pedal and fills the audio ring (no audio device).
    #[test]
    fn runs_and_revs() {
        let (e, i, x) = (samples::i4(), samples::i4_intake(), samples::i4_exhaust());
        let b = Build::new(&e)
            .system("intake", &i)
            .system("exhaust", &x)
            .quality(Quality::Draft);
        let (mut rt, mut out) = Realtime::start(b, SoundSettings::default()).unwrap();
        let mut buf = vec![0.0f32; 2048];
        let t0 = Instant::now();
        rt.controls.pedal.set(0.6);
        let mut peak_rpm: f64 = 0.0;
        while t0.elapsed() < Duration::from_millis(1500) {
            std::thread::sleep(Duration::from_millis(20));
            out.fill(&mut buf);
            peak_rpm = peak_rpm.max(rt.telemetry().rpm);
        }
        let t = rt.telemetry();
        rt.stop();
        assert!(t.time > 0.5, "the engine thread ran ({} s)", t.time);
        assert!(peak_rpm > 1500.0, "revved to {peak_rpm}");
        assert!(buf.iter().any(|v| v.abs() > 1e-4));
    }
}
